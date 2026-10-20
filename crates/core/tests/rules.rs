//! Fixture-based rule tests: a tempfile tree mirroring the test-mode layout
//! (`$FDC_TEST_ROOT/Users/tester` = home), walked with std::fs, then classified.

use fdc_core::model::{Node, NodeId, NodeKind, ScanTree};
use fdc_core::rules::{ClassifyParams, Matcher, Roots, classify_tree, rules};
use fdc_core::safety::{Classified, Status, Target};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// (path relative to fs root, expected rule id, expected status). One row per path rule.
#[rustfmt::skip]
const EXPECTED: &[(&str, &str, Status)] = &[
    ("Users/tester/Library/Containers/com.docker.docker", "docker_vm", Status::DoNotTouch),
    ("Users/tester/Library/Mobile Documents", "icloud_drive", Status::DoNotTouch),
    ("Users/tester/Library/Keychains", "keychains", Status::DoNotTouch),
    ("Users/tester/Library/Mail", "mail", Status::DoNotTouch),
    ("Users/tester/Library/Messages", "messages", Status::DoNotTouch),
    ("System", "system", Status::DoNotTouch),
    ("private/var/vm", "vm_swap", Status::DoNotTouch),
    ("private/var/db", "var_db", Status::DoNotTouch),
    ("private/var/folders", "var_folders", Status::DoNotTouch),
    ("usr", "usr", Status::DoNotTouch),
    ("Library/Developer/CoreSimulator/Volumes", "simulator_runtimes", Status::DoNotTouch),
    ("Users/tester/Library/Developer/Xcode/DerivedData/App-abc", "derived_data", Status::Safe),
    ("Users/tester/Library/Developer/Xcode/Archives/2026-01-01", "xcode_archives", Status::Caution),
    ("Users/tester/Library/Developer/Xcode/iOS DeviceSupport/17.0", "ios_device_support", Status::Safe),
    ("Users/tester/Library/Developer/Xcode/watchOS DeviceSupport/10.0", "watchos_device_support", Status::Safe),
    ("Users/tester/Library/Developer/CoreSimulator/Caches", "simulator_caches", Status::Safe),
    ("Users/tester/Library/Developer/CoreSimulator/Devices/UUID-1", "simulator_devices", Status::Caution),
    ("Users/tester/Library/Caches/com.apple.dt.Xcode", "xcode_cache", Status::Safe),
    ("Users/tester/Library/Developer/Xcode/UserData/Previews", "xcode_previews", Status::Safe),
    ("Users/tester/.gradle/caches", "gradle_caches", Status::Safe),
    ("Users/tester/.gradle/wrapper/dists", "gradle_wrapper", Status::Safe),
    ("Users/tester/.android/avd/Pixel.avd", "android_avd", Status::Caution),
    ("Users/tester/Library/Android/sdk/system-images/android-34", "android_system_images", Status::Caution),
    ("Users/tester/.npm/_cacache", "npm_cache", Status::Safe),
    ("Users/tester/Library/Caches/Yarn", "yarn_cache", Status::Safe),
    ("Users/tester/.yarn/berry/cache", "yarn_berry_cache", Status::Safe),
    ("Users/tester/Library/pnpm/store", "pnpm_store", Status::Safe),
    ("Users/tester/Library/Caches/Homebrew", "homebrew_cache", Status::Safe),
    ("Users/tester/Library/Caches/CocoaPods", "cocoapods_cache", Status::Safe),
    ("Users/tester/.cocoapods/repos", "cocoapods_repos", Status::Safe),
    ("Users/tester/Library/Logs", "user_logs", Status::Safe),
    ("Library/Logs/DiagnosticReports", "diagnostic_reports", Status::Caution),
    ("Library/Caches/com.example.daemon", "system_caches", Status::Caution),
    ("Users/tester/Library/Caches/com.example.app", "user_caches", Status::Caution),
    ("Users/tester/code/fresh/node_modules", "node_modules", Status::Safe),
    ("Users/tester/code/old/node_modules", "node_modules", Status::Safe),
];

fn walk(tree: &mut ScanTree, id: NodeId, path: &Path) {
    let mut entries: Vec<_> = fs::read_dir(path)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        let md = fs::symlink_metadata(&p).unwrap();
        let child = tree.nodes.len();
        tree.nodes.push(Node {
            name: p.file_name().unwrap().into(),
            parent: Some(id),
            children: Vec::new(),
            kind: if md.is_dir() {
                NodeKind::Dir
            } else {
                NodeKind::File
            },
            own_size: md.len(),
            total_size: md.len(),
            mtime: md.modified().ok(),
            atime: md.accessed().ok(),
            newest_mtime: md.modified().ok(),
            error: None,
        });
        tree.nodes[id].children.push(child);
        if md.is_dir() {
            walk(tree, child, &p);
        }
        let (size, newest) = (tree.nodes[child].total_size, tree.nodes[child].newest_mtime);
        tree.nodes[id].total_size += size;
        tree.nodes[id].newest_mtime = tree.nodes[id].newest_mtime.max(newest);
    }
}

fn scan(root: &Path) -> ScanTree {
    let mut tree = ScanTree {
        nodes: vec![Node {
            name: root.into(),
            parent: None,
            children: Vec::new(),
            kind: NodeKind::Dir,
            own_size: 0,
            total_size: 0,
            mtime: None,
            atime: None,
            newest_mtime: None,
            error: None,
        }],
    };
    walk(&mut tree, ScanTree::ROOT, root);
    tree
}

fn touch(path: &Path, mtime: SystemTime) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"x").unwrap();
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
}

#[test]
fn every_rule_on_fixture_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let root: PathBuf = tmp.path().canonicalize().unwrap();
    let now = SystemTime::now();
    let day = Duration::from_secs(86_400);
    for (rel, _, _) in EXPECTED {
        touch(&root.join(rel).join("payload"), now);
    }
    touch(&root.join("private/var/vm/sleepimage"), now);
    // Nested node_modules is part of the outer item, not separate.
    touch(
        &root.join("Users/tester/code/fresh/node_modules/a/node_modules/b/index.js"),
        now,
    );
    touch(&root.join("Users/tester/code/fresh/package.json"), now);
    touch(
        &root.join("Users/tester/code/old/package.json"),
        now - 100 * day,
    );
    touch(
        &root.join("Users/tester/code/old/yarn.lock"),
        now - 90 * day,
    );
    // node_modules inside a "Do not touch" area is covered by that area.
    touch(
        &root.join("Users/tester/Library/Mobile Documents/proj/node_modules/x.js"),
        now,
    );
    // Unknown paths get no status.
    touch(&root.join("Users/tester/Documents/notes.txt"), now);
    touch(
        &root.join("Users/tester/Library/Preferences/com.example.plist"),
        now,
    );

    let roots = Roots {
        fs_root: root.clone(),
        home: root.join("Users/tester"),
    };
    let params = ClassifyParams {
        stale_node_modules_after: 30 * day,
        now,
    };
    let items = classify_tree(&scan(&root), &roots, &params);

    let got: BTreeMap<String, (&str, Status, bool)> = items
        .iter()
        .map(|c: &Classified| {
            let Target::Path(p) = &c.info().target else {
                panic!("command target from a tree")
            };
            let rel = p
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            (rel, (c.info().rule_id, c.status(), c.info().stale))
        })
        .collect();
    assert_eq!(got.len(), items.len(), "duplicate item paths");
    let want: BTreeMap<String, (&str, Status, bool)> = EXPECTED
        .iter()
        .map(|&(rel, id, st)| (rel.to_owned(), (id, st, rel.contains("/old/"))))
        .collect();
    assert_eq!(got, want);

    // Every path rule has a fixture row.
    for r in rules() {
        if let Matcher::Path(..) = r.matcher {
            assert!(
                EXPECTED.iter().any(|e| e.1 == r.id),
                "no fixture for {}",
                r.id
            );
        }
    }
}

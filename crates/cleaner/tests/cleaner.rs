//! Mocks + tempfile trees only. Never spawns docker/tmutil/xcrun, never touches the real disk or Trash.

use fdc_cleaner::*;
use fdc_core::rules::{ClassifyParams, Roots, classify_command, classify_tree};
use fdc_core::safety::{Classified, CommandTarget, Deletable, DeletableStatus, Target};
use fdc_core::scan::{ScanOptions, scan};
use proptest::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

type TestCleaner = Cleaner<RealFs, MockTrasher, MockRunner>;

fn roots_in(dir: &Path) -> Roots {
    Roots {
        fs_root: dir.into(),
        home: dir.join("Users/tester"),
    }
}

/// Creates `<root>/<rel>/f` (4 KiB) for each `rel`.
fn make(root: &Path, rels: &[&str]) {
    for rel in rels {
        let d = root.join(rel);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("f"), vec![7u8; 4096]).unwrap();
    }
}

fn classify(scan_root: &Path, roots: &Roots) -> Vec<Classified> {
    let tree = scan(
        scan_root,
        &ScanOptions::default(),
        &|_| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    let params = ClassifyParams {
        stale_node_modules_after: Duration::from_secs(30 * 86400),
        now: SystemTime::now(),
    };
    classify_tree(&tree, roots, &params)
}

fn deletable_at(scan_root: &Path, roots: &Roots, path: &Path) -> Deletable {
    classify(scan_root, roots)
        .into_iter()
        .filter_map(|c| c.as_deletable().cloned())
        .find(|d| d.path() == Some(path))
        .unwrap_or_else(|| panic!("no deletable at {path:?}"))
}

fn cleaner(roots: Roots, trash: &Path) -> TestCleaner {
    Cleaner::new(
        RealFs,
        MockTrasher::new(trash),
        MockRunner::test_mode(),
        roots,
    )
}

fn cmd_item(t: CommandTarget) -> Deletable {
    classify_command(t, 1000).as_deletable().unwrap().clone()
}

// ---------- commands ----------

#[test]
fn exact_command_strings() {
    use CommandTarget::*;
    let cases = [
        (DockerBuildCache, "docker builder prune -f"),
        (DockerDanglingImages, "docker image prune -f"),
        (DockerUnusedImages, "docker image prune -a -f"),
        (DockerStoppedContainers, "docker container prune -f"),
        (DockerVolumes, "docker volume prune -f"),
        (
            TimeMachineSnapshot("2026-10-01-123456".into()),
            "tmutil deletelocalsnapshots 2026-10-01-123456",
        ),
        (SimctlUnavailableDevices, "xcrun simctl delete unavailable"),
        (
            SimctlRuntime("5E1F7C2A-9B3D".into()),
            "xcrun simctl runtime delete 5E1F7C2A-9B3D",
        ),
    ];
    for (t, s) in cases {
        assert_eq!(command_for(&t).to_string(), s);
        assert!(
            !command_for(&t)
                .args
                .iter()
                .any(|a| a.contains("Docker.raw"))
        );
    }
}

#[test]
fn display_quotes_special_args() {
    let c = CommandSpec::new(
        "docker",
        &["system", "df", "--format", "{{json .}}", "it's"],
    );
    assert_eq!(
        c.to_string(),
        r"docker system df --format '{{json .}}' 'it'\''s'"
    );
}

#[test]
fn docker_size_units() {
    assert_eq!(parse_docker_size("0B"), Some(0));
    assert_eq!(parse_docker_size("512kB"), Some(512_000));
    assert_eq!(parse_docker_size("1.2GB"), Some(1_200_000_000));
    assert_eq!(parse_docker_size("512MB (40%)"), Some(512_000_000));
    assert_eq!(parse_docker_size("1.5TB"), Some(1_500_000_000_000));
    assert_eq!(parse_docker_size("N/A"), None);
}

#[test]
fn parses_docker_df() {
    let u = docker_usage(&MockRunner::test_mode()).unwrap();
    assert_eq!(
        u.images,
        Usage {
            size: 3_452_000_000,
            reclaimable: 2_315_000_000
        }
    );
    assert_eq!(
        u.containers,
        Usage {
            size: 13_970_000,
            reclaimable: 12_500_000
        }
    );
    assert_eq!(
        u.volumes,
        Usage {
            size: 1_280_000_000,
            reclaimable: 512_000_000
        }
    );
    assert_eq!(
        u.build_cache,
        Usage {
            size: 1_873_000_000,
            reclaimable: 1_873_000_000
        }
    );
    assert_eq!(u.dangling_images, 421_000_000 + 88_400_000);

    let items = docker_items(&MockRunner::test_mode()).unwrap();
    let got: Vec<_> = items
        .iter()
        .map(|c| (c.info().target.clone(), c.info().size))
        .collect();
    use CommandTarget::*;
    let t = |c| Target::Command(c);
    assert_eq!(
        got,
        [
            (t(DockerBuildCache), 1_873_000_000),
            (t(DockerDanglingImages), 509_400_000),
            (t(DockerUnusedImages), 2_315_000_000),
            (t(DockerStoppedContainers), 12_500_000),
            (t(DockerVolumes), 512_000_000),
        ]
    );
    let vol = items[4].as_deletable().unwrap();
    assert!(!vol.selected_by_default());
}

#[test]
fn docker_zero_sizes_skipped() {
    let df = r#"{"Type":"Images","Size":"0B","Reclaimable":"0B"}
{"Type":"Build Cache","Size":"10MB","Reclaimable":"10MB"}"#;
    let r = MockRunner::new()
        .with(
            "docker system df --format '{{json .}}'",
            CommandOutput::ok(df),
        )
        .with(
            "docker images -f dangling=true --format '{{json .}}'",
            CommandOutput::ok(""),
        );
    let items = docker_items(&r).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].info().target,
        Target::Command(CommandTarget::DockerBuildCache)
    );
}

#[test]
fn docker_not_running() {
    // Not installed: spawn fails.
    assert!(matches!(
        docker_items(&MockRunner::new()),
        Err(QueryError::Unavailable(..))
    ));
    // Daemon down.
    let r = MockRunner::new().with_fallback(CommandOutput::fail(
        "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?",
    ));
    assert!(matches!(docker_usage(&r), Err(QueryError::Unavailable(..))));
    // Garbage output.
    let r = MockRunner::new().with_fallback(CommandOutput::ok("not json"));
    assert!(matches!(docker_usage(&r), Err(QueryError::Parse(..))));
}

#[test]
fn parses_tmutil() {
    let items = tmutil_items(&MockRunner::test_mode()).unwrap();
    let dates: Vec<_> = items
        .iter()
        .map(|c| match &c.info().target {
            Target::Command(CommandTarget::TimeMachineSnapshot(d)) => d.as_str(),
            t => panic!("{t:?}"),
        })
        .collect();
    assert_eq!(
        dates,
        [
            "2026-10-17-221030",
            "2026-10-18-120001",
            "2026-10-19-091502"
        ]
    );
    assert!(items.iter().all(|c| c.info().hint_key.is_some()));
    let none = MockRunner::new().with_fallback(CommandOutput::ok("Snapshots for disk /:\n"));
    assert!(tmutil_items(&none).unwrap().is_empty());
}

#[test]
fn parses_simctl() {
    let items = simctl_items(&MockRunner::test_mode()).unwrap();
    let got: Vec<_> = items
        .iter()
        .map(|c| (c.info().target.clone(), c.info().size))
        .collect();
    use CommandTarget::*;
    assert_eq!(
        got,
        [
            (Target::Command(SimctlUnavailableDevices), 1_610_612_736),
            (
                Target::Command(SimctlRuntime("5E1F7C2A-9B3D-4E8F-A1C2-3D4E5F6A7B8C".into())),
                7_134_511_104
            ),
            (
                Target::Command(SimctlRuntime("A9B8C7D6-E5F4-4321-8765-0FEDCBA98765".into())),
                9_012_345_678
            ),
        ]
    );
}

#[test]
fn test_mode_runs_every_command_through_mock() {
    let tmp = TempDir::new().unwrap();
    let c = cleaner(roots_in(tmp.path()), &tmp.path().join(".trash"));
    let mut items = docker_items(&c.runner).unwrap();
    items.extend(tmutil_items(&c.runner).unwrap());
    items.extend(simctl_items(&c.runner).unwrap());
    for it in &items {
        let d = it
            .as_deletable()
            .expect("all listed command items are deletable");
        let o = c.run_command(d).unwrap();
        let docker =
            matches!(&d.info().target, Target::Command(t) if command_for(t).program == "docker");
        assert_eq!(o.docker_disk_note, docker);
        assert_eq!(
            o.reclaimed,
            if docker { 1_500_000_000 } else { d.info().size }
        );
        assert!(!o.trashed);
    }
    let calls = c.runner.calls();
    assert!(calls.contains(&"docker volume prune -f".to_string()));
    assert!(calls.contains(&"tmutil deletelocalsnapshots 2026-10-17-221030".to_string()));
    assert!(calls.contains(&"xcrun simctl delete unavailable".to_string()));
}

#[test]
fn command_failure_and_wrong_kind() {
    let tmp = TempDir::new().unwrap();
    let mut c = cleaner(roots_in(tmp.path()), &tmp.path().join(".trash"));
    c.runner = MockRunner::new().with_fallback(CommandOutput::fail("boom"));
    let e = c
        .run_command(&cmd_item(CommandTarget::DockerBuildCache))
        .unwrap_err();
    assert!(
        matches!(e, CleanError::CommandFailed { ref command, ref stderr } if command == "docker builder prune -f" && stderr == "boom")
    );
    c.runner = MockRunner::new(); // spawn error
    assert!(matches!(
        c.run_command(&cmd_item(CommandTarget::SimctlUnavailableDevices)),
        Err(CleanError::CommandFailed { .. })
    ));
    assert!(matches!(
        c.delete(&cmd_item(CommandTarget::DockerBuildCache), Method::Trash),
        Err(CleanError::NotAPath)
    ));
}

// ---------- path deletion ----------

const DD: &str = "Users/tester/Library/Developer/Xcode/DerivedData/AppA-abc";
const ARCHIVE: &str = "Users/tester/Library/Developer/Xcode/Archives/2026-01-15";
const BREW: &str = "Users/tester/Library/Caches/Homebrew";

struct Env {
    tmp: TempDir,
    trash: TempDir,
    roots: Roots,
}

fn env() -> Env {
    let tmp = TempDir::new().unwrap();
    make(
        tmp.path(),
        &[
            DD,
            ARCHIVE,
            BREW,
            "Users/tester/Projects/app/node_modules/react",
        ],
    );
    let roots = roots_in(tmp.path());
    Env {
        tmp,
        trash: TempDir::new().unwrap(),
        roots,
    }
}

impl Env {
    fn p(&self, rel: &str) -> PathBuf {
        self.tmp.path().join(rel)
    }
    fn item(&self, rel: &str) -> Deletable {
        deletable_at(self.tmp.path(), &self.roots, &self.p(rel))
    }
    fn cleaner(&self) -> TestCleaner {
        cleaner(self.roots.clone(), self.trash.path())
    }
}

#[test]
fn trash_is_default_and_reports_zero_reclaimed() {
    let e = env();
    let item = e.item(DD);
    let c = e.cleaner();
    let o = c.delete(&item, Method::Trash).unwrap();
    assert_eq!(
        o,
        Outcome {
            estimated: item.info().size,
            reclaimed: 0,
            trashed: true,
            docker_disk_note: false
        }
    );
    assert!(item.info().size > 0);
    assert!(!e.p(DD).exists());
    assert_eq!(c.trasher.trashed(), [e.p(DD)]);
    assert!(e.trash.path().join("0-AppA-abc/f").exists());
}

#[test]
fn direct_delete_safe_removes_and_reports_size() {
    let e = env();
    let item = e.item(DD);
    let token = item.direct_delete_token().unwrap();
    let o = e.cleaner().delete(&item, Method::Direct(token)).unwrap();
    assert_eq!(o.reclaimed, item.info().size);
    assert!(!o.trashed);
    assert!(!e.p(DD).exists());
    assert!(e.p(ARCHIVE).exists());
}

#[test]
fn caution_cannot_be_deleted_directly() {
    let e = env();
    let caution = e.item(ARCHIVE);
    assert_eq!(caution.status(), DeletableStatus::Caution);
    assert!(caution.direct_delete_token().is_none());
    // A token from another (Safe) item does not unlock it.
    let token = e.item(DD).direct_delete_token().unwrap();
    let err = e
        .cleaner()
        .delete(&caution, Method::Direct(token))
        .unwrap_err();
    assert!(matches!(err, CleanError::NotDirectDeletable(_)));
    assert!(e.p(ARCHIVE).exists());
    // Trash still works for Caution.
    assert!(e.cleaner().delete(&caution, Method::Trash).unwrap().trashed);
}

#[test]
fn vanished_is_rejected() {
    let e = env();
    let item = e.item(DD);
    fs::remove_dir_all(e.p(DD)).unwrap();
    assert!(matches!(
        e.cleaner().delete(&item, Method::Trash),
        Err(CleanError::Vanished(_))
    ));
}

#[test]
fn replaced_by_symlink_is_rejected() {
    let e = env();
    let item = e.item(DD);
    let victim = e.tmp.path().join("precious");
    make(e.tmp.path(), &["precious"]);
    fs::remove_dir_all(e.p(DD)).unwrap();
    std::os::unix::fs::symlink(&victim, e.p(DD)).unwrap();
    let c = e.cleaner();
    assert!(matches!(
        c.delete(&item, Method::Trash),
        Err(CleanError::Symlink(_))
    ));
    let token = item.direct_delete_token().unwrap();
    assert!(matches!(
        c.delete(&item, Method::Direct(token)),
        Err(CleanError::Symlink(_))
    ));
    assert!(victim.join("f").exists());
    assert!(c.trasher.trashed().is_empty());
}

#[test]
fn symlinked_ancestor_escaping_root_is_rejected() {
    let e = env();
    let item = e.item(BREW);
    // Library/Caches becomes a symlink to a directory outside the allowed area.
    let outside = TempDir::new().unwrap();
    make(outside.path(), &["Caches/Homebrew"]);
    let caches = e.p("Users/tester/Library/Caches");
    fs::remove_dir_all(&caches).unwrap();
    std::os::unix::fs::symlink(outside.path().join("Caches"), &caches).unwrap();
    let token = item.direct_delete_token().unwrap();
    let c = e.cleaner();
    assert!(matches!(
        c.delete(&item, Method::Direct(token)),
        Err(CleanError::Escapes(_))
    ));
    assert!(matches!(
        c.delete(&item, Method::Trash),
        Err(CleanError::Escapes(_))
    ));
    assert!(outside.path().join("Caches/Homebrew/f").exists());
}

#[test]
fn now_inside_do_not_touch_is_rejected() {
    // Classified while the area was not protected (different home), deleted with the real roots:
    // the path is now inside `Library/Mobile Documents` (iCloud Drive, Do not touch).
    let e = env();
    let rel = "Users/tester/Library/Mobile Documents/proj/node_modules";
    make(e.tmp.path(), &[rel]);
    let old_home = e.p("Users/tester/Library/Mobile Documents");
    let old_roots = Roots {
        fs_root: old_home.clone(),
        home: old_home.clone(),
    };
    let item = deletable_at(&old_home, &old_roots, &e.p(rel));
    let err = e.cleaner().delete(&item, Method::Trash).unwrap_err();
    assert!(
        matches!(err, CleanError::ProtectedAncestor { ref ancestor, .. } if *ancestor == old_home)
    );
    assert!(e.p(rel).exists());
}

#[test]
fn rule_mismatch_is_rejected() {
    // Classified as `system_caches` (fs_root-anchored Library/Caches/*); really `homebrew_cache`.
    let e = env();
    let old = Roots {
        fs_root: e.p("Users/tester"),
        home: e.p("elsewhere"),
    };
    let item = deletable_at(&e.p("Users/tester"), &old, &e.p(BREW));
    assert_ne!(item.info().rule_id, "homebrew_cache");
    let err = e.cleaner().delete(&item, Method::Trash).unwrap_err();
    assert!(matches!(err, CleanError::RuleMismatch { .. }));
    assert!(e.p(BREW).exists());
}

#[test]
fn path_outside_roots_is_rejected() {
    let e = env();
    let other = TempDir::new().unwrap();
    make(other.path(), &[DD]);
    let item = deletable_at(
        other.path(),
        &roots_in(other.path()),
        &other.path().join(DD),
    );
    assert!(e.cleaner().delete(&item, Method::Trash).is_err());
    assert!(other.path().join(DD).exists());
}

// ---------- properties ----------

const SEGS: &[&str] = &[
    "Library",
    "Caches",
    "Keychains",
    "Mobile Documents",
    "Mail",
    "Developer",
    "Xcode",
    "DerivedData",
    "Archives",
    "node_modules",
    "System",
    "usr",
    "proj",
    "Homebrew",
    ".gradle",
    "caches",
    "Yarn",
    "Logs",
];

fn rel_paths() -> impl Strategy<Value = Vec<String>> {
    let path = (
        prop::sample::select(vec!["", "Users/tester/", "Users/tester/Library/"]),
        prop::collection::vec(prop::sample::select(SEGS.to_vec()), 1..5),
        any::<bool>(),
    )
        .prop_map(|(prefix, segs, nm)| {
            let mut p = format!("{prefix}{}", segs.join("/"));
            if nm {
                p.push_str("/node_modules");
            }
            p
        });
    prop::collection::vec(path, 1..8)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Nothing classified as deletable lies in or under a "Do not touch" match, and the cleaner
    /// accepts every one of them (no false rejections on an unchanged tree).
    #[test]
    fn deletion_list_never_contains_do_not_touch(rels in rel_paths()) {
        let tmp = TempDir::new().unwrap();
        let trash = TempDir::new().unwrap();
        make(tmp.path(), &rels.iter().map(String::as_str).collect::<Vec<_>>());
        make(tmp.path(), &["Users/tester/zz/node_modules"]); // never vacuous
        let roots = roots_in(tmp.path());
        let list: Vec<Deletable> =
            classify(tmp.path(), &roots).iter().filter_map(|c| c.as_deletable().cloned()).collect();
        for d in &list {
            let p = d.path().unwrap();
            for a in p.ancestors().take_while(|a| a.starts_with(tmp.path())) {
                let st = fdc_core::rules::match_path(a, &roots).map(|r| r.status);
                prop_assert_ne!(st, Some(fdc_core::safety::Status::DoNotTouch), "{:?}", p);
            }
        }
        prop_assert!(!list.is_empty());
        let c = cleaner(roots, trash.path());
        for d in &list {
            prop_assert!(c.delete(d, Method::Trash).is_ok(), "{:?}", d.path());
        }
    }

    /// Items classified under an anchor that is really a "Do not touch" area are all rejected.
    #[test]
    fn cleaner_rejects_anything_under_do_not_touch(
        area in prop::sample::select(vec![
            "Users/tester/Library/Keychains", "Users/tester/Library/Mobile Documents",
            "Users/tester/Library/Mail", "System", "usr",
        ]),
        rels in rel_paths(),
    ) {
        let tmp = TempDir::new().unwrap();
        let trash = TempDir::new().unwrap();
        let base = tmp.path().join(area);
        make(&base, &rels.iter().map(String::as_str).collect::<Vec<_>>());
        make(&base, &["zz/node_modules"]); // never vacuous
        let fake = Roots { fs_root: base.clone(), home: base.clone() };
        let c = cleaner(roots_in(tmp.path()), trash.path());
        let list: Vec<Deletable> =
            classify(&base, &fake).iter().filter_map(|c| c.as_deletable().cloned()).collect();
        prop_assert!(!list.is_empty());
        for d in &list {
            prop_assert!(c.delete(d, Method::Trash).is_err());
            if let Some(t) = d.direct_delete_token() {
                prop_assert!(c.delete(d, Method::Direct(t)).is_err());
            }
            prop_assert!(d.path().unwrap().exists());
        }
        prop_assert!(c.trasher.trashed().is_empty());
    }
}

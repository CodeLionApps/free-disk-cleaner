use super::*;
use crate::model::{Node, NodeKind};
use proptest::prelude::*;
use std::time::UNIX_EPOCH;

fn roots() -> Roots {
    Roots {
        fs_root: "/r".into(),
        home: "/r/Users/tester".into(),
    }
}

fn params(now: SystemTime, after: Duration) -> ClassifyParams {
    ClassifyParams {
        stale_node_modules_after: after,
        now,
    }
}

fn default_params() -> ClassifyParams {
    params(
        UNIX_EPOCH + Duration::from_secs(1_000_000),
        Duration::from_secs(1000),
    )
}

fn node(name: &str, parent: Option<NodeId>) -> Node {
    Node {
        name: name.into(),
        parent,
        children: Vec::new(),
        kind: NodeKind::Dir,
        own_size: 1,
        total_size: 1,
        mtime: None,
        atime: None,
        newest_mtime: None,
        error: None,
    }
}

/// Tree rooted at `root` containing every relative path; the leaf gets `mtime` (and newest_mtime).
fn tree_of(root: &str, paths: &[(Vec<&str>, Option<SystemTime>)]) -> ScanTree {
    let mut t = ScanTree {
        nodes: vec![node(root, None)],
    };
    for (segs, mtime) in paths {
        let mut cur = ScanTree::ROOT;
        for s in segs {
            let found = t.nodes[cur]
                .children
                .iter()
                .copied()
                .find(|&c| t.nodes[c].name == *s);
            cur = found.unwrap_or_else(|| {
                let id = t.nodes.len();
                t.nodes.push(node(s, Some(cur)));
                t.nodes[cur].children.push(id);
                id
            });
        }
        t.nodes[cur].mtime = *mtime;
        t.nodes[cur].newest_mtime = *mtime;
    }
    t
}

fn path_of(c: &Classified) -> &Path {
    match &c.info().target {
        Target::Path(p) => p,
        Target::Command(_) => panic!("command item from classify_tree"),
    }
}

fn ids(items: &[Classified]) -> Vec<(&Path, &str, Status)> {
    items
        .iter()
        .map(|c| (path_of(c), c.info().rule_id, c.status()))
        .collect()
}

fn segs(p: &str) -> Vec<&str> {
    p.split('/').collect()
}

#[test]
fn rule_ids_unique_and_every_command_kind_has_one_rule() {
    let mut ids: Vec<_> = rules().iter().map(|r| r.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), rules().len());
    for t in all_command_targets() {
        let kind = CommandKind::of(&t);
        let n = rules()
            .iter()
            .filter(|r| r.matcher == Matcher::Command(kind))
            .count();
        assert_eq!(n, 1, "{kind:?}");
    }
}

fn all_command_targets() -> Vec<CommandTarget> {
    vec![
        CommandTarget::DockerBuildCache,
        CommandTarget::DockerDanglingImages,
        CommandTarget::DockerUnusedImages,
        CommandTarget::DockerStoppedContainers,
        CommandTarget::DockerVolumes,
        CommandTarget::TimeMachineSnapshot("2026-10-01-120000".into()),
        CommandTarget::SimctlUnavailableDevices,
        CommandTarget::SimctlRuntime("ABC".into()),
    ]
}

#[test]
fn translation_keys_cover_table_once() {
    let keys = all_translation_keys();
    for r in rules() {
        assert!(keys.contains(&r.rationale_key));
        assert!(r.hint_key.is_none_or(|h| keys.contains(&h)));
    }
    let mut d = keys.clone();
    d.sort();
    d.dedup();
    assert_eq!(d.len(), keys.len());
    assert!(keys.contains(&"rule.derived_data.why"));
    assert!(keys.contains(&"hint.tm_keep_newest"));
    assert!(keys.contains(&"hint.runtime_keep_current"));
}

#[test]
fn glob_semantics() {
    fn s(p: &str) -> Vec<&OsStr> {
        p.split('/').map(OsStr::new).collect()
    }
    assert!(glob(&["**", "node_modules"], &s("node_modules")));
    assert!(glob(&["**", "node_modules"], &s("a/b/node_modules")));
    assert!(!glob(&["**", "node_modules"], &s("a/node_modules/b")));
    assert!(glob(&["a", "*"], &s("a/b")));
    assert!(!glob(&["a", "*"], &s("a")));
    assert!(!glob(&["a", "*"], &s("a/b/c")));
    // Wildcards skip hidden segments; literal ones still match.
    assert!(!glob(&["a", "*"], &s("a/.b")));
    assert!(!glob(
        &["**", "node_modules"],
        &s(".nvm/versions/node/v22/lib/node_modules")
    ));
    assert!(!glob(
        &["**", "node_modules"],
        &s("code/.cache/node_modules")
    ));
    assert!(glob(&[".npm", "_cacache"], &s(".npm/_cacache")));
}

#[test]
fn global_node_installs_are_not_node_modules_items() {
    let r = roots();
    let id = |p: &str| match_path(Path::new(p), &r).map(|r| r.id);
    for p in [
        "/r/Users/tester/.nvm/versions/node/v22.22.0/lib/node_modules",
        "/r/Users/tester/.volta/tools/image/node/22.0.0/lib/node_modules",
        "/r/Users/tester/.vscode/extensions/foo/node_modules",
    ] {
        assert_eq!(id(p), None, "{p}");
    }
    assert_eq!(
        id("/r/Users/tester/Library/pnpm/global/5/node_modules"),
        Some("pnpm_global")
    );
    assert_eq!(id("/r/Users/tester/Library/pnpm/store"), Some("pnpm_store"));
}

#[test]
fn do_not_touch_ancestor_wins_over_node_modules() {
    let r = roots();
    let nm = Path::new("/r/Users/tester/Library/Mobile Documents/proj/node_modules");
    assert_eq!(match_path(nm, &r).unwrap().id, "icloud_drive");
    let t = tree_of(
        "/r",
        &[(
            segs("Users/tester/Library/Mobile Documents/proj/node_modules/x"),
            None,
        )],
    );
    let items = classify_tree(&t, &r, &default_params());
    assert_eq!(
        ids(&items),
        vec![(
            Path::new("/r/Users/tester/Library/Mobile Documents"),
            "icloud_drive",
            Status::DoNotTouch
        )]
    );
}

#[test]
fn scan_root_inside_do_not_touch_is_protected() {
    let t = tree_of(
        "/r/Users/tester/Library/Mobile Documents/proj",
        &[(segs("node_modules/x"), None)],
    );
    let items = classify_tree(&t, &roots(), &default_params());
    assert_eq!(items.len(), 1);
    assert!(items[0].as_deletable().is_none());
}

#[test]
fn matched_dirs_are_not_descended() {
    let t = tree_of(
        "/r",
        &[
            (
                segs("Users/tester/code/app/node_modules/a/node_modules/b"),
                None,
            ),
            (segs("Users/tester/Documents/notes.txt"), None),
        ],
    );
    let items = classify_tree(&t, &roots(), &default_params());
    assert_eq!(
        ids(&items),
        vec![(
            Path::new("/r/Users/tester/code/app/node_modules"),
            "node_modules",
            Status::Caution
        )]
    );
}

#[test]
fn specific_caches_win_over_catch_all() {
    let r = roots();
    let id = |p: &str| match_path(Path::new(p), &r).map(|r| r.id);
    assert_eq!(
        id("/r/Users/tester/Library/Caches/Homebrew"),
        Some("homebrew_cache")
    );
    assert_eq!(
        id("/r/Users/tester/Library/Caches/Yarn"),
        Some("yarn_cache")
    );
    assert_eq!(
        id("/r/Users/tester/Library/Caches/CocoaPods"),
        Some("cocoapods_cache")
    );
    assert_eq!(
        id("/r/Users/tester/Library/Caches/com.apple.dt.Xcode"),
        Some("xcode_cache")
    );
    assert_eq!(
        id("/r/Users/tester/Library/Caches/com.foo"),
        Some("user_caches")
    );
    assert_eq!(id("/r/Users/tester/Library/Caches"), None);
}

#[test]
fn parent_dir_components_never_match() {
    let r = roots();
    let p = Path::new("/r/Users/tester/Library/Caches/../Keychains");
    assert!(match_path(p, &r).is_none());
    let p = Path::new("/r/Users/tester/Library/Caches/Homebrew/../../Keychains");
    // The `Homebrew` ancestor is matched; the cleaner compares the item path itself.
    assert_eq!(match_path(p, &r).unwrap().id, "homebrew_cache");
    assert!(match_exact(p, &r).is_none());
}

#[test]
fn real_mode_roots() {
    let r = Roots {
        fs_root: "/".into(),
        home: "/Users/me".into(),
    };
    let id = |p: &str| match_path(Path::new(p), &r).map(|r| r.id);
    // The Data volume firmlink mirrors home under /System: protected, not node_modules.
    assert_eq!(
        id("/System/Volumes/Data/Users/me/x/node_modules"),
        Some("system")
    );
    assert_eq!(id("/Users/me/x/node_modules"), Some("node_modules"));
    assert_eq!(id("/Library/Caches/com.apple.foo"), Some("system_caches"));
    assert_eq!(id("/private/var/vm/sleepimage"), Some("vm_swap"));
    assert_eq!(id("/Users/me/Documents"), None);
    assert_eq!(id("/"), None);
}

#[test]
fn command_items() {
    for t in all_command_targets() {
        let c = classify_command(t.clone(), 42);
        let d = c.as_deletable().expect("no command rule is DoNotTouch");
        assert_eq!(d.info().target, Target::Command(t.clone()));
        assert_eq!(d.info().size, 42);
        assert!(d.direct_delete_token().is_none(), "{t:?}");
        let (status, selected, hint) = match t {
            CommandTarget::DockerBuildCache | CommandTarget::DockerDanglingImages => {
                (DeletableStatus::Safe, true, None)
            }
            CommandTarget::SimctlUnavailableDevices => (DeletableStatus::Safe, true, None),
            CommandTarget::DockerUnusedImages
            | CommandTarget::DockerStoppedContainers
            | CommandTarget::DockerVolumes => (DeletableStatus::Caution, false, None),
            CommandTarget::TimeMachineSnapshot(_) => {
                (DeletableStatus::Caution, false, Some("hint.tm_keep_newest"))
            }
            CommandTarget::SimctlRuntime(_) => (
                DeletableStatus::Caution,
                false,
                Some("hint.runtime_keep_current"),
            ),
        };
        assert_eq!(d.status(), status, "{t:?}");
        assert_eq!(d.selected_by_default(), selected, "{t:?}");
        assert_eq!(d.info().hint_key, hint, "{t:?}");
    }
}

#[test]
fn direct_delete_token_only_for_safe_paths() {
    let t = tree_of(
        "/r",
        &[
            (
                segs("Users/tester/Library/Developer/Xcode/DerivedData/App"),
                None,
            ),
            (
                segs("Users/tester/Library/Developer/Xcode/Archives/A"),
                None,
            ),
        ],
    );
    let items = classify_tree(&t, &roots(), &default_params());
    let tok = |id: &str| {
        items
            .iter()
            .find(|c| c.info().rule_id == id)
            .and_then(Classified::as_deletable)
            .unwrap()
            .direct_delete_token()
            .is_some()
    };
    assert!(tok("derived_data"));
    assert!(!tok("xcode_archives"));
}

#[test]
fn stale_falls_back_to_node_modules_mtime() {
    let p = default_params();
    let old = p.now - Duration::from_secs(5000);
    let t = tree_of("/r", &[(segs("Users/tester/a/node_modules"), Some(old))]);
    let items = classify_tree(&t, &roots(), &p);
    assert!(items[0].info().stale);
    let t = tree_of("/r", &[(segs("Users/tester/a/node_modules"), None)]);
    assert!(!classify_tree(&t, &roots(), &p)[0].info().stale);
}

const VOCAB: &[&str] = &[
    "Library",
    "Caches",
    "Homebrew",
    "Yarn",
    "Developer",
    "Xcode",
    "DerivedData",
    "Archives",
    "CoreSimulator",
    "Devices",
    "Volumes",
    "Mobile Documents",
    "Keychains",
    "Mail",
    "Containers",
    "com.docker.docker",
    "node_modules",
    "proj",
    "private",
    "var",
    "vm",
    "db",
    "folders",
    "System",
    "usr",
    "Logs",
    "DiagnosticReports",
    ".gradle",
    "caches",
    ".npm",
    "_cacache",
    "x",
];

fn path_strategy() -> impl Strategy<Value = Vec<&'static str>> {
    (
        any::<bool>(),
        prop::collection::vec(prop::sample::select(VOCAB), 0..8),
    )
        .prop_map(|(in_home, mut segs)| {
            if in_home {
                segs.splice(0..0, ["Users", "tester"]);
            }
            segs
        })
}

fn tree_strategy() -> impl Strategy<Value = ScanTree> {
    prop::collection::vec(path_strategy(), 1..30).prop_map(|paths| {
        let paths: Vec<_> = paths.into_iter().map(|p| (p, None)).collect();
        tree_of("/r", &paths)
    })
}

proptest! {
    #[test]
    fn do_not_touch_never_deletable(t in tree_strategy()) {
        let r = roots();
        let items = classify_tree(&t, &r, &default_params());
        let deletion_list: Vec<&Path> = items
            .iter()
            .filter_map(Classified::as_deletable)
            .filter_map(Deletable::path)
            .collect();
        for d in &deletion_list {
            prop_assert_ne!(match_path(d, &r).unwrap().status, Status::DoNotTouch);
        }
        for id in 0..t.nodes.len() {
            let p = t.path(id);
            if match_exact(&p, &r).is_some_and(|rule| rule.status == Status::DoNotTouch) {
                for d in &deletion_list {
                    prop_assert!(!p.starts_with(d), "{} inside deletable {}", p.display(), d.display());
                }
            }
        }
    }

    #[test]
    fn classification_is_deterministic(t in tree_strategy()) {
        let r = roots();
        prop_assert_eq!(classify_tree(&t, &r, &default_params()), classify_tree(&t, &r, &default_params()));
    }

    #[test]
    fn only_matched_paths_get_a_status(t in tree_strategy()) {
        let r = roots();
        let items = classify_tree(&t, &r, &default_params());
        for c in &items {
            prop_assert_eq!(match_path(path_of(c), &r).map(|r| r.id), Some(c.info().rule_id));
        }
        // Node is covered by an item iff match_path recognizes it.
        for id in 0..t.nodes.len() {
            let p = t.path(id);
            let covered = items.iter().any(|c| p.starts_with(path_of(c)));
            prop_assert_eq!(covered, match_path(&p, &r).is_some(), "{}", p.display());
        }
    }

    #[test]
    fn stale_node_modules(
        pkg in prop::option::of(0u64..2000),
        lock in prop::option::of(0u64..2000),
        nm in prop::option::of(0u64..2000),
        now in 0u64..3000,
        after in 0u64..3000,
    ) {
        let at = |s: u64| UNIX_EPOCH + Duration::from_secs(s);
        let mut paths = vec![(segs("Users/tester/p/node_modules"), nm.map(at))];
        if let Some(s) = pkg {
            paths.push((segs("Users/tester/p/package.json"), Some(at(s))));
        }
        if let Some(s) = lock {
            paths.push((segs("Users/tester/p/yarn.lock"), Some(at(s))));
        }
        let t = tree_of("/r", &paths);
        let items = classify_tree(&t, &roots(), &params(at(now), Duration::from_secs(after)));
        prop_assert_eq!(items.len(), 1);
        let cutoff = now as i128 - after as i128;
        let known: Vec<u64> = pkg.into_iter().chain(lock).chain(nm).collect();
        let expected = !known.is_empty() && known.iter().all(|&s| (s as i128) < cutoff);
        prop_assert_eq!(items[0].info().stale, expected);
        let status = items[0].as_deletable().map(|d| d.status());
        let want = if expected { DeletableStatus::Safe } else { DeletableStatus::Caution };
        prop_assert_eq!(status, Some(want));
    }
}

use fdc_core::model::{NodeError, NodeKind, ScanTree};
use fdc_core::scan::{ScanOptions, scan};
use fdc_fixtures as fx;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

const MIB: u64 = 1 << 20;

/// Generates fixtures, scans, then restores the locked dir so the tempdir can be removed.
fn scan_fixture(root: &Path) -> ScanTree {
    fx::generate(root).unwrap();
    let tree = scan(
        root,
        &ScanOptions::default(),
        &|_| {},
        &AtomicBool::new(false),
    );
    fs::set_permissions(root.join(fx::LOCKED_DIR), fs::Permissions::from_mode(0o755)).unwrap();
    tree.unwrap()
}

fn listing(tree: &ScanTree) -> Vec<(PathBuf, NodeKind, u64)> {
    let root = tree.root_path();
    (0..tree.nodes.len())
        .map(|i| {
            let p = tree.path(i).strip_prefix(root).unwrap().to_path_buf();
            (p, tree.node(i).kind, tree.node(i).own_size)
        })
        .collect()
}

#[test]
fn fixture_scan_is_deterministic_and_accounted() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (ta, tb) = (scan_fixture(a.path()), scan_fixture(b.path()));
    assert_eq!(listing(&ta), listing(&tb));

    let root = a.path();
    let at = |rel: &str| ta.node(ta.find(&root.join(rel)).unwrap());

    let sparse = at(fx::SPARSE).total_size;
    assert!(
        (fx::SPARSE_WRITTEN..2 * fx::SPARSE_WRITTEN).contains(&sparse),
        "{sparse}"
    );
    assert_eq!(at("Users/tester/Projects/hardlinks").total_size, MIB);
    assert_eq!(at("Users/tester/Projects/clones").total_size, MIB);
    let link = at(fx::ESCAPE_LINK);
    assert_eq!(link.kind, NodeKind::Symlink);
    assert!(link.children.is_empty());
    // SAFETY: trivial libc call.
    if unsafe { libc::geteuid() } != 0 {
        assert_eq!(at(fx::LOCKED_DIR).error, Some(NodeError::PermissionDenied));
    }
    let stale = at(fx::STALE_PROJECT).newest_mtime.unwrap();
    assert!(stale.elapsed().unwrap() >= fx::STALE_AGE);
    assert!(
        at("Users/tester/Projects/fresh-app")
            .newest_mtime
            .unwrap()
            .elapsed()
            .unwrap()
            < fx::STALE_AGE
    );

    // Every table file except the one inside the locked dir, plus the sparse file and marker.
    let table: u64 = fx::FILES
        .iter()
        .filter(|(p, _)| !p.starts_with(fx::LOCKED_DIR))
        .map(|&(_, s)| s)
        .sum();
    let expected = table + sparse + at(fx::MARKER).own_size;
    assert_eq!(ta.node(ScanTree::ROOT).total_size, expected);
}

#[test]
fn fixtures_refuse_unmarked_non_empty_dir() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("keep.txt"), b"x").unwrap();
    assert!(fx::generate(dir.path()).is_err());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

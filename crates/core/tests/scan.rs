use fdc_core::model::{NodeError, NodeKind, ScanTree};
use fdc_core::scan::{ScanError, ScanOptions, scan};
use proptest::prelude::*;
use std::ffi::CString;
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const MIB: usize = 1 << 20;

fn run(root: &Path) -> ScanTree {
    scan(
        root,
        &ScanOptions::default(),
        &|_| {},
        &AtomicBool::new(false),
    )
    .unwrap()
}

fn size_of(tree: &ScanTree, path: &Path) -> u64 {
    tree.node(tree.find(path).unwrap()).total_size
}

#[test]
fn sparse_file_reports_allocated_not_logical() {
    let dir = tempfile::tempdir().unwrap();
    let f = File::create(dir.path().join("sparse")).unwrap();
    f.set_len(10 << 30).unwrap();
    (&f).write_all(&[1; 64 * 1024]).unwrap();
    let tree = run(dir.path());
    let size = size_of(&tree, &dir.path().join("sparse"));
    assert!((64 * 1024..MIB as u64).contains(&size), "{size}");
}

#[test]
fn hardlink_counted_once() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a"), vec![1; MIB]).unwrap();
    fs::hard_link(dir.path().join("a"), dir.path().join("b")).unwrap();
    let tree = run(dir.path());
    let (a, b) = (
        size_of(&tree, &dir.path().join("a")),
        size_of(&tree, &dir.path().join("b")),
    );
    assert_eq!((a, b), (MIB as u64, 0));
    assert_eq!(tree.node(ScanTree::ROOT).total_size, MIB as u64);
}

#[test]
fn clone_counted_once() {
    let dir = tempfile::tempdir().unwrap();
    let (orig, clone) = (dir.path().join("orig"), dir.path().join("clone"));
    fs::write(&orig, vec![1; MIB]).unwrap();
    let c = |p: &Path| CString::new(p.as_os_str().as_bytes()).unwrap();
    // SAFETY: two valid NUL-terminated paths.
    assert_eq!(
        unsafe { libc::clonefile(c(&orig).as_ptr(), c(&clone).as_ptr(), 0) },
        0
    );
    let tree = run(dir.path());
    assert_eq!(size_of(&tree, &clone) + size_of(&tree, &orig), MIB as u64);
    assert_eq!(tree.node(ScanTree::ROOT).total_size, MIB as u64);
}

#[test]
fn symlinks_not_followed() {
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("big"), vec![1; 4 * MIB]).unwrap();
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("inner")).unwrap();
    fs::write(dir.path().join("inner/f"), vec![1; MIB]).unwrap();
    symlink(outside.path(), dir.path().join("out-link")).unwrap();
    symlink(dir.path().join("inner"), dir.path().join("in-link")).unwrap();

    let tree = run(dir.path());
    for name in ["out-link", "in-link"] {
        let n = tree.node(tree.find(&dir.path().join(name)).unwrap());
        assert_eq!(n.kind, NodeKind::Symlink);
        assert!(n.children.is_empty());
        assert!(n.total_size < 64 * 1024);
    }
    assert!(tree.node(ScanTree::ROOT).total_size < 2 * MIB as u64);
}

#[test]
fn unreadable_dir_reported_and_scan_finishes() {
    // SAFETY: trivial libc call.
    if unsafe { libc::geteuid() } == 0 {
        return; // root ignores mode 000
    }
    let dir = tempfile::tempdir().unwrap();
    let locked = dir.path().join("locked");
    fs::create_dir(&locked).unwrap();
    fs::write(locked.join("f"), b"x").unwrap();
    fs::write(dir.path().join("ok"), vec![1; MIB]).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    let result = scan(
        dir.path(),
        &ScanOptions::default(),
        &|_| {},
        &AtomicBool::new(false),
    );
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    let tree = result.unwrap();
    let n = tree.node(tree.find(&locked).unwrap());
    assert_eq!(n.error, Some(NodeError::PermissionDenied));
    assert!(n.children.is_empty());
    assert_eq!(size_of(&tree, &dir.path().join("ok")), MIB as u64);
}

#[test]
fn unreadable_root_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let r = scan(
        &dir.path().join("nope"),
        &ScanOptions::default(),
        &|_| {},
        &AtomicBool::new(false),
    );
    assert!(matches!(r, Err(ScanError::RootUnreadable(_))));
}

#[test]
fn skip_list_is_listed_not_descended() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("skipme")).unwrap();
    fs::write(dir.path().join("skipme/f"), vec![1; MIB]).unwrap();
    let opts = ScanOptions {
        skip: vec![dir.path().join("skipme")],
        threads: Some(2),
    };
    let tree = scan(dir.path(), &opts, &|_| {}, &AtomicBool::new(false)).unwrap();
    let n = tree.node(tree.find(&dir.path().join("skipme")).unwrap());
    assert!(n.children.is_empty());
}

#[test]
fn cancellation_stops_promptly() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..200 {
        fs::create_dir_all(dir.path().join(format!("d{i}/sub"))).unwrap();
        fs::write(dir.path().join(format!("d{i}/sub/f")), b"x").unwrap();
    }
    let opts = ScanOptions::default();

    let pre = AtomicBool::new(true);
    assert!(matches!(
        scan(dir.path(), &opts, &|_| {}, &pre),
        Err(ScanError::Cancelled)
    ));

    // The first progress event fires on the first entry; cancelling there must stop right away.
    let cancel = AtomicBool::new(false);
    let events = AtomicUsize::new(0);
    let on_event = |_| {
        events.fetch_add(1, Ordering::Relaxed);
        cancel.store(true, Ordering::Relaxed);
    };
    assert!(matches!(
        scan(dir.path(), &opts, &on_event, &cancel),
        Err(ScanError::Cancelled)
    ));
    assert_eq!(events.load(Ordering::Relaxed), 1);
}

#[derive(Debug, Clone)]
struct Entry {
    parent: prop::sample::Index,
    dir: bool,
    size: usize,
}

fn entries() -> impl Strategy<Value = Vec<Entry>> {
    prop::collection::vec(
        (any::<prop::sample::Index>(), any::<bool>(), 0usize..40_000)
            .prop_map(|(parent, dir, size)| Entry { parent, dir, size }),
        1..40,
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn totals_are_sums(entries in entries()) {
        let dir = tempfile::tempdir().unwrap();
        let mut dirs = vec![dir.path().to_path_buf()];
        let mut expected = 0;
        for (i, e) in entries.iter().enumerate() {
            let path = dirs[e.parent.index(dirs.len())].join(format!("n{i}"));
            if e.dir {
                fs::create_dir(&path).unwrap();
                dirs.push(path);
            } else {
                fs::write(&path, vec![7; e.size]).unwrap();
                expected += fs::symlink_metadata(&path).unwrap().blocks() * 512;
            }
        }
        for d in &dirs {
            expected += fs::symlink_metadata(d).unwrap().blocks() * 512;
        }

        let tree = run(dir.path());
        prop_assert_eq!(tree.nodes.len(), entries.len() + 1);
        for n in &tree.nodes {
            let children: u64 = n.children.iter().map(|&c| tree.node(c).total_size).sum();
            prop_assert_eq!(n.total_size, n.own_size + children);
            for &c in &n.children {
                prop_assert!(n.newest_mtime >= tree.node(c).newest_mtime);
            }
        }
        let own: u64 = tree.nodes.iter().map(|n| n.own_size).sum();
        prop_assert_eq!(tree.node(ScanTree::ROOT).total_size, own);
        prop_assert_eq!(own, expected);
    }
}

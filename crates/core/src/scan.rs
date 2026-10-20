//! Parallel, read-only scanner (jwalk). Builds a [`ScanTree`] with allocated sizes.
//!
//! Size = `st_blocks * 512`. Symlinks are never followed. Hardlinks (same `(dev, ino)`) and pure
//! APFS clones (same `(dev, clone id)`) are counted once: later occurrences get `own_size` 0 (or
//! their private bytes for clones).

use crate::model::{Node, NodeError, NodeKind, ScanTree};
use std::collections::HashSet;
use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    /// Absolute paths that are listed but not descended (e.g. `/System/Volumes`, `/Volumes`, `/dev`).
    pub skip: Vec<PathBuf>,
    /// Worker threads; `None` = one per core.
    pub threads: Option<usize>,
}

#[derive(Debug, Clone)]
pub enum ScanEvent {
    /// First entry, then throttled (~every 50 ms). `files` counts all entries seen so far, `bytes` their allocated size.
    Progress {
        files: u64,
        bytes: u64,
        current: PathBuf,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("cannot read scan root: {0}")]
    RootUnreadable(#[source] io::Error),
    #[error("scan cancelled")]
    Cancelled,
}

const PROGRESS_EVERY: Duration = Duration::from_millis(50);

/// Per-entry metadata, gathered on the worker threads.
#[derive(Debug)]
struct Stat {
    alloc: u64,
    dev: u64,
    ino: u64,
    nlink: u64,
    mtime: Option<SystemTime>,
    atime: Option<SystemTime>,
    clone: Option<CloneInfo>,
}

#[derive(Debug, Clone, Copy)]
struct CloneInfo {
    id: u64,
    private: u64,
}

type Walk = jwalk::WalkDirGeneric<((), Option<io::Result<Stat>>)>;

pub fn scan(
    root: &Path,
    opts: &ScanOptions,
    on_event: &(dyn Fn(ScanEvent) + Sync),
    cancel: &AtomicBool,
) -> Result<ScanTree, ScanError> {
    fs::read_dir(root).map_err(ScanError::RootUnreadable)?;

    // jwalk needs a 'static callback, so workers watch our own flag, mirrored from `cancel`.
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let skip = opts.skip.clone();
    let walk = Walk::new(root)
        .skip_hidden(false)
        .follow_links(false)
        .sort(true) // deterministic order => deterministic dedupe winner
        .parallelism(jwalk::Parallelism::RayonNewPool(opts.threads.unwrap_or(0)))
        .process_read_dir(move |_, _, _, entries| {
            if worker_stop.load(Ordering::Relaxed) {
                entries.clear();
                return;
            }
            for e in entries.iter_mut().flatten() {
                let path = e.path();
                if e.depth > 0 && e.file_type.is_dir() && skip.contains(&path) {
                    e.read_children = None;
                }
                e.client_state = Some(stat(&path, e.file_type.is_file()));
            }
        });

    let mut nodes: Vec<Node> = Vec::new();
    let mut dir_stack: Vec<usize> = Vec::new(); // dir_stack[d] = node id of the open dir at depth d
    let mut seen_inodes = HashSet::new();
    let mut seen_clones = HashSet::new();
    let (mut files, mut bytes) = (0u64, 0u64);
    let mut last = Instant::now() - PROGRESS_EVERY; // first event right away

    for item in walk {
        if cancel.load(Ordering::Relaxed) {
            stop.store(true, Ordering::Relaxed);
            return Err(ScanError::Cancelled);
        }
        let mut e = match item {
            Ok(e) => e,
            Err(err) => {
                // ponytail: entry-level walk errors without a node (readdir hiccup) are pinned on
                // the parent dir; one error per node is enough for the UI.
                let parent = err.depth().checked_sub(1).and_then(|d| dir_stack.get(d));
                if let Some(&p) = parent {
                    nodes[p].error.get_or_insert(node_error(err.io_error()));
                }
                continue;
            }
        };

        let kind = if e.file_type.is_dir() {
            NodeKind::Dir
        } else if e.file_type.is_file() {
            NodeKind::File
        } else if e.file_type.is_symlink() {
            NodeKind::Symlink
        } else {
            NodeKind::Other
        };
        let mut node = Node {
            name: if e.depth == 0 {
                root.as_os_str().to_owned()
            } else {
                e.file_name.clone()
            },
            parent: None,
            children: Vec::new(),
            kind,
            own_size: 0,
            total_size: 0,
            mtime: None,
            atime: None,
            newest_mtime: None,
            error: None,
        };
        // Only missing if the workers were told to stop; stat inline then.
        let st = e
            .client_state
            .take()
            .unwrap_or_else(|| stat(&e.path(), false));
        match st {
            Ok(st) => {
                node.own_size = if kind != NodeKind::Dir
                    && st.nlink > 1
                    && !seen_inodes.insert((st.dev, st.ino))
                {
                    0
                } else {
                    match st.clone {
                        Some(c) if !seen_clones.insert((st.dev, c.id)) => c.private,
                        _ => st.alloc,
                    }
                };
                node.mtime = st.mtime;
                node.atime = st.atime;
                node.newest_mtime = st.mtime;
            }
            Err(err) => node.error = Some(node_error(Some(&err))),
        }
        if let Some(err) = e.read_children.as_ref().and_then(|r| r.error()) {
            node.error = Some(node_error(err.io_error()));
        }

        let id = nodes.len();
        if e.depth > 0 {
            let parent = dir_stack[e.depth - 1];
            node.parent = Some(parent);
            nodes[parent].children.push(id);
        }
        if kind == NodeKind::Dir {
            dir_stack.truncate(e.depth);
            dir_stack.push(id);
        }
        files += 1;
        bytes += node.own_size;
        nodes.push(node);

        if last.elapsed() >= PROGRESS_EVERY {
            last = Instant::now();
            on_event(ScanEvent::Progress {
                files,
                bytes,
                current: e.path(),
            });
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(ScanError::Cancelled);
    }

    // Preorder arena: every child has a larger id than its parent, so one reverse pass aggregates.
    for n in &mut nodes {
        n.total_size = n.own_size;
    }
    for i in (1..nodes.len()).rev() {
        let (total, newest) = (nodes[i].total_size, nodes[i].newest_mtime);
        let parent = nodes[i].parent.expect("non-root has a parent");
        let p = &mut nodes[parent];
        p.total_size += total;
        p.newest_mtime = p.newest_mtime.max(newest);
    }
    Ok(ScanTree { nodes })
}

fn stat(path: &Path, is_file: bool) -> io::Result<Stat> {
    let m = fs::symlink_metadata(path)?;
    Ok(Stat {
        alloc: m.blocks() * 512,
        dev: m.dev() as u64,
        ino: m.ino(),
        nlink: m.nlink(),
        mtime: m.modified().ok(),
        atime: m.accessed().ok(),
        clone: if is_file { clone_info(path) } else { None },
    })
}

fn node_error(err: Option<&io::Error>) -> NodeError {
    match err.map(io::Error::kind) {
        Some(io::ErrorKind::PermissionDenied) => NodeError::PermissionDenied,
        Some(io::ErrorKind::NotFound) => NodeError::Vanished,
        Some(_) => NodeError::Other(err.map(ToString::to_string).unwrap_or_default()),
        None => NodeError::Other("walk error".into()),
    }
}

/// `EF_MAY_SHARE_BLOCKS` from `<sys/stat.h>` (not in the `libc` crate).
const EF_MAY_SHARE_BLOCKS: u64 = 0x1;

#[repr(C, packed(4))]
struct CloneAttrs {
    len: u32,
    // Order follows the bit order of the requested ATTR_CMNEXT_* attributes.
    private_size: i64,
    clone_id: u64,
    ext_flags: u64,
}

/// APFS clone data via `getattrlist` (ATTR_CMNEXT_PRIVATESIZE / CLONEID / EXT_FLAGS).
///
/// Measured on APFS: pure clones (`clonefile`) share one clone id, private size 0. Writing to a
/// clone gives it a new clone id and private size = the rewritten blocks.
// ponytail: dedupe is by clone id, so pure clones count once but a partially modified clone counts
// its still-shared blocks again (overcount <= shared bytes). Exact accounting needs extent maps.
// ponytail: one extra syscall per regular file; getattrlistbulk per dir (as dua-core does) if the
// D2 benchmark says it matters.
fn clone_info(path: &Path) -> Option<CloneInfo> {
    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut req = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: 0,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: libc::ATTR_CMNEXT_PRIVATESIZE
            | libc::ATTR_CMNEXT_CLONEID
            | libc::ATTR_CMNEXT_EXT_FLAGS,
    };
    let mut buf = CloneAttrs {
        len: 0,
        private_size: 0,
        clone_id: 0,
        ext_flags: 0,
    };
    // SAFETY: valid NUL-terminated path, request struct, and a buffer of the size we pass.
    let rc = unsafe {
        libc::getattrlist(
            c_path.as_ptr(),
            (&raw mut req).cast(),
            (&raw mut buf).cast(),
            size_of::<CloneAttrs>(),
            libc::FSOPT_NOFOLLOW | libc::FSOPT_ATTR_CMN_EXTENDED,
        )
    };
    // Unsupported attributes are omitted from the buffer, so a short length means "no clone data"
    // (non-APFS volume).
    let full = rc == 0 && buf.len as usize == size_of::<CloneAttrs>();
    (full && buf.ext_flags & EF_MAY_SHARE_BLOCKS != 0).then(|| CloneInfo {
        id: buf.clone_id,
        private: buf.private_size.max(0) as u64,
    })
}

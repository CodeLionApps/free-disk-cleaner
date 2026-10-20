//! Scan result model: an arena tree of filesystem nodes with aggregated allocated sizes.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub type NodeId = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    File,
    Dir,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeError {
    PermissionDenied,
    Vanished,
    Other(String),
}

#[derive(Debug, Clone)]
pub struct Node {
    /// File name; for the root node, the full root path.
    pub name: OsString,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub kind: NodeKind,
    /// Allocated bytes of this entry itself (`st_blocks * 512`), 0 if already counted (hardlink/clone).
    pub own_size: u64,
    /// `own_size` + `total_size` of all children.
    pub total_size: u64,
    pub mtime: Option<SystemTime>,
    pub atime: Option<SystemTime>,
    /// Newest mtime of this node and all descendants ("last used" for directories).
    pub newest_mtime: Option<SystemTime>,
    pub error: Option<NodeError>,
}

/// Node 0 is always the root.
#[derive(Debug, Clone)]
pub struct ScanTree {
    pub nodes: Vec<Node>,
}

impl ScanTree {
    pub const ROOT: NodeId = 0;

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    pub fn root_path(&self) -> &Path {
        Path::new(&self.nodes[Self::ROOT].name)
    }

    pub fn path(&self, id: NodeId) -> PathBuf {
        let mut parts = Vec::new();
        let mut cur = Some(id);
        while let Some(i) = cur {
            parts.push(&self.nodes[i].name);
            cur = self.nodes[i].parent;
        }
        parts.iter().rev().collect()
    }

    /// Finds the node for an absolute path under the root.
    pub fn find(&self, path: &Path) -> Option<NodeId> {
        let rel = path.strip_prefix(self.root_path()).ok()?;
        let mut cur = Self::ROOT;
        for comp in rel.components() {
            cur = *self.nodes[cur]
                .children
                .iter()
                .find(|&&c| self.nodes[c].name == comp.as_os_str())?;
        }
        Some(cur)
    }
}

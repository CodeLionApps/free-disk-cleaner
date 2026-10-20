//! Safety-rated classification results.
//!
//! Invariant: a "Do not touch" item can never become [`Deletable`]. [`Deletable`] and
//! [`DirectDeleteToken`] have private fields and are only constructed by the classifier in this
//! crate; [`Protected`] has no conversion into either.
//!
//! ```compile_fail
//! fn f(p: fdc_core::safety::Protected) -> fdc_core::safety::Deletable { p.into() }
//! ```
//!
//! ```compile_fail
//! let _ = fdc_core::safety::DirectDeleteToken(());
//! ```
//!
//! A [`Protected`] item offers no way to obtain a [`DirectDeleteToken`]:
//!
//! ```compile_fail
//! use fdc_core::safety::{Classified, DirectDeleteToken};
//! fn f(c: &Classified) -> Option<DirectDeleteToken> {
//!     match c {
//!         Classified::Deletable(d) => d.direct_delete_token(),
//!         Classified::Protected(p) => p.direct_delete_token(),
//!     }
//! }
//! ```
//!
//! Nor can a [`Deletable`] be built from its info outside this crate:
//!
//! ```compile_fail
//! use fdc_core::safety::{Deletable, DeletableStatus, Protected};
//! fn f(p: &Protected) -> Deletable { Deletable::new(p.info().clone(), DeletableStatus::Safe) }
//! ```

use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Status {
    Safe,
    Caution,
    DoNotTouch,
}

/// Status of something that may be deleted. Has no "Do not touch" variant on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeletableStatus {
    Safe,
    Caution,
}

impl From<DeletableStatus> for Status {
    fn from(s: DeletableStatus) -> Self {
        match s {
            DeletableStatus::Safe => Status::Safe,
            DeletableStatus::Caution => Status::Caution,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ToolGroup {
    XcodeIos,
    Android,
    Docker,
    NodeJs,
    HomebrewOther,
    AppsSystem,
}

/// Items removed by a CLI command instead of a path deletion.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CommandTarget {
    DockerBuildCache,
    DockerDanglingImages,
    DockerUnusedImages,
    DockerStoppedContainers,
    DockerVolumes,
    /// `tmutil deletelocalsnapshots <date>`; the string is the snapshot date id.
    TimeMachineSnapshot(String),
    SimctlUnavailableDevices,
    /// `xcrun simctl runtime delete <id>`.
    SimctlRuntime(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Target {
    Path(PathBuf),
    Command(CommandTarget),
}

/// Rule metadata copied onto every classified item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemInfo {
    pub target: Target,
    pub rule_id: &'static str,
    pub group: ToolGroup,
    /// Translation key for "why this status".
    pub rationale_key: &'static str,
    /// Optional translation key for keep/remove hints (e.g. "keep newest snapshot").
    pub hint_key: Option<&'static str>,
    pub size: u64,
    pub last_used: Option<SystemTime>,
    /// Stale `node_modules` (project untouched longer than the configured threshold).
    pub stale: bool,
}

/// Safe or Caution item that may be passed to the cleaner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deletable {
    info: ItemInfo,
    status: DeletableStatus,
}

/// "Do not touch" item. Info only; there is no deletion path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Protected {
    info: ItemInfo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classified {
    Deletable(Deletable),
    Protected(Protected),
}

/// Permission to bypass the Trash. Only obtainable from a Safe, path-based [`Deletable`].
#[derive(Debug)]
pub struct DirectDeleteToken(());

impl Deletable {
    pub(crate) fn new(info: ItemInfo, status: DeletableStatus) -> Self {
        Self { info, status }
    }
    pub fn info(&self) -> &ItemInfo {
        &self.info
    }
    pub fn status(&self) -> DeletableStatus {
        self.status
    }
    pub fn path(&self) -> Option<&Path> {
        match &self.info.target {
            Target::Path(p) => Some(p),
            Target::Command(_) => None,
        }
    }
    /// Caution items and Docker volumes start unselected.
    pub fn selected_by_default(&self) -> bool {
        self.status == DeletableStatus::Safe
            && self.info.target != Target::Command(CommandTarget::DockerVolumes)
    }
    pub fn direct_delete_token(&self) -> Option<DirectDeleteToken> {
        (self.status == DeletableStatus::Safe && self.path().is_some())
            .then_some(DirectDeleteToken(()))
    }
}

impl Protected {
    pub(crate) fn new(info: ItemInfo) -> Self {
        Self { info }
    }
    pub fn info(&self) -> &ItemInfo {
        &self.info
    }
}

impl Classified {
    pub fn info(&self) -> &ItemInfo {
        match self {
            Classified::Deletable(d) => d.info(),
            Classified::Protected(p) => p.info(),
        }
    }
    pub fn status(&self) -> Status {
        match self {
            Classified::Deletable(d) => d.status().into(),
            Classified::Protected(_) => Status::DoNotTouch,
        }
    }
    pub fn as_deletable(&self) -> Option<&Deletable> {
        match self {
            Classified::Deletable(d) => Some(d),
            Classified::Protected(_) => None,
        }
    }
}

//! Deterministic rule table and classifier. No heuristics: a path is either matched by a rule in
//! [`rules()`] or it has no status at all.
//!
//! Matching is "shallowest match wins": walking from the filesystem root down, the first ancestor
//! (or the path itself) that matches any rule decides. A matched directory is one item and is never
//! descended into, so a "Do not touch" area can't be shadowed by a broader rule matching inside it.
//! Among rules matching the same path, table order decides (specific before generic).

use crate::model::{NodeId, ScanTree};
use crate::safety::{
    Classified, CommandTarget, Deletable, DeletableStatus, ItemInfo, Protected, Status, Target,
    ToolGroup,
};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

#[cfg(test)]
mod tests;

/// Anchors that rule patterns are relative to. Real mode: `fs_root = "/"`, `home = $HOME`.
/// Test mode: `fs_root = $FDC_TEST_ROOT`, `home = $FDC_TEST_ROOT/Users/tester`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots {
    pub fs_root: PathBuf,
    pub home: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    FsRoot,
    Home,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Matcher {
    /// Path pattern relative to the anchor; segments may be `*` (one segment) or `**` (any depth).
    Path(Anchor, &'static str),
    /// Command-based item (Docker, tmutil, simctl).
    Command(CommandKind),
}

/// Discriminant of [`CommandTarget`] without the payload, usable in a static table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    DockerBuildCache,
    DockerDanglingImages,
    DockerUnusedImages,
    DockerStoppedContainers,
    DockerVolumes,
    TimeMachineSnapshot,
    SimctlUnavailableDevices,
    SimctlRuntime,
}

impl CommandKind {
    pub fn of(target: &CommandTarget) -> Self {
        match target {
            CommandTarget::DockerBuildCache => Self::DockerBuildCache,
            CommandTarget::DockerDanglingImages => Self::DockerDanglingImages,
            CommandTarget::DockerUnusedImages => Self::DockerUnusedImages,
            CommandTarget::DockerStoppedContainers => Self::DockerStoppedContainers,
            CommandTarget::DockerVolumes => Self::DockerVolumes,
            CommandTarget::TimeMachineSnapshot(_) => Self::TimeMachineSnapshot,
            CommandTarget::SimctlUnavailableDevices => Self::SimctlUnavailableDevices,
            CommandTarget::SimctlRuntime(_) => Self::SimctlRuntime,
        }
    }
}

#[derive(Debug)]
pub struct Rule {
    pub id: &'static str,
    pub group: ToolGroup,
    pub matcher: Matcher,
    pub status: Status,
    pub rationale_key: &'static str,
    pub hint_key: Option<&'static str>,
}

#[derive(Debug, Clone, Copy)]
pub struct ClassifyParams {
    pub stale_node_modules_after: Duration,
    pub now: SystemTime,
}

const NODE_MODULES: &str = "node_modules";
/// Project files whose mtimes decide whether a sibling `node_modules` is stale.
const PROJECT_FILES: [&str; 4] = [
    "package.json",
    "yarn.lock",
    "package-lock.json",
    "pnpm-lock.yaml",
];

macro_rules! rule {
    ($id:literal, $group:ident, $matcher:expr, $status:ident) => {
        rule!($id, $group, $matcher, $status, None)
    };
    ($id:literal, $group:ident, $matcher:expr, $status:ident, $hint:expr) => {
        Rule {
            id: $id,
            group: ToolGroup::$group,
            matcher: $matcher,
            status: Status::$status,
            rationale_key: concat!("rule.", $id, ".why"),
            hint_key: $hint,
        }
    };
}

const fn home(p: &'static str) -> Matcher {
    Matcher::Path(Anchor::Home, p)
}
const fn root(p: &'static str) -> Matcher {
    Matcher::Path(Anchor::FsRoot, p)
}
const fn cmd(k: CommandKind) -> Matcher {
    Matcher::Command(k)
}

// Order: "Do not touch" first, then specific rules, then catch-alls (`Library/Caches/*`,
// `**/node_modules`). Only matters for rules matching the very same path.
#[rustfmt::skip]
static RULES: &[Rule] = &[
    // Do not touch.
    rule!("docker_vm", Docker, home("Library/Containers/com.docker.docker"), DoNotTouch),
    rule!("icloud_drive", AppsSystem, home("Library/Mobile Documents"), DoNotTouch),
    rule!("keychains", AppsSystem, home("Library/Keychains"), DoNotTouch),
    rule!("mail", AppsSystem, home("Library/Mail"), DoNotTouch),
    rule!("messages", AppsSystem, home("Library/Messages"), DoNotTouch),
    rule!("system", AppsSystem, root("System"), DoNotTouch),
    rule!("vm_swap", AppsSystem, root("private/var/vm"), DoNotTouch),
    rule!("var_db", AppsSystem, root("private/var/db"), DoNotTouch),
    rule!("var_folders", AppsSystem, root("private/var/folders"), DoNotTouch),
    rule!("usr", AppsSystem, root("usr"), DoNotTouch),
    rule!("simulator_runtimes", AppsSystem, root("Library/Developer/CoreSimulator/Volumes"), DoNotTouch),
    // Xcode & iOS.
    rule!("derived_data", XcodeIos, home("Library/Developer/Xcode/DerivedData/*"), Safe),
    rule!("xcode_archives", XcodeIos, home("Library/Developer/Xcode/Archives/*"), Caution),
    rule!("ios_device_support", XcodeIos, home("Library/Developer/Xcode/iOS DeviceSupport/*"), Safe),
    rule!("watchos_device_support", XcodeIos, home("Library/Developer/Xcode/watchOS DeviceSupport/*"), Safe),
    rule!("simulator_caches", XcodeIos, home("Library/Developer/CoreSimulator/Caches"), Safe),
    rule!("simulator_devices", XcodeIos, home("Library/Developer/CoreSimulator/Devices/*"), Caution),
    rule!("xcode_cache", XcodeIos, home("Library/Caches/com.apple.dt.Xcode"), Safe),
    rule!("xcode_previews", XcodeIos, home("Library/Developer/Xcode/UserData/Previews"), Safe),
    // Android.
    rule!("gradle_caches", Android, home(".gradle/caches"), Safe),
    rule!("gradle_wrapper", Android, home(".gradle/wrapper/dists"), Safe),
    rule!("android_avd", Android, home(".android/avd/*"), Caution),
    rule!("android_system_images", Android, home("Library/Android/sdk/system-images/*"), Caution),
    // Docker (CLI only).
    rule!("docker_build_cache", Docker, cmd(CommandKind::DockerBuildCache), Safe),
    rule!("docker_dangling_images", Docker, cmd(CommandKind::DockerDanglingImages), Safe),
    rule!("docker_unused_images", Docker, cmd(CommandKind::DockerUnusedImages), Caution),
    rule!("docker_stopped_containers", Docker, cmd(CommandKind::DockerStoppedContainers), Caution),
    rule!("docker_volumes", Docker, cmd(CommandKind::DockerVolumes), Caution),
    // Node/JS.
    rule!("npm_cache", NodeJs, home(".npm/_cacache"), Safe),
    rule!("yarn_cache", NodeJs, home("Library/Caches/Yarn"), Safe),
    rule!("yarn_berry_cache", NodeJs, home(".yarn/berry/cache"), Safe),
    rule!("pnpm_store", NodeJs, home("Library/pnpm/store"), Safe),
    // Homebrew & other.
    rule!("homebrew_cache", HomebrewOther, home("Library/Caches/Homebrew"), Safe),
    rule!("cocoapods_cache", HomebrewOther, home("Library/Caches/CocoaPods"), Safe),
    rule!("cocoapods_repos", HomebrewOther, home(".cocoapods/repos"), Safe),
    // Apps & system.
    rule!("user_logs", AppsSystem, home("Library/Logs"), Safe),
    rule!("diagnostic_reports", AppsSystem, root("Library/Logs/DiagnosticReports"), Caution),
    rule!("tm_snapshots", AppsSystem, cmd(CommandKind::TimeMachineSnapshot), Caution, Some("hint.tm_keep_newest")),
    rule!("simctl_runtimes", AppsSystem, cmd(CommandKind::SimctlRuntime), Caution, Some("hint.runtime_keep_current")),
    rule!("simctl_unavailable_devices", AppsSystem, cmd(CommandKind::SimctlUnavailableDevices), Safe),
    // Catch-alls: must stay after every specific rule above.
    rule!("system_caches", AppsSystem, root("Library/Caches/*"), Caution),
    rule!("user_caches", AppsSystem, home("Library/Caches/*"), Caution),
    rule!("node_modules", NodeJs, home("**/node_modules"), Safe),
];

pub fn rules() -> &'static [Rule] {
    RULES
}

/// Every translation key referenced by the rule table (rationales and hints), deduplicated.
pub fn all_translation_keys() -> Vec<&'static str> {
    let mut keys = Vec::new();
    for k in RULES
        .iter()
        .flat_map(|r| std::iter::once(r.rationale_key).chain(r.hint_key))
    {
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    keys
}

/// Rule deciding `path` (absolute): the first rule matching the shallowest ancestor-or-self, or
/// `None` if unrecognized. A path inside a matched item reports that item's rule, so anything under
/// a "Do not touch" area is "Do not touch".
pub fn match_path(path: &Path, roots: &Roots) -> Option<&'static Rule> {
    path.ancestors()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .find_map(|p| match_exact(p, roots))
}

/// First rule whose pattern matches exactly `path` (ancestors not considered).
fn match_exact(path: &Path, roots: &Roots) -> Option<&'static Rule> {
    RULES.iter().find(|r| match r.matcher {
        Matcher::Path(anchor, pattern) => {
            let base = match anchor {
                Anchor::FsRoot => &roots.fs_root,
                Anchor::Home => &roots.home,
            };
            let Ok(rel) = path.strip_prefix(base) else {
                return false;
            };
            // `..`/`.` would let `*` match outside the intended area.
            let segs: Option<Vec<&OsStr>> = rel
                .components()
                .map(|c| match c {
                    Component::Normal(s) => Some(s),
                    _ => None,
                })
                .collect();
            let pat: Vec<&str> = pattern.split('/').collect();
            segs.is_some_and(|s| glob(&pat, &s))
        }
        Matcher::Command(_) => false,
    })
}

fn glob(pat: &[&str], segs: &[&OsStr]) -> bool {
    match (pat.first(), segs.first()) {
        (None, None) => true,
        (Some(&"**"), _) => glob(&pat[1..], segs) || (!segs.is_empty() && glob(pat, &segs[1..])),
        (Some(&p), Some(&s)) => (p == "*" || s == p) && glob(&pat[1..], &segs[1..]),
        _ => false,
    }
}

/// Classifies every matched node; matched nodes are not descended into. Unmatched nodes are absent.
/// Output is in tree (pre-order) order.
pub fn classify_tree(tree: &ScanTree, roots: &Roots, params: &ClassifyParams) -> Vec<Classified> {
    let mut out = Vec::new();
    let root = tree.root_path().to_path_buf();
    // The root may itself lie inside a matched area (e.g. scanning a folder in iCloud Drive).
    if let Some(rule) = match_path(&root, roots) {
        out.push(item(tree, ScanTree::ROOT, root, rule, params));
        return out;
    }
    let mut stack: Vec<(NodeId, PathBuf)> = tree
        .node(ScanTree::ROOT)
        .children
        .iter()
        .rev()
        .map(|&c| (c, root.join(&tree.node(c).name)))
        .collect();
    while let Some((id, path)) = stack.pop() {
        if let Some(rule) = match_exact(&path, roots) {
            out.push(item(tree, id, path, rule, params));
            continue;
        }
        stack.extend(
            tree.node(id)
                .children
                .iter()
                .rev()
                .map(|&c| (c, path.join(&tree.node(c).name))),
        );
    }
    out
}

fn item(
    tree: &ScanTree,
    id: NodeId,
    path: PathBuf,
    rule: &'static Rule,
    params: &ClassifyParams,
) -> Classified {
    let node = tree.node(id);
    let stale = rule.id == NODE_MODULES && is_stale(tree, id, params);
    build(
        rule,
        Target::Path(path),
        node.total_size,
        node.newest_mtime,
        stale,
    )
}

/// Stale when every project file next to `node_modules` (package.json + lockfiles that exist, or
/// `node_modules` itself if none do) is older than `now - stale_node_modules_after`.
fn is_stale(tree: &ScanTree, nm: NodeId, params: &ClassifyParams) -> bool {
    let Some(cutoff) = params.now.checked_sub(params.stale_node_modules_after) else {
        return false;
    };
    let mut mtimes: Vec<SystemTime> = tree
        .node(nm)
        .parent
        .map(|p| &tree.node(p).children)
        .into_iter()
        .flatten()
        .map(|&c| tree.node(c))
        .filter(|n| PROJECT_FILES.iter().any(|f| n.name == *f))
        .filter_map(|n| n.mtime)
        .collect();
    if mtimes.is_empty() {
        mtimes.extend(tree.node(nm).newest_mtime);
    }
    !mtimes.is_empty() && mtimes.iter().all(|&t| t < cutoff)
}

/// Classifies a command-based item (Docker, tmutil, simctl).
pub fn classify_command(target: CommandTarget, size: u64) -> Classified {
    let kind = CommandKind::of(&target);
    let rule = RULES
        .iter()
        .find(|r| r.matcher == Matcher::Command(kind))
        .expect("every CommandKind has a rule");
    build(rule, Target::Command(target), size, None, false)
}

/// The only place a rule status becomes a [`Classified`]; DoNotTouch can only yield [`Protected`].
fn build(
    rule: &'static Rule,
    target: Target,
    size: u64,
    last_used: Option<SystemTime>,
    stale: bool,
) -> Classified {
    let info = ItemInfo {
        target,
        rule_id: rule.id,
        group: rule.group,
        rationale_key: rule.rationale_key,
        hint_key: rule.hint_key,
        size,
        last_used,
        stale,
    };
    match rule.status {
        Status::DoNotTouch => Classified::Protected(Protected::new(info)),
        Status::Safe => Classified::Deletable(Deletable::new(info, DeletableStatus::Safe)),
        Status::Caution => Classified::Deletable(Deletable::new(info, DeletableStatus::Caution)),
    }
}

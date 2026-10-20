//! UI-independent app state: groups, table rows, sorting, selection, drill-down.

use crate::settings::Settings;
use fdc_core::model::{NodeId, ScanTree};
use fdc_core::safety::{Classified, CommandTarget, Deletable, Status, Target, ToolGroup};
use fdc_core::treemap::{Rect, squarify};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const GROUPS: [&str; 9] = [
    "xcode",
    "android",
    "docker",
    "node",
    "brew",
    "sys-removable",
    "sys-critical",
    "stale",
    "all",
];

pub const SORT_NAME: i32 = 0;
pub const SORT_SIZE: i32 = 1;
pub const SORT_DATE: i32 = 2;
pub const SORT_STATUS: i32 = 3;

/// Status code shared with the UI: 0 Safe, 1 Caution, 2 Do not touch, 3 unrecognized.
pub fn status_code(s: Option<Status>) -> i32 {
    match s {
        Some(Status::Safe) => 0,
        Some(Status::Caution) => 1,
        Some(Status::DoNotTouch) => 2,
        None => 3,
    }
}

pub fn group_of(c: &Classified) -> &'static str {
    match c.info().group {
        ToolGroup::XcodeIos => "xcode",
        ToolGroup::Android => "android",
        ToolGroup::Docker => "docker",
        ToolGroup::NodeJs => "node",
        ToolGroup::HomebrewOther => "brew",
        ToolGroup::AppsSystem if c.status() == Status::DoNotTouch => "sys-critical",
        ToolGroup::AppsSystem => "sys-removable",
    }
}

/// UI code for a command item kind plus its instance suffix (snapshot date, runtime id).
pub fn command_kind(t: &CommandTarget) -> (&'static str, String) {
    match t {
        CommandTarget::DockerBuildCache => ("docker_build_cache", String::new()),
        CommandTarget::DockerDanglingImages => ("docker_dangling_images", String::new()),
        CommandTarget::DockerUnusedImages => ("docker_unused_images", String::new()),
        CommandTarget::DockerStoppedContainers => ("docker_stopped_containers", String::new()),
        CommandTarget::DockerVolumes => ("docker_volumes", String::new()),
        CommandTarget::TimeMachineSnapshot(d) => ("tm_snapshot", d.clone()),
        CommandTarget::SimctlUnavailableDevices => ("simctl_unavailable", String::new()),
        CommandTarget::SimctlRuntime(id) => ("simctl_runtime", id.clone()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowSrc {
    Item(usize),
    Node(NodeId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RowData {
    pub src: RowSrc,
    pub item: Option<usize>,
    pub kind: &'static str,
    pub name: String,
    pub path: String,
    pub size: u64,
    pub last_used: Option<SystemTime>,
    pub rationale_key: &'static str,
    pub hint_key: &'static str,
    pub status: i32,
    pub selectable: bool,
    pub selected: bool,
    pub large: bool,
    pub stale: bool,
    pub command: String,
    pub can_open: bool,
}

pub struct State {
    pub tree: Option<ScanTree>,
    pub items: Vec<Classified>,
    pub removed: HashSet<usize>,
    node_item: HashMap<NodeId, usize>,
    pub selected: BTreeSet<usize>,
    pub group: &'static str,
    pub browse: Vec<NodeId>,
    pub sort_col: i32,
    pub sort_asc: bool,
    pub rows: Vec<RowData>,
    pub settings: Settings,
    pub docker_running: bool,
    /// Formats the exact command line shown before running a command item.
    pub command_line: fn(&CommandTarget) -> String,
    /// Paths under home are shown as `~/...`.
    pub home: PathBuf,
}

impl State {
    pub fn new(
        settings: Settings,
        command_line: fn(&CommandTarget) -> String,
        home: PathBuf,
    ) -> Self {
        Self {
            tree: None,
            items: Vec::new(),
            removed: HashSet::new(),
            node_item: HashMap::new(),
            selected: BTreeSet::new(),
            group: GROUPS[0],
            browse: Vec::new(),
            sort_col: SORT_SIZE,
            sort_asc: false,
            rows: Vec::new(),
            settings,
            docker_running: true,
            command_line,
            home,
        }
    }

    pub fn set_results(
        &mut self,
        tree: Option<ScanTree>,
        items: Vec<Classified>,
        docker_running: bool,
    ) {
        self.node_item.clear();
        if let Some(t) = &tree {
            for (i, c) in items.iter().enumerate() {
                if let Target::Path(p) = &c.info().target
                    && let Some(n) = t.find(p)
                {
                    self.node_item.insert(n, i);
                }
            }
        }
        self.selected = items
            .iter()
            .enumerate()
            .filter(|(_, c)| c.as_deletable().is_some_and(Deletable::selected_by_default))
            .map(|(i, _)| i)
            .collect();
        self.tree = tree;
        self.items = items;
        self.removed.clear();
        self.docker_running = docker_running;
        self.browse.clear();
        self.group = self.first_nonempty_group();
        self.rebuild();
    }

    fn live_items(&self) -> impl Iterator<Item = (usize, &Classified)> {
        self.items
            .iter()
            .enumerate()
            .filter(|(i, _)| !self.removed.contains(i))
    }

    fn in_group(&self, key: &str, c: &Classified) -> bool {
        match key {
            "stale" => c.info().stale,
            "all" => false,
            k => group_of(c) == k,
        }
    }

    fn first_nonempty_group(&self) -> &'static str {
        GROUPS
            .into_iter()
            .find(|g| self.live_items().any(|(_, c)| self.in_group(g, c)))
            .unwrap_or("all")
    }

    /// (key, total bytes, item count) per group; "all" = whole scan.
    pub fn groups(&self) -> Vec<(&'static str, u64, usize)> {
        GROUPS
            .into_iter()
            .map(|g| {
                if g == "all" {
                    let total = self
                        .tree
                        .as_ref()
                        .map_or(0, |t| t.node(ScanTree::ROOT).total_size);
                    return (g, total, 0);
                }
                let (mut size, mut n) = (0, 0);
                for (_, c) in self.live_items().filter(|(_, c)| self.in_group(g, c)) {
                    size += c.info().size;
                    n += 1;
                }
                (g, size, n)
            })
            .collect()
    }

    pub fn group_note(&self) -> &'static str {
        match self.group {
            _ if !self.browse.is_empty() => "",
            "docker" if !self.docker_running => "docker-not-running",
            "sys-critical" => "sys-critical",
            "sys-removable" => "sys-removable",
            "stale" => "stale",
            _ => "",
        }
    }

    pub fn select_group(&mut self, key: &str) {
        if let Some(g) = GROUPS.into_iter().find(|g| *g == key) {
            self.group = g;
            self.browse.clear();
            self.rebuild();
        }
    }

    pub fn open(&mut self, row: usize) {
        let node = match self.rows.get(row).map(|r| r.src) {
            Some(RowSrc::Node(n)) => Some(n),
            Some(RowSrc::Item(i)) => self.item_node(i),
            None => None,
        };
        if let Some(n) = node.filter(|&n| {
            self.tree
                .as_ref()
                .is_some_and(|t| !t.node(n).children.is_empty())
        }) {
            self.browse.push(n);
            self.rebuild();
        }
    }

    pub fn up(&mut self) {
        self.browse.pop();
        self.rebuild();
    }

    pub fn toggle(&mut self, row: usize) {
        let Some(r) = self.rows.get_mut(row).filter(|r| r.selectable) else {
            return;
        };
        let i = r.item.expect("selectable rows are items");
        r.selected = !r.selected;
        if r.selected {
            self.selected.insert(i);
        } else {
            self.selected.remove(&i);
        }
    }

    pub fn select_stale(&mut self) {
        let stale: Vec<usize> = self
            .live_items()
            .filter(|(_, c)| c.info().stale && c.as_deletable().is_some())
            .map(|(i, _)| i)
            .collect();
        self.selected.extend(stale);
        self.rebuild();
    }

    pub fn sort(&mut self, col: i32) {
        if self.sort_col == col {
            self.sort_asc = !self.sort_asc;
        } else {
            self.sort_col = col;
            self.sort_asc = col == SORT_NAME || col == SORT_STATUS;
        }
        self.sort_rows();
    }

    /// Selected, still-present deletable items.
    pub fn selection(&self) -> Vec<(usize, &Deletable)> {
        self.selected
            .iter()
            .filter(|i| !self.removed.contains(i))
            .filter_map(|&i| self.items[i].as_deletable().map(|d| (i, d)))
            .collect()
    }

    pub fn selected_total(&self) -> (usize, u64) {
        let s = self.selection();
        (s.len(), s.iter().map(|(_, d)| d.info().size).sum())
    }

    pub fn breadcrumb(&self) -> Vec<String> {
        let Some(t) = &self.tree else {
            return Vec::new();
        };
        self.browse
            .iter()
            .map(|&n| self.display(&t.path(n)))
            .collect()
    }

    pub fn mark_removed(&mut self, done: impl IntoIterator<Item = usize>) {
        for i in done {
            self.removed.insert(i);
            self.selected.remove(&i);
        }
        self.browse.clear();
        self.rebuild();
    }

    pub fn display(&self, p: &Path) -> String {
        match p.strip_prefix(&self.home) {
            Ok(rel) if rel.as_os_str().is_empty() => "~".into(),
            Ok(rel) => format!("~/{}", rel.display()),
            Err(_) => p.display().to_string(),
        }
    }

    fn item_node(&self, i: usize) -> Option<NodeId> {
        let t = self.tree.as_ref()?;
        match &self.items[i].info().target {
            Target::Path(p) => t.find(p),
            Target::Command(_) => None,
        }
    }

    fn item_row(&self, i: usize, src: RowSrc) -> RowData {
        let c = &self.items[i];
        let info = c.info();
        let deletable = c.as_deletable().is_some();
        let (kind, name, path, command) = match &info.target {
            Target::Path(p) => {
                let file = p
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default();
                // "node_modules" alone says nothing; show which project it belongs to.
                let name = match p.parent().and_then(|d| d.file_name()) {
                    Some(parent) if file == "node_modules" => {
                        format!("{}/{file}", parent.to_string_lossy())
                    }
                    _ => file,
                };
                ("", name, self.display(p), String::new())
            }
            Target::Command(t) => {
                let (kind, suffix) = command_kind(t);
                (kind, suffix, String::new(), (self.command_line)(t))
            }
        };
        let can_open = self.item_node(i).is_some_and(|n| {
            self.tree
                .as_ref()
                .is_some_and(|t| !t.node(n).children.is_empty())
        });
        RowData {
            src,
            item: Some(i),
            kind,
            name,
            path,
            size: info.size,
            last_used: info.last_used,
            rationale_key: info.rationale_key,
            hint_key: info.hint_key.unwrap_or(""),
            status: status_code(Some(c.status())),
            selectable: deletable,
            selected: deletable && self.selected.contains(&i),
            large: deletable && info.size > self.settings.large_item_bytes(),
            stale: info.stale,
            command,
            can_open,
        }
    }

    fn node_row(&self, n: NodeId) -> RowData {
        let t = self.tree.as_ref().expect("browsing needs a tree");
        if let Some(&i) = self.node_item.get(&n) {
            return self.item_row(i, RowSrc::Node(n));
        }
        let node = t.node(n);
        // Inside a classified item: inherit its rating for display, never selectable on its own.
        let mut anc = node.parent;
        let mut inherited = None;
        while let Some(a) = anc {
            if let Some(&i) = self.node_item.get(&a) {
                inherited = Some(&self.items[i]);
                break;
            }
            anc = t.node(a).parent;
        }
        RowData {
            src: RowSrc::Node(n),
            item: None,
            kind: "",
            name: node.name.to_string_lossy().into_owned(),
            path: self.display(&t.path(n)),
            size: node.total_size,
            last_used: node.newest_mtime,
            rationale_key: inherited.map_or("", |c| c.info().rationale_key),
            hint_key: "",
            status: status_code(inherited.map(Classified::status)),
            selectable: false,
            selected: false,
            large: false,
            stale: false,
            command: String::new(),
            can_open: !node.children.is_empty(),
        }
    }

    pub fn rebuild(&mut self) {
        self.rows = match (self.browse.last(), &self.tree) {
            (Some(&n), Some(t)) => t
                .node(n)
                .children
                .iter()
                .filter(|c| {
                    self.node_item
                        .get(c)
                        .is_none_or(|i| !self.removed.contains(i))
                })
                .map(|&c| self.node_row(c))
                .collect(),
            _ if self.group == "all" => match &self.tree {
                Some(t) => t
                    .node(ScanTree::ROOT)
                    .children
                    .iter()
                    .map(|&c| self.node_row(c))
                    .collect(),
                None => Vec::new(),
            },
            _ => self
                .live_items()
                .filter(|(_, c)| self.in_group(self.group, c))
                .map(|(i, _)| self.item_row(i, RowSrc::Item(i)))
                .collect(),
        };
        self.sort_rows();
    }

    fn sort_rows(&mut self) {
        let (col, asc) = (self.sort_col, self.sort_asc);
        self.rows.sort_by(|a, b| {
            let o = match col {
                SORT_SIZE => a.size.cmp(&b.size),
                SORT_DATE => a.last_used.cmp(&b.last_used),
                SORT_STATUS => a.status.cmp(&b.status),
                _ => std::cmp::Ordering::Equal,
            };
            let o = o.then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            if asc { o } else { o.reverse() }
        });
    }

    /// Treemap of the current rows in a `w`×`h` area: (row index, rect).
    pub fn map_rects(&self, w: f32, h: f32) -> Vec<(usize, Rect)> {
        let sizes: Vec<u64> = self.rows.iter().map(|r| r.size).collect();
        squarify(
            &sizes,
            Rect {
                x: 0.0,
                y: 0.0,
                w,
                h,
            },
        )
        .into_iter()
        .enumerate()
        .filter(|(_, r)| r.w > 0.5 && r.h > 0.5)
        .collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use fdc_core::rules::{ClassifyParams, Roots, classify_command, classify_tree};
    use fdc_core::scan::{ScanOptions, scan};
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    /// Scans a freshly generated fixture tree plus a few command items.
    pub fn fixture_state(settings: Settings) -> (tempfile::TempDir, State) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap().join("fx");
        fdc_fixtures::generate(&root).unwrap();
        let roots = Roots {
            fs_root: root.clone(),
            home: root.join(fdc_fixtures::HOME_REL),
        };
        let tree = scan(
            &root,
            &ScanOptions::default(),
            &|_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        let params = ClassifyParams {
            stale_node_modules_after: Duration::from_secs(30 * 86_400),
            now: SystemTime::now(),
        };
        let mut items = classify_tree(&tree, &roots, &params);
        items.push(classify_command(
            CommandTarget::DockerBuildCache,
            3_000_000_000,
        ));
        items.push(classify_command(
            CommandTarget::DockerVolumes,
            2_000_000_000,
        ));
        items.push(classify_command(
            CommandTarget::TimeMachineSnapshot("2026-10-01-120000".into()),
            0,
        ));
        let mut st = State::new(settings, |t| format!("cmd {t:?}"), roots.home.clone());
        st.set_results(Some(tree), items, true);
        (dir, st)
    }

    fn row_named<'a>(st: &'a State, name: &str) -> (usize, &'a RowData) {
        st.rows
            .iter()
            .enumerate()
            .find(|(_, r)| r.name == name)
            .unwrap_or_else(|| panic!("no row {name}"))
    }

    #[test]
    fn caution_and_volumes_start_unselected_safe_selected() {
        let (_d, st) = fixture_state(Settings::default());
        for (i, c) in st.items.iter().enumerate() {
            let selected = st.selected.contains(&i);
            match c.status() {
                Status::Safe => assert_eq!(
                    selected,
                    c.info().target != Target::Command(CommandTarget::DockerVolumes)
                ),
                Status::Caution | Status::DoNotTouch => assert!(!selected, "{:?}", c.info().target),
            }
        }
        assert!(
            !st.selected.iter().any(
                |&i| st.items[i].info().target == Target::Command(CommandTarget::DockerVolumes)
            )
        );
    }

    #[test]
    fn groups_rows_badges_and_protected_rows_not_selectable() {
        let (_d, mut st) = fixture_state(Settings::default());
        st.select_group("sys-critical");
        assert!(!st.rows.is_empty());
        assert!(
            st.rows
                .iter()
                .all(|r| r.status == 2 && !r.selectable && !r.selected)
        );
        assert_eq!(st.group_note(), "sys-critical");
        st.select_group("xcode");
        let (_, r) = row_named(&st, "AppA-abc123");
        assert_eq!((r.status, r.selectable, r.selected), (0, true, true));
        assert!(r.path.starts_with("~/Library/Developer/Xcode/DerivedData"));
        let (_, r) = row_named(&st, "2026-01-15");
        assert_eq!((r.status, r.selectable, r.selected), (1, true, false));
        st.select_group("docker");
        let (_, r) = row_named(&st, "");
        assert!(r.command.starts_with("cmd "));
    }

    #[test]
    fn sorting_by_size_name_date_status() {
        let (_d, mut st) = fixture_state(Settings::default());
        st.select_group("xcode");
        assert_eq!((st.sort_col, st.sort_asc), (SORT_SIZE, false));
        assert!(st.rows.windows(2).all(|w| w[0].size >= w[1].size));
        st.sort(SORT_SIZE);
        assert!(st.rows.windows(2).all(|w| w[0].size <= w[1].size));
        st.sort(SORT_NAME);
        assert!(
            st.rows
                .windows(2)
                .all(|w| w[0].name.to_lowercase() <= w[1].name.to_lowercase())
        );
        st.sort(SORT_STATUS);
        assert!(st.rows.windows(2).all(|w| w[0].status <= w[1].status));
        st.sort(SORT_DATE);
        st.sort(SORT_DATE);
        assert!(st.rows.windows(2).all(|w| w[0].last_used <= w[1].last_used));
    }

    #[test]
    fn toggle_drill_down_and_stale_bulk_selection() {
        let (_d, mut st) = fixture_state(Settings::default());
        st.select_group("xcode");
        let (i, _) = row_named(&st, "AppA-abc123");
        let before = st.selected_total();
        st.toggle(i);
        assert_eq!(st.selected_total().0, before.0 - 1);
        st.open(i);
        assert_eq!(st.browse.len(), 1);
        assert!(st.breadcrumb()[0].ends_with("AppA-abc123"));
        // Children inherit the rating for display but are never selectable themselves.
        assert!(st.rows.iter().all(|r| r.status == 0 && !r.selectable));
        st.up();
        assert!(st.browse.is_empty());

        st.select_group("stale");
        assert!(!st.rows.is_empty() && st.rows.iter().all(|r| r.stale));
        st.select_group("node");
        let fresh = st.rows.iter().filter(|r| !r.stale).count();
        assert!(fresh > 0, "fixtures have a fresh project too");
        for i in 0..st.rows.len() {
            if st.rows[i].selected {
                st.toggle(i);
            }
        }
        st.select_stale();
        assert!(
            st.selection()
                .iter()
                .filter(|(_, d)| d.info().rule_id == "node_modules")
                .all(|(_, d)| d.info().stale)
        );
    }

    #[test]
    fn browse_all_and_map() {
        let (_d, mut st) = fixture_state(Settings::default());
        st.select_group("all");
        assert!(row_named(&st, "Users").1.can_open);
        let (i, _) = row_named(&st, "System");
        assert_eq!(st.rows[i].status, 2);
        let rects = st.map_rects(800.0, 600.0);
        assert!(!rects.is_empty());
        assert!(
            rects.iter().all(|(_, r)| r.x >= 0.0
                && r.y >= 0.0
                && r.x + r.w <= 800.01
                && r.y + r.h <= 600.01)
        );
        st.open(row_named(&st, "Users").0);
        assert_eq!(st.rows.len(), 1);
    }

    #[test]
    fn mark_removed_drops_items() {
        let (_d, mut st) = fixture_state(Settings::default());
        st.select_group("xcode");
        let n = st.rows.len();
        let idx = st.rows[0].item.unwrap();
        st.mark_removed([idx]);
        assert_eq!(st.rows.len(), n - 1);
        assert!(!st.selected.contains(&idx));
    }
}

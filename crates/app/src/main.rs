mod fda;
mod format;
mod plan;
mod settings;
mod state;

use anyhow::Context;
use fdc_cleaner::{
    CleanError, Cleaner, CommandRunner, CommandSpec, Method, MockRunner, MockTrasher, Outcome,
    RealFs, SystemRunner, SystemTrash, Trasher, command_for,
};
use fdc_core::rules::{ClassifyParams, Roots, classify_tree};
use fdc_core::safety::{Classified, Deletable};
use fdc_core::scan::{ScanEvent, ScanOptions, scan};
use plan::Plan;
use settings::Settings;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use state::State;
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

slint::include_modules!();

thread_local! {
    // UI-thread state; worker threads reach it through `upgrade_in_event_loop`.
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    /// Pending cleanup plan and per-row "large item confirmed" flags.
    static PLAN: RefCell<Option<(Plan, Vec<bool>)>> = const { RefCell::new(None) };
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(s.borrow_mut().as_mut().expect("state initialized in main")))
}

type Runner = Arc<dyn CommandRunner + Send + Sync>;

/// Everything that differs between real mode and test mode (`FDC_TEST_ROOT`).
#[derive(Clone)]
struct Env {
    test_root: Option<PathBuf>,
    roots: Roots,
    scan_skip: Vec<PathBuf>,
    settings_path: PathBuf,
    runner: Runner,
    trasher: Arc<dyn Trasher + Send + Sync>,
    /// Test mode only: lets E2E checks read back which commands ran.
    mock_runner: Option<Arc<MockRunner>>,
}

impl Env {
    fn detect() -> anyhow::Result<Self> {
        if let Some(root) = std::env::var_os("FDC_TEST_ROOT") {
            let root =
                std::fs::canonicalize(PathBuf::from(root)).context("FDC_TEST_ROOT must exist")?;
            let mock = Arc::new(MockRunner::test_mode());
            return Ok(Self {
                roots: Roots {
                    fs_root: root.clone(),
                    home: root.join(fdc_fixtures::HOME_REL),
                },
                scan_skip: vec![root.join(".fdc-trash")],
                settings_path: settings::settings_path(Some(&root)),
                runner: mock.clone(),
                trasher: Arc::new(MockTrasher::new(root.join(".fdc-trash"))),
                mock_runner: Some(mock),
                test_root: Some(root),
            });
        }
        let home = directories::BaseDirs::new()
            .context("no home directory")?
            .home_dir()
            .to_path_buf();
        Ok(Self {
            roots: Roots {
                fs_root: "/".into(),
                home,
            },
            // Firmlinked data volume (duplicates /Users etc.), external volumes, devices.
            scan_skip: ["/System/Volumes", "/Volumes", "/dev"]
                .map(PathBuf::from)
                .to_vec(),
            settings_path: settings::settings_path(None),
            runner: Arc::new(SystemRunner),
            trasher: Arc::new(SystemTrash),
            mock_runner: None,
            test_root: None,
        })
    }

    fn has_full_disk_access(&self) -> bool {
        match &self.test_root {
            // Test mode: FDA is "denied" while this marker exists.
            Some(root) => !root.join(".fdc-fda-denied").exists(),
            None => fda::has_full_disk_access(&fda::probe_paths(&self.roots.home), fda::real_read),
        }
    }
}

fn apply_language(lang: &str) {
    let lang = match lang {
        "pl" | "en" => lang.to_string(),
        _ => sys_locale::get_locale().unwrap_or_default(),
    };
    let lang = if lang.starts_with("pl") { "pl" } else { "en" };
    if let Err(e) = slint::select_bundled_translation(lang) {
        tracing::warn!("translation {lang}: {e}");
    }
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

fn refresh(ui: &AppWindow, st: &State) {
    ui.set_groups(model(
        st.groups()
            .into_iter()
            .filter(|(k, _, n)| *n > 0 || matches!(*k, "all" | "docker"))
            .map(|(k, size, n)| GroupRow {
                key: k.into(),
                size: format::size(size).into(),
                count: n as i32,
                critical: k == "sys-critical",
            })
            .collect(),
    ));
    ui.set_current_group(st.group.into());
    ui.set_rows(model(
        st.rows
            .iter()
            .enumerate()
            .map(|(i, r)| ItemRow {
                id: i as i32,
                kind: r.kind.into(),
                name: r.name.as_str().into(),
                path: r.path.as_str().into(),
                size: format::size(r.size).into(),
                last_used: format::date(r.last_used).into(),
                rationale_key: r.rationale_key.into(),
                hint_key: r.hint_key.into(),
                status: r.status,
                selectable: r.selectable,
                selected: r.selected,
                large: r.large,
                stale: r.stale,
                command: r.command.as_str().into(),
                can_open: r.can_open,
            })
            .collect(),
    ));
    ui.set_breadcrumb(model(
        st.breadcrumb()
            .into_iter()
            .map(SharedString::from)
            .collect(),
    ));
    ui.set_group_note(st.group_note().into());
    ui.set_sort_column(st.sort_col);
    ui.set_sort_ascending(st.sort_asc);
    ui.set_show_stale_actions(st.group == "stale" && st.browse.is_empty());
    refresh_selection(ui, st);
    refresh_map(ui, st);
}

fn refresh_selection(ui: &AppWindow, st: &State) {
    let (n, bytes) = st.selected_total();
    ui.set_selected_count(n as i32);
    ui.set_selected_size(format::size(bytes).into());
    ui.set_can_clean(n > 0);
}

fn refresh_map(ui: &AppWindow, st: &State) {
    if !ui.get_map_mode() {
        return;
    }
    let rects = st
        .map_rects(ui.get_map_width(), ui.get_map_height())
        .into_iter()
        .map(|(i, r)| {
            let row = &st.rows[i];
            MapRect {
                id: i as i32,
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
                name: row.name.as_str().into(),
                size: format::size(row.size).into(),
                status: row.status,
                can_open: row.can_open,
            }
        })
        .collect();
    ui.set_map_rects(model(rects));
}

fn error_code(e: &CleanError) -> &'static str {
    match e {
        CleanError::Vanished(_) => "vanished",
        CleanError::Symlink(_) => "symlink",
        CleanError::RuleMismatch { .. } => "rule-changed",
        CleanError::ProtectedAncestor { .. } => "protected",
        CleanError::Escapes(_) => "outside",
        CleanError::CommandFailed { .. } => "command-failed",
        _ => "failed",
    }
}

struct Job {
    index: usize,
    item: Deletable,
    direct: bool,
}

fn clean_one(
    cleaner: &Cleaner<RealFs, Arc<dyn Trasher + Send + Sync>, Runner>,
    job: &Job,
) -> Result<Outcome, CleanError> {
    if job.item.path().is_none() {
        return cleaner.run_command(&job.item);
    }
    let method = match job.item.direct_delete_token() {
        Some(token) if job.direct => Method::Direct(token),
        _ => Method::Trash,
    };
    cleaner.delete(&job.item, method)
}

fn start_scan(ui: &AppWindow, env: &Env, cancel: &Arc<AtomicBool>) {
    if ui.get_scanning() {
        return;
    }
    cancel.store(false, Ordering::Relaxed);
    ui.set_scanning(true);
    ui.set_scan_error("".into());
    ui.set_scan_files(0);
    ui.set_scan_bytes(format::size(0).into());
    ui.set_scan_current("".into());
    ui.set_page(Page::Home);
    let stale_days = with_state(|st| st.settings.stale_node_modules_days);
    let (env, weak, cancel) = (env.clone(), ui.as_weak(), cancel.clone());
    std::thread::spawn(move || {
        let progress = {
            let weak = weak.clone();
            move |ev: ScanEvent| {
                let ScanEvent::Progress {
                    files,
                    bytes,
                    current,
                } = ev;
                let current = current.display().to_string();
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    ui.set_scan_files(files.min(i32::MAX as u64) as i32);
                    ui.set_scan_bytes(format::size(bytes).into());
                    ui.set_scan_current(current.into());
                });
            }
        };
        let opts = ScanOptions {
            skip: env.scan_skip.clone(),
            threads: None,
        };
        let result = scan(&env.roots.fs_root, &opts, &progress, &cancel).map(|tree| {
            let params = ClassifyParams {
                stale_node_modules_after: Duration::from_secs(u64::from(stale_days) * 86_400),
                now: SystemTime::now(),
            };
            let mut items = classify_tree(&tree, &env.roots, &params);
            let docker = fdc_cleaner::docker_items(env.runner.as_ref());
            let docker_running = docker.is_ok();
            let listings: [Result<Vec<Classified>, _>; 3] = [
                docker,
                fdc_cleaner::tmutil_items(env.runner.as_ref()),
                fdc_cleaner::simctl_items(env.runner.as_ref()),
            ];
            for r in listings {
                match r {
                    Ok(v) => items.extend(v),
                    Err(e) => tracing::info!("listing skipped: {e}"),
                }
            }
            (tree, items, docker_running)
        });
        let _ = weak.upgrade_in_event_loop(move |ui| {
            ui.set_scanning(false);
            match result {
                Ok((tree, items, docker)) => {
                    with_state(|st| {
                        st.set_results(Some(tree), items, docker);
                        refresh(&ui, st);
                    });
                    ui.set_page(Page::Results);
                }
                Err(fdc_core::scan::ScanError::Cancelled) => ui.set_scan_error("cancelled".into()),
                Err(e) => ui.set_scan_error(e.to_string().into()),
            }
        });
    });
}

fn show_plan(ui: &AppWindow) {
    let rows = PLAN.with(|p| {
        let p = p.borrow();
        let (plan, confirmed) = p.as_ref()?;
        let rows: Vec<PlanRow> = with_state(|st| {
            plan.items
                .iter()
                .enumerate()
                .map(|(i, pl)| {
                    let item = st.items[pl.index]
                        .as_deletable()
                        .expect("plans hold deletables");
                    let (kind, name, method, command) = match item.path() {
                        Some(path) => (
                            "",
                            st.display(path),
                            if pl.direct { "direct" } else { "trash" },
                            String::new(),
                        ),
                        None => {
                            let t = match &item.info().target {
                                fdc_core::safety::Target::Command(t) => t,
                                fdc_core::safety::Target::Path(_) => unreachable!(),
                            };
                            let (kind, suffix) = state::command_kind(t);
                            (kind, suffix, "command", command_for(t).to_string())
                        }
                    };
                    PlanRow {
                        id: i as i32,
                        kind: kind.into(),
                        name: name.into(),
                        size: format::size(item.info().size).into(),
                        method: method.into(),
                        command: command.into(),
                        large: pl.large,
                        confirmed: confirmed[i],
                    }
                })
                .collect()
        });
        let all = plan
            .items
            .iter()
            .zip(confirmed)
            .all(|(pl, c)| !pl.large || *c);
        Some((rows, plan.estimated, plan.bulk, plan.irreversible, all))
    });
    let Some((rows, estimated, bulk, irreversible, all)) = rows else {
        return;
    };
    let docker = rows.iter().any(|r| r.kind.starts_with("docker"));
    ui.set_plan(model(rows));
    ui.set_plan_total(format::size(estimated).into());
    ui.set_plan_bulk(bulk);
    ui.set_plan_irreversible(irreversible);
    ui.set_plan_docker(docker);
    ui.set_plan_confirmed(all);
}

fn run_clean(ui: &AppWindow, env: &Env) {
    let Some(plan) = PLAN.with(|p| p.borrow_mut().take().map(|(plan, _)| plan)) else {
        return;
    };
    let jobs: Vec<Job> = with_state(|st| {
        plan.items
            .iter()
            .map(|pl| Job {
                index: pl.index,
                item: st.items[pl.index]
                    .as_deletable()
                    .expect("plans hold deletables")
                    .clone(),
                direct: pl.direct,
            })
            .collect()
    });
    ui.set_clean_done(0);
    ui.set_clean_total(jobs.len() as i32);
    ui.set_page(Page::Cleaning);
    let (env, weak) = (env.clone(), ui.as_weak());
    std::thread::spawn(move || {
        let cleaner = Cleaner::new(
            RealFs,
            env.trasher.clone(),
            env.runner.clone(),
            env.roots.clone(),
        );
        let mut results = Vec::new();
        for (n, job) in jobs.iter().enumerate() {
            let r = clean_one(&cleaner, job);
            if let Err(e) = &r {
                tracing::warn!("clean {:?}: {e}", job.item.info().target);
            }
            results.push((job.index, r.map_err(|e| (error_code(&e), e.to_string()))));
            let _ = weak.upgrade_in_event_loop(move |ui| ui.set_clean_done(n as i32 + 1));
        }
        if let (Some(mock), Some(root)) = (&env.mock_runner, &env.test_root) {
            // Test mode: expose the commands the mock "ran" for E2E checks.
            let _ = std::fs::write(
                root.join(".fdc-commands.log"),
                mock.calls().join("\n") + "\n",
            );
        }
        let _ = weak.upgrade_in_event_loop(move |ui| finish_clean(&ui, results));
    });
}

type CleanResult = Result<Outcome, (&'static str, String)>;

fn finish_clean(ui: &AppWindow, results: Vec<(usize, CleanResult)>) {
    let (mut reclaimed, mut estimated, mut trashed, mut docker) = (0, 0, 0, false);
    let rows = with_state(|st| {
        let rows: Vec<ResultRow> = results
            .iter()
            .map(|(i, r)| {
                let info = st.items[*i].info();
                let name = match &info.target {
                    fdc_core::safety::Target::Path(p) => st.display(p),
                    fdc_core::safety::Target::Command(t) => command_for(t).to_string(),
                };
                let (ok, code, detail) = match r {
                    Ok(o) => {
                        reclaimed += o.reclaimed;
                        estimated += o.estimated;
                        docker |= o.docker_disk_note;
                        if o.trashed {
                            trashed += o.estimated;
                        }
                        let code = if o.trashed {
                            "trashed"
                        } else if matches!(info.target, fdc_core::safety::Target::Command(_)) {
                            "ran"
                        } else {
                            "deleted"
                        };
                        (
                            true,
                            code,
                            format::size(if o.trashed { o.estimated } else { o.reclaimed }),
                        )
                    }
                    Err((code, msg)) => (false, *code, msg.clone()),
                };
                ResultRow {
                    name: name.into(),
                    detail_code: code.into(),
                    detail: detail.into(),
                    ok,
                }
            })
            .collect();
        st.mark_removed(results.iter().filter(|(_, r)| r.is_ok()).map(|(i, _)| *i));
        refresh(ui, st);
        rows
    });
    ui.set_reclaimed(format::size(reclaimed).into());
    ui.set_estimated(format::size(estimated).into());
    ui.set_trashed(if trashed > 0 {
        format::size(trashed).into()
    } else {
        "".into()
    });
    ui.set_done_docker(docker);
    ui.set_results_list(model(rows));
    ui.set_page(Page::Done);
}

fn load_settings_form(ui: &AppWindow, s: &Settings) {
    ui.set_set_large_gb(s.large_item_gb.to_string().into());
    ui.set_set_bulk_gb(s.bulk_total_gb.to_string().into());
    ui.set_set_stale_days(s.stale_node_modules_days.to_string().into());
    ui.set_set_language(match s.language.as_str() {
        "pl" => 1,
        "en" => 2,
        _ => 0,
    });
    ui.set_settings_error("".into());
}

fn read_settings_form(ui: &AppWindow) -> Option<Settings> {
    let large: f64 = ui
        .get_set_large_gb()
        .trim()
        .replace(',', ".")
        .parse()
        .ok()?;
    let bulk: f64 = ui.get_set_bulk_gb().trim().replace(',', ".").parse().ok()?;
    let stale: u32 = ui.get_set_stale_days().trim().parse().ok()?;
    if !(large > 0.0 && bulk > 0.0 && stale > 0) {
        return None;
    }
    let language = ["auto", "pl", "en"][ui.get_set_language().clamp(0, 2) as usize].to_string();
    Some(Settings {
        large_item_gb: large,
        bulk_total_gb: bulk,
        stale_node_modules_days: stale,
        language,
    })
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let env = Env::detect()?;
    let settings = Settings::load(&env.settings_path);
    let language = settings.language.clone();
    STATE.with(|s| {
        *s.borrow_mut() = Some(State::new(
            settings,
            |t| command_for(t).to_string(),
            env.roots.home.clone(),
        ))
    });

    let ui = AppWindow::new()?;
    // Bundled translations are registered when the component is created.
    apply_language(&language);
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    ui.set_test_mode(env.test_root.is_some());
    if let Some(root) = &env.test_root {
        ui.set_test_root(root.display().to_string().into());
    }
    ui.set_scan_root(env.roots.fs_root.display().to_string().into());
    ui.set_page(if env.has_full_disk_access() {
        Page::Home
    } else {
        Page::Onboarding
    });
    let cancel = Arc::new(AtomicBool::new(false));

    // Onboarding (Full Disk Access).
    ui.on_open_fda_settings({
        let env = env.clone();
        move || {
            if let Err(e) = env
                .runner
                .run(&CommandSpec::new("open", &[fda::SETTINGS_URL]))
            {
                tracing::warn!("open System Settings: {e}");
            }
        }
    });
    ui.on_recheck_fda({
        let (env, weak) = (env.clone(), ui.as_weak());
        move || {
            let ui = weak.unwrap();
            if env.has_full_disk_access() {
                ui.set_limited(false);
                ui.set_page(Page::Home);
            }
        }
    });
    ui.on_skip_fda({
        let weak = ui.as_weak();
        move || {
            let ui = weak.unwrap();
            ui.set_limited(true);
            ui.set_page(Page::Home);
        }
    });

    // Scan.
    for set in [AppWindow::on_scan, AppWindow::on_rescan] {
        let (env, weak, cancel) = (env.clone(), ui.as_weak(), cancel.clone());
        set(&ui, move || start_scan(&weak.unwrap(), &env, &cancel));
    }
    ui.on_cancel_scan(move || cancel.store(true, Ordering::Relaxed));

    // Results.
    macro_rules! on_state {
        ($set:ident, |$st:ident $(, $arg:ident: $ty:ty)*| $body:expr) => {{
            let weak = ui.as_weak();
            ui.$set(move |$($arg: $ty),*| {
                let ui = weak.unwrap();
                with_state(|$st| {
                    $body;
                    refresh(&ui, $st);
                });
            });
        }};
    }
    on_state!(on_select_group, |st, key: SharedString| st
        .select_group(&key));
    on_state!(on_open, |st, row: i32| st.open(row as usize));
    on_state!(on_up, |st| st.up());
    on_state!(on_sort, |st, col: i32| st.sort(col));
    on_state!(on_select_stale, |st| st.select_stale());
    ui.on_map_resized({
        let weak = ui.as_weak();
        move || with_state(|st| refresh_map(&weak.unwrap(), st))
    });
    ui.on_toggle({
        let weak = ui.as_weak();
        move |row| {
            let ui = weak.unwrap();
            with_state(|st| {
                st.toggle(row as usize);
                refresh_selection(&ui, st);
                // Keep the row model in place (no scroll reset): patch the one row.
                if let Some(r) = st.rows.get(row as usize) {
                    let rows = ui.get_rows();
                    if let Some(mut item) = rows.row_data(row as usize) {
                        item.selected = r.selected;
                        rows.set_row_data(row as usize, item);
                    }
                }
            });
        }
    });

    // Confirm + clean.
    ui.on_clean({
        let weak = ui.as_weak();
        move || {
            let ui = weak.unwrap();
            let plan =
                with_state(|st| plan::build(&st.selection(), ui.get_skip_trash(), &st.settings));
            if plan.items.is_empty() {
                return;
            }
            let confirmed = vec![false; plan.items.len()];
            PLAN.with(|p| *p.borrow_mut() = Some((plan, confirmed)));
            with_state(|st| {
                ui.set_large_limit(format::size(st.settings.large_item_bytes()).into());
                ui.set_bulk_limit(format::size(st.settings.bulk_total_bytes()).into());
            });
            show_plan(&ui);
            ui.set_page(Page::Confirm);
        }
    });
    ui.on_confirm_large({
        let weak = ui.as_weak();
        move |i| {
            PLAN.with(|p| {
                if let Some(flag) = p
                    .borrow_mut()
                    .as_mut()
                    .and_then(|(_, c)| c.get_mut(i as usize))
                {
                    *flag = !*flag;
                }
            });
            show_plan(&weak.unwrap());
        }
    });
    ui.on_run_clean({
        let (env, weak) = (env.clone(), ui.as_weak());
        move || {
            let ui = weak.unwrap();
            if ui.get_plan_confirmed() {
                run_clean(&ui, &env);
            }
        }
    });

    // Settings.
    ui.on_open_settings({
        let weak = ui.as_weak();
        move || with_state(|st| load_settings_form(&weak.unwrap(), &st.settings))
    });
    ui.on_save_settings({
        let (env, weak) = (env.clone(), ui.as_weak());
        move || {
            let ui = weak.unwrap();
            let Some(new) = read_settings_form(&ui) else {
                ui.set_settings_error("invalid".into());
                return;
            };
            if let Err(e) = new.save(&env.settings_path) {
                tracing::error!("saving settings: {e:#}");
            }
            apply_language(&new.language);
            with_state(|st| {
                st.settings = new;
                st.rebuild();
                refresh(&ui, st);
            });
            ui.invoke_close_overlay();
        }
    });

    ui.run()?;
    Ok(())
}

#[cfg(test)]
mod ui_tests {
    use super::*;
    use i_slint_backend_testing::{ElementHandle, init_no_event_loop};

    fn labels(ui: &AppWindow, label: &str) -> usize {
        ElementHandle::find_by_accessible_label(ui, label).count()
    }

    fn results_ui() -> (tempfile::TempDir, AppWindow, std::rc::Rc<RefCell<State>>) {
        init_no_event_loop();
        let (dir, st) = state::tests::fixture_state(Settings::default());
        let ui = AppWindow::new().unwrap();
        ui.set_page(Page::Results);
        refresh(&ui, &st);
        (dir, ui, std::rc::Rc::new(RefCell::new(st)))
    }

    #[test]
    fn status_badges_and_no_checkbox_for_do_not_touch() {
        let (_d, ui, st) = results_ui();
        assert!(labels(&ui, "Safe") > 0);
        assert!(labels(&ui, "Caution") > 0);
        st.borrow_mut().select_group("sys-critical");
        refresh(&ui, &st.borrow());
        assert!(labels(&ui, "Do not touch") > 0);
        assert_eq!(labels(&ui, "Safe") + labels(&ui, "Caution"), 0);
        let select_boxes = || {
            i_slint_backend_testing::ElementQuery::from_root(&ui)
                .match_descendants()
                .match_accessible_role(i_slint_backend_testing::AccessibleRole::Checkbox)
                .find_all()
                .into_iter()
                .filter(|e| {
                    e.accessible_label()
                        .is_some_and(|l| l.starts_with("Select "))
                })
                .count()
        };
        assert_eq!(select_boxes(), 0, "Do not touch rows must have no checkbox");
        st.borrow_mut().select_group("xcode");
        refresh(&ui, &st.borrow());
        assert!(select_boxes() > 0);
        st.borrow_mut().select_group("all");
        refresh(&ui, &st.borrow());
        assert!(
            labels(&ui, "No recommendation") > 0,
            "unrecognized folders have no rating"
        );
    }

    #[test]
    fn checkbox_toggles_selection_through_callback() {
        let (_d, ui, st) = results_ui();
        ui.on_toggle({
            let (st, weak) = (st.clone(), ui.as_weak());
            move |row| {
                st.borrow_mut().toggle(row as usize);
                refresh(&weak.unwrap(), &st.borrow());
            }
        });
        let before = ui.get_selected_count();
        let name = st
            .borrow()
            .rows
            .iter()
            .find(|r| r.selected)
            .unwrap()
            .name
            .clone();
        let cb = ElementHandle::find_by_accessible_label(&ui, &format!("Select {name}"))
            .next()
            .unwrap();
        assert_eq!(cb.accessible_checked(), Some(true));
        cb.invoke_accessible_default_action();
        assert_eq!(ui.get_selected_count(), before - 1);
    }

    #[test]
    fn sort_header_reorders_table() {
        let (_d, ui, st) = results_ui();
        ui.on_sort({
            let (st, weak) = (st.clone(), ui.as_weak());
            move |col| {
                st.borrow_mut().sort(col);
                refresh(&weak.unwrap(), &st.borrow());
            }
        });
        let names = || {
            ui.get_rows()
                .iter()
                .map(|r| r.name.to_string())
                .collect::<Vec<_>>()
        };
        ElementHandle::find_by_accessible_label(&ui, "Name")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        let mut sorted = names();
        sorted.sort_by_key(|n| n.to_lowercase());
        assert_eq!(names(), sorted);
        assert_eq!(ui.get_sort_column(), state::SORT_NAME);
    }

    #[test]
    fn scan_progress_is_shown() {
        init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        ui.set_page(Page::Home);
        ui.set_scanning(true);
        ui.set_scan_files(42);
        ui.set_scan_bytes("1.5 MB".into());
        assert_eq!(labels(&ui, "Scanned 42 files, 1.5 MB"), 1);
        assert_eq!(labels(&ui, "Cancel"), 1);
        ui.set_scan_files(43);
        assert_eq!(labels(&ui, "Scanned 43 files, 1.5 MB"), 1);
    }

    #[test]
    fn test_mode_banner_and_about_slint() {
        init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        ui.set_test_mode(true);
        ui.set_test_root("/tmp/fx".into());
        assert_eq!(labels(&ui, "TEST MODE · /tmp/fx"), 1);
        ElementHandle::find_by_accessible_label(&ui, "About")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        assert_eq!(ui.get_page(), Page::About);
        assert!(
            ElementHandle::find_by_element_type_name(&ui, "AboutSlint")
                .next()
                .is_some()
        );
    }
}

//! Trash, deletion, Docker/`tmutil`/`simctl` adapters. The only crate that deletes or spawns,
//! always behind [`FileSystem`] / [`Trasher`] / [`CommandRunner`].

mod clean;
mod query;

pub use clean::{CleanError, Cleaner, Method, Outcome};
pub use query::{
    DockerUsage, QueryError, Usage, docker_items, docker_usage, parse_docker_size, simctl_items,
    tmutil_items,
};

use fdc_core::safety::CommandTarget;
use std::collections::HashMap;
use std::fmt;
use std::fs::{self, Metadata};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

pub trait FileSystem {
    fn symlink_metadata(&self, path: &Path) -> io::Result<Metadata>;
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf>;
    fn remove_dir_all(&self, path: &Path) -> io::Result<()>;
    fn remove_file(&self, path: &Path) -> io::Result<()>;
}

pub trait Trasher {
    fn trash(&self, path: &Path) -> io::Result<()>;
}

pub trait CommandRunner {
    /// Spawn error (e.g. program not installed) is `Err`; a non-zero exit is `Ok` with `success: false`.
    fn run(&self, cmd: &CommandSpec) -> io::Result<CommandOutput>;
}

// Lets the app hold `Arc<dyn Trasher>` / `Arc<MockRunner>` and keep a handle for inspection.
impl<T: Trasher + ?Sized> Trasher for Arc<T> {
    fn trash(&self, path: &Path) -> io::Result<()> {
        (**self).trash(path)
    }
}
impl<C: CommandRunner + ?Sized> CommandRunner for Arc<C> {
    fn run(&self, cmd: &CommandSpec) -> io::Result<CommandOutput> {
        (**self).run(cmd)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn ok(stdout: &str) -> Self {
        Self {
            success: true,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }
    pub fn fail(stderr: &str) -> Self {
        Self {
            success: false,
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }
}

/// A command as data. `Display` is the exact string shown to the user before running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl CommandSpec {
    pub fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
        }
    }
}

impl fmt::Display for CommandSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.program)?;
        for a in &self.args {
            let plain = !a.is_empty()
                && a.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./=:@,+".contains(c));
            if plain {
                write!(f, " {a}")?;
            } else {
                write!(f, " '{}'", a.replace('\'', r"'\''"))?;
            }
        }
        Ok(())
    }
}

/// The exact command that removes `target`. Nothing here ever touches Docker's virtual disk file.
pub fn command_for(target: &CommandTarget) -> CommandSpec {
    match target {
        CommandTarget::DockerBuildCache => CommandSpec::new("docker", &["builder", "prune", "-f"]),
        CommandTarget::DockerDanglingImages => {
            CommandSpec::new("docker", &["image", "prune", "-f"])
        }
        CommandTarget::DockerUnusedImages => {
            CommandSpec::new("docker", &["image", "prune", "-a", "-f"])
        }
        CommandTarget::DockerStoppedContainers => {
            CommandSpec::new("docker", &["container", "prune", "-f"])
        }
        // No `-a`: since Docker 23 this prunes only anonymous volumes and keeps named ones, which is
        // where local databases live. The `docker system df` estimate counts named volumes too, so
        // the reclaimed number (parsed from Docker's output) may be lower than the estimate.
        CommandTarget::DockerVolumes => CommandSpec::new("docker", &["volume", "prune", "-f"]),
        CommandTarget::TimeMachineSnapshot(date) => {
            CommandSpec::new("tmutil", &["deletelocalsnapshots", date])
        }
        CommandTarget::SimctlUnavailableDevices => {
            CommandSpec::new("xcrun", &["simctl", "delete", "unavailable"])
        }
        CommandTarget::SimctlRuntime(id) => {
            CommandSpec::new("xcrun", &["simctl", "runtime", "delete", id])
        }
    }
}

pub struct RealFs;

impl FileSystem for RealFs {
    fn symlink_metadata(&self, path: &Path) -> io::Result<Metadata> {
        fs::symlink_metadata(path)
    }
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        fs::canonicalize(path)
    }
    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        fs::remove_dir_all(path) // does not follow symlinks inside the tree
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }
}

/// The user's real macOS Trash.
pub struct SystemTrash;

impl Trasher for SystemTrash {
    fn trash(&self, path: &Path) -> io::Result<()> {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        // ponytail: NSFileManager instead of Finder/AppleScript: no Automation permission prompt;
        // macOS may hide "Put Back", but the item is in Trash and can be dragged out.
        let mut ctx = trash::TrashContext::default();
        ctx.set_delete_method(DeleteMethod::NsFileManager);
        ctx.delete(path).map_err(io::Error::other)
    }
}

pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, cmd: &CommandSpec) -> io::Result<CommandOutput> {
        // A Finder-launched app gets launchd's minimal PATH; docker lives in Homebrew/Docker.app dirs.
        // Setting PATH on the Command makes std resolve the program with it (and docker's helpers too).
        let path = std::env::var("PATH").unwrap_or_default();
        let path = format!(
            "{path}:/usr/local/bin:/opt/homebrew/bin:/Applications/Docker.app/Contents/Resources/bin"
        );
        let out = Command::new(&cmd.program)
            .args(&cmd.args)
            .env("PATH", path)
            .output()?;
        Ok(CommandOutput {
            success: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// Test-mode Trash: moves items into `dir` (inside the test root) so the user's real Trash is
/// never touched and tests/E2E can see the files left their original place.
pub struct MockTrasher {
    pub dir: PathBuf,
    trashed: Mutex<Vec<PathBuf>>,
}

impl MockTrasher {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            trashed: Mutex::default(),
        }
    }
    /// Original paths, in the order they were trashed.
    pub fn trashed(&self) -> Vec<PathBuf> {
        self.trashed.lock().unwrap().clone()
    }
}

impl Trasher for MockTrasher {
    fn trash(&self, path: &Path) -> io::Result<()> {
        let mut trashed = self.trashed.lock().unwrap();
        fs::create_dir_all(&self.dir)?;
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        fs::rename(path, self.dir.join(format!("{}-{name}", trashed.len())))?;
        trashed.push(path.to_path_buf());
        Ok(())
    }
}

/// Canned responses keyed by the command's display string; records every call.
/// Commands without a response fail to spawn (`NotFound`), like a missing tool.
#[derive(Default)]
pub struct MockRunner {
    responses: HashMap<String, CommandOutput>,
    fallback: Option<CommandOutput>,
    calls: Mutex<Vec<String>>,
}

impl MockRunner {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with(mut self, cmd: &str, out: CommandOutput) -> Self {
        self.responses.insert(cmd.into(), out);
        self
    }
    /// Response for every command without a specific one.
    pub fn with_fallback(mut self, out: CommandOutput) -> Self {
        self.fallback = Some(out);
        self
    }
    /// Display strings of all commands run so far.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    /// Docker/tmutil/simctl listings from `tests/data`; every other command (prune/delete) succeeds.
    pub fn test_mode() -> Self {
        let prune = "Deleted build cache objects:\nabc123\n\nTotal reclaimed space: 1.5GB\n";
        Self::new()
            .with(
                &query::docker_df().to_string(),
                CommandOutput::ok(DOCKER_DF),
            )
            .with(
                &query::docker_dangling().to_string(),
                CommandOutput::ok(DOCKER_DANGLING),
            )
            .with(
                &query::tmutil_list().to_string(),
                CommandOutput::ok(TMUTIL_LIST),
            )
            .with(
                &query::simctl_devices().to_string(),
                CommandOutput::ok(SIMCTL_DEVICES),
            )
            .with(
                &query::simctl_runtimes().to_string(),
                CommandOutput::ok(SIMCTL_RUNTIMES),
            )
            .with_fallback(CommandOutput::ok(prune))
    }
}

impl CommandRunner for MockRunner {
    fn run(&self, cmd: &CommandSpec) -> io::Result<CommandOutput> {
        let key = cmd.to_string();
        self.calls.lock().unwrap().push(key.clone());
        self.responses
            .get(&key)
            .or(self.fallback.as_ref())
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("mock: {key}")))
    }
}

const DOCKER_DF: &str = include_str!("../tests/data/docker_system_df.jsonl");
const DOCKER_DANGLING: &str = include_str!("../tests/data/docker_images_dangling.jsonl");
const TMUTIL_LIST: &str = include_str!("../tests/data/tmutil_listlocalsnapshots.txt");
const SIMCTL_DEVICES: &str = include_str!("../tests/data/simctl_devices_unavailable.json");
const SIMCTL_RUNTIMES: &str = include_str!("../tests/data/simctl_runtime_list.json");

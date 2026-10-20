//! Read-only listings: Docker usage, Time Machine local snapshots, simulator devices/runtimes.

use crate::{CommandRunner, CommandSpec};
use fdc_core::rules::classify_command;
use fdc_core::safety::{Classified, CommandTarget};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    /// Tool not installed, Docker daemon not running, or the command exited non-zero.
    #[error("`{0}` is not available: {1}")]
    Unavailable(String, String),
    #[error("unexpected output from `{0}`: {1}")]
    Parse(String, String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub size: u64,
    pub reclaimable: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DockerUsage {
    pub images: Usage,
    pub containers: Usage,
    pub volumes: Usage,
    pub build_cache: Usage,
    /// Sum of dangling image sizes. ponytail: `docker images` size includes layers shared with
    /// other images, so this can overstate; `docker system df -v` per-image unique size if it matters.
    pub dangling_images: u64,
}

pub(crate) fn docker_df() -> CommandSpec {
    CommandSpec::new("docker", &["system", "df", "--format", "{{json .}}"])
}
pub(crate) fn docker_dangling() -> CommandSpec {
    CommandSpec::new(
        "docker",
        &["images", "-f", "dangling=true", "--format", "{{json .}}"],
    )
}
pub(crate) fn tmutil_list() -> CommandSpec {
    CommandSpec::new("tmutil", &["listlocalsnapshots", "/"])
}
pub(crate) fn simctl_devices() -> CommandSpec {
    CommandSpec::new("xcrun", &["simctl", "list", "devices", "unavailable", "-j"])
}
pub(crate) fn simctl_runtimes() -> CommandSpec {
    CommandSpec::new("xcrun", &["simctl", "runtime", "list", "-j"])
}

fn query(runner: &(impl CommandRunner + ?Sized), cmd: &CommandSpec) -> Result<String, QueryError> {
    let unavailable = |why: String| QueryError::Unavailable(cmd.to_string(), why);
    match runner.run(cmd) {
        Ok(o) if o.success => Ok(o.stdout),
        Ok(o) => Err(unavailable(o.stderr.trim().into())),
        Err(e) => Err(unavailable(e.to_string())),
    }
}

fn parse_err(cmd: &CommandSpec, why: impl ToString) -> QueryError {
    QueryError::Parse(cmd.to_string(), why.to_string())
}

/// Docker's human sizes (decimal units): "0B", "512kB", "1.2GB", "512MB (40%)".
pub fn parse_docker_size(s: &str) -> Option<u64> {
    let s = s.split_whitespace().next()?;
    let (num, unit) = s.split_at(s.find(|c: char| c.is_ascii_alphabetic())?);
    let mult = match unit.to_ascii_lowercase().as_str() {
        "b" => 1e0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        "pb" => 1e15,
        _ => return None,
    };
    let n: f64 = num.parse().ok()?;
    (n >= 0.0).then(|| (n * mult).round() as u64)
}

/// Each line of a `--format '{{json .}}'` listing, parsed.
fn json_lines<T: for<'de> Deserialize<'de>>(
    cmd: &CommandSpec,
    out: &str,
) -> Result<Vec<T>, QueryError> {
    out.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|e| parse_err(cmd, e)))
        .collect()
}

fn size(cmd: &CommandSpec, s: &str) -> Result<u64, QueryError> {
    parse_docker_size(s).ok_or_else(|| parse_err(cmd, format!("bad size {s:?}")))
}

pub fn docker_usage(runner: &(impl CommandRunner + ?Sized)) -> Result<DockerUsage, QueryError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Row {
        r#type: String,
        size: String,
        reclaimable: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Image {
        size: String,
    }

    let cmd = docker_df();
    let mut u = DockerUsage::default();
    for row in json_lines::<Row>(&cmd, &query(runner, &cmd)?)? {
        let slot = match row.r#type.as_str() {
            "Images" => &mut u.images,
            "Containers" => &mut u.containers,
            "Local Volumes" => &mut u.volumes,
            "Build Cache" => &mut u.build_cache,
            _ => continue,
        };
        *slot = Usage {
            size: size(&cmd, &row.size)?,
            reclaimable: size(&cmd, &row.reclaimable)?,
        };
    }

    let cmd = docker_dangling();
    for img in json_lines::<Image>(&cmd, &query(runner, &cmd)?)? {
        u.dangling_images += size(&cmd, &img.size)?;
    }
    Ok(u)
}

/// Docker items with a non-zero reclaimable size. Note: the unused-images estimate includes dangling
/// images, so selecting both double-counts the estimate (Docker's reported reclaimed space is exact).
pub fn docker_items(runner: &(impl CommandRunner + ?Sized)) -> Result<Vec<Classified>, QueryError> {
    let u = docker_usage(runner)?;
    Ok([
        (CommandTarget::DockerBuildCache, u.build_cache.reclaimable),
        (CommandTarget::DockerDanglingImages, u.dangling_images),
        (CommandTarget::DockerUnusedImages, u.images.reclaimable),
        (
            CommandTarget::DockerStoppedContainers,
            u.containers.reclaimable,
        ),
        (CommandTarget::DockerVolumes, u.volumes.reclaimable),
    ]
    .into_iter()
    .filter(|&(_, size)| size > 0)
    .map(|(t, size)| classify_command(t, size))
    .collect())
}

/// Local Time Machine snapshots, sorted oldest → newest (the last one is the newest; the UI should
/// leave it unselected). ponytail: size 0, tmutil does not report per-snapshot size.
pub fn tmutil_items(runner: &(impl CommandRunner + ?Sized)) -> Result<Vec<Classified>, QueryError> {
    let out = query(runner, &tmutil_list())?;
    let mut dates: Vec<&str> = out
        .lines()
        .filter_map(|l| {
            l.trim()
                .strip_prefix("com.apple.TimeMachine.")?
                .strip_suffix(".local")
        })
        .collect();
    dates.sort_unstable(); // YYYY-MM-DD-HHMMSS sorts chronologically
    dates.dedup();
    Ok(dates
        .into_iter()
        .map(|d| classify_command(CommandTarget::TimeMachineSnapshot(d.into()), 0))
        .collect())
}

/// One item for all unavailable simulator devices (size = sum of `dataPathSize` where reported),
/// plus one per deletable runtime (sorted by identifier).
pub fn simctl_items(runner: &(impl CommandRunner + ?Sized)) -> Result<Vec<Classified>, QueryError> {
    #[derive(Deserialize)]
    struct Devices {
        devices: BTreeMap<String, Vec<Device>>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Device {
        data_path_size: Option<u64>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Runtime {
        identifier: String,
        size_bytes: Option<u64>,
        deletable: Option<bool>,
    }

    let mut items = Vec::new();

    let cmd = simctl_devices();
    let devs: Devices =
        serde_json::from_str(&query(runner, &cmd)?).map_err(|e| parse_err(&cmd, e))?;
    let devs: Vec<Device> = devs.devices.into_values().flatten().collect();
    if !devs.is_empty() {
        let size = devs.iter().filter_map(|d| d.data_path_size).sum();
        items.push(classify_command(
            CommandTarget::SimctlUnavailableDevices,
            size,
        ));
    }

    let cmd = simctl_runtimes();
    let rts: BTreeMap<String, Runtime> =
        serde_json::from_str(&query(runner, &cmd)?).map_err(|e| parse_err(&cmd, e))?;
    for rt in rts.into_values().filter(|r| r.deletable != Some(false)) {
        let size = rt.size_bytes.unwrap_or(0);
        items.push(classify_command(
            CommandTarget::SimctlRuntime(rt.identifier),
            size,
        ));
    }
    Ok(items)
}

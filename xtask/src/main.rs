use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

mod package;

const DEFAULT_MCP_PORT: &str = "8765";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("dev-mcp") => dev_mcp(&args[1..]),
        Some("fixtures") => {
            let dir = args.get(1).map_or("fixtures-out", String::as_str);
            fdc_fixtures::generate(Path::new(dir)).with_context(|| format!("generating {dir}"))?;
            println!("fixtures written to {dir}");
            Ok(())
        }
        Some("package") => package::package(&args[1..]),
        Some("icon") => package::icon(),
        Some("bench") => bench(Path::new(
            args.get(1).context("usage: cargo xtask bench <path>")?,
        )),
        _ => bail!(
            "usage: cargo xtask <dev-mcp | fixtures [dir] | bench <path> | package [--sign <id>] [--notarize] | icon>"
        ),
    }
}

/// Runs the app with the Slint MCP server. Refuses to start outside test mode.
fn dev_mcp(extra: &[String]) -> Result<()> {
    let root = std::env::var_os("FDC_TEST_ROOT")
        .context("FDC_TEST_ROOT is not set; dev-mcp only runs in test mode on fixtures")?;
    if !std::path::Path::new(&root).is_dir() {
        bail!("FDC_TEST_ROOT={root:?} is not a directory");
    }
    let port = std::env::var("SLINT_MCP_PORT").unwrap_or_else(|_| DEFAULT_MCP_PORT.into());
    let status = Command::new(env!("CARGO"))
        .args(["run", "-p", "app", "--features", "mcp", "--"])
        .args(extra)
        .env("SLINT_EMIT_DEBUG_INFO", "1")
        .env("SLINT_MCP_PORT", &port)
        .status()?;
    if !status.success() {
        bail!("app exited with {status}");
    }
    Ok(())
}

/// Read-only scan for the D2 benchmark. Links fdc-core only, never fdc-cleaner.
fn bench(path: &Path) -> Result<()> {
    let skip = if path == Path::new("/") {
        ["/System/Volumes", "/Volumes", "/dev"]
            .map(Into::into)
            .to_vec()
    } else {
        Vec::new()
    };
    let opts = fdc_core::scan::ScanOptions {
        skip,
        threads: None,
    };
    let start = Instant::now();
    let tree = fdc_core::scan::scan(path, &opts, &|_| {}, &AtomicBool::new(false))?;
    let wall = start.elapsed();
    let files = tree
        .nodes
        .iter()
        .filter(|n| n.kind == fdc_core::model::NodeKind::File)
        .count();
    let errors = tree.nodes.iter().filter(|n| n.error.is_some()).count();
    let total = tree.node(fdc_core::model::ScanTree::ROOT).total_size;
    println!("path:     {}", path.display());
    println!("entries:  {}", tree.nodes.len());
    println!("files:    {files}");
    println!(
        "total:    {total} bytes ({:.2} GiB allocated)",
        total as f64 / (1u64 << 30) as f64
    );
    println!("errors:   {errors}");
    println!("wall:     {wall:.2?}");
    Ok(())
}

# AGENTS.md

> Instructions for AI agents. Read before changing code.
> This app deletes user files. The safety rules below are not style preferences.

## Project

FreeDiskCleaner: macOS (Apple Silicon only) app that scans the disk, classifies developer junk,
and deletes it safely. Rust (stable, edition 2024) Cargo workspace, UI in Slint (Cupertino style).
No backend, no network, no telemetry. Spec and rationale: `prd-freediskcleaner.md`, `freediskcleaner-tsd.md`.

<!-- UNVERIFIED: planned layout from TSD. Update when crates exist. -->

- `crates/core` - scanner, model, rule engine, safety rating, treemap layout. No UI deps, no side effects.
- `crates/cleaner` - Trash, deletion, Docker/`tmutil`/`simctl` adapters. The only crate that touches the
  filesystem destructively or spawns processes, always behind the `FileSystem` / `CommandRunner` / `Trasher` traits.
- `crates/app` - binary: state, threads, channels, Slint glue, `.slint` files in `ui/`, `.po` in `translations/`.
- `xtask/` - `cargo xtask` tasks (fixtures, dev-mcp, package, release).

## Commands

<!-- UNVERIFIED: written from spec, no code on disk yet. Run each one and fix before trusting. -->

- Build: `cargo build --workspace`
- Run (test mode): `FDC_TEST_ROOT=<fixture-dir> cargo run -p app`
- Generate fixtures: `cargo xtask fixtures`
- Run with Slint MCP (agent UI control): `cargo xtask dev-mcp` (must always run in test mode on fixtures)
  MCP is plain HTTP JSON-RPC on `http://127.0.0.1:8765/mcp` (`SLINT_MCP_PORT`); test-mode helpers:
  marker `<root>/.fdc-fda-denied` simulates missing Full Disk Access, mock commands run are written to
  `<root>/.fdc-commands.log`, test Trash is `<root>/.fdc-trash`.
- Scan benchmark (read-only, for D2): `cargo run --release -p xtask -- bench <path>`
- Package: `cargo xtask package [--sign <identity>] [--notarize]` → `target/package/*.app|*.dmg`
- Format: `cargo fmt --all --check`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings`
- Test: `cargo nextest run --workspace` | single: `cargo nextest run -p <crate> <filter>`
- Deps/licenses: `cargo deny check`

## Done

Run before reporting a task complete, all green:

```
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo nextest run --workspace && cargo deny check
```

UI change: also run the app in test mode via `cargo xtask dev-mcp`, click through the changed view,
and take a screenshot. When a test fails, the code is the suspect, not the test.

## Safety invariants (NEVER break)

1. "Do not touch" (Nie ruszać) items have **no deletion code path**. Make it unrepresentable in types,
   not hidden in UI. ❌ `if status != DoNotTouch { show_button() }` → ✅ deletable items are a separate type
   that a DoNotTouch item cannot be converted into.
2. Right before any deletion, re-match the path against its rule and reject symlinks resolving outside the
   allowed area. Scan results are stale by the time the user clicks.
3. Default deletion = Trash (`trash` crate). Direct `remove_dir_all` only for "Safe" items after an explicit
   user action.
4. Docker: CLI only (`docker system df`, `docker ... prune`), show the exact command before running.
   Never touch the Docker virtual disk file.
5. Docker volumes and "Caution" items start unselected.
6. 5 GB (large item) and 25 GB (bulk summary) thresholds plus the irreversible-operation warning live in
   `app` and are read from settings. Never hardcode them in `core`/`cleaner`.
7. Size = allocated blocks (`st_blocks * 512`), not logical length. Hardlinks and APFS clones counted once.
8. Safety ratings come from the deterministic rule table in `core`. No heuristics, no AI. Unrecognized
   path = shown with size, no recommendation.

## Testing

- NEVER run cleaning against the real disk, including "just to check". Use `FDC_TEST_ROOT` + fixtures;
  in that mode Docker/`tmutil`/`simctl` go through mocks and the window shows a "TEST MODE" banner.
  If the banner is missing, stop.
- Logic tests: `tempfile` trees, never real home-dir paths. Mock `CommandRunner` - tests must not spawn
  `docker`, `tmutil` or `xcrun`.
- Every new rule gets a fixture-based test. Safety invariants get `proptest` properties
  (e.g. nothing "Do not touch" ever reaches the deletion list).
- Slint components: headless via `i-slint-backend-testing`, no window.

## UI and i18n

- Every user-visible string goes through `@tr()` and gets **both** PL and EN entries in the same change.
  Rule rationales ("why this is Caution") are translation keys too, never inline strings in Rust.
- Do not remove the `AboutSlint` widget or the "Made with Slint" badge in README. They are the conditions
  of the Slint Royalty-free license that keeps this project MIT.
- Feature `slint/mcp` is dev-only. NEVER enable it in release builds or as a default feature.

## Boundaries

- No network calls, no telemetry, no analytics. Ask before adding any dependency that does I/O over the network.
- Errors: `thiserror` in `core`/`cleaner`, `anyhow` only in `app`. Logs via `tracing`, local only.
- Release signing/notarization secrets live in GitHub Actions secrets only. Never write cert, `.p12`,
  `.p8` or key IDs into the repo.
- Open decisions, do not settle them silently, ask: packaging tool (`cargo-packager` vs xtask script),
  `jwalk` vs custom `rayon` walker (needs a real-disk benchmark), minimum macOS version,
  which system-data locations are "Do not touch" vs "Caution".

## Commits and PRs

Conventional commits (`feat:`, `fix:`, `chore:`, ...). Run "Done" before opening a PR.
Stage only files relevant to the task, no `git add -A`.

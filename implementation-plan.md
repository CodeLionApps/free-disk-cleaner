# Implementation Plan: FreeDiskCleaner v1

> Source: `prd-freediskcleaner.md`, `freediskcleaner-tsd.md`, `AGENTS.md`.
> Order: de-risk first (tooling, Slint MCP), then pure logic (`core`, `cleaner`), then UI, then distribution.
> Every phase ends with the **Done gate** from `AGENTS.md`; extra checks are listed per phase.

```
Done gate = cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings \
            && cargo nextest run --workspace && cargo deny check
```

## Decision gates (ask the user, do not settle silently)

| # | Decision | Needed before | Input |
|---|---|---|---|
| D1 | Slint MCP good enough, or fallback to `agent-desktop` | end of Phase 0 | spike result |
| D2 | `jwalk` vs custom `rayon` walker | end of Phase 1 | real-disk benchmark (read-only scan, run by user) |
| D3 | System-data rule list: which locations are "Do not touch" vs "Caution" | Phase 2 | proposed table, user approves |
| D4 | Minimum macOS version | Phase 1 (APFS clone API) / Phase 8 (target config) | API availability |
| D5 | Packaging: `cargo-packager` vs `xtask` script | Phase 8 | prototype |

---

## Phase 0: Scaffold and de-risk

Goal: empty but green workspace, test mode wired from day one, proof that the agent can drive the UI.

- Cargo workspace (edition 2024, `aarch64-apple-darwin`): `crates/core`, `crates/cleaner`, `crates/app`, `xtask`.
- Pin Slint 1.x (Cupertino style, winit + Femtovg). `slint/mcp` behind a dev-only feature of `app`, not default.
- `deny.toml` (MIT-compatible licenses, Slint Royalty-free allowed), `LICENSE` (MIT), `README.md` with "Made with Slint" badge.
- `FDC_TEST_ROOT` handling in `app`: if set, window shows "TEST MODE" banner. Banner exists before any deletion code does.
- Minimal window: "Scan" button + label, `@tr()` from the start, `translations/` with PL + EN `.po`.
- `cargo xtask dev-mcp` (runs app with `slint/mcp`, `SLINT_EMIT_DEBUG_INFO=1`, `SLINT_MCP_PORT`, refuses to start without `FDC_TEST_ROOT`), `.mcp.json`.
- `.github/workflows/ci.yml`: Done gate + guard that release build of `app` has no `mcp` feature
  (e.g. `cargo tree -p app -e features --release` must not contain `slint/mcp`).

**Verify**
- Done gate green locally and in CI.
- Spike (D1): agent starts `cargo xtask dev-mcp`, reads UI tree, clicks "Scan", takes a screenshot, sees the TEST MODE banner.
- `cargo xtask dev-mcp` without `FDC_TEST_ROOT` exits with an error.

---

## Phase 1: Scanner and model (`crates/core`)

Goal: fast, correct size accounting on any directory tree. No UI, no side effects.

- Model: tree of nodes (path, allocated size, mtime/atime, kind), aggregated sizes per directory.
- Size = `st_blocks * 512`. Hardlinks deduped by `(dev, ino)`. Symlinks never followed.
- APFS clones counted once: spike on detection (`getattrlist` clone id / private size); result feeds D4.
- Parallel walk (D2 decides library). Progress events + cancellation over a channel.
- Errors (permission denied, vanished files) recorded per node, scan continues. `thiserror`.
- `cargo xtask fixtures`: generates trees mimicking DerivedData, simulators, `node_modules` (fresh + stale),
  yarn/npm/Gradle/Homebrew caches, sparse file, hardlinks, clones, symlink pointing outside the root,
  unreadable dir. Controlled sizes, deterministic.
- Benchmark binary/xtask for D2: read-only scan, no `cleaner` linked, run by the user on the real disk.

**Verify**
- Tests on `tempfile` trees: sparse file reports allocated not logical size; hardlink counted once;
  clone counted once; symlink not followed; unreadable dir reported, scan finishes; cancellation stops promptly;
  directory totals = sum of children (proptest).
- Benchmark numbers recorded, D2 decided.

---

## Phase 2: Rule engine and safety rating (`crates/core`)

Goal: deterministic classification. "Do not touch" unrepresentable as deletable.

- Rule table as data: id, tool group (Xcode/iOS, Android, Docker, Node/JS, Homebrew & other, Apps & system),
  path matcher, status (Safe / Caution / DoNotTouch), rationale translation key, removal method (path or command).
- Types: classified item enum; only Safe/Caution matches can produce a `Deletable` value.
  No conversion from DoNotTouch exists. Direct (non-Trash) deletion token constructible only from Safe.
- Unrecognized path = size + path, no recommendation.
- Stale `node_modules`: based on project `package.json`/lockfile dates, threshold passed in as a parameter.
- System data split into "critical" (DoNotTouch, red) and "removable" (Caution, yellow, unselected) per D3.
  Hints like "keep newest snapshot, remove older".
- Initial rules: DerivedData, Xcode Archives, iOS DeviceSupport, simulator caches/devices, Android emulator + Gradle caches,
  `node_modules`, yarn/npm/pnpm caches, Homebrew cache, Docker items (command-based), Time Machine local snapshots (command-based).

**Verify**
- One fixture-based test per rule.
- proptest: no DoNotTouch item ever reaches a deletion list; classification is deterministic (same tree → same result);
  unknown paths never get a status.
- `compile_fail` doctest: DoNotTouch cannot be turned into `Deletable`.
- D3 table approved by user and committed as tests.

---

## Phase 3: Cleaner (`crates/cleaner`)

Goal: the only crate that deletes or spawns, all behind `FileSystem` / `CommandRunner` / `Trasher`.

- Deletion pipeline per item: re-stat → re-match against its rule → canonicalize and reject symlinks escaping
  the allowed area → Trash (default, `trash` crate) or `remove_dir_all` (Safe + explicit token only).
- Measure reclaimed space (size before vs after; Trash = 0 reclaimed until emptied, report honestly).
- Docker adapter: `docker system df` (JSON format) parsing into images / containers / volumes / build cache;
  prune commands built as data, exact command string exposed for preview. Docker not running = clear error, not crash.
  Never touches the virtual disk file.
- `tmutil` adapter: list/delete local snapshots. `xcrun simctl` adapter: list/delete unavailable devices and old runtimes.
- Mock implementations of all three traits, used by tests and by test mode.

**Verify**
- Tests with mocks only (no real `docker`/`tmutil`/`xcrun` spawn): command strings exact; parse of captured
  `docker system df` / `tmutil` / `simctl` outputs (stored as test data).
- Revalidation: item changed between scan and delete (path replaced, became symlink, now matches DoNotTouch) → rejected.
- Symlink escaping the root → rejected. Trash is default; direct delete impossible for Caution (type-level).
- proptest: deletion list built from arbitrary classified items never contains DoNotTouch.

---

## Phase 4: App shell and scan view (`crates/app`)

Goal: real scan visible in the UI, live progress, grouped results.

- App state, worker thread for scan, channel to UI, cancellation.
- Settings JSON via `directories` (thresholds 5 GB / 25 GB, stale `node_modules` age, language). `anyhow` + `tracing` (local only).
- Test mode: `FDC_TEST_ROOT` → scan root = fixture dir, cleaner gets mock `CommandRunner` (canned Docker/tmutil/simctl data).
- Views: scan screen with live progress; results grouped by tool; virtualized table (sort by size/date/name,
  path, size, last used, description, status badge); drill-down group → folder.
- "About" with `AboutSlint` reachable from menu. Language switch PL/EN.

**Verify**
- Headless component tests (`i-slint-backend-testing`): table sorting, badge per status, progress updates.
- Every new string has PL + EN entry (script/test that `.po` files cover all `@tr()` keys).
- `cargo xtask dev-mcp` on fixtures: scan, open each group, screenshot. Banner visible.

---

## Phase 5: Cleaning flow and warnings (`crates/app`)

Goal: full scan → select → confirm → clean → result, with all PRD warnings.

- Selection: Caution items and Docker volumes start unselected. DoNotTouch rows have no checkbox/button.
- Summary dialog: exactly what goes, estimated space.
- Item > large threshold → extra confirmation. Total > bulk threshold → summary screen with warning.
  Irreversible ops (direct delete, Docker commands) → warning regardless of size. Thresholds from settings only.
- Docker: show exact command before running; after run, note that the virtual disk may not shrink immediately.
- "Delete immediately, skip Trash" option for Safe items only.
- Result screen: actually reclaimed space vs estimate, failures listed.
- Stale `node_modules` view with bulk selection.
- System data: two categories (red critical, info only; yellow removable, unselected) with keep/remove hints.
- Settings view: thresholds, stale age, language.

**Verify**
- Headless tests: threshold logic with custom settings values; volumes/Caution default unselected;
  irreversible warning shown for direct delete and Docker.
- E2E in test mode via MCP: scan fixtures → select Safe + one Caution → confirm → clean → fixture files are in
  Trash or gone as expected, mock runner recorded the expected commands, result screen numbers match.
- Large / bulk thresholds triggered with a generated fixture above them (or lowered thresholds in settings).

---

## Phase 6: Disk map (treemap)

Goal: visual map with drill-down. Independent; can slip after Phase 7 if needed.

- Squarified layout as a pure function in `core` (input: sizes + rect, output: rects).
- Slint view: rectangles model, color by status, click to drill down, breadcrumb back, toggle table/map.

**Verify**
- proptest on layout: rects inside bounds, no overlaps, area proportional to size (tolerance), order stable.
- MCP click-through: map renders on fixtures, drill-down works, screenshot.

---

## Phase 7: Onboarding and Full Disk Access

Goal: first run on a friend's Mac works without help.

- Detect missing FDA by reading a protected location; explain why it's needed; open
  System Settings → Privacy & Security → Full Disk Access (via `CommandRunner`, `open x-apple.systempreferences:...`).
- Re-check on return, then go to scan. Skippable with a degraded-scan notice.
- All copy PL + EN.

**Verify**
- Unit test detection logic with mock `FileSystem` (granted / denied).
- MCP click-through of onboarding in test mode (FDA state mocked).
- Manual (user): fresh macOS user account without FDA → onboarding → grant → scan.

---

## Phase 8: Packaging, signing, release

Goal: signed, notarized `.dmg` on GitHub Releases from a tag. D4, D5 decided here.

- `packaging/`: `Info.plist`, icon, entitlements (hardened runtime, no sandbox).
- `cargo xtask package`: release build → `.app` → `.dmg` (per D5).
- `.github/workflows/release.yml` on `vX.Y.Z`: build → sign (Developer ID, temp keychain, deleted after) →
  `notarytool` with App Store Connect API key → `stapler` → upload to Releases. Secrets only in GitHub Actions,
  not available to fork workflows. Nothing secret in the repo.
- Release build guard: `mcp` feature absent (same check as CI).

**Verify**
- Dry-run tag (e.g. `v0.0.1-rc1`) produces a release.
- On another Mac / clean user: `spctl -a -vv` accepts, `xcrun stapler validate` passes, app opens without Gatekeeper warning.
- `strings`/feature check confirms no MCP server in the shipped binary.

---

## Phase 9: v1 acceptance

- PRD walk-through, feature by feature, in test mode via MCP (screenshots per section of PRD §5).
- Real disk, **done by the user, not the agent**: scan only, review classifications and sizes vs Finder / `du`,
  then a user-chosen small Safe cleanup via Trash.
- Scan-time benchmark on real disk recorded in README. README: install steps, FDA note, "Made with Slint" badge, screenshots.
- Tag `v1.0.0`.

---

## Cross-cutting rules (every phase)

- Safety invariants 1-8 from `AGENTS.md` are tested, not just followed. A new deletion path without a proptest is not done.
- Agent never runs cleaning outside `FDC_TEST_ROOT`. No banner → stop.
- No network, no telemetry. New dependency with network I/O → ask first.
- Conventional commits, staged per task, Done gate before every PR.

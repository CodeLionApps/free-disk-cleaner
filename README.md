# FreeDiskCleaner

[![Made with Slint](https://raw.githubusercontent.com/slint-ui/slint/master/logo/MadeWithSlint-logo-whitebg.png)](https://slint.dev)

Free, open-source (MIT) disk cleaner for developers on macOS (Apple Silicon, macOS 13+).
Scans the disk, recognizes developer junk (Xcode, simulators, Android, Docker, `node_modules`,
package-manager caches, Homebrew), rates every item **Safe / Caution / Do not touch** with a
deterministic rule table, and deletes safely (Trash by default).

No accounts, no network, no telemetry.

## Install

Download the signed and notarized `.dmg` from GitHub Releases, drag the app to Applications, open it.
On first run the app asks for **Full Disk Access** (System Settings → Privacy & Security → Full Disk Access);
without it some locations (e.g. Time Machine data, other apps' containers) can't be measured.

## Build from source

```sh
cargo build --release -p app
cargo xtask package          # .app + .dmg in target/package (ad-hoc signed unless an identity is given)
cargo xtask package --sign "Developer ID Application: ..." --notarize   # needs APPLE_API_KEY_PATH/_ID/_ISSUER
```

## Release

Push a tag `vX.Y.Z` (or `vX.Y.Z-rc1` for a pre-release). `.github/workflows/release.yml` runs the Done gate,
signs with Developer ID, notarizes, staples and uploads the `.dmg` to GitHub Releases. Signing material lives
only in GitHub Actions secrets: `MACOS_CERT_P12_BASE64`, `MACOS_CERT_PASSWORD`, `MACOS_SIGN_IDENTITY`,
`APPLE_API_KEY_P8_BASE64`, `APPLE_API_KEY_ID`, `APPLE_API_ISSUER`.

## Development

See `AGENTS.md`. Test mode never touches your real files:

```sh
cargo xtask fixtures                          # generates fixture tree in ./fixtures-out
FDC_TEST_ROOT=$PWD/fixtures-out cargo run -p app
FDC_TEST_ROOT=$PWD/fixtures-out cargo xtask dev-mcp   # + Slint MCP server on :8765 for agents
```

## License

MIT. UI built with [Slint](https://slint.dev) under the Slint Royalty-free license.
Bundled font: [Inter](https://rsms.me/inter/), SIL Open Font License 1.1 (`crates/app/ui/fonts/Inter-LICENSE.txt`).

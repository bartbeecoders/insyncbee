# Changelog

All notable changes to InSyncBee are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] — 2026-05-09

### Added
- **System tray** — InSyncBee now lives in the tray. Left-click toggles the
  main window; right-click opens a menu with **Open InSyncBee** and **Quit**.
- **Close-to-tray** — closing the window hides it instead of quitting, so
  syncing keeps running in the background.
- **Start on login** — new **Settings** tab with a toggle that registers
  InSyncBee to launch with your desktop session (writes a standard XDG
  autostart entry on Linux, LaunchAgent on macOS, registry entry on Windows).
- **Headless boot flags** — pass `--tray`, `--background`, or `--hidden` to
  start without showing the main window. Used automatically by autostart.
- **Per-sync-pair client-side encryption** with OS keyring storage — files are
  encrypted locally before upload; keys never leave your device's keyring.
- **Test pyramid** — unit, integration, CLI smoke, and end-to-end suites
  shipped together for regression coverage.

### Fixed
- **Folder deletes propagate** — folders are now indexed alongside files, so
  deleting a folder locally removes it from Drive instead of having sync
  re-create it on the next pass.
- **Nested uploads land in the right folder** — parent IDs are resolved at
  execute time, fixing a race where a freshly-created remote folder wasn't
  visible to its children's upload tasks.
- **Files added to a previously-synced folder are picked up** — covered by a
  dedicated regression test.

### Changed
- Tray configuration moved from `tauri.conf.json` to programmatic setup,
  so the menu and click handlers can be wired in Rust.

## [0.1.0] — 2026-04-18

Initial release. Bidirectional Google Drive sync for Linux, macOS, and
Windows: account login, multiple sync pairs, file watcher, change journal,
conflict surfacing, resumable chunked uploads with progress events.

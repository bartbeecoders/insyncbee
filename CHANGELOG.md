# Changelog

All notable changes to InSyncBee are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.4] — 2026-08-08

### Fixed
- **The desktop app can find its OAuth credentials.** It read them only from
  environment variables, so an app launched from the desktop menu, a file
  manager, or an autostart entry — none of which read your shell profile —
  failed every login and reconnect with "OAuth credentials not configured",
  even with the exports sitting in `~/.bashrc`. Credentials now fall back to
  `~/.config/insyncbee/credentials.json`, which both launch paths can see.
- Credentials are resolved per operation instead of once at startup, so
  configuring them (or fixing a typo) no longer needs an app restart.

### Added
- **`insyncbee configure`** — writes `~/.config/insyncbee/credentials.json`
  with owner-only permissions. With no arguments it copies whatever is already
  in the environment, so existing shell-profile setups become one command.
- The "not configured" error now names both ways to fix it instead of only
  the environment variables.

## [0.2.3] — 2026-08-08

### Fixed
- **The desktop app now starts.** On machines where WebKitGTK's DMABUF renderer
  can't reach a GPU — NVIDIA drivers, VMs, several Wayland compositors — the
  v0.2.2 app aborted before drawing anything, printing only
  `Could not create GBM EGL display: EGL_SUCCESS. Aborting...`. It now sets
  `WEBKIT_DISABLE_DMABUF_RENDERER=1` at startup unless you set it yourself, so
  `WEBKIT_DISABLE_DMABUF_RENDERER=0` still opts back into the accelerated path.
  `scripts/dev-gui.sh` had always exported this for development; the shipped
  app never did.
- Documentation said the tray icon needed an AppIndicator library installed by
  hand. It doesn't: the AppImage bundles `libayatana-appindicator3.so.1`, and
  the `.deb`/`.rpm` declare it as a dependency. Only source builds need it.

### Added
- The release pipeline now launches the built AppImage under Xvfb and fails if
  it exits within 20 seconds — a runner has no GPU, so it reproduces exactly
  the configuration that broke v0.2.2.

## [0.2.2] — 2026-08-08

### Added
- **The desktop app is now a download.** Every release builds the Tauri app —
  the one with the window and the tray icon — for Linux x86_64 and publishes
  it as `.AppImage`, `.deb`, and `.rpm` with checksums. Until now the only
  published artifact was the headless db-service, so there was no way to get
  the UI without building it yourself.
- **The download page offers both products**, with the desktop app first and
  the db-service described for what it is: the same sync engine with no UI,
  for servers and headless boxes.

### Fixed
- `docs/USAGE.md` and `scripts/build.sh` pointed at `target/release/insyncbee-daemon`
  and `src-tauri/target/release/bundle/`. The binary is `target/release/insyncbee`
  and, because the workspace shares one target dir, bundles land in
  `target/release/bundle/`.
- `scripts/upload-release.sh` read its version from a `releases.json` that does
  not exist at the repo root, so `VERSION` was always empty.

### Removed
- `insyncbee.portal/releases.json` — an unused duplicate of `src/data/releases.ts`
  that still advertised an Intel Mac build the pipeline no longer produces.

## [0.2.1] — 2026-08-08

### Fixed
- **Conflicts now converge** — resolving a conflict records the outcome as the
  new base state. Previously every sync cycle recomputed the same conflict
  from the same stale base: **Keep Both** created a new timestamped copy on
  every poll interval, and the overwrite policies re-transferred the same file
  forever. Conflicts where one side was deleted stay pending on purpose —
  there is no base state for them that doesn't tell the next cycle to finish
  the deletion.
- **Adopting a folder that already has files no longer conflicts everything** —
  the first-sync comparison hashed the local file with blake3 and compared it
  against Drive's MD5, so identical files looked divergent and each got a
  conflicted copy. That one cross-boundary comparison now uses MD5.
- **Hidden files are ignored on both sides** — a remote dot-file was downloaded
  and then skipped by the local scanner, so the next cycle read it as a local
  deletion and trashed it on Drive.

### Added
- **Live end-to-end test harness** (`tests/e2e`) that runs against a real
  Google Drive account, opt-in via `INSYNCBEE_E2E=1`, each scenario sandboxed
  under `.insyncbee-e2e/` on both sides.
- **`tests/SCENARIOS.md`** — the full scenario catalogue, marking what is
  covered live, covered against the fake backend, manual, or a known gap.

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

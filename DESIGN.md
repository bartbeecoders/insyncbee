# InSyncBee — Design Document

## 1. Vision

InSyncBee is a fast, lightweight, cross-platform (Linux, macOS, Windows) Google Drive sync application built in Rust. It targets the gap left by Google's refusal to ship a Linux client and the reliability problems plaguing Insync (data loss, memory leaks, stale development). InSyncBee aims to be the first Google Drive sync client that combines **block-level delta sync**, **on-demand file placeholders**, and **rock-solid conflict resolution** — features no existing tool offers together.

**Tagline:** *Your files. Your rules. No surprises.*

---

## 2. Competitive Landscape

| Feature | Google Drive Desktop | Insync | rclone | odrive | **InSyncBee** |
|---|---|---|---|---|---|
| Linux support | No | Yes | Yes | Yes | **Yes** |
| Native GUI | Yes | Yes | No | Yes | **Yes** |
| Block-level delta sync | No | No | No | No | **Yes** |
| On-demand placeholders | Yes (buggy) | No | Via mount | Yes (.cloud stubs) | **Yes** |
| Bidirectional sync | Partial | Yes | bisync (fragile) | Yes | **Yes** |
| Real-time sync | Yes | Yes | No (cron) | Yes | **Yes** |
| Conflict preview & resolution | No | Basic | Flags only | No | **Yes** |
| Google Docs conversion | Native | Yes | Export/import | Via provider | **Yes** |
| Bandwidth control | Via admin | No | Yes | No | **Yes** |
| Open source | No | No | Yes | No | **Yes** |
| Price | Free | $40/account | Free | $150/yr | **Free** |

### Key Insync Weaknesses to Exploit
- **Data loss**: Insync's #1 user complaint — files disappearing, folders deleted unexpectedly. InSyncBee must make data safety its core identity.
- **No bandwidth throttling**: Insync saturates connections. Easy win.
- **No file placeholders/streaming**: Insync downloads everything. Wastes disk.
- **Slow development**: Insync v3 shipped in 2019 with incremental patches since. The codebase feels stagnant.
- **Memory leaks**: Historical Insync problem. Rust's ownership model eliminates this class of bug entirely.

---

## 3. Architecture Overview

```
┌──────────────────────────────────────────────────────────────┐
│                        Tauri Shell                           │
│  ┌────────────────────────────────────────────────────────┐  │
│  │              Frontend (SolidJS + TypeScript)            │  │
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐  │  │
│  │  │ Dashboard │ │  Sync    │ │ Conflict │ │ Settings │  │  │
│  │  │   View   │ │ Browser  │ │ Resolver │ │  Panel   │  │  │
│  │  └──────────┘ └──────────┘ └──────────┘ └──────────┘  │  │
│  └──────────────────────┬─────────────────────────────────┘  │
│                    Tauri IPC                                  │
│  ┌──────────────────────┴─────────────────────────────────┐  │
│  │                  Rust Backend                           │  │
│  │  ┌─────────────┐  ┌──────────────┐  ┌──────────────┐  │  │
│  │  │ Sync Engine │  │ Google Drive  │  │   Conflict   │  │  │
│  │  │  Scheduler  │  │  API Client   │  │   Manager    │  │  │
│  │  └──────┬──────┘  └──────┬───────┘  └──────┬───────┘  │  │
│  │  ┌──────┴──────┐  ┌──────┴───────┐  ┌──────┴───────┐  │  │
│  │  │    File     │  │   OAuth2     │  │  Placeholder  │  │  │
│  │  │   Watcher   │  │   Manager    │  │   Manager     │  │  │
│  │  └──────┬──────┘  └──────────────┘  └──────────────┘  │  │
│  │  ┌──────┴──────────────────────────────────────────┐   │  │
│  │  │            State Database (SQLite)               │   │  │
│  │  │  file_index | sync_pairs | change_log | config  │   │  │
│  │  └─────────────────────────────────────────────────┘   │  │
│  └────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────┘
```

### Technology Choices

| Layer | Choice | Rationale |
|---|---|---|
| GUI framework | **Tauri v2** | Most mature Rust desktop framework. System tray, notifications, auto-update built in. Small binaries (~5 MB). |
| Frontend | **SolidJS + TypeScript** | Finest-grained reactivity (no virtual DOM), tiny bundle, fast. Good for real-time sync status updates. |
| Async runtime | **Tokio** | Industry standard for async Rust. Required by reqwest, notify, etc. |
| HTTP client | **reqwest** | Mature, async, TLS support. Direct REST API calls to Drive v3. |
| Google Drive API | **reqwest + serde** (manual) | Cleaner than auto-generated `google-drive3`. We only need ~10 endpoints. |
| OAuth2 | **oauth2** crate | More idiomatic than `yup-oauth2`. Supports installed-app loopback flow. |
| File watching | **notify + notify-debouncer-full** | Cross-platform, debounced, rename-aware. |
| Hashing | **blake3** | SIMD-optimized, 3x faster than SHA-256. Content-addressable change detection. |
| Delta sync | **fast_rsync** (by Dropbox) | Pure Rust rsync rolling-checksum. Proven at scale. |
| Chunking | **fastcdc** | Content-defined chunking for efficient large-file transfer. |
| Local database | **rusqlite** (SQLite) | Single-file, zero-config, ACID. Syncthing v2 migrated to SQLite for the same reasons. |
| Compression | **zstd** | Fast compression before upload. Saves bandwidth. |
| Logging | **tracing** | Structured, async-aware, span-based. |

---

## 4. Core Features

### 4.1 Sync Engine

#### Sync Pair Model
A sync pair binds a local directory to a Google Drive folder. Users can create multiple independent pairs. Each pair has its own:
- Sync mode (two-way, local-to-cloud, cloud-to-local)
- Conflict policy
- Ignore rules
- Schedule (continuous, interval, or manual)

#### Change Detection
**Local changes:** `notify` + `notify-debouncer-full` watches the filesystem in real time. On app startup, a full scan compares the filesystem against the SQLite index to catch offline changes.

**Remote changes:** Google Drive's Changes API with `startPageToken`. Poll every 30 seconds (configurable). Each poll fetches only changes since the last token, making it efficient even for large Drives.

**Change comparison logic** (three-way):
```
Local state vs. Base state (last-synced) vs. Remote state

- Only local changed  → Upload
- Only remote changed → Download
- Both changed, same content (hash match) → No-op
- Both changed, different content → CONFLICT
- Local deleted, remote unchanged → Delete remote (or archive)
- Remote deleted, local unchanged → Delete local (or archive)
- Both deleted → Remove from index
```

#### Delta Sync (Novel — No Competitor Does This for Google Drive)
For files > 1 MB that have changed:
1. Compute `fast_rsync` signature of the base version (stored locally in a chunk cache).
2. Compute delta between the new version and the base signature.
3. If delta < 50% of file size, store the delta locally and upload the full new file via resumable upload (Google Drive doesn't accept deltas, but the delta is used to optimize the **local-side** of the sync — fast diff computation, bandwidth estimation, and progress reporting).
4. For **local downloads**, if we have a cached base version, apply the delta to reconstruct the file without downloading the full file again.

This is most valuable for large files (videos, archives, database dumps) where remote edits are incremental.

#### Content-Defined Chunking for Deduplication
Use `fastcdc` to chunk files. Store chunk hashes in SQLite. When a file moves or is copied, detect that the chunks already exist and avoid re-uploading. This also enables:
- **Instant folder copy detection** — if a user duplicates a folder locally, InSyncBee recognizes the content is already on Drive and creates server-side copies instead of re-uploading.
- **Move detection** — a file that disappears from one path and appears at another with the same hash is a move, not a delete+create.

### 4.2 File Placeholders (On-Demand Files)

**The problem:** Users with 100 GB+ Google Drives can't sync everything to a laptop with 256 GB SSD.

**The solution:** Placeholder files. A placeholder is a small local file (few hundred bytes) that looks like the real file in the file manager but doesn't contain the full data. When opened, InSyncBee transparently downloads the real content.

#### Implementation

**Linux:** Use FUSE (via the `fuser` crate) to mount a virtual filesystem overlay. Files appear with correct names, sizes, and dates. Read operations trigger on-demand download. A local LRU cache stores recently accessed files.

**macOS:** Use Apple's FileProvider framework (available since macOS 11). This is the same mechanism iCloud Drive uses for on-demand files. Integrates natively with Finder (cloud badge icons, right-click "Download Now").

**Windows:** Use Windows Cloud Files API (CfRegisterSyncRoot). This is what OneDrive uses. Files show cloud/pinned status in Explorer natively.

#### Placeholder States
- **Cloud-only** ☁️ — metadata only, no local content
- **Downloading** ⬇️ — content being fetched
- **Available** ✓ — full content cached locally
- **Pinned** 📌 — always keep local (user-pinned, never evict)
- **Evicting** — cache pressure, moving back to cloud-only

#### Cache Management
- LRU eviction when cache exceeds user-configured limit
- "Pin" individual files or folders to always keep locally
- Background pre-fetch of recently created files (likely to be opened soon)
- Smart eviction: never evict files with unsaved changes or files opened by another process

### 4.3 Conflict Resolution

Conflicts are the #1 source of data loss in sync tools. InSyncBee takes a **zero-data-loss** approach.

#### Conflict Detection
A conflict occurs when both the local and remote versions of a file have changed since the last sync. Detection uses the three-way comparison (Section 4.1).

#### Resolution Strategies (User-Configurable Per Sync Pair)

| Strategy | Behavior |
|---|---|
| **Ask me** (default) | Queue the conflict, notify the user, show a diff/comparison view |
| **Keep both** | Create a conflicted copy: `report (conflict 2026-04-15 14.30.22).docx` |
| **Prefer local** | Local version wins, remote is overwritten |
| **Prefer remote** | Remote version wins, local is overwritten |
| **Newest wins** | Most recent modification time wins |

#### Conflict Resolution UI (Novel)
When strategy is "Ask me", the conflict resolver shows:
- **Side-by-side file metadata** (size, modified date, who modified on Drive)
- **Visual diff for text files** (unified or split view)
- **Image comparison** for image files (slider overlay, side-by-side, or blink comparison)
- **"Keep both" / "Keep left" / "Keep right"** buttons
- Batch resolution: apply the same choice to all conflicts in a folder

#### Safety Net: The Conflict Archive
Every overwritten file version is saved to a hidden `.insyncbee/archive/` directory (locally) with a 30-day retention. This means even "Prefer remote" or "Newest wins" strategies are reversible within the retention window.

### 4.4 Google Docs Handling

Google Docs, Sheets, and Slides are not real files — they're cloud-native documents with zero bytes on Drive. InSyncBee handles them as follows:

- **Default:** Export as Office formats (`.docx`, `.xlsx`, `.pptx`) for local editing. Re-upload converts back.
- **Alternative:** Export as `.gdoc`/`.gsheet`/`.gslides` shortcut files that open in the browser when double-clicked.
- **Configurable per sync pair** — power users can choose ODF (`.odt`, `.ods`, `.odp`) or PDF export.

Round-trip editing: when a locally-edited `.docx` is synced back, InSyncBee uploads it as a new revision of the original Google Doc (not as a separate file), preserving sharing permissions and comment history.

### 4.5 Ignore Rules

`.insyncbee-ignore` files (one per directory, inherited by subdirectories) using gitignore syntax:

```gitignore
# Editor temp files
*.swp
*~
.#*

# OS junk
.DS_Store
Thumbs.db
desktop.ini

# Build artifacts
node_modules/
target/
__pycache__/

# Large media (sync manually)
*.iso
*.dmg
```

Additionally, a global ignore list in settings for patterns that apply to all sync pairs.

### 4.6 Bandwidth Management

- **Upload/download speed limits** — configurable in KB/s or MB/s, or "unlimited"
- **Schedule-based limits** — e.g., unlimited at night, 1 MB/s during work hours
- **Per-network profiles** — auto-apply limits when on metered WiFi vs. Ethernet
- **Pause/resume** — global and per-sync-pair

### 4.7 Multi-Account Support

Multiple Google accounts, each with their own sync pairs. Accounts are authenticated independently. No per-account licensing — InSyncBee is free and open source.

---

## 5. Novel Ideas

### 5.1 Sync Snapshots (Time Machine for Google Drive)

**The idea:** Periodically (daily by default), InSyncBee takes a "snapshot" — a lightweight record of every file's hash, path, and remote revision ID. Snapshots are stored in SQLite and cost almost zero disk space (just metadata).

**What this enables:**
- **"What changed since Tuesday?"** — browse any snapshot and see a diff of files added, removed, or modified.
- **Point-in-time restore** — select a snapshot and restore any file or folder to its state at that time (pulls the old revision from Google Drive's version history or from the local conflict archive).
- **Accidental delete recovery** — if a file vanishes, snapshots show exactly when it disappeared and from which side (local or remote), making recovery trivial.

No competing tool offers this. rclone can't do it (no persistent state). Insync doesn't track historical state. Google Drive has version history per-file but no folder-level snapshots.

### 5.2 Sync Rules Engine

Beyond simple ignore patterns, InSyncBee offers a rules engine for power users:

```yaml
rules:
  # Auto-organize photos by date
  - match: "Camera Upload/*.jpg"
    action: move
    destination: "Photos/{exif:year}/{exif:month}/"

  # Compress old logs before uploading
  - match: "logs/*.log"
    condition: "age > 7d"
    action: compress
    format: zstd

  # Convert HEIC to JPEG on sync
  - match: "**/*.heic"
    action: convert
    format: jpeg
    quality: 90

  # Notify on shared folder changes
  - match: "Shared Projects/**"
    action: notify
    message: "{user} modified {file}"
```

Rules are defined in `.insyncbee-rules.yaml` at the sync pair root. They're processed after change detection but before the actual sync operation.

### 5.3 LAN-First Sync

**The idea:** If the same Google Drive account is synced on two machines on the same LAN, file transfers happen directly between machines at LAN speed (gigabit) instead of going cloud → download (limited by internet speed).

**How it works:**
1. InSyncBee instances on the same LAN discover each other via mDNS/DNS-SD.
2. When machine A uploads a file, it broadcasts the file hash + metadata to LAN peers.
3. Machine B, which also syncs the same Drive folder, checks if it needs that file.
4. If yes, B pulls the file directly from A over a local encrypted channel (TLS with pre-shared key derived from the Google OAuth token).
5. B then marks the file as synced without downloading from Google.

**Why this matters:** A 2 GB video file that takes 10 minutes to download from Google takes 16 seconds on a gigabit LAN. For offices or homes with multiple synced machines, this is transformative.

Syncthing and Dropbox both do this. No Google Drive client does.

### 5.4 Integrity Verification (Paranoid Mode)

A toggleable "paranoid mode" for users who've been burned by data loss (Insync refugees):

- After every upload, immediately download the file's metadata and checksum from Google Drive and verify it matches the local hash.
- Weekly full-integrity scan: hash every local file and compare against Drive metadata.
- If any mismatch is detected, pause sync and alert the user with a detailed report.
- All verification results are logged for audit.

This directly addresses Insync's biggest weakness — users not trusting that their files actually made it to the cloud intact.

### 5.5 Smart Sync Scheduling

Instead of a fixed poll interval, InSyncBee adapts:

- **Active editing detection:** When the user is actively saving files in a synced folder, increase poll frequency to 5 seconds for near-real-time sync.
- **Idle detection:** When no local changes for 10 minutes, slow polls to every 2 minutes to save API quota and battery.
- **Battery awareness:** On laptops running on battery, reduce sync frequency and pause large uploads until plugged in (unless pinned as urgent).
- **Network quality adaptation:** On slow or metered connections, batch small files and prioritize recently-touched files.

### 5.6 Sync Dashboard with Real Insight

Not just a progress bar — a dashboard that answers real questions:

- **"What's taking so long?"** — a ranked list of files currently syncing, with speed and ETA per file.
- **"What changed?"** — an activity feed showing uploads, downloads, renames, deletes, and conflicts with timestamps.
- **"Am I in sync?"** — a single status indicator with three states: Synced, Syncing (N files remaining), or Attention Needed (N conflicts / errors).
- **"How much bandwidth am I using?"** — real-time upload/download speed graph.
- **"What's using my disk?"** — breakdown of local cache vs. pinned files vs. placeholder files.
- **Storage quota** — Google Drive usage shown alongside local disk usage.

### 5.7 Portable Sync Profiles

Export/import sync configurations as a `.insyncbee-profile.toml` file:

```toml
[profile]
name = "Work Setup"

[[sync_pairs]]
local = "~/Documents/Work"
remote = "/Work Documents"
mode = "two-way"
conflict = "ask"
ignore = ["*.tmp", "~$*"]

[[sync_pairs]]
local = "~/Photos"
remote = "/Photos"
mode = "local-to-cloud"
schedule = "daily 02:00"
```

Use case: set up a new machine in seconds by importing a profile. Share configurations across a team.

### 5.8 Dry Run Mode

Before activating a new sync pair or changing settings, run a "dry run" that simulates the sync and shows exactly what would happen:

```
Dry Run Results for "Work Documents" (two-way sync):
  ↑ Upload:    47 files (312 MB)
  ↓ Download:  23 files (1.2 GB)
  ⚡ Conflict:   3 files
  🗑 Delete:     0 files

Review changes before proceeding? [Start Sync] [Review Files] [Cancel]
```

FreeFileSync and Cyberduck offer preview-before-sync. No real-time sync tool does. This builds trust, especially for first-time setup.

### 5.9 Extension / Plugin System

A lightweight plugin API (Rust trait-based, loaded as dynamic libraries or WASM modules) for extending sync behavior:

- **Encryption plugin** — AES-256-GCM encrypt files before upload, decrypt on download. Zero-knowledge cloud storage.
- **Thumbnail generator** — generate thumbnails for images/videos and attach as Drive properties for faster browsing.
- **Webhook notifier** — POST to a URL when sync events occur (for integrating with CI/CD, Slack, etc.).
- **Custom conflict resolver** — programmatic conflict resolution for specific file types.

Ship with encryption and webhook notifier as first-party plugins. Open the API for community extensions.

### 5.10 CLI-First with GUI on Top

The sync engine is a standalone daemon (`insyncbee-daemon`) controllable via CLI (`insyncbee`):

```bash
# Add a sync pair
insyncbee add --local ~/Documents --remote "/My Documents" --mode two-way

# Check status
insyncbee status
# ✓ "My Documents" — Synced (last: 2 min ago)
# ⟳ "Photos"       — Syncing 3/47 files (12%)

# Trigger immediate sync
insyncbee sync --now

# List conflicts
insyncbee conflicts
# report.docx — modified locally 14:30, remotely 14:22

# Resolve a conflict
insyncbee resolve report.docx --keep local

# Pause/resume
insyncbee pause "My Documents"
insyncbee resume "My Documents"

# Dry run
insyncbee sync --dry-run
```

The GUI (Tauri app) communicates with the daemon over a local Unix socket / named pipe. This means:
- Headless Linux servers can run InSyncBee without a GUI (like rclone, but with real-time sync).
- The daemon survives GUI crashes.
- Multiple UIs could connect (CLI, GUI, web dashboard, even a mobile companion app via the API).

---

## 6. Data Model

### SQLite Schema (Core Tables)

```sql
-- Sync pair configuration
CREATE TABLE sync_pairs (
    id          TEXT PRIMARY KEY,  -- UUID
    name        TEXT NOT NULL,
    local_root  TEXT NOT NULL,
    remote_root TEXT NOT NULL,
    mode        TEXT NOT NULL CHECK (mode IN ('two-way', 'local-to-cloud', 'cloud-to-local')),
    conflict    TEXT NOT NULL DEFAULT 'ask',
    status      TEXT NOT NULL DEFAULT 'active',
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

-- File index (the "base state" for three-way comparison)
CREATE TABLE file_index (
    id              INTEGER PRIMARY KEY,
    sync_pair_id    TEXT NOT NULL REFERENCES sync_pairs(id),
    relative_path   TEXT NOT NULL,
    local_hash      TEXT,           -- blake3 hash of local content
    remote_hash     TEXT,           -- MD5 from Google Drive API
    remote_id       TEXT,           -- Google Drive file ID
    remote_rev      TEXT,           -- Google Drive revision/head revision
    size            INTEGER,
    local_mtime     TEXT,
    remote_mtime    TEXT,
    is_placeholder  INTEGER NOT NULL DEFAULT 0,
    is_pinned       INTEGER NOT NULL DEFAULT 0,
    is_google_doc   INTEGER NOT NULL DEFAULT 0,
    state           TEXT NOT NULL DEFAULT 'synced',  -- synced, modified, conflict, error
    UNIQUE(sync_pair_id, relative_path)
);

-- Change log for activity feed and snapshots
CREATE TABLE change_log (
    id          INTEGER PRIMARY KEY,
    sync_pair_id TEXT NOT NULL REFERENCES sync_pairs(id),
    path        TEXT NOT NULL,
    action      TEXT NOT NULL,  -- upload, download, delete, rename, conflict, error
    detail      TEXT,           -- JSON with extra context
    timestamp   TEXT NOT NULL
);

-- Sync snapshots
CREATE TABLE snapshots (
    id          INTEGER PRIMARY KEY,
    sync_pair_id TEXT NOT NULL REFERENCES sync_pairs(id),
    timestamp   TEXT NOT NULL,
    file_count  INTEGER NOT NULL,
    total_size  INTEGER NOT NULL
);

CREATE TABLE snapshot_entries (
    snapshot_id INTEGER NOT NULL REFERENCES snapshots(id),
    path        TEXT NOT NULL,
    hash        TEXT NOT NULL,
    size        INTEGER NOT NULL,
    remote_rev  TEXT,
    PRIMARY KEY (snapshot_id, path)
);

-- Conflict archive metadata
CREATE TABLE conflict_archive (
    id          INTEGER PRIMARY KEY,
    sync_pair_id TEXT NOT NULL REFERENCES sync_pairs(id),
    path        TEXT NOT NULL,
    hash        TEXT NOT NULL,
    archive_path TEXT NOT NULL,   -- path in .insyncbee/archive/
    source      TEXT NOT NULL,    -- 'local' or 'remote'
    created_at  TEXT NOT NULL,
    expires_at  TEXT NOT NULL     -- 30-day retention
);

-- Google OAuth tokens (encrypted at rest)
CREATE TABLE accounts (
    id              TEXT PRIMARY KEY,
    email           TEXT NOT NULL,
    display_name    TEXT,
    access_token    BLOB NOT NULL,   -- encrypted
    refresh_token   BLOB NOT NULL,   -- encrypted
    token_expiry    TEXT NOT NULL,
    change_token    TEXT,            -- Drive Changes API startPageToken
    created_at      TEXT NOT NULL
);
```

---

## 7. Security Model

### Authentication
- OAuth2 "installed application" flow with loopback redirect (`http://127.0.0.1:{random_port}`).
- Tokens encrypted at rest using OS keychain (Linux: libsecret/Secret Service, macOS: Keychain, Windows: Credential Manager) via the `keyring` crate.
- Minimal Google Drive scopes: `drive.file` by default (only files created/opened by InSyncBee). Full `drive` scope available as opt-in for users who want to sync their entire Drive.

### Local Security
- SQLite database encrypted with SQLCipher (optional, for users on shared machines).
- Daemon socket is user-owned (Unix: `0600` permissions, Windows: named pipe with user-only ACL).
- No telemetry, no analytics, no network calls except to Google APIs and LAN discovery.

### Encryption Plugin (Optional)
- AES-256-GCM encryption of file content before upload.
- Key derived from user passphrase via Argon2id.
- File names optionally encrypted (base64-encoded ciphertext).
- Google sees only encrypted blobs — zero-knowledge storage.

---

## 8. Platform-Specific Considerations

### Linux
- **File manager integration:** Nautilus/Nemo/Dolphin extensions for sync status emblems (overlay icons).
- **Systemd service:** `insyncbee-daemon` ships as a systemd user service for headless operation.
- **Tray icon:** via `libappindicator` (Ubuntu/GNOME) or XEmbed (older DEs). Tauri v2 handles this.
- **FUSE for placeholders:** requires `fuse3` dev libraries. Graceful fallback to full-sync if FUSE unavailable.
- **Packaging:** `.deb`, `.rpm`, Flatpak, AppImage, AUR (Arch).

### macOS
- **FileProvider:** Native on-demand file support with Finder integration.
- **Notarization:** Required for distribution outside App Store.
- **Spotlight integration:** Index synced file metadata for search.
- **Packaging:** `.dmg`, Homebrew cask.

### Windows
- **Cloud Files API:** Native placeholder support with Explorer integration.
- **Windows Service:** Optional — run as a service for always-on sync without a logged-in user.
- **Context menu:** Shell extension for right-click sync actions.
- **Packaging:** `.msi`, WinGet, Chocolatey.

---

## 9. User Interface Design

### System Tray (Primary Interface)
Most of the time, InSyncBee lives in the system tray. The tray icon changes to reflect status:
- **Green bee** 🟢 — all synced
- **Animated bee** 🔄 — syncing in progress
- **Yellow bee** 🟡 — attention needed (conflicts or warnings)
- **Red bee** 🔴 — error (auth failure, network down, disk full)

Clicking the tray icon opens a compact popover:
- Current sync status per pair
- Recent activity (last 5 events)
- Quick actions: Pause All, Sync Now, Open Dashboard

### Main Window (On Demand)
Opened from tray or via CLI (`insyncbee ui`). Four tabs:

**1. Dashboard** — overview of all sync pairs, storage usage, bandwidth graph, quick health check.

**2. Files** — file browser showing synced files with status icons. Can browse both local and remote trees. Search across all synced files.

**3. Activity** — chronological feed of all sync events. Filterable by sync pair, action type, date range. Exportable.

**4. Conflicts** — list of unresolved conflicts with side-by-side comparison and resolution buttons.

### First-Run Wizard
1. Sign in with Google (OAuth flow opens browser)
2. Choose a local folder (or create one)
3. Browse and select a Google Drive folder
4. Choose sync mode (two-way / one-way) with plain-language explanation
5. **Dry run** — show what will happen on first sync
6. Start syncing

### Design Principles
- **No surprises:** Every destructive action (delete, overwrite) requires confirmation or shows a preview first.
- **Progressive disclosure:** Simple defaults, advanced settings tucked away but accessible.
- **Status always visible:** The tray icon + popover should answer "am I in sync?" in under 2 seconds.
- **Keyboard-first CLI:** Everything the GUI does, the CLI does too.

---

## 10. Performance Targets

| Metric | Target |
|---|---|
| Memory usage (idle, 10k files indexed) | < 50 MB |
| Memory usage (active sync, 1000 files) | < 150 MB |
| CPU usage (idle) | < 1% |
| Time to detect local change | < 2 seconds |
| Time to detect remote change | ≤ poll interval (30s default) |
| Small file upload (< 1 MB) | < 3 seconds (API overhead) |
| Large file upload (100 MB) | Limited by bandwidth, not CPU |
| Startup time (daemon) | < 1 second |
| Startup time (GUI) | < 2 seconds |
| SQLite index for 100k files | < 50 MB on disk |

---

## 11. Development Phases

### Phase 1 — Foundation (MVP)
- Project scaffolding: Tauri v2 + SolidJS + Rust workspace
- Google OAuth2 flow (sign in, token refresh, token storage)
- Google Drive API client (list, upload, download, changes)
- SQLite database with file index
- One-way sync: local → cloud (upload only)
- One-way sync: cloud → local (download only)
- Basic CLI: `add`, `status`, `sync`
- System tray with status icon

### Phase 2 — Real Sync
- Two-way bidirectional sync with three-way change detection
- File watcher (notify + debouncer) for real-time local changes
- Conflict detection and "keep both" resolution
- Ignore rules (`.insyncbee-ignore`)
- Bandwidth throttling
- Activity feed and change log
- Basic GUI dashboard

### Phase 3 — Placeholders & Polish
- File placeholders (FUSE on Linux, FileProvider on macOS, Cloud Files API on Windows)
- Placeholder cache management (LRU eviction, pinning)
- Conflict resolution UI (side-by-side comparison)
- Google Docs export/import with round-trip editing
- Multiple accounts
- Dry run mode
- Sync snapshots

### Phase 4 — Advanced Features
- LAN-first sync (mDNS discovery, peer-to-peer transfer)
- Delta sync with `fast_rsync`
- Smart sync scheduling (adaptive polling)
- Sync rules engine
- Plugin system (encryption, webhooks)
- Portable sync profiles (export/import)
- Integrity verification (paranoid mode)

### Phase 5 — Distribution
- Linux packages: .deb, .rpm, Flatpak, AppImage, AUR
- macOS: .dmg, Homebrew, notarization
- Windows: .msi, WinGet, Chocolatey
- Auto-update mechanism
- Documentation site
- Community plugin repository

---

## 12. Risks and Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Google Drive API rate limits (2 files/sec) | Slow sync for large initial imports | Batch API calls (`--fast-list` style). Exponential backoff. Show clear progress. |
| Google Docs round-trip conversion fidelity | Formatting loss when editing exported docs locally | Warn users about potential formatting changes. Default to shortcut files for docs. |
| FUSE complexity on Linux | Placeholder bugs, kernel version issues | Ship FUSE support as optional. Full sync works without it. |
| Platform-specific placeholder APIs | 3 different implementations to maintain | Abstract behind a `PlaceholderProvider` trait. Each platform implements it. |
| OAuth token revocation/expiry | Sync stops without user noticing | Proactive token refresh. Clear notification + re-auth flow if refresh fails. |
| SQLite corruption during unexpected shutdown | Loss of sync state, potential re-download of everything | WAL mode + periodic checkpoints. Database backup before schema migrations. |
| Data loss perception (Insync's biggest problem) | Users afraid to trust a new sync tool | Paranoid mode. Dry runs. Conflict archive. Extensive logging. "No silent deletes" policy. |

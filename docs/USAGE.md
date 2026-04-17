# InSyncBee Usage Guide

## Prerequisites

- **Rust** (1.77.2+) — [rustup.rs](https://rustup.rs)
- **pnpm** — `npm install -g pnpm`
- **Tauri CLI** — `cargo install tauri-cli` (for GUI development)
- **System libraries** (Linux): `sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev` (Debian/Ubuntu) or equivalent for your distro

## Google OAuth Setup

InSyncBee needs Google OAuth credentials to access Google Drive. You must create your own credentials:

1. Go to [Google Cloud Console](https://console.cloud.google.com/)
2. Create a new project (or use an existing one)
3. Enable the **Google Drive API** and **Google OAuth2 API**
4. Go to **Credentials** > **Create Credentials** > **OAuth client ID**
5. Application type: **Desktop app**
6. Download the credentials

Set the credentials as environment variables:

```bash
export INSYNCBEE_CLIENT_ID="your-client-id.apps.googleusercontent.com"
export INSYNCBEE_CLIENT_SECRET="your-client-secret"
```

Add these to your shell profile (`~/.bashrc`, `~/.zshrc`, etc.) so they persist across sessions.

## Quick Start

```bash
# 1. Install dependencies
./scripts/setup.sh

# 2. Set OAuth credentials (see above)
export INSYNCBEE_CLIENT_ID="..."
export INSYNCBEE_CLIENT_SECRET="..."

# 3. Sign in with Google
./scripts/dev-cli.sh login

# 4. Add a sync pair
./scripts/dev-cli.sh add \
    --name "My Documents" \
    --local ~/Documents/Drive \
    --remote-id root \
    --account <account-id-from-step-3>

# 5. Run a sync
./scripts/dev-cli.sh sync
```

## Scripts

All scripts are in the `scripts/` directory and should be run from the project root.

| Script | Purpose |
|---|---|
| `scripts/setup.sh` | Install all dependencies (Rust crates + frontend npm packages) |
| `scripts/dev-cli.sh` | Run the CLI in dev mode. All arguments are forwarded to the binary |
| `scripts/dev-gui.sh` | Run the Tauri desktop app with hot-reload |
| `scripts/build.sh` | Build release binaries for both CLI and GUI |

## CLI Reference

The CLI binary is `insyncbee-daemon` (run via `./scripts/dev-cli.sh <command>`).

### Account management

```bash
# Sign in with Google (opens browser)
./scripts/dev-cli.sh login

# List connected accounts
./scripts/dev-cli.sh accounts

# Remove an account
./scripts/dev-cli.sh logout <account-id>
```

### Sync pair management

```bash
# Add a sync pair
./scripts/dev-cli.sh add \
    --name "Work Docs" \
    --local ~/Work \
    --remote-id <google-drive-folder-id> \
    --remote-path "/Work" \
    --account <account-id> \
    --mode two-way          # two-way | local-to-cloud | cloud-to-local

# List sync pairs
./scripts/dev-cli.sh list

# Show status of all pairs
./scripts/dev-cli.sh status

# Pause / resume a pair
./scripts/dev-cli.sh pause <pair-id>
./scripts/dev-cli.sh resume <pair-id>

# Remove a pair
./scripts/dev-cli.sh remove <pair-id>
```

### Syncing

```bash
# Sync all active pairs
./scripts/dev-cli.sh sync

# Sync a specific pair
./scripts/dev-cli.sh sync <pair-id>

# Dry run (preview what would happen, no changes made)
./scripts/dev-cli.sh sync --dry-run
```

### Daemon mode

Run InSyncBee as a background daemon that watches for local file changes and polls Google Drive for remote changes:

```bash
./scripts/dev-cli.sh daemon
```

The daemon will:
- Perform an initial sync for all active pairs on startup
- Watch local folders for file changes (triggers immediate sync)
- Poll Google Drive for remote changes at each pair's configured interval (default: 30 seconds)
- Respect pause/resume status of individual pairs
- Shut down cleanly on Ctrl+C

### Finding Google Drive folder IDs

The `--remote-id` parameter requires a Google Drive folder ID. To find it:

- Use `root` for the top-level "My Drive" folder
- For other folders: open the folder in Google Drive in your browser. The URL will be `https://drive.google.com/drive/folders/<folder-id>` — copy the `<folder-id>` part

## GUI

Start the desktop app in development mode:

```bash
./scripts/dev-gui.sh
```

The GUI provides:

- **Dashboard** — view connected accounts, sync pair status, trigger syncs, pause/resume pairs, and add new Google accounts
- **Activity** — chronological feed of all sync events (uploads, downloads, deletes, conflicts)
- **Conflicts** — list of unresolved conflicts with resolution buttons (Keep Local / Keep Remote / Keep Both)

## Building for Release

```bash
./scripts/build.sh
```

This produces:
- CLI binary: `target/release/insyncbee-daemon`
- GUI bundles: `src-tauri/target/release/bundle/` (`.deb`, `.AppImage`, etc.)

## Data Location

InSyncBee stores its database and logs in your platform's data directory:

| Platform | Path |
|---|---|
| Linux | `~/.local/share/insyncbee/` |
| macOS | `~/Library/Application Support/insyncbee/` |
| Windows | `C:\Users\<user>\AppData\Roaming\insyncbee\` |

- `insyncbee.db` — SQLite database (accounts, sync pairs, file index, change log)
- `logs/` — application logs

## Environment Variables

| Variable | Required | Description |
|---|---|---|
| `INSYNCBEE_CLIENT_ID` | Yes | Google OAuth client ID |
| `INSYNCBEE_CLIENT_SECRET` | Yes | Google OAuth client secret |
| `RUST_LOG` | No | Log level filter (e.g. `info`, `debug`, `insyncbee_core=debug`) |

## Troubleshooting

**"INSYNCBEE_CLIENT_ID env var not set"**
Set the OAuth credentials as described in the Google OAuth Setup section above.

**Login opens browser but nothing happens**
Make sure no firewall is blocking localhost connections. The OAuth callback uses a random local port (`127.0.0.1:<port>`).

**Sync errors with 401/403**
Your access token may have expired. The app refreshes tokens automatically, but if the refresh token itself is revoked, run `./scripts/dev-cli.sh login` again.

**Database issues**
The SQLite database is at `~/.local/share/insyncbee/insyncbee.db`. You can delete it to start fresh (you'll need to log in and reconfigure sync pairs).

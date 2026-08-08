use insyncbee_core::auth::{AuthManager, OAuthCredentials};
use insyncbee_core::crypto::{self, FileCipher};
use insyncbee_core::db::models::{
    Account, ChangeLogEntry, ConflictPolicy, FileEntry, FileState, SyncMode, SyncPair,
    SyncPairStatus, TransferStats,
};
use insyncbee_core::db::Database;
use insyncbee_core::drive::{HttpDriveClient, TransferKind};
use insyncbee_core::keystore;
use std::sync::Arc;
use tauri::Emitter;
use insyncbee_core::sync_engine::{SyncEngine, SyncStatus};
use insyncbee_core::AppPaths;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, State, WindowEvent};
use tauri_plugin_autostart::{ManagerExt, MacosLauncher};

struct AppState {
    db: Database,
    /// Credentials are resolved per call rather than cached at startup, so
    /// configuring them (or fixing a typo) takes effect without restarting
    /// the app.
    credentials_path: std::path::PathBuf,
}

type DbState = Mutex<AppState>;

// ── Tauri Commands ───────────────────────────────────────────────────

#[tauri::command]
fn list_accounts(state: State<DbState>) -> Result<Vec<Account>, String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    s.db.with_conn(|conn| Account::list(conn))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_sync_pairs(state: State<DbState>) -> Result<Vec<SyncPair>, String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    s.db.with_conn(|conn| SyncPair::list(conn))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_sync_pair(state: State<DbState>, id: String) -> Result<Option<SyncPair>, String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    s.db.with_conn(|conn| SyncPair::get_by_id(conn, &id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_files(state: State<DbState>, sync_pair_id: String) -> Result<Vec<FileEntry>, String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    s.db.with_conn(|conn| FileEntry::list_by_sync_pair(conn, &sync_pair_id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_conflicts(state: State<DbState>, sync_pair_id: String) -> Result<Vec<FileEntry>, String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    s.db.with_conn(|conn| {
        FileEntry::list_by_state(conn, &sync_pair_id, &FileState::Conflict)
    })
    .map_err(|e| e.to_string())
}

/// Totals for the statistics page: one sync pair, or every pair when
/// `sync_pair_id` is omitted. Returns all-time alongside the last 7 days so
/// the page can show "lifetime" and "recent" without a second round-trip.
#[tauri::command]
fn get_transfer_stats(
    state: State<DbState>,
    sync_pair_id: Option<String>,
) -> Result<StatsPayload, String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    s.db.with_conn(|conn| {
        Ok(StatsPayload {
            all_time: TransferStats::compute(conn, sync_pair_id.as_deref(), None)?,
            last_7_days: TransferStats::compute(
                conn,
                sync_pair_id.as_deref(),
                Some("datetime('now','-7 days')"),
            )?,
        })
    })
    .map_err(|e| e.to_string())
}

/// Per-pair totals, so the statistics page can break the numbers down
/// without one round-trip per sync pair.
#[tauri::command]
fn get_transfer_stats_by_pair(state: State<DbState>) -> Result<Vec<PairStats>, String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    s.db.with_conn(|conn| {
        let pairs = SyncPair::list(conn)?;
        let mut out = Vec::with_capacity(pairs.len());
        for pair in pairs {
            out.push(PairStats {
                stats: TransferStats::compute(conn, Some(&pair.id), None)?,
                sync_pair_id: pair.id,
                name: pair.name,
            });
        }
        Ok(out)
    })
    .map_err(|e| e.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatsPayload {
    all_time: TransferStats,
    last_7_days: TransferStats,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PairStats {
    sync_pair_id: String,
    name: String,
    stats: TransferStats,
}

#[tauri::command]
fn get_recent_activity(
    state: State<DbState>,
    sync_pair_id: String,
    limit: i64,
) -> Result<Vec<ChangeLogEntry>, String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    s.db.with_conn(|conn| ChangeLogEntry::recent(conn, &sync_pair_id, limit))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn pause_sync_pair(state: State<DbState>, id: String) -> Result<(), String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    s.db.with_conn(|conn| SyncPair::update_status(conn, &id, &SyncPairStatus::Paused))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn resume_sync_pair(state: State<DbState>, id: String) -> Result<(), String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    s.db.with_conn(|conn| SyncPair::update_status(conn, &id, &SyncPairStatus::Active))
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn start_login(state: State<'_, DbState>) -> Result<Account, String> {
    let (db, creds) = {
        let s = state.lock().map_err(|e| e.to_string())?;
        let creds = OAuthCredentials::load(&s.credentials_path).map_err(|e| e.to_string())?;
        (s.db.clone(), creds)
    };

    let auth = AuthManager::new(creds, db);
    auth.login().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn reconnect_account(
    state: State<'_, DbState>,
    account_id: String,
) -> Result<Account, String> {
    let (db, creds) = {
        let s = state.lock().map_err(|e| e.to_string())?;
        let creds = OAuthCredentials::load(&s.credentials_path).map_err(|e| e.to_string())?;
        (s.db.clone(), creds)
    };

    let auth = AuthManager::new(creds, db);
    auth.reconnect_account(&account_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn logout(state: State<DbState>, account_id: String) -> Result<(), String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    s.db.with_conn(|conn| Account::delete(conn, &account_id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn resolve_conflict(
    state: State<'_, DbState>,
    sync_pair_id: String,
    relative_path: String,
    resolution: String, // "keep-local", "keep-remote", "keep-both"
) -> Result<(), String> {
    let (db, creds) = {
        let s = state.lock().map_err(|e| e.to_string())?;
        let creds = OAuthCredentials::load(&s.credentials_path).map_err(|e| e.to_string())?;
        (s.db.clone(), creds)
    };

    let pair = db
        .with_conn(|conn| SyncPair::get_by_id(conn, &sync_pair_id))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Sync pair not found: {sync_pair_id}"))?;

    let entry = db
        .with_conn(|conn| FileEntry::get_by_path(conn, &sync_pair_id, &relative_path))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("File not found: {relative_path}"))?;

    let remote_id = entry
        .remote_id
        .as_deref()
        .ok_or_else(|| "No remote ID for this file".to_string())?;

    let local_path = std::path::PathBuf::from(&pair.local_root).join(&relative_path);

    let auth = AuthManager::new(creds, db.clone());
    let drive = HttpDriveClient::new(auth, pair.account_id.clone());

    // Build the engine — encrypted pairs need the cipher loaded so the
    // upload/download helpers do the right thing here.
    let mut engine = SyncEngine::new(db.clone(), pair.clone());
    if pair.encryption_enabled {
        let cipher = keystore::load_cipher(&pair.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| {
                "This sync pair is encrypted but the key is not in the keyring. \
                 Unlock it with the passphrase first."
                    .to_string()
            })?;
        engine = engine.with_cipher(Arc::new(cipher));
    }

    match resolution.as_str() {
        "keep-local" => {
            if local_path.exists() {
                let file = engine
                    .update_remote_via(&drive, remote_id, &local_path)
                    .await
                    .map_err(|e| e.to_string())?;
                // Update index to synced state
                let local_hash = insyncbee_core::watcher::hash_file(&local_path).ok();
                db.with_conn(|conn| {
                    let mut entry = entry.clone();
                    entry.state = FileState::Synced;
                    entry.local_hash = local_hash;
                    entry.remote_md5 = file.md5_checksum.clone();
                    entry.last_synced_at = Some(chrono::Utc::now().to_rfc3339());
                    entry.upsert(conn)
                })
                .map_err(|e| e.to_string())?;
            } else {
                return Err("Local file does not exist".to_string());
            }
        }
        "keep-remote" => {
            engine
                .download_via(&drive, remote_id, &local_path)
                .await
                .map_err(|e| e.to_string())?;
            let file = drive
                .get_file(remote_id)
                .await
                .map_err(|e| e.to_string())?;
            let local_hash = insyncbee_core::watcher::hash_file(&local_path).ok();
            db.with_conn(|conn| {
                let mut entry = entry.clone();
                entry.state = FileState::Synced;
                entry.local_hash = local_hash;
                entry.remote_md5 = file.md5_checksum.clone();
                entry.last_synced_at = Some(chrono::Utc::now().to_rfc3339());
                entry.upsert(conn)
            })
            .map_err(|e| e.to_string())?;
        }
        "keep-both" => {
            // Download remote as a conflicted copy
            let stem = local_path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy();
            let ext = local_path
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            let timestamp = chrono::Utc::now().format("%Y-%m-%d %H.%M.%S");
            let conflict_name = format!("{stem} (conflict {timestamp}){ext}");
            let conflict_path = local_path.with_file_name(&conflict_name);

            engine
                .download_via(&drive, remote_id, &conflict_path)
                .await
                .map_err(|e| e.to_string())?;

            // Mark the original as synced (local version stays, remote copy saved)
            db.with_conn(|conn| {
                let mut entry = entry.clone();
                entry.state = FileState::Synced;
                entry.last_synced_at = Some(chrono::Utc::now().to_rfc3339());
                entry.upsert(conn)
            })
            .map_err(|e| e.to_string())?;
        }
        _ => return Err(format!("Invalid resolution: {resolution}")),
    }

    // Log the resolution
    db.with_conn(|conn| {
        ChangeLogEntry::insert(
            conn,
            &sync_pair_id,
            &relative_path,
            "resolve",
            Some(&format!("resolved as {resolution}")),
        )
    })
    .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
async fn trigger_sync(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    inflight: State<'_, InFlight>,
    sync_pair_id: String,
) -> Result<String, String> {
    let (db, credentials_path) = {
        let s = state.lock().map_err(|e| e.to_string())?;
        (s.db.clone(), s.credentials_path.clone())
    };

    match run_sync(&app, &db, &credentials_path, &sync_pair_id, &inflight.0).await {
        SyncOutcome::Done(report) => Ok(report),
        SyncOutcome::AlreadyRunning => {
            Ok("A sync is already running for this pair.".to_string())
        }
        SyncOutcome::Failed(e) => Err(e),
    }
}

enum SyncOutcome {
    Done(String),
    /// Another sync for this pair is mid-flight. Two engines on one pair
    /// race each other into duplicate uploads and phantom conflicts, so
    /// the second caller backs off instead.
    AlreadyRunning,
    Failed(String),
}

/// Sync one pair, emitting the progress/status/finished events the UI
/// listens for. Shared by the Sync Now button and the background loop so
/// both produce identical feedback and honour the same in-flight guard.
async fn run_sync(
    app: &tauri::AppHandle,
    db: &Database,
    credentials_path: &Path,
    sync_pair_id: &str,
    inflight: &InFlightSet,
) -> SyncOutcome {
    // Claim the pair, or bail if someone else holds it.
    {
        let mut set = match inflight.lock() {
            Ok(s) => s,
            Err(e) => return SyncOutcome::Failed(e.to_string()),
        };
        if !set.insert(sync_pair_id.to_string()) {
            return SyncOutcome::AlreadyRunning;
        }
    }

    let result = run_sync_inner(app, db, credentials_path, sync_pair_id).await;

    if let Ok(mut set) = inflight.lock() {
        set.remove(sync_pair_id);
    }
    // Always tell the UI the pair is done, including on failure — otherwise
    // its activity indicator spins forever on a sync that already died.
    let _ = app.emit("sync-finished", sync_pair_id);

    match result {
        Ok(report) => SyncOutcome::Done(report),
        Err(e) => SyncOutcome::Failed(e),
    }
}

async fn run_sync_inner(
    app: &tauri::AppHandle,
    db: &Database,
    credentials_path: &Path,
    sync_pair_id: &str,
) -> Result<String, String> {
    let creds = OAuthCredentials::load(credentials_path).map_err(|e| e.to_string())?;

    let pair = db
        .with_conn(|conn| SyncPair::get_by_id(conn, sync_pair_id))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Sync pair not found: {sync_pair_id}"))?;

    let auth = AuthManager::new(creds, db.clone());

    // Bridge the drive client's per-chunk updates into Tauri events the
    // frontend can listen for. Cloning the AppHandle is cheap (an Arc).
    let app_clone = app.clone();
    let pair_id_for_cb = sync_pair_id.to_string();
    let progress_cb: insyncbee_core::drive::ProgressCallback = Arc::new(
        move |local_path: &std::path::Path, kind: TransferKind, bytes: u64, total: u64| {
            let name = local_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| local_path.to_string_lossy().into_owned());
            let payload = TransferProgress {
                sync_pair_id: pair_id_for_cb.clone(),
                kind: kind.as_str(),
                name,
                path: local_path.to_string_lossy().into_owned(),
                bytes,
                total,
            };
            let _ = app_clone.emit("transfer-progress", payload);
        },
    );

    let drive = HttpDriveClient::new(auth, pair.account_id.clone())
        .with_progress_callback(progress_cb);

    // Phase reporting: scanning and listing can take a while on a large
    // pair, and folder work never moves bytes, so without this the
    // dashboard has nothing to show for large stretches of a real sync.
    let app_for_status = app.clone();
    let pair_id_for_status = sync_pair_id.to_string();
    let status_cb: insyncbee_core::sync_engine::StatusCallback =
        Arc::new(move |status: SyncStatus| {
            let _ = app_for_status.emit(
                "sync-status",
                SyncStatusPayload {
                    sync_pair_id: pair_id_for_status.clone(),
                    status,
                },
            );
        });

    let mut engine = SyncEngine::new(db.clone(), pair.clone()).with_status_callback(status_cb);
    if pair.encryption_enabled {
        let cipher = keystore::load_cipher(&pair.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| {
                "This sync pair is encrypted but the key is not in the keyring. \
                 Unlock it with the passphrase first."
                    .to_string()
            })?;
        engine = engine.with_cipher(Arc::new(cipher));
    }

    let report = engine.sync(&drive).await.map_err(|e| e.to_string())?;
    Ok(report.to_string())
}

/// User settings that live next to the credentials, in
/// `~/.config/insyncbee/settings.json`.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct AppSettings {
    /// Sync automatically in the background. On by default: an app that
    /// only syncs when you press a button is not a sync app.
    auto_sync: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self { auto_sync: true }
    }
}

/// Settings live beside `credentials.json`, so the path is derived from it
/// rather than re-deriving the config dir in two places.
fn settings_path(credentials_path: &Path) -> PathBuf {
    credentials_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("settings.json")
}

fn load_settings(credentials_path: &Path) -> AppSettings {
    std::fs::read_to_string(settings_path(credentials_path))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        // A missing or corrupt file must not disable syncing — fall back
        // to the default rather than leaving the app silently idle.
        .unwrap_or_default()
}

fn auto_sync_enabled(credentials_path: &Path) -> bool {
    load_settings(credentials_path).auto_sync
}

#[tauri::command]
fn get_auto_sync(state: State<DbState>) -> Result<bool, String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    Ok(auto_sync_enabled(&s.credentials_path))
}

#[tauri::command]
fn set_auto_sync(state: State<DbState>, enabled: bool) -> Result<(), String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    let path = settings_path(&s.credentials_path);
    let settings = AppSettings { auto_sync: enabled };
    let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, json).map_err(|e| e.to_string())
}

/// Pairs with a sync in flight right now, shared by the button and the
/// background loop.
type InFlightSet = Arc<Mutex<std::collections::HashSet<String>>>;
struct InFlight(InFlightSet);

/// Background auto-sync: the reason the desktop app syncs at all without
/// someone pressing a button.
///
/// The CLI has always done this (`insyncbee daemon`), but the GUI never
/// did — it only ever synced a pair when its Sync Now button was clicked,
/// so an app left open sat idle while Drive and disk drifted apart.
///
/// Mirrors the daemon's behaviour: an initial pass at startup, a
/// filesystem watcher per pair for immediate local changes, and a poll at
/// each pair's `poll_interval_secs` to pick up remote ones.
fn spawn_auto_sync(
    app: tauri::AppHandle,
    db: Database,
    credentials_path: PathBuf,
    inflight: InFlightSet,
) {
    tauri::async_runtime::spawn(async move {
        use insyncbee_core::watcher::FileWatcher;
        use std::collections::HashMap;
        use tokio::time::{Duration, Instant};

        // Keyed by pair id. Watchers must be kept alive — dropping one
        // silently stops the notifications.
        let mut watchers: HashMap<String, (FileWatcher, _)> = HashMap::new();
        let mut last_sync: HashMap<String, Instant> = HashMap::new();
        let mut ticker = tokio::time::interval(Duration::from_secs(5));

        loop {
            ticker.tick().await;

            if !auto_sync_enabled(&credentials_path) {
                continue;
            }

            // Re-read every tick so pairs added, edited, paused, or
            // deleted in the UI take effect without a restart.
            let pairs = match db.with_conn(|conn| SyncPair::list(conn)) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!("auto-sync: could not list sync pairs: {e}");
                    continue;
                }
            };
            let active: Vec<SyncPair> = pairs
                .into_iter()
                .filter(|p| p.status == SyncPairStatus::Active)
                .collect();

            // Drop watchers for pairs that are gone or paused.
            let active_ids: std::collections::HashSet<&str> =
                active.iter().map(|p| p.id.as_str()).collect();
            watchers.retain(|id, _| active_ids.contains(id.as_str()));

            for pair in &active {
                // Start watching a pair the first time we see it active.
                if !watchers.contains_key(&pair.id) {
                    let root = PathBuf::from(&pair.local_root);
                    if root.exists() {
                        match FileWatcher::start(&root, 2000) {
                            Ok((w, rx)) => {
                                tracing::info!("auto-sync: watching '{}'", pair.name);
                                watchers.insert(pair.id.clone(), (w, rx));
                            }
                            Err(e) => {
                                tracing::warn!("auto-sync: cannot watch '{}': {e}", pair.name)
                            }
                        }
                    }
                }

                // A local change syncs immediately; otherwise wait out the
                // pair's poll interval. A pair we've never synced in this
                // session syncs right away — that's the startup pass.
                let mut local_changed = false;
                if let Some((_, rx)) = watchers.get_mut(&pair.id) {
                    while let Ok(events) = rx.try_recv() {
                        if !events.is_empty() {
                            local_changed = true;
                        }
                    }
                }

                let due = match last_sync.get(&pair.id) {
                    None => true,
                    Some(t) => {
                        Instant::now().duration_since(*t)
                            >= Duration::from_secs(pair.poll_interval_secs.max(5) as u64)
                    }
                };

                if !(local_changed || due) {
                    continue;
                }

                // An encrypted pair whose key isn't in the keyring cannot
                // sync. Skip quietly rather than failing every file every
                // interval and burying the activity feed in errors — the
                // user unlocks it from the dashboard.
                if pair.encryption_enabled {
                    match keystore::load_cipher(&pair.id) {
                        Ok(Some(_)) => {}
                        _ => {
                            last_sync.insert(pair.id.clone(), Instant::now());
                            continue;
                        }
                    }
                }

                last_sync.insert(pair.id.clone(), Instant::now());
                match run_sync(&app, &db, &credentials_path, &pair.id, &inflight).await {
                    SyncOutcome::Done(report) => {
                        tracing::debug!("auto-sync '{}': {report}", pair.name);
                    }
                    SyncOutcome::AlreadyRunning => {}
                    SyncOutcome::Failed(e) => {
                        tracing::warn!("auto-sync '{}' failed: {e}", pair.name);
                    }
                }
            }
        }
    });
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SyncStatusPayload {
    sync_pair_id: String,
    status: SyncStatus,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TransferProgress {
    sync_pair_id: String,
    /// "upload" or "download" — the frontend keys its live speed readouts
    /// off this, so the two directions are never summed together.
    kind: &'static str,
    name: String,
    path: String,
    bytes: u64,
    total: u64,
}

#[tauri::command]
fn add_sync_pair(
    state: State<DbState>,
    name: String,
    account_id: String,
    local_root: String,
    remote_root_id: String,
    remote_root_path: String,
    mode: String,
    conflict_policy: Option<String>,
    poll_interval_secs: Option<i64>,
    encryption_passphrase: Option<String>,
) -> Result<SyncPair, String> {
    let s = state.lock().map_err(|e| e.to_string())?;

    let local_path = std::path::Path::new(&local_root);
    if !local_path.exists() {
        std::fs::create_dir_all(local_path).map_err(|e| e.to_string())?;
    }

    let sync_mode: SyncMode = mode.parse().map_err(|e: insyncbee_core::Error| e.to_string())?;
    let policy: ConflictPolicy = match conflict_policy.as_deref() {
        Some(p) => p.parse().map_err(|e: insyncbee_core::Error| e.to_string())?,
        None => ConflictPolicy::KeepBoth,
    };

    // ── Encryption setup (if requested) ───────────────────────────
    // We do this BEFORE inserting the pair so a keyring failure aborts
    // the whole creation rather than leaving an orphan DB row pointing
    // at a non-existent key.
    let pair_id = uuid::Uuid::new_v4().to_string();
    let (enc_enabled, enc_salt, enc_verifier) =
        match encryption_passphrase.as_deref().filter(|p| !p.is_empty()) {
            Some(passphrase) => {
                let salt = crypto::random_salt();
                let cipher = FileCipher::from_passphrase(passphrase, &salt)
                    .map_err(|e| format!("derive key: {e}"))?;
                let verifier = cipher
                    .make_verifier(pair_id.as_bytes())
                    .map_err(|e| format!("make verifier: {e}"))?;
                keystore::store_key(&pair_id, &cipher)
                    .map_err(|e| format!("store key in keyring: {e}"))?;
                (true, Some(salt.to_vec()), Some(verifier))
            }
            None => (false, None, None),
        };

    let pair = SyncPair {
        id: pair_id,
        name,
        account_id,
        local_root,
        remote_root_id,
        remote_root_path,
        mode: sync_mode,
        conflict_policy: policy,
        status: SyncPairStatus::Active,
        poll_interval_secs: poll_interval_secs.unwrap_or(30),
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        encryption_enabled: enc_enabled,
        encryption_salt: enc_salt,
        encryption_verifier: enc_verifier,
    };

    if let Err(e) = s.db.with_conn(|conn| pair.insert(conn)) {
        // Roll back the keyring write so the user isn't left with a
        // dangling entry from a failed creation.
        if pair.encryption_enabled {
            let _ = keystore::delete_key(&pair.id);
        }
        return Err(e.to_string());
    }

    Ok(pair)
}

#[tauri::command]
fn update_sync_pair(
    state: State<DbState>,
    id: String,
    name: String,
    mode: String,
    conflict_policy: String,
    poll_interval_secs: i64,
) -> Result<SyncPair, String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    let sync_mode: SyncMode = mode.parse().map_err(|e: insyncbee_core::Error| e.to_string())?;
    let policy: ConflictPolicy = conflict_policy
        .parse()
        .map_err(|e: insyncbee_core::Error| e.to_string())?;

    s.db.with_conn(|conn| {
        SyncPair::update_settings(conn, &id, &name, &sync_mode, &policy, poll_interval_secs)
    })
    .map_err(|e| e.to_string())?;

    let updated = s
        .db
        .with_conn(|conn| SyncPair::get_by_id(conn, &id))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Sync pair not found: {id}"))?;
    Ok(updated)
}

#[tauri::command]
fn delete_sync_pair(state: State<DbState>, id: String) -> Result<(), String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    s.db.with_conn(|conn| SyncPair::delete(conn, &id))
        .map_err(|e| e.to_string())?;
    // Best-effort keyring cleanup. We don't fail the delete if the
    // keyring is unavailable — the DB row is already gone, and the
    // orphaned key is harmless (no pair references it anymore).
    if let Err(e) = keystore::delete_key(&id) {
        tracing::warn!("failed to delete keyring entry for pair {id}: {e}");
    }
    Ok(())
}

/// Re-derive the encryption key from a passphrase and stash it in the OS
/// keyring. Used when the daemon wakes up on a fresh machine where the
/// keyring entry doesn't exist yet, or after the user revoked the
/// keyring entry. Verifies the passphrase against the stored verifier
/// before writing — wrong passphrases are rejected, no keyring write.
#[tauri::command]
fn unlock_encryption(
    state: State<DbState>,
    sync_pair_id: String,
    passphrase: String,
) -> Result<(), String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    let pair = s
        .db
        .with_conn(|conn| SyncPair::get_by_id(conn, &sync_pair_id))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Sync pair not found: {sync_pair_id}"))?;
    if !pair.encryption_enabled {
        return Err("Sync pair is not encrypted".to_string());
    }
    let salt = pair
        .encryption_salt
        .as_deref()
        .ok_or_else(|| "encrypted pair has no salt — DB corrupt".to_string())?;
    let verifier = pair
        .encryption_verifier
        .as_deref()
        .ok_or_else(|| "encrypted pair has no verifier — DB corrupt".to_string())?;

    let cipher =
        FileCipher::from_passphrase(&passphrase, salt).map_err(|e| format!("derive key: {e}"))?;
    let ok = cipher
        .verify(pair.id.as_bytes(), verifier)
        .map_err(|e| format!("verify: {e}"))?;
    if !ok {
        return Err("Wrong passphrase".to_string());
    }
    keystore::store_key(&pair.id, &cipher).map_err(|e| format!("store key: {e}"))?;
    Ok(())
}

/// Whether the OS keyring currently holds the encryption key for this
/// pair. The UI uses this to decide between showing "Sync now" and
/// "Unlock to sync".
#[tauri::command]
fn encryption_unlocked(
    state: State<DbState>,
    sync_pair_id: String,
) -> Result<bool, String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    let pair = s
        .db
        .with_conn(|conn| SyncPair::get_by_id(conn, &sync_pair_id))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Sync pair not found: {sync_pair_id}"))?;
    if !pair.encryption_enabled {
        // Plain pairs don't need unlocking — they're always "ready".
        return Ok(true);
    }
    Ok(keystore::load_cipher(&pair.id)
        .map_err(|e| e.to_string())?
        .is_some())
}

// ── Autostart ────────────────────────────────────────────────────────

#[tauri::command]
fn autostart_enabled(app: tauri::AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
fn autostart_enable(app: tauri::AppHandle) -> Result<(), String> {
    app.autolaunch().enable().map_err(|e| e.to_string())
}

#[tauri::command]
fn autostart_disable(app: tauri::AppHandle) -> Result<(), String> {
    app.autolaunch().disable().map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct DriveFolder {
    id: String,
    name: String,
}

#[derive(Serialize)]
struct LocalFolder {
    name: String,
    path: String,
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[tauri::command]
fn default_local_folder() -> Result<String, String> {
    let path = home_dir().unwrap_or_else(|| PathBuf::from("/"));
    Ok(display_path(&path))
}

#[tauri::command]
fn parent_local_folder(path: String) -> Result<Option<String>, String> {
    let path = PathBuf::from(path);
    Ok(path.parent().map(display_path))
}

#[tauri::command]
fn list_local_folders(path: String) -> Result<Vec<LocalFolder>, String> {
    let mut folders = Vec::new();
    for entry in fs::read_dir(&path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        if file_type.is_dir() {
            folders.push(LocalFolder {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: display_path(&entry.path()),
            });
        }
    }
    folders.sort_by_key(|f| f.name.to_lowercase());
    Ok(folders)
}

#[tauri::command]
fn create_local_folder(parent_path: String, name: String) -> Result<LocalFolder, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Folder name is required".to_string());
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err("Folder name cannot contain path separators".to_string());
    }

    let path = PathBuf::from(parent_path).join(trimmed);
    fs::create_dir(&path).map_err(|e| e.to_string())?;
    Ok(LocalFolder {
        name: trimmed.to_string(),
        path: display_path(&path),
    })
}

#[tauri::command]
async fn list_drive_folders(
    state: State<'_, DbState>,
    account_id: String,
    parent_id: Option<String>,
) -> Result<Vec<DriveFolder>, String> {
    let (db, creds) = {
        let s = state.lock().map_err(|e| e.to_string())?;
        let creds = OAuthCredentials::load(&s.credentials_path).map_err(|e| e.to_string())?;
        (s.db.clone(), creds)
    };

    let auth = AuthManager::new(creds, db);
    let drive = HttpDriveClient::new(auth, account_id);
    let parent = parent_id.as_deref().unwrap_or("root");

    let files = drive
        .list_all_files(parent)
        .await
        .map_err(|e| e.to_string())?;

    let folders = files
        .into_iter()
        .filter(|f| f.is_folder())
        .map(|f| DriveFolder {
            id: f.id,
            name: f.name,
        })
        .collect();
    Ok(folders)
}

#[tauri::command]
async fn create_drive_folder(
    state: State<'_, DbState>,
    account_id: String,
    parent_id: String,
    name: String,
) -> Result<DriveFolder, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Folder name is required".to_string());
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err("Folder name cannot contain path separators".to_string());
    }

    let (db, creds) = {
        let s = state.lock().map_err(|e| e.to_string())?;
        let creds = OAuthCredentials::load(&s.credentials_path).map_err(|e| e.to_string())?;
        (s.db.clone(), creds)
    };

    let auth = AuthManager::new(creds, db);
    let drive = HttpDriveClient::new(auth, account_id);
    let folder = drive
        .create_folder(&parent_id, trimmed)
        .await
        .map_err(|e| e.to_string())?;

    Ok(DriveFolder {
        id: folder.id,
        name: folder.name,
    })
}

// ── App Setup ────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Work around WebKitGTK's DMABUF renderer aborting at startup.
///
/// On a fair number of Linux setups — NVIDIA drivers, VMs, several Wayland
/// compositors — WebKitGTK 2.42+ fails to create a GBM EGL display and calls
/// `abort()` before a window ever appears:
///
/// ```text
/// Could not create GBM EGL display: EGL_SUCCESS. Aborting...
/// ```
///
/// The app dies with SIGABRT and the user sees nothing at all. `scripts/dev-gui.sh`
/// has always exported this for development; shipping without it meant every
/// affected user downloaded an app that could not start.
///
/// Only set when unset, so `WEBKIT_DISABLE_DMABUF_RENDERER=0` still opts back
/// into the accelerated path on hardware where it works.
#[cfg(target_os = "linux")]
fn disable_dmabuf_renderer_if_unset() {
    const VAR: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";
    if std::env::var_os(VAR).is_none() {
        std::env::set_var(VAR, "1");
    }
}

pub fn run() {
    #[cfg(target_os = "linux")]
    disable_dmabuf_renderer_if_unset();

    let paths = AppPaths::new().expect("Failed to initialize app paths");
    let db = Database::open(&paths.db_path).expect("Failed to open database");

    let app_state = AppState {
        db: db.clone(),
        credentials_path: paths.credentials_path.clone(),
    };
    let inflight: InFlightSet = Arc::new(Mutex::new(std::collections::HashSet::new()));
    let inflight_for_loop = Arc::clone(&inflight);
    let credentials_for_loop = paths.credentials_path.clone();

    let start_in_tray = std::env::args()
        .any(|a| a == "--tray" || a == "--background" || a == "--hidden");

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--tray"]),
        ))
        .setup(move |app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // Start syncing without waiting for a button press.
            spawn_auto_sync(
                app.handle().clone(),
                db.clone(),
                credentials_for_loop.clone(),
                Arc::clone(&inflight_for_loop),
            );

            let show_item =
                MenuItem::with_id(app, "show", "Open InSyncBee", true, None::<&str>)?;
            let separator = PredefinedMenuItem::separator(app)?;
            let quit_item =
                MenuItem::with_id(app, "quit", "Quit InSyncBee", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &separator, &quit_item])?;

            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().expect("missing default icon").clone())
                .icon_as_template(true)
                .tooltip("InSyncBee - Google Drive Sync")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            if window.is_visible().unwrap_or(false) {
                                let _ = window.hide();
                            } else {
                                let _ = window.show();
                                let _ = window.unminimize();
                                let _ = window.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;

            if !start_in_tray {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .manage(Mutex::new(app_state))
        .manage(InFlight(inflight))
        .invoke_handler(tauri::generate_handler![
            list_accounts,
            list_sync_pairs,
            get_sync_pair,
            get_files,
            get_conflicts,
            get_recent_activity,
            get_auto_sync,
            set_auto_sync,
            get_transfer_stats,
            get_transfer_stats_by_pair,
            pause_sync_pair,
            resume_sync_pair,
            start_login,
            reconnect_account,
            logout,
            resolve_conflict,
            trigger_sync,
            add_sync_pair,
            update_sync_pair,
            delete_sync_pair,
            default_local_folder,
            parent_local_folder,
            list_local_folders,
            create_local_folder,
            list_drive_folders,
            create_drive_folder,
            unlock_encryption,
            encryption_unlocked,
            autostart_enabled,
            autostart_enable,
            autostart_disable,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

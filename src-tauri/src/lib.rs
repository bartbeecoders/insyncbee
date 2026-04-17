use insyncbee_core::auth::{AuthManager, OAuthCredentials};
use insyncbee_core::db::models::{
    Account, ChangeLogEntry, ConflictPolicy, FileEntry, FileState, SyncMode, SyncPair,
    SyncPairStatus,
};
use insyncbee_core::db::Database;
use insyncbee_core::drive::DriveClient;
use insyncbee_core::sync_engine::SyncEngine;
use insyncbee_core::AppPaths;
use serde::Serialize;
use std::sync::Mutex;
use tauri::{Manager, State};

struct AppState {
    db: Database,
    paths: AppPaths,
    creds: Option<OAuthCredentials>,
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
        let creds = s
            .creds
            .clone()
            .ok_or_else(|| "OAuth credentials not configured (set INSYNCBEE_CLIENT_ID and INSYNCBEE_CLIENT_SECRET)".to_string())?;
        (s.db.clone(), creds)
    };

    let auth = AuthManager::new(creds, db);
    auth.login().await.map_err(|e| e.to_string())
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
        let creds = s
            .creds
            .clone()
            .ok_or_else(|| "OAuth credentials not configured".to_string())?;
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
    let drive = DriveClient::new(auth, pair.account_id.clone());

    match resolution.as_str() {
        "keep-local" => {
            if local_path.exists() {
                let file = drive
                    .update_file(remote_id, &local_path)
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
            drive
                .download_file(remote_id, &local_path)
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

            drive
                .download_file(remote_id, &conflict_path)
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
async fn trigger_sync(state: State<'_, DbState>, sync_pair_id: String) -> Result<String, String> {
    let (db, creds) = {
        let s = state.lock().map_err(|e| e.to_string())?;
        let creds = s
            .creds
            .clone()
            .ok_or_else(|| "OAuth credentials not configured".to_string())?;
        (s.db.clone(), creds)
    };

    let pair = db
        .with_conn(|conn| SyncPair::get_by_id(conn, &sync_pair_id))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Sync pair not found: {sync_pair_id}"))?;

    let auth = AuthManager::new(creds, db.clone());
    let drive = DriveClient::new(auth, pair.account_id.clone());
    let engine = SyncEngine::new(db, pair);

    let report = engine.sync(&drive).await.map_err(|e| e.to_string())?;
    Ok(report.to_string())
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
    let pair = SyncPair {
        id: uuid::Uuid::new_v4().to_string(),
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
    };

    s.db.with_conn(|conn| pair.insert(conn))
        .map_err(|e| e.to_string())?;

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
        .map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct DriveFolder {
    id: String,
    name: String,
}

#[tauri::command]
async fn list_drive_folders(
    state: State<'_, DbState>,
    account_id: String,
    parent_id: Option<String>,
) -> Result<Vec<DriveFolder>, String> {
    let (db, creds) = {
        let s = state.lock().map_err(|e| e.to_string())?;
        let creds = s
            .creds
            .clone()
            .ok_or_else(|| "OAuth credentials not configured".to_string())?;
        (s.db.clone(), creds)
    };

    let auth = AuthManager::new(creds, db);
    let drive = DriveClient::new(auth, account_id);
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

// ── App Setup ────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let paths = AppPaths::new().expect("Failed to initialize app paths");
    let db = Database::open(&paths.db_path).expect("Failed to open database");

    let creds = OAuthCredentials::from_env().ok();

    let app_state = AppState { db, paths, creds };

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // Show main window after setup
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
            }

            Ok(())
        })
        .manage(Mutex::new(app_state))
        .invoke_handler(tauri::generate_handler![
            list_accounts,
            list_sync_pairs,
            get_sync_pair,
            get_files,
            get_conflicts,
            get_recent_activity,
            pause_sync_pair,
            resume_sync_pair,
            start_login,
            logout,
            resolve_conflict,
            trigger_sync,
            add_sync_pair,
            update_sync_pair,
            delete_sync_pair,
            list_drive_folders,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

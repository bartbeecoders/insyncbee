//! Core sync engine: orchestrates change detection, conflict resolution,
//! and file transfer between local filesystem and Google Drive.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::db::models::{
    ChangeLogEntry, ConflictPolicy, FileEntry, FileState, SyncMode, SyncPair,
};
use crate::db::Database;
use crate::drive::{DriveClient, DriveFile};
use crate::watcher;

/// Describes what action to take for a single file.
#[derive(Debug, Clone)]
pub enum SyncAction {
    Upload {
        relative_path: String,
        local_path: PathBuf,
        remote_parent_id: String,
    },
    UpdateRemote {
        relative_path: String,
        local_path: PathBuf,
        remote_id: String,
    },
    Download {
        relative_path: String,
        remote_id: String,
        local_path: PathBuf,
    },
    DeleteLocal {
        relative_path: String,
        local_path: PathBuf,
    },
    DeleteRemote {
        relative_path: String,
        remote_id: String,
    },
    CreateLocalDir {
        relative_path: String,
        local_path: PathBuf,
    },
    CreateRemoteDir {
        relative_path: String,
        remote_parent_id: String,
        name: String,
    },
    Conflict {
        relative_path: String,
        local_path: PathBuf,
        remote_id: String,
    },
    Skip {
        relative_path: String,
        reason: String,
    },
}

/// The sync engine coordinates syncing for a single sync pair.
pub struct SyncEngine {
    db: Database,
    pair: SyncPair,
}

impl SyncEngine {
    pub fn new(db: Database, pair: SyncPair) -> Self {
        Self { db, pair }
    }

    /// Perform a full sync cycle: scan local, fetch remote, compare, execute actions.
    pub async fn sync(&self, drive: &DriveClient) -> anyhow::Result<SyncReport> {
        let mut report = SyncReport::default();

        tracing::info!("Starting sync for pair '{}' ({})", self.pair.name, self.pair.mode);

        // 1. Scan local filesystem
        let local_root = PathBuf::from(&self.pair.local_root);
        let local_files = watcher::scan_directory(&local_root)?;
        let local_map: HashMap<String, watcher::LocalFileInfo> = local_files
            .into_iter()
            .map(|f| (f.relative_path.clone(), f))
            .collect();

        // 2. Fetch remote file list (recursively)
        let remote_files = self.fetch_remote_tree(drive, &self.pair.remote_root_id, "").await?;
        let remote_map: HashMap<String, DriveFile> = remote_files
            .iter()
            .map(|(path, file)| (path.clone(), file.clone()))
            .collect();

        // 3. Load the base state from database
        let base_entries = self.db.with_conn(|conn| {
            FileEntry::list_by_sync_pair(conn, &self.pair.id)
        })?;
        let base_map: HashMap<String, FileEntry> = base_entries
            .into_iter()
            .map(|e| (e.relative_path.clone(), e))
            .collect();

        // 4. Compute sync actions via three-way comparison
        let actions = self.compute_actions(&local_map, &remote_map, &base_map, &local_root);

        // 5. Execute actions
        for action in &actions {
            match self.execute_action(action, drive, &local_root).await {
                Ok(()) => match action {
                    SyncAction::Upload { relative_path, .. }
                    | SyncAction::UpdateRemote { relative_path, .. } => {
                        report.uploaded += 1;
                        self.log_change(relative_path, "upload", None);
                    }
                    SyncAction::Download { relative_path, .. } => {
                        report.downloaded += 1;
                        self.log_change(relative_path, "download", None);
                    }
                    SyncAction::DeleteLocal { relative_path, .. } => {
                        report.deleted += 1;
                        self.log_change(relative_path, "delete-local", None);
                    }
                    SyncAction::DeleteRemote { relative_path, .. } => {
                        report.deleted += 1;
                        self.log_change(relative_path, "delete-remote", None);
                    }
                    SyncAction::Conflict { relative_path, .. } => {
                        report.conflicts += 1;
                        self.log_change(relative_path, "conflict", None);
                    }
                    SyncAction::CreateLocalDir { .. } | SyncAction::CreateRemoteDir { .. } => {}
                    SyncAction::Skip { .. } => {
                        report.skipped += 1;
                    }
                },
                Err(e) => {
                    report.errors += 1;
                    if let Some(path) = action_path(action) {
                        tracing::error!("Sync error for {path}: {e}");
                        self.log_change(&path, "error", Some(&e.to_string()));
                    }
                }
            }
        }

        tracing::info!(
            "Sync complete for '{}': {} up, {} down, {} deleted, {} conflicts, {} errors",
            self.pair.name,
            report.uploaded,
            report.downloaded,
            report.deleted,
            report.conflicts,
            report.errors
        );

        Ok(report)
    }

    /// Compute what actions need to be taken based on three-way comparison.
    pub fn compute_actions(
        &self,
        local: &HashMap<String, watcher::LocalFileInfo>,
        remote: &HashMap<String, DriveFile>,
        base: &HashMap<String, FileEntry>,
        local_root: &Path,
    ) -> Vec<SyncAction> {
        let mut actions = Vec::new();

        // Collect all known paths
        let mut all_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
        all_paths.extend(local.keys().cloned());
        all_paths.extend(remote.keys().cloned());
        all_paths.extend(base.keys().cloned());

        for path in &all_paths {
            let in_local = local.contains_key(path);
            let in_remote = remote.contains_key(path);
            let in_base = base.contains_key(path);

            let action = match (in_local, in_remote, in_base) {
                // New local file, not on remote or in base
                (true, false, false) => {
                    let info = &local[path];
                    if info.is_directory {
                        if self.pair.mode != SyncMode::CloudToLocal {
                            SyncAction::CreateRemoteDir {
                                relative_path: path.clone(),
                                remote_parent_id: self.resolve_remote_parent_id(path, remote).to_string(),
                                name: Path::new(path)
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string(),
                            }
                        } else {
                            SyncAction::Skip {
                                relative_path: path.clone(),
                                reason: "cloud-to-local mode, ignoring local new dir".into(),
                            }
                        }
                    } else if self.pair.mode != SyncMode::CloudToLocal {
                        SyncAction::Upload {
                            relative_path: path.clone(),
                            local_path: local_root.join(path),
                            remote_parent_id: self.resolve_remote_parent_id(path, remote).to_string(),
                        }
                    } else {
                        SyncAction::Skip {
                            relative_path: path.clone(),
                            reason: "cloud-to-local mode, ignoring local new file".into(),
                        }
                    }
                }
                // New remote file, not local or in base
                (false, true, false) => {
                    let file = &remote[path];
                    if file.is_folder() {
                        if self.pair.mode != SyncMode::LocalToCloud {
                            SyncAction::CreateLocalDir {
                                relative_path: path.clone(),
                                local_path: local_root.join(path),
                            }
                        } else {
                            SyncAction::Skip {
                                relative_path: path.clone(),
                                reason: "local-to-cloud mode, ignoring remote new dir".into(),
                            }
                        }
                    } else if self.pair.mode != SyncMode::LocalToCloud {
                        SyncAction::Download {
                            relative_path: path.clone(),
                            remote_id: file.id.clone(),
                            local_path: local_root.join(path),
                        }
                    } else {
                        SyncAction::Skip {
                            relative_path: path.clone(),
                            reason: "local-to-cloud mode, ignoring remote new file".into(),
                        }
                    }
                }
                // File exists on both sides but not in base (first sync)
                (true, true, false) => {
                    let info = &local[path];
                    let file = &remote[path];
                    if info.is_directory || file.is_folder() {
                        SyncAction::Skip {
                            relative_path: path.clone(),
                            reason: "directory exists on both sides".into(),
                        }
                    } else {
                        // Compare content
                        let local_hash = watcher::hash_file(&info.absolute_path).ok();
                        if local_hash.as_deref() == file.md5_checksum.as_deref() {
                            SyncAction::Skip {
                                relative_path: path.clone(),
                                reason: "identical content".into(),
                            }
                        } else {
                            SyncAction::Conflict {
                                relative_path: path.clone(),
                                local_path: local_root.join(path),
                                remote_id: file.id.clone(),
                            }
                        }
                    }
                }
                // File exists everywhere — check for changes
                (true, true, true) => {
                    let info = &local[path];
                    let file = &remote[path];
                    let entry = &base[path];

                    if info.is_directory || file.is_folder() {
                        SyncAction::Skip {
                            relative_path: path.clone(),
                            reason: "directory".into(),
                        }
                    } else {
                        let local_hash = watcher::hash_file(&info.absolute_path).ok();
                        let local_changed = local_hash.as_deref() != entry.local_hash.as_deref();
                        let remote_changed = file.md5_checksum.as_deref() != entry.remote_md5.as_deref();

                        match (local_changed, remote_changed) {
                            (false, false) => SyncAction::Skip {
                                relative_path: path.clone(),
                                reason: "no changes".into(),
                            },
                            (true, false) if self.pair.mode != SyncMode::CloudToLocal => {
                                SyncAction::UpdateRemote {
                                    relative_path: path.clone(),
                                    local_path: local_root.join(path),
                                    remote_id: file.id.clone(),
                                }
                            }
                            (false, true) if self.pair.mode != SyncMode::LocalToCloud => {
                                SyncAction::Download {
                                    relative_path: path.clone(),
                                    remote_id: file.id.clone(),
                                    local_path: local_root.join(path),
                                }
                            }
                            (true, true) => SyncAction::Conflict {
                                relative_path: path.clone(),
                                local_path: local_root.join(path),
                                remote_id: file.id.clone(),
                            },
                            _ => SyncAction::Skip {
                                relative_path: path.clone(),
                                reason: "mode restriction".into(),
                            },
                        }
                    }
                }
                // File deleted locally but still on remote and in base
                (false, true, true) => {
                    if self.pair.mode == SyncMode::TwoWay || self.pair.mode == SyncMode::LocalToCloud {
                        let file = &remote[path];
                        let entry = &base[path];
                        let remote_changed = file.md5_checksum.as_deref() != entry.remote_md5.as_deref();
                        if remote_changed {
                            // Remote was also modified — conflict
                            SyncAction::Conflict {
                                relative_path: path.clone(),
                                local_path: local_root.join(path),
                                remote_id: file.id.clone(),
                            }
                        } else {
                            SyncAction::DeleteRemote {
                                relative_path: path.clone(),
                                remote_id: file.id.clone(),
                            }
                        }
                    } else {
                        SyncAction::Skip {
                            relative_path: path.clone(),
                            reason: "mode restriction on delete".into(),
                        }
                    }
                }
                // File deleted remotely but still local and in base
                (true, false, true) => {
                    if self.pair.mode == SyncMode::TwoWay || self.pair.mode == SyncMode::CloudToLocal {
                        let info = &local[path];
                        let entry = &base[path];
                        let local_hash = watcher::hash_file(&info.absolute_path).ok();
                        let local_changed = local_hash.as_deref() != entry.local_hash.as_deref();
                        if local_changed {
                            SyncAction::Conflict {
                                relative_path: path.clone(),
                                local_path: local_root.join(path),
                                remote_id: entry.remote_id.clone().unwrap_or_default(),
                            }
                        } else {
                            SyncAction::DeleteLocal {
                                relative_path: path.clone(),
                                local_path: local_root.join(path),
                            }
                        }
                    } else {
                        SyncAction::Skip {
                            relative_path: path.clone(),
                            reason: "mode restriction on delete".into(),
                        }
                    }
                }
                // File only in base (deleted from both sides)
                (false, false, true) => SyncAction::Skip {
                    relative_path: path.clone(),
                    reason: "deleted from both sides".into(),
                },
                // Shouldn't happen
                (false, false, false) => SyncAction::Skip {
                    relative_path: path.clone(),
                    reason: "unknown state".into(),
                },
            };

            actions.push(action);
        }

        actions
    }

    /// Execute a single sync action.
    async fn execute_action(
        &self,
        action: &SyncAction,
        drive: &DriveClient,
        _local_root: &Path,
    ) -> anyhow::Result<()> {
        match action {
            SyncAction::Upload {
                relative_path,
                local_path,
                remote_parent_id,
            } => {
                let name = Path::new(relative_path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                tracing::info!("Uploading: {relative_path}");
                let file = drive.upload_file(remote_parent_id, &name, local_path).await?;
                self.update_index(relative_path, local_path, &file)?;
            }
            SyncAction::UpdateRemote {
                relative_path,
                local_path,
                remote_id,
            } => {
                tracing::info!("Updating remote: {relative_path}");
                let file = drive.update_file(remote_id, local_path).await?;
                self.update_index(relative_path, local_path, &file)?;
            }
            SyncAction::Download {
                relative_path,
                remote_id,
                local_path,
            } => {
                tracing::info!("Downloading: {relative_path}");
                drive.download_file(remote_id, local_path).await?;
                let file = drive.get_file(remote_id).await?;
                self.update_index(relative_path, local_path, &file)?;
            }
            SyncAction::DeleteLocal {
                relative_path,
                local_path,
            } => {
                tracing::info!("Deleting local: {relative_path}");
                if local_path.is_dir() {
                    tokio::fs::remove_dir_all(local_path).await?;
                } else {
                    tokio::fs::remove_file(local_path).await?;
                }
                self.db.with_conn(|conn| {
                    FileEntry::delete_by_path(conn, &self.pair.id, relative_path)?;
                    Ok(())
                })?;
            }
            SyncAction::DeleteRemote {
                relative_path,
                remote_id,
            } => {
                tracing::info!("Trashing remote: {relative_path}");
                drive.trash_file(remote_id).await?;
                self.db.with_conn(|conn| {
                    FileEntry::delete_by_path(conn, &self.pair.id, relative_path)?;
                    Ok(())
                })?;
            }
            SyncAction::CreateLocalDir {
                relative_path,
                local_path,
            } => {
                tracing::info!("Creating local dir: {relative_path}");
                tokio::fs::create_dir_all(local_path).await?;
            }
            SyncAction::CreateRemoteDir {
                relative_path,
                remote_parent_id,
                name,
            } => {
                tracing::info!("Creating remote dir: {relative_path}");
                drive.create_folder(remote_parent_id, name).await?;
            }
            SyncAction::Conflict {
                relative_path,
                local_path,
                remote_id,
            } => {
                tracing::warn!("Conflict detected: {relative_path}");
                self.handle_conflict(relative_path, local_path, remote_id, drive).await?;
            }
            SyncAction::Skip {
                relative_path,
                reason,
            } => {
                tracing::debug!("Skipping {relative_path}: {reason}");
            }
        }
        Ok(())
    }

    /// Handle a conflict according to the pair's conflict policy.
    async fn handle_conflict(
        &self,
        relative_path: &str,
        local_path: &Path,
        remote_id: &str,
        drive: &DriveClient,
    ) -> anyhow::Result<()> {
        match self.pair.conflict_policy {
            ConflictPolicy::KeepBoth => {
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

                drive.download_file(remote_id, &conflict_path).await?;
                tracing::info!("Created conflicted copy: {}", conflict_path.display());
            }
            ConflictPolicy::PreferLocal => {
                // Upload local, overwriting remote
                drive.update_file(remote_id, local_path).await?;
            }
            ConflictPolicy::PreferRemote => {
                // Download remote, overwriting local
                drive.download_file(remote_id, local_path).await?;
            }
            ConflictPolicy::NewestWins => {
                // Compare modification times
                let remote_file = drive.get_file(remote_id).await?;
                let local_mtime = std::fs::metadata(local_path)?
                    .modified()
                    .ok()
                    .map(|t| {
                        let dt: chrono::DateTime<chrono::Utc> = t.into();
                        dt
                    });
                let remote_mtime = remote_file
                    .modified_time
                    .as_deref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&chrono::Utc));

                match (local_mtime, remote_mtime) {
                    (Some(l), Some(r)) if l >= r => {
                        drive.update_file(remote_id, local_path).await?;
                    }
                    _ => {
                        drive.download_file(remote_id, local_path).await?;
                    }
                }
            }
            ConflictPolicy::Ask => {
                // Mark as conflict in the database for UI resolution
                self.db.with_conn(|conn| {
                    let mut entry = FileEntry::get_by_path(conn, &self.pair.id, relative_path)?
                        .unwrap_or_else(|| FileEntry {
                            id: None,
                            sync_pair_id: self.pair.id.clone(),
                            relative_path: relative_path.to_string(),
                            local_hash: None,
                            remote_md5: None,
                            remote_id: Some(remote_id.to_string()),
                            remote_rev: None,
                            size: None,
                            local_mtime: None,
                            remote_mtime: None,
                            is_directory: false,
                            is_google_doc: false,
                            state: FileState::Conflict,
                            last_synced_at: None,
                        });
                    entry.state = FileState::Conflict;
                    entry.upsert(conn)?;
                    Ok(())
                })?;
            }
        }
        Ok(())
    }

    /// Perform a dry run: compute actions without executing them.
    pub async fn dry_run(&self, drive: &DriveClient) -> anyhow::Result<(Vec<SyncAction>, SyncReport)> {
        tracing::info!("Dry run for pair '{}' ({})", self.pair.name, self.pair.mode);

        let local_root = PathBuf::from(&self.pair.local_root);
        let local_files = watcher::scan_directory(&local_root)?;
        let local_map: HashMap<String, watcher::LocalFileInfo> = local_files
            .into_iter()
            .map(|f| (f.relative_path.clone(), f))
            .collect();

        let remote_files = self.fetch_remote_tree(drive, &self.pair.remote_root_id, "").await?;
        let remote_map: HashMap<String, DriveFile> = remote_files
            .iter()
            .map(|(path, file)| (path.clone(), file.clone()))
            .collect();

        let base_entries = self.db.with_conn(|conn| {
            FileEntry::list_by_sync_pair(conn, &self.pair.id)
        })?;
        let base_map: HashMap<String, FileEntry> = base_entries
            .into_iter()
            .map(|e| (e.relative_path.clone(), e))
            .collect();

        let actions = self.compute_actions(&local_map, &remote_map, &base_map, &local_root);

        let mut report = SyncReport::default();
        for action in &actions {
            match action {
                SyncAction::Upload { .. } | SyncAction::UpdateRemote { .. } => report.uploaded += 1,
                SyncAction::Download { .. } => report.downloaded += 1,
                SyncAction::DeleteLocal { .. } | SyncAction::DeleteRemote { .. } => report.deleted += 1,
                SyncAction::Conflict { .. } => report.conflicts += 1,
                SyncAction::Skip { .. } => report.skipped += 1,
                SyncAction::CreateLocalDir { .. } | SyncAction::CreateRemoteDir { .. } => {}
            }
        }

        Ok((actions, report))
    }

    /// Recursively fetch the remote file tree.
    fn fetch_remote_tree<'a>(
        &'a self,
        drive: &'a DriveClient,
        folder_id: &'a str,
        prefix: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Vec<(String, DriveFile)>>> + Send + 'a>> {
        Box::pin(async move {
            let mut results = Vec::new();
            let files = drive.list_all_files(folder_id).await?;

            for file in files {
                let path = if prefix.is_empty() {
                    file.name.clone()
                } else {
                    format!("{prefix}/{}", file.name)
                };

                if file.is_folder() {
                    let sub = self.fetch_remote_tree(drive, &file.id, &path).await?;
                    results.push((path, file));
                    results.extend(sub);
                } else {
                    results.push((path, file));
                }
            }

            Ok(results)
        })
    }

    /// Update the file index after a successful sync action.
    fn update_index(
        &self,
        relative_path: &str,
        local_path: &Path,
        remote_file: &DriveFile,
    ) -> anyhow::Result<()> {
        let local_hash = if local_path.exists() && local_path.is_file() {
            Some(watcher::hash_file(local_path)?)
        } else {
            None
        };

        let local_mtime = std::fs::metadata(local_path)
            .ok()
            .and_then(|m| m.modified().ok())
            .map(|t| {
                let dt: chrono::DateTime<chrono::Utc> = t.into();
                dt.to_rfc3339()
            });

        let entry = FileEntry {
            id: None,
            sync_pair_id: self.pair.id.clone(),
            relative_path: relative_path.to_string(),
            local_hash,
            remote_md5: remote_file.md5_checksum.clone(),
            remote_id: Some(remote_file.id.clone()),
            remote_rev: None,
            size: Some(remote_file.size_bytes()),
            local_mtime,
            remote_mtime: remote_file.modified_time.clone(),
            is_directory: remote_file.is_folder(),
            is_google_doc: remote_file.is_google_doc(),
            state: FileState::Synced,
            last_synced_at: Some(chrono::Utc::now().to_rfc3339()),
        };

        self.db.with_conn(|conn| {
            entry.upsert(conn)?;
            Ok(())
        })?;

        Ok(())
    }

    /// Resolve the remote ID of the parent folder for a given relative path.
    /// Falls back to the sync pair's remote root ID for top-level entries.
    fn resolve_remote_parent_id<'a>(
        &'a self,
        relative_path: &str,
        remote: &'a HashMap<String, DriveFile>,
    ) -> &'a str {
        match Path::new(relative_path).parent() {
            Some(parent) if parent != Path::new("") => {
                let parent_str = parent.to_string_lossy();
                if let Some(parent_file) = remote.get(parent_str.as_ref()) {
                    return &parent_file.id;
                }
                &self.pair.remote_root_id
            }
            _ => &self.pair.remote_root_id,
        }
    }

    fn log_change(&self, path: &str, action: &str, detail: Option<&str>) {
        let _ = self.db.with_conn(|conn| {
            ChangeLogEntry::insert(conn, &self.pair.id, path, action, detail)?;
            Ok(())
        });
    }
}

impl SyncAction {
    /// Human-readable description for dry-run output.
    pub fn describe(&self) -> String {
        match self {
            Self::Upload { relative_path, .. } => format!("  ↑ Upload: {relative_path}"),
            Self::UpdateRemote { relative_path, .. } => format!("  ↑ Update remote: {relative_path}"),
            Self::Download { relative_path, .. } => format!("  ↓ Download: {relative_path}"),
            Self::DeleteLocal { relative_path, .. } => format!("  × Delete local: {relative_path}"),
            Self::DeleteRemote { relative_path, .. } => format!("  × Delete remote: {relative_path}"),
            Self::CreateLocalDir { relative_path, .. } => format!("  + Create local dir: {relative_path}"),
            Self::CreateRemoteDir { relative_path, .. } => format!("  + Create remote dir: {relative_path}"),
            Self::Conflict { relative_path, .. } => format!("  ⚡ Conflict: {relative_path}"),
            Self::Skip { relative_path, reason } => format!("  · Skip: {relative_path} ({reason})"),
        }
    }
}

fn action_path(action: &SyncAction) -> Option<String> {
    match action {
        SyncAction::Upload { relative_path, .. }
        | SyncAction::UpdateRemote { relative_path, .. }
        | SyncAction::Download { relative_path, .. }
        | SyncAction::DeleteLocal { relative_path, .. }
        | SyncAction::DeleteRemote { relative_path, .. }
        | SyncAction::CreateLocalDir { relative_path, .. }
        | SyncAction::CreateRemoteDir { relative_path, .. }
        | SyncAction::Conflict { relative_path, .. }
        | SyncAction::Skip { relative_path, .. } => Some(relative_path.clone()),
    }
}

#[derive(Debug, Default)]
pub struct SyncReport {
    pub uploaded: usize,
    pub downloaded: usize,
    pub deleted: usize,
    pub conflicts: usize,
    pub skipped: usize,
    pub errors: usize,
}

impl std::fmt::Display for SyncReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} uploaded, {} downloaded, {} deleted, {} conflicts, {} errors",
            self.uploaded, self.downloaded, self.deleted, self.conflicts, self.errors
        )
    }
}

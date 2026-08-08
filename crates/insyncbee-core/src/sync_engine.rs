//! Core sync engine: orchestrates change detection, conflict resolution,
//! and file transfer between local filesystem and Google Drive.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::crypto::FileCipher;
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
        // Carry the source DriveFile so we can index the new directory and
        // detect later remote deletions (otherwise the folder is re-uploaded
        // on the next sync).
        remote: DriveFile,
    },
    CreateRemoteDir {
        relative_path: String,
        local_path: PathBuf,
        remote_parent_id: String,
        name: String,
    },
    Conflict {
        relative_path: String,
        local_path: PathBuf,
        remote_id: String,
        kind: ConflictKind,
    },
    Skip {
        relative_path: String,
        reason: String,
    },
}

/// Which of the three-way comparison arms produced a conflict.
///
/// This drives one decision: whether resolving the conflict may write a new
/// base state into `file_index`. When both sides still hold a live file,
/// recording the resolution is what makes the pair *converge* — without it
/// the same conflict re-fires on every cycle forever (and `KeepBoth` spawns
/// a fresh timestamped copy each time).
///
/// When one side was deleted, no safe base state exists: writing one would
/// make the next cycle read the surviving file as "unchanged since the
/// delete" and propagate the deletion, destroying the very edit the
/// conflict was protecting. Those arms stay unresolved on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// Both sides edited a tracked file between cycles.
    BothModified,
    /// The file exists on both sides with different content and no base
    /// entry — the first sync of a folder that already had contents.
    FirstSyncDivergent,
    /// Deleted locally, modified remotely.
    LocalDeletedRemoteModified,
    /// Deleted remotely, modified locally.
    RemoteDeletedLocalModified,
}

impl ConflictKind {
    /// True when a live file exists on *both* sides, so the resolution can
    /// be recorded as the new base state.
    pub fn both_sides_present(&self) -> bool {
        matches!(self, Self::BothModified | Self::FirstSyncDivergent)
    }
}

/// The sync engine coordinates syncing for a single sync pair.
pub struct SyncEngine {
    db: Database,
    pair: SyncPair,
    /// Cipher used to encrypt/decrypt file contents at the Drive boundary.
    /// `Some` iff `pair.encryption_enabled`. Leaving it `None` for
    /// encrypted pairs means uploads/downloads will fail loudly rather
    /// than silently writing plaintext to Drive — that's the intended
    /// safety property: callers MUST attach a cipher before sync.
    cipher: Option<Arc<FileCipher>>,
    /// Reports which phase the cycle is in. A sync spends real time
    /// scanning and listing before a single byte moves, and folder
    /// creates and deletes never produce byte progress at all — without
    /// this the UI has nothing to show during those stretches and a
    /// working sync looks like a hung one.
    status: Option<StatusCallback>,
}

/// Called as a sync cycle moves between phases.
pub type StatusCallback = Arc<dyn Fn(SyncStatus) + Send + Sync>;

/// Which phase a sync cycle is in, and how far through the work it is.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub phase: SyncPhase,
    /// Actions completed / total, meaningful in [`SyncPhase::Executing`].
    pub done: usize,
    pub total: usize,
    /// What is being done right now, e.g. "upload" or "create-remote-dir".
    pub action: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SyncPhase {
    ScanningLocal,
    ListingRemote,
    Comparing,
    Executing,
    Finished,
}

impl SyncEngine {
    pub fn new(db: Database, pair: SyncPair) -> Self {
        Self { db, pair, cipher: None, status: None }
    }

    /// Attach a phase reporter. The GUI uses this to drive its per-pair
    /// activity indicator; the CLI leaves it unset.
    pub fn with_status_callback(mut self, cb: StatusCallback) -> Self {
        self.status = Some(cb);
        self
    }

    fn report_status(
        &self,
        phase: SyncPhase,
        done: usize,
        total: usize,
        action: Option<&str>,
        path: Option<&str>,
    ) {
        if let Some(cb) = &self.status {
            cb(SyncStatus {
                phase,
                done,
                total,
                action: action.map(str::to_string),
                path: path.map(str::to_string),
            });
        }
    }

    /// Attach the cipher derived from the pair's passphrase. Required for
    /// any pair where `encryption_enabled = true` — without it the engine
    /// refuses to upload or download (returning an error per file rather
    /// than ever leaking plaintext to Drive or writing ciphertext to disk).
    pub fn with_cipher(mut self, cipher: Arc<FileCipher>) -> Self {
        self.cipher = Some(cipher);
        self
    }

    /// Encrypt `local_path` to a fresh temp file and hand the temp file
    /// to `drive.upload_file`. Falls back to a direct upload when the pair
    /// isn't encrypted. Errors loudly if the pair is marked encrypted but
    /// no cipher was attached, so we never push plaintext under that mode.
    pub async fn upload_via(
        &self,
        drive: &dyn DriveClient,
        parent_id: &str,
        name: &str,
        local_path: &Path,
    ) -> anyhow::Result<DriveFile> {
        if self.pair.encryption_enabled {
            let cipher = self.require_cipher()?;
            let temp = tempfile::NamedTempFile::new()?;
            cipher.encrypt_file(local_path, temp.path()).await?;
            let result = drive.upload_file(parent_id, name, temp.path()).await;
            drop(temp);
            result
        } else {
            drive.upload_file(parent_id, name, local_path).await
        }
    }

    pub async fn update_remote_via(
        &self,
        drive: &dyn DriveClient,
        remote_id: &str,
        local_path: &Path,
    ) -> anyhow::Result<DriveFile> {
        if self.pair.encryption_enabled {
            let cipher = self.require_cipher()?;
            let temp = tempfile::NamedTempFile::new()?;
            cipher.encrypt_file(local_path, temp.path()).await?;
            let result = drive.update_file(remote_id, temp.path()).await;
            drop(temp);
            result
        } else {
            drive.update_file(remote_id, local_path).await
        }
    }

    /// Download `remote_id` and place the plaintext at `local_path`. For
    /// encrypted pairs we download to a temp file first and decrypt
    /// onto the destination; that keeps the on-disk file plaintext, which
    /// is what every other part of the engine (hashing, conflict logic,
    /// the user's filesystem) expects.
    pub async fn download_via(
        &self,
        drive: &dyn DriveClient,
        remote_id: &str,
        local_path: &Path,
    ) -> anyhow::Result<()> {
        if self.pair.encryption_enabled {
            let cipher = self.require_cipher()?;
            let temp = tempfile::NamedTempFile::new()?;
            drive.download_file(remote_id, temp.path()).await?;
            cipher.decrypt_file(temp.path(), local_path).await?;
            drop(temp);
            Ok(())
        } else {
            drive.download_file(remote_id, local_path).await
        }
    }

    fn require_cipher(&self) -> anyhow::Result<&FileCipher> {
        self.cipher.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "sync pair '{}' is encrypted but no key is loaded — unlock it from the UI before syncing",
                self.pair.name
            )
        })
    }

    /// Perform a full sync cycle: scan local, fetch remote, compare, execute actions.
    pub async fn sync(&self, drive: &dyn DriveClient) -> anyhow::Result<SyncReport> {
        let mut report = SyncReport::default();

        tracing::info!("Starting sync for pair '{}' ({})", self.pair.name, self.pair.mode);

        // 1. Scan local filesystem
        self.report_status(SyncPhase::ScanningLocal, 0, 0, None, None);
        let local_root = PathBuf::from(&self.pair.local_root);
        let local_files = watcher::scan_directory(&local_root)?;
        let local_map: HashMap<String, watcher::LocalFileInfo> = local_files
            .into_iter()
            .map(|f| (f.relative_path.clone(), f))
            .collect();

        // 2. Fetch remote file list (recursively)
        self.report_status(SyncPhase::ListingRemote, 0, 0, None, None);
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
        self.report_status(SyncPhase::Comparing, 0, 0, None, None);
        let mut actions = self.compute_actions(&local_map, &remote_map, &base_map, &local_root);

        // 4b. Sort so dependencies are respected:
        //     creates → file ops → deletes (children-first) → conflicts → skips
        sort_actions(&mut actions);

        // 4c. Track every known remote folder by relative path so child
        //     uploads can find their parent's drive ID even when the parent
        //     was created earlier in the same sync.
        let mut remote_ids: HashMap<String, String> = remote_map
            .iter()
            .filter(|(_, f)| f.is_folder())
            .map(|(p, f)| (p.clone(), f.id.clone()))
            .collect();

        // 5. Execute actions
        //
        // Skips are counted as work so the progress fraction matches the
        // list the user would see: a cycle that skips 900 unchanged files
        // and uploads 2 should not sit at "2 of 2" for its whole duration.
        let total_actions = actions.len();
        for (index, action) in actions.iter().enumerate() {
            self.report_status(
                SyncPhase::Executing,
                index,
                total_actions,
                Some(action.kind()),
                action_path(action).as_deref(),
            );
            match self.execute_action(action, drive, &local_root, &mut remote_ids).await {
                Ok(outcome) => match action {
                    SyncAction::Upload { relative_path, .. }
                    | SyncAction::UpdateRemote { relative_path, .. } => {
                        report.uploaded += 1;
                        report.bytes_uploaded += outcome.metrics.map_or(0, |m| m.bytes);
                        self.log_transfer(relative_path, "upload", outcome.metrics);
                    }
                    SyncAction::Download { relative_path, .. } => {
                        report.downloaded += 1;
                        report.bytes_downloaded += outcome.metrics.map_or(0, |m| m.bytes);
                        self.log_transfer(relative_path, "download", outcome.metrics);
                    }
                    SyncAction::DeleteLocal { relative_path, .. } => {
                        report.deleted += 1;
                        self.log_change(relative_path, "delete-local", outcome.detail());
                    }
                    SyncAction::DeleteRemote { relative_path, .. } => {
                        report.deleted += 1;
                        self.log_change(relative_path, "delete-remote", outcome.detail());
                    }
                    SyncAction::Conflict { relative_path, .. } => {
                        report.conflicts += 1;
                        self.log_change(relative_path, "conflict", None);
                    }
                    // Folder creates were previously silent. A sync that
                    // only made directories logged nothing at all, so the
                    // activity feed showed an empty result for real work.
                    SyncAction::CreateLocalDir { relative_path, .. } => {
                        report.dirs_created += 1;
                        self.log_change(relative_path, "create-local-dir", None);
                    }
                    SyncAction::CreateRemoteDir { relative_path, .. } => {
                        report.dirs_created += 1;
                        self.log_change(relative_path, "create-remote-dir", None);
                    }
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
            "Sync complete for '{}': {} up, {} down, {} dirs, {} deleted, {} conflicts, {} errors",
            self.pair.name,
            report.uploaded,
            report.downloaded,
            report.dirs_created,
            report.deleted,
            report.conflicts,
            report.errors
        );

        self.report_status(
            SyncPhase::Finished,
            total_actions,
            total_actions,
            None,
            None,
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
                                local_path: local_root.join(path),
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
                                remote: file.clone(),
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
                    } else if self.pair.encryption_enabled {
                        // For encrypted pairs we can't compare hashes
                        // here: local hash is plaintext blake3, remote
                        // MD5 is over ciphertext we didn't produce. Defer
                        // to the conflict handler so the user resolves
                        // it once on first sync, then the base entry
                        // exists for all future cycles.
                        SyncAction::Conflict {
                            relative_path: path.clone(),
                            local_path: local_root.join(path),
                            remote_id: file.id.clone(),
                            kind: ConflictKind::FirstSyncDivergent,
                        }
                    } else {
                        // Compare content across the local/remote boundary.
                        // This is the ONE place where we compare a local file
                        // to a remote one without a base entry to route
                        // through, so it must use MD5 — the only hash Drive
                        // exposes. Comparing blake3 here would make every
                        // adopted folder look divergent and spawn a
                        // conflicted copy of every single file.
                        let local_md5 = watcher::md5_file(&info.absolute_path).ok();
                        if local_md5.is_some() && local_md5.as_deref() == file.md5_checksum.as_deref() {
                            SyncAction::Skip {
                                relative_path: path.clone(),
                                reason: "identical content".into(),
                            }
                        } else {
                            SyncAction::Conflict {
                                relative_path: path.clone(),
                                local_path: local_root.join(path),
                                remote_id: file.id.clone(),
                                kind: ConflictKind::FirstSyncDivergent,
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
                                kind: ConflictKind::BothModified,
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
                                kind: ConflictKind::LocalDeletedRemoteModified,
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
                                kind: ConflictKind::RemoteDeletedLocalModified,
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
    ///
    /// `remote_ids` maps relative_path → drive folder ID for every folder
    /// known to exist on the remote at this moment in the sync — including
    /// folders just created earlier in the same sync. The Upload arm
    /// consults it so a child uploaded after its parent was created lands
    /// in the right folder instead of falling back to the sync root.
    async fn execute_action(
        &self,
        action: &SyncAction,
        drive: &dyn DriveClient,
        _local_root: &Path,
        remote_ids: &mut HashMap<String, String>,
    ) -> anyhow::Result<ActionOutcome> {
        // Filled in by the arms that have something to report; everything
        // else logs without a size, a speed, or a detail.
        let mut outcome = ActionOutcome::default();
        let metrics = &mut outcome.metrics;

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
                // Re-resolve the parent at execute time: the
                // remote_parent_id snapshotted by compute_actions can be
                // stale when this child's parent folder was itself created
                // earlier in the same sync.
                let parent_id = self
                    .resolve_parent_runtime(relative_path, remote_ids)
                    .unwrap_or_else(|| remote_parent_id.clone());
                tracing::info!("Uploading: {relative_path} (parent {parent_id})");
                let started = std::time::Instant::now();
                let file = self.upload_via(drive, &parent_id, &name, local_path).await?;
                *metrics = Some(TransferMetrics::measured(local_size(local_path), started));
                self.update_index(relative_path, local_path, &file)?;
            }
            SyncAction::UpdateRemote {
                relative_path,
                local_path,
                remote_id,
            } => {
                tracing::info!("Updating remote: {relative_path}");
                let started = std::time::Instant::now();
                let file = self.update_remote_via(drive, remote_id, local_path).await?;
                *metrics = Some(TransferMetrics::measured(local_size(local_path), started));
                self.update_index(relative_path, local_path, &file)?;
            }
            SyncAction::Download {
                relative_path,
                remote_id,
                local_path,
            } => {
                tracing::info!("Downloading: {relative_path}");
                let started = std::time::Instant::now();
                self.download_via(drive, remote_id, local_path).await?;
                // Measure before get_file: that metadata round-trip is not
                // part of the transfer and would drag the speed down.
                *metrics = Some(TransferMetrics::measured(local_size(local_path), started));
                let file = drive.get_file(remote_id).await?;
                self.update_index(relative_path, local_path, &file)?;
            }
            SyncAction::DeleteLocal {
                relative_path,
                local_path,
            } => {
                tracing::info!("Deleting local: {relative_path}");
                // Be tolerant of already-gone targets: when a folder delete
                // runs first, its children's actions still fire afterwards
                // but the paths no longer exist. We still want the index
                // cleaned up, not a spurious error.
                if local_path.is_dir() {
                    outcome.was_directory = true;
                    if let Err(e) = tokio::fs::remove_dir_all(local_path).await {
                        if e.kind() != std::io::ErrorKind::NotFound {
                            return Err(e.into());
                        }
                    }
                } else if local_path.exists() {
                    if let Err(e) = tokio::fs::remove_file(local_path).await {
                        if e.kind() != std::io::ErrorKind::NotFound {
                            return Err(e.into());
                        }
                    }
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
                // Read the indexed entry before deleting it: the local copy
                // is already gone (that's why we're here), so the index is
                // the only remaining record of whether this was a folder.
                outcome.was_directory = self
                    .db
                    .with_conn(|conn| FileEntry::get_by_path(conn, &self.pair.id, relative_path))
                    .ok()
                    .flatten()
                    .map(|e| e.is_directory)
                    .unwrap_or(false);
                // Trashing a folder cascades to its contents on Drive's side,
                // so child trash calls afterwards may 404. Treat that as
                // success — we still want the local index cleaned up.
                if let Err(e) = drive.trash_file(remote_id).await {
                    let msg = e.to_string();
                    let already_gone = msg.contains("404")
                        || msg.contains("notFound")
                        || msg.contains("not found");
                    if !already_gone {
                        return Err(e);
                    }
                }
                self.db.with_conn(|conn| {
                    FileEntry::delete_by_path(conn, &self.pair.id, relative_path)?;
                    Ok(())
                })?;
            }
            SyncAction::CreateLocalDir {
                relative_path,
                local_path,
                remote,
            } => {
                tracing::info!("Creating local dir: {relative_path}");
                tokio::fs::create_dir_all(local_path).await?;
                // Index the new directory so a later remote-side delete is
                // detected as (true, false, true) → DeleteLocal instead of
                // (true, false, false) → re-upload.
                self.update_index(relative_path, local_path, remote)?;
            }
            SyncAction::CreateRemoteDir {
                relative_path,
                local_path,
                remote_parent_id,
                name,
            } => {
                // Same execute-time parent resolution as Upload — handles
                // nested local-originated folders (a/b/c) where b is also
                // brand-new in this same sync.
                let parent_id = self
                    .resolve_parent_runtime(relative_path, remote_ids)
                    .unwrap_or_else(|| remote_parent_id.clone());
                tracing::info!("Creating remote dir: {relative_path} (parent {parent_id})");
                let created = drive.create_folder(&parent_id, name).await?;
                // Register the new folder so subsequent uploads / nested
                // CreateRemoteDir actions can resolve it as their parent.
                remote_ids.insert(relative_path.clone(), created.id.clone());
                // Index the folder so a later local delete is detected as
                // (false, true, true) → DeleteRemote instead of being
                // re-discovered as a brand-new remote folder.
                self.update_index(relative_path, local_path, &created)?;
            }
            SyncAction::Conflict {
                relative_path,
                local_path,
                remote_id,
                kind,
            } => {
                tracing::warn!("Conflict detected: {relative_path} ({kind:?})");
                self.handle_conflict(relative_path, local_path, remote_id, *kind, drive)
                    .await?;
            }
            SyncAction::Skip {
                relative_path,
                reason,
            } => {
                tracing::debug!("Skipping {relative_path}: {reason}");
            }
        }
        Ok(outcome)
    }

    /// Handle a conflict according to the pair's conflict policy.
    ///
    /// Every policy except [`ConflictPolicy::Ask`] *resolves* the conflict,
    /// and a resolution is only complete once the outcome is recorded as the
    /// new base state — otherwise the next cycle recomputes the identical
    /// conflict from the identical stale base and resolves it again, forever.
    /// For `KeepBoth` that meant a new timestamped copy on every poll
    /// interval; for the overwrite policies, re-transferring the same file
    /// on every cycle.
    ///
    /// The write-back is gated on [`ConflictKind::both_sides_present`]: when
    /// one side was deleted there is no base state that doesn't tell the next
    /// cycle to finish the deletion, so those conflicts deliberately stay
    /// pending. See `tests/SCENARIOS.md` (D3/D4) for that open gap.
    async fn handle_conflict(
        &self,
        relative_path: &str,
        local_path: &Path,
        remote_id: &str,
        kind: ConflictKind,
        drive: &dyn DriveClient,
    ) -> anyhow::Result<()> {
        // Set by each resolving branch to the remote file as it stands after
        // resolution; `None` means "left unresolved on purpose".
        let resolved: Option<DriveFile>;

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

                self.download_via(drive, remote_id, &conflict_path).await?;
                tracing::info!("Created conflicted copy: {}", conflict_path.display());
                // Both versions are now on disk, so the divergence has been
                // dealt with as far as the engine is concerned. Recording it
                // is what stops the next cycle producing another copy.
                resolved = Some(drive.get_file(remote_id).await?);
            }
            ConflictPolicy::PreferLocal => {
                // Upload local, overwriting remote
                resolved = Some(self.update_remote_via(drive, remote_id, local_path).await?);
            }
            ConflictPolicy::PreferRemote => {
                // Download remote, overwriting local
                self.download_via(drive, remote_id, local_path).await?;
                resolved = Some(drive.get_file(remote_id).await?);
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
                        resolved =
                            Some(self.update_remote_via(drive, remote_id, local_path).await?);
                    }
                    _ => {
                        self.download_via(drive, remote_id, local_path).await?;
                        resolved = Some(drive.get_file(remote_id).await?);
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
                // Deliberately unresolved: the user hasn't chosen yet, so the
                // conflict must keep being reported on every cycle until they do.
                resolved = None;
            }
        }

        if let (true, Some(remote_file)) = (kind.both_sides_present(), resolved) {
            self.update_index(relative_path, local_path, &remote_file)?;
        }

        Ok(())
    }

    /// Perform a dry run: compute actions without executing them.
    pub async fn dry_run(&self, drive: &dyn DriveClient) -> anyhow::Result<(Vec<SyncAction>, SyncReport)> {
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

        let mut actions = self.compute_actions(&local_map, &remote_map, &base_map, &local_root);
        sort_actions(&mut actions);

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
        drive: &'a dyn DriveClient,
        folder_id: &'a str,
        prefix: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Vec<(String, DriveFile)>>> + Send + 'a>> {
        Box::pin(async move {
            let mut results = Vec::new();
            let files = drive.list_all_files(folder_id).await?;

            for file in files {
                // Apply the scanner's hidden-file rule to the remote side
                // too. `watcher::scan_directory` skips dot-prefixed entries,
                // so a remote dotfile we downloaded would be invisible to the
                // very next scan — the three-way comparison would read
                // `(local=false, remote=true, base=true)`, conclude the user
                // deleted it, and trash it on Drive. Ignoring it on both
                // sides keeps the rule symmetric and the file safe.
                if is_hidden_name(&file.name) {
                    tracing::debug!("Ignoring hidden remote entry: {}/{}", prefix, file.name);
                    continue;
                }

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

    /// Resolve the parent's drive ID using a runtime path → id map that
    /// includes folders just created earlier in this same sync. Returns
    /// `Some(self.pair.remote_root_id)` for top-level entries.
    ///
    /// Returns `None` only when the path has a non-empty parent that we
    /// genuinely don't know about — callers fall back to the parent ID
    /// snapshotted at compute time.
    fn resolve_parent_runtime(
        &self,
        relative_path: &str,
        remote_ids: &HashMap<String, String>,
    ) -> Option<String> {
        match Path::new(relative_path).parent() {
            Some(p) if !p.as_os_str().is_empty() => {
                let key = p.to_string_lossy().to_string();
                remote_ids.get(&key).cloned()
            }
            _ => Some(self.pair.remote_root_id.clone()),
        }
    }

    fn log_change(&self, path: &str, action: &str, detail: Option<&str>) {
        let _ = self.db.with_conn(|conn| {
            ChangeLogEntry::insert(conn, &self.pair.id, path, action, detail)?;
            Ok(())
        });
    }

    /// Log a transfer with what it moved, so the activity feed can show a
    /// speed and the statistics page can sum it.
    fn log_transfer(&self, path: &str, action: &str, metrics: Option<TransferMetrics>) {
        let _ = self.db.with_conn(|conn| {
            ChangeLogEntry::insert_transfer(
                conn,
                &self.pair.id,
                path,
                action,
                None,
                metrics.map(|m| m.bytes as i64),
                metrics.map(|m| m.duration_ms as i64),
            )?;
            Ok(())
        });
    }
}

/// What executing one action produced, for the caller to log.
#[derive(Debug, Clone, Default)]
struct ActionOutcome {
    metrics: Option<TransferMetrics>,
    /// Whether a delete removed a directory rather than a file. The
    /// activity feed reads "deleted" very differently for a folder, and by
    /// log time the path is gone so it can no longer be inspected.
    was_directory: bool,
}

impl ActionOutcome {
    fn detail(&self) -> Option<&'static str> {
        self.was_directory.then_some("folder")
    }
}

/// What a single transfer moved, and how long it took.
#[derive(Debug, Clone, Copy)]
pub struct TransferMetrics {
    pub bytes: u64,
    pub duration_ms: u64,
}

impl TransferMetrics {
    fn measured(bytes: u64, started: std::time::Instant) -> Self {
        Self {
            bytes,
            // Round up: a sub-millisecond transfer of a small file would
            // otherwise record 0 ms and make the speed calculation divide
            // by zero.
            duration_ms: (started.elapsed().as_millis() as u64).max(1),
        }
    }
}

/// Render a byte count the way a person reads it: `1.4 MB`, not `1468006`.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// Size of a local file, or 0 when it can't be read.
///
/// For encrypted pairs this is the plaintext size, not the ciphertext that
/// actually crossed the wire. That's the deliberate choice: users measure
/// their transfers against the files they can see, and the difference is a
/// small constant per-chunk overhead.
fn local_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

impl SyncAction {
    /// Stable identifier for this kind of action. Matches the `action`
    /// values written to the change log, so the live status the UI shows
    /// mid-sync and the row it shows afterwards use the same vocabulary.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Upload { .. } | Self::UpdateRemote { .. } => "upload",
            Self::Download { .. } => "download",
            Self::DeleteLocal { .. } => "delete-local",
            Self::DeleteRemote { .. } => "delete-remote",
            Self::CreateLocalDir { .. } => "create-local-dir",
            Self::CreateRemoteDir { .. } => "create-remote-dir",
            Self::Conflict { .. } => "conflict",
            Self::Skip { .. } => "skip",
        }
    }

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

/// Order actions so dependencies always resolve:
///
/// 1. Folder creates first, **shallowest first** so a child folder's parent
///    has already been created (and registered in `remote_ids`) when its
///    own create runs.
/// 2. File ops next.
/// 3. Deletes last, **deepest first** so a `remove_dir_all` doesn't precede
///    its children's individual delete actions and turn them into NotFound
///    errors. (We tolerate NotFound anyway, but ordering keeps the logs clean.)
/// 4. Conflicts and Skips at the end — they don't mutate either side, so
///    their position only affects log readability.
fn sort_actions(actions: &mut Vec<SyncAction>) {
    actions.sort_by_key(|a| {
        let path = action_path(a).unwrap_or_default();
        let depth = path.matches(std::path::MAIN_SEPARATOR).count() as i64
            + path.matches('/').count() as i64;
        let (bucket, depth_key): (i32, i64) = match a {
            SyncAction::CreateLocalDir { .. } | SyncAction::CreateRemoteDir { .. } => (0, depth),
            SyncAction::Upload { .. }
            | SyncAction::UpdateRemote { .. }
            | SyncAction::Download { .. } => (1, depth),
            SyncAction::DeleteLocal { .. } | SyncAction::DeleteRemote { .. } => (2, -depth),
            SyncAction::Conflict { .. } => (3, depth),
            SyncAction::Skip { .. } => (4, depth),
        };
        (bucket, depth_key, path)
    });
}

/// Entries InSyncBee deliberately does not sync, in either direction.
///
/// Must stay in agreement with the filter in
/// [`watcher::scan_directory`] — an entry that one side hides and the other
/// syncs is read as a deletion by the three-way comparison.
fn is_hidden_name(name: &str) -> bool {
    name.starts_with('.')
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
    pub bytes_uploaded: u64,
    pub bytes_downloaded: u64,
    pub dirs_created: usize,
}

impl std::fmt::Display for SyncReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} uploaded ({}), {} downloaded ({}), {} folders created, {} deleted, {} conflicts, {} errors",
            self.uploaded,
            format_bytes(self.bytes_uploaded),
            self.downloaded,
            format_bytes(self.bytes_downloaded),
            self.dirs_created,
            self.deleted,
            self.conflicts,
            self.errors
        )
    }
}

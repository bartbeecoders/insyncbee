//! Shared test scaffolding: an in-memory `FakeDriveClient`, a tempdir
//! helper, and convenience builders for `SyncPair` / `Account`.
//!
//! Tests rely on this module via `mod common;`.

#![allow(dead_code)] // not every test uses every helper

use async_trait::async_trait;
use insyncbee_core::db::models::{
    Account, ConflictPolicy, SyncMode, SyncPair, SyncPairStatus,
};
use insyncbee_core::db::Database;
use insyncbee_core::drive::{DriveClient, DriveFile};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tempfile::TempDir;

// ── FakeDriveClient ──────────────────────────────────────────────────

/// In-memory implementation of [`DriveClient`] used by integration tests.
///
/// Files live in a `HashMap<id, FakeFile>`; folders are just `FakeFile`
/// records with the Drive folder MIME. Synthetic IDs are minted from a
/// monotonic counter.
pub struct FakeDriveClient {
    files: Mutex<HashMap<String, FakeFile>>,
    next_id: AtomicU64,
    pub call_log: Mutex<Vec<String>>,
}

#[derive(Clone)]
pub struct FakeFile {
    pub meta: DriveFile,
    pub bytes: Vec<u8>,
}

impl FakeDriveClient {
    pub fn new() -> Self {
        Self {
            files: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            call_log: Mutex::new(Vec::new()),
        }
    }

    /// Create the synthetic "root" folder that a sync pair will point at.
    /// Returns the new folder's ID.
    pub fn make_root(&self, name: &str) -> String {
        self.insert_folder(name, None)
    }

    /// Insert a folder. Returns its ID.
    pub fn insert_folder(&self, name: &str, parent: Option<&str>) -> String {
        let id = self.mint_id("folder");
        let meta = DriveFile {
            id: id.clone(),
            name: name.to_string(),
            mime_type: "application/vnd.google-apps.folder".to_string(),
            md5_checksum: None,
            size: None,
            modified_time: Some(chrono::Utc::now().to_rfc3339()),
            parents: parent.map(|p| vec![p.to_string()]),
        };
        self.files
            .lock()
            .unwrap()
            .insert(id.clone(), FakeFile { meta, bytes: Vec::new() });
        id
    }

    /// Insert a file with explicit bytes. Returns its ID.
    pub fn insert_file(&self, name: &str, parent: &str, bytes: Vec<u8>) -> String {
        let id = self.mint_id("file");
        let meta = DriveFile {
            id: id.clone(),
            name: name.to_string(),
            mime_type: "text/plain".to_string(),
            md5_checksum: Some(md5_hex(&bytes)),
            size: Some(bytes.len().to_string()),
            modified_time: Some(chrono::Utc::now().to_rfc3339()),
            parents: Some(vec![parent.to_string()]),
        };
        self.files
            .lock()
            .unwrap()
            .insert(id.clone(), FakeFile { meta, bytes });
        id
    }

    /// Total number of files+folders the fake currently holds.
    pub fn count(&self) -> usize {
        self.files.lock().unwrap().len()
    }

    /// Snapshot of every file the fake holds, keyed by name (for quick
    /// lookup in assertions). Last-write-wins on duplicate names.
    pub fn snapshot_by_name(&self) -> HashMap<String, FakeFile> {
        self.files
            .lock()
            .unwrap()
            .values()
            .map(|f| (f.meta.name.clone(), f.clone()))
            .collect()
    }

    pub fn calls(&self) -> Vec<String> {
        self.call_log.lock().unwrap().clone()
    }

    fn mint_id(&self, prefix: &str) -> String {
        let n = self.next_id.fetch_add(1, Ordering::SeqCst);
        format!("{prefix}_{n:08}")
    }

    fn record(&self, call: &str) {
        self.call_log.lock().unwrap().push(call.to_string());
    }
}

#[async_trait]
impl DriveClient for FakeDriveClient {
    async fn list_all_files(&self, folder_id: &str) -> anyhow::Result<Vec<DriveFile>> {
        self.record(&format!("list_all_files({folder_id})"));
        let files = self.files.lock().unwrap();
        Ok(files
            .values()
            .filter(|f| {
                f.meta
                    .parents
                    .as_deref()
                    .map(|ps| ps.iter().any(|p| p == folder_id))
                    .unwrap_or(false)
            })
            .map(|f| f.meta.clone())
            .collect())
    }

    async fn get_file(&self, file_id: &str) -> anyhow::Result<DriveFile> {
        self.record(&format!("get_file({file_id})"));
        self.files
            .lock()
            .unwrap()
            .get(file_id)
            .map(|f| f.meta.clone())
            .ok_or_else(|| anyhow::anyhow!("FakeDriveClient: no such file {file_id}"))
    }

    async fn download_file(&self, file_id: &str, dest: &Path) -> anyhow::Result<()> {
        self.record(&format!("download_file({file_id})"));
        let bytes = {
            let files = self.files.lock().unwrap();
            files
                .get(file_id)
                .map(|f| f.bytes.clone())
                .ok_or_else(|| anyhow::anyhow!("FakeDriveClient: no such file {file_id}"))?
        };
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(dest, &bytes).await?;
        Ok(())
    }

    async fn upload_file(
        &self,
        parent_id: &str,
        name: &str,
        local_path: &Path,
    ) -> anyhow::Result<DriveFile> {
        self.record(&format!("upload_file({parent_id}, {name})"));
        let bytes = tokio::fs::read(local_path).await?;
        let id = self.insert_file(name, parent_id, bytes);
        Ok(self.files.lock().unwrap().get(&id).unwrap().meta.clone())
    }

    async fn update_file(&self, file_id: &str, local_path: &Path) -> anyhow::Result<DriveFile> {
        self.record(&format!("update_file({file_id})"));
        let bytes = tokio::fs::read(local_path).await?;
        let mut files = self.files.lock().unwrap();
        let f = files
            .get_mut(file_id)
            .ok_or_else(|| anyhow::anyhow!("FakeDriveClient: no such file {file_id}"))?;
        f.meta.md5_checksum = Some(md5_hex(&bytes));
        f.meta.size = Some(bytes.len().to_string());
        f.meta.modified_time = Some(chrono::Utc::now().to_rfc3339());
        f.bytes = bytes;
        Ok(f.meta.clone())
    }

    async fn create_folder(&self, parent_id: &str, name: &str) -> anyhow::Result<DriveFile> {
        self.record(&format!("create_folder({parent_id}, {name})"));
        let id = self.insert_folder(name, Some(parent_id));
        Ok(self.files.lock().unwrap().get(&id).unwrap().meta.clone())
    }

    async fn trash_file(&self, file_id: &str) -> anyhow::Result<()> {
        self.record(&format!("trash_file({file_id})"));
        self.files.lock().unwrap().remove(file_id);
        Ok(())
    }
}

// ── Builders ─────────────────────────────────────────────────────────

pub fn test_db() -> Database {
    Database::open_in_memory().expect("open in-memory DB")
}

/// Build an in-memory DB plus an `Account` row referenced by sync pairs.
pub fn test_db_with_account() -> (Database, Account) {
    let db = test_db();
    let account = Account {
        id: "acc-test".to_string(),
        email: "test@example.com".to_string(),
        display_name: Some("Test User".to_string()),
        access_token: "fake-access".to_string(),
        refresh_token: "fake-refresh".to_string(),
        token_expiry: chrono::Utc::now().to_rfc3339(),
        change_token: None,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    db.with_conn(|conn| account.insert(conn)).unwrap();
    (db, account)
}

/// Build a `SyncPair` rooted at the given local directory and remote ID.
pub fn make_pair(account_id: &str, local: &Path, remote_root_id: &str, mode: SyncMode) -> SyncPair {
    SyncPair {
        id: format!("pair-{}", uuid::Uuid::new_v4()),
        name: "test-pair".to_string(),
        account_id: account_id.to_string(),
        local_root: local.to_string_lossy().to_string(),
        remote_root_id: remote_root_id.to_string(),
        remote_root_path: "/".to_string(),
        mode,
        conflict_policy: ConflictPolicy::KeepBoth,
        status: SyncPairStatus::Active,
        poll_interval_secs: 30,
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    }
}

/// Bundle a tempdir with a pair persisted to the DB. Drop the bundle to
/// clean everything up.
pub struct SyncFixture {
    pub db: Database,
    pub pair: SyncPair,
    pub fake: FakeDriveClient,
    pub local: TempDir,
    pub remote_root: String,
}

impl SyncFixture {
    pub fn new(mode: SyncMode) -> Self {
        let (db, account) = test_db_with_account();
        let fake = FakeDriveClient::new();
        let remote_root = fake.make_root("Test Root");
        let local = TempDir::new().expect("tempdir");
        let pair = make_pair(&account.id, local.path(), &remote_root, mode);
        db.with_conn(|conn| pair.insert(conn)).unwrap();
        Self { db, pair, fake, local, remote_root }
    }

    pub fn local_path(&self) -> PathBuf {
        self.local.path().to_path_buf()
    }

    pub fn write_local(&self, rel: &str, contents: &str) -> PathBuf {
        let p = self.local.path().join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, contents).unwrap();
        p
    }
}

// ── md5 ──────────────────────────────────────────────────────────────
// Drive content checksums are MD5. We emit a small reimplementation here so
// the integration tests don't have to take a runtime dependency on a md5
// crate just for one helper.

fn md5_hex(bytes: &[u8]) -> String {
    // Use a minimal stable hash to mimic Drive's md5Checksum field. We don't
    // actually need cryptographic md5 here — only stability across runs.
    // blake3 is already a dependency, so use a truncated blake3 hex string.
    let h = blake3::hash(bytes);
    h.to_hex().as_str().chars().take(32).collect()
}

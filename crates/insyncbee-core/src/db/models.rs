use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::Result;

// ── Account ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub email: String,
    pub display_name: Option<String>,
    pub access_token: String,
    pub refresh_token: String,
    pub token_expiry: String,
    pub change_token: Option<String>,
    pub created_at: String,
}

impl Account {
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            email: row.get("email")?,
            display_name: row.get("display_name")?,
            access_token: row.get("access_token")?,
            refresh_token: row.get("refresh_token")?,
            token_expiry: row.get("token_expiry")?,
            change_token: row.get("change_token")?,
            created_at: row.get("created_at")?,
        })
    }

    pub fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT INTO accounts (id, email, display_name, access_token, refresh_token, token_expiry)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                self.id,
                self.email,
                self.display_name,
                self.access_token,
                self.refresh_token,
                self.token_expiry,
            ],
        )?;
        Ok(())
    }

    pub fn update_tokens(conn: &Connection, id: &str, access: &str, expiry: &str) -> Result<()> {
        conn.execute(
            "UPDATE accounts SET access_token = ?1, token_expiry = ?2 WHERE id = ?3",
            params![access, expiry, id],
        )?;
        Ok(())
    }

    pub fn update_credentials(
        conn: &Connection,
        id: &str,
        access: &str,
        refresh: &str,
        expiry: &str,
        display_name: Option<&str>,
    ) -> Result<()> {
        conn.execute(
            "UPDATE accounts
             SET access_token = ?1, refresh_token = ?2, token_expiry = ?3, display_name = ?4
             WHERE id = ?5",
            params![access, refresh, expiry, display_name, id],
        )?;
        Ok(())
    }

    pub fn update_change_token(conn: &Connection, id: &str, token: &str) -> Result<()> {
        conn.execute(
            "UPDATE accounts SET change_token = ?1 WHERE id = ?2",
            params![token, id],
        )?;
        Ok(())
    }

    pub fn get_by_id(conn: &Connection, id: &str) -> Result<Option<Self>> {
        let result = conn
            .query_row("SELECT * FROM accounts WHERE id = ?1", params![id], Self::from_row)
            .optional()?;
        Ok(result)
    }

    pub fn list(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare("SELECT * FROM accounts ORDER BY email")?;
        let rows = stmt.query_map([], Self::from_row)?;
        let mut accounts = Vec::new();
        for row in rows {
            accounts.push(row?);
        }
        Ok(accounts)
    }

    pub fn delete(conn: &Connection, id: &str) -> Result<()> {
        conn.execute("DELETE FROM accounts WHERE id = ?1", params![id])?;
        Ok(())
    }
}

// ── SyncPair ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPair {
    pub id: String,
    pub name: String,
    pub account_id: String,
    pub local_root: String,
    pub remote_root_id: String,
    pub remote_root_path: String,
    pub mode: SyncMode,
    pub conflict_policy: ConflictPolicy,
    pub status: SyncPairStatus,
    pub poll_interval_secs: i64,
    pub created_at: String,
    pub updated_at: String,

    /// True when files for this pair are encrypted before upload.
    /// When set, `encryption_salt` and `encryption_verifier` MUST be Some.
    #[serde(default)]
    pub encryption_enabled: bool,
    /// Argon2id salt used to derive the encryption key from the user's
    /// passphrase. Skipped from JSON since the UI never needs it.
    #[serde(skip)]
    pub encryption_salt: Option<Vec<u8>>,
    /// AEAD blob used to verify a passphrase without trying a real file
    /// (see `crypto::FileCipher::make_verifier`). Skipped from JSON.
    #[serde(skip)]
    pub encryption_verifier: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum SyncMode {
    TwoWay,
    LocalToCloud,
    CloudToLocal,
}

impl std::fmt::Display for SyncMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TwoWay => write!(f, "two-way"),
            Self::LocalToCloud => write!(f, "local-to-cloud"),
            Self::CloudToLocal => write!(f, "cloud-to-local"),
        }
    }
}

impl std::str::FromStr for SyncMode {
    type Err = crate::Error;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "two-way" => Ok(Self::TwoWay),
            "local-to-cloud" => Ok(Self::LocalToCloud),
            "cloud-to-local" => Ok(Self::CloudToLocal),
            _ => Err(crate::Error::Other(format!("Invalid sync mode: {s}"))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ConflictPolicy {
    Ask,
    KeepBoth,
    PreferLocal,
    PreferRemote,
    NewestWins,
}

impl std::fmt::Display for ConflictPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ask => write!(f, "ask"),
            Self::KeepBoth => write!(f, "keep-both"),
            Self::PreferLocal => write!(f, "prefer-local"),
            Self::PreferRemote => write!(f, "prefer-remote"),
            Self::NewestWins => write!(f, "newest-wins"),
        }
    }
}

impl std::str::FromStr for ConflictPolicy {
    type Err = crate::Error;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "ask" => Ok(Self::Ask),
            "keep-both" => Ok(Self::KeepBoth),
            "prefer-local" => Ok(Self::PreferLocal),
            "prefer-remote" => Ok(Self::PreferRemote),
            "newest-wins" => Ok(Self::NewestWins),
            _ => Err(crate::Error::Other(format!("Invalid conflict policy: {s}"))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum SyncPairStatus {
    Active,
    Paused,
    Error,
}

impl std::fmt::Display for SyncPairStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Paused => write!(f, "paused"),
            Self::Error => write!(f, "error"),
        }
    }
}

impl std::str::FromStr for SyncPairStatus {
    type Err = crate::Error;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "active" => Ok(Self::Active),
            "paused" => Ok(Self::Paused),
            "error" => Ok(Self::Error),
            _ => Err(crate::Error::Other(format!("Invalid status: {s}"))),
        }
    }
}

impl SyncPair {
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let mode: String = row.get("mode")?;
        let conflict_policy: String = row.get("conflict_policy")?;
        let status: String = row.get("status")?;
        let encryption_enabled: i64 = row.get("encryption_enabled")?;
        Ok(Self {
            id: row.get("id")?,
            name: row.get("name")?,
            account_id: row.get("account_id")?,
            local_root: row.get("local_root")?,
            remote_root_id: row.get("remote_root_id")?,
            remote_root_path: row.get("remote_root_path")?,
            mode: mode.parse().unwrap_or(SyncMode::TwoWay),
            conflict_policy: conflict_policy.parse().unwrap_or(ConflictPolicy::KeepBoth),
            status: status.parse().unwrap_or(SyncPairStatus::Active),
            poll_interval_secs: row.get("poll_interval_secs")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
            encryption_enabled: encryption_enabled != 0,
            encryption_salt: row.get("encryption_salt")?,
            encryption_verifier: row.get("encryption_verifier")?,
        })
    }

    pub fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT INTO sync_pairs (id, name, account_id, local_root, remote_root_id, remote_root_path, mode, conflict_policy, status, poll_interval_secs, encryption_enabled, encryption_salt, encryption_verifier)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                self.id,
                self.name,
                self.account_id,
                self.local_root,
                self.remote_root_id,
                self.remote_root_path,
                self.mode.to_string(),
                self.conflict_policy.to_string(),
                self.status.to_string(),
                self.poll_interval_secs,
                self.encryption_enabled as i64,
                self.encryption_salt,
                self.encryption_verifier,
            ],
        )?;
        Ok(())
    }

    /// Persist encryption settings for an existing pair. Used when the
    /// user enables encryption on a previously-unencrypted pair, or when
    /// they re-key (which the caller is responsible for actually applying
    /// to remote files — this method only flips the database flags).
    pub fn update_encryption(
        conn: &Connection,
        id: &str,
        enabled: bool,
        salt: Option<&[u8]>,
        verifier: Option<&[u8]>,
    ) -> Result<()> {
        conn.execute(
            "UPDATE sync_pairs
             SET encryption_enabled = ?1,
                 encryption_salt = ?2,
                 encryption_verifier = ?3,
                 updated_at = datetime('now')
             WHERE id = ?4",
            params![enabled as i64, salt, verifier, id],
        )?;
        Ok(())
    }

    pub fn get_by_id(conn: &Connection, id: &str) -> Result<Option<Self>> {
        let result = conn
            .query_row("SELECT * FROM sync_pairs WHERE id = ?1", params![id], Self::from_row)
            .optional()?;
        Ok(result)
    }

    pub fn list(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare("SELECT * FROM sync_pairs ORDER BY name")?;
        let rows = stmt.query_map([], Self::from_row)?;
        let mut pairs = Vec::new();
        for row in rows {
            pairs.push(row?);
        }
        Ok(pairs)
    }

    pub fn update_status(conn: &Connection, id: &str, status: &SyncPairStatus) -> Result<()> {
        conn.execute(
            "UPDATE sync_pairs SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![status.to_string(), id],
        )?;
        Ok(())
    }

    /// Update editable settings of a sync pair.
    pub fn update_settings(
        conn: &Connection,
        id: &str,
        name: &str,
        mode: &SyncMode,
        conflict_policy: &ConflictPolicy,
        poll_interval_secs: i64,
    ) -> Result<()> {
        conn.execute(
            "UPDATE sync_pairs SET name = ?1, mode = ?2, conflict_policy = ?3, poll_interval_secs = ?4, updated_at = datetime('now') WHERE id = ?5",
            params![
                name,
                mode.to_string(),
                conflict_policy.to_string(),
                poll_interval_secs,
                id,
            ],
        )?;
        Ok(())
    }

    pub fn delete(conn: &Connection, id: &str) -> Result<()> {
        conn.execute("DELETE FROM sync_pairs WHERE id = ?1", params![id])?;
        Ok(())
    }
}

// ── FileEntry ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub id: Option<i64>,
    pub sync_pair_id: String,
    pub relative_path: String,
    pub local_hash: Option<String>,
    pub remote_md5: Option<String>,
    pub remote_id: Option<String>,
    pub remote_rev: Option<String>,
    pub size: Option<i64>,
    pub local_mtime: Option<String>,
    pub remote_mtime: Option<String>,
    pub is_directory: bool,
    pub is_google_doc: bool,
    pub state: FileState,
    pub last_synced_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum FileState {
    Synced,
    LocalModified,
    RemoteModified,
    Conflict,
    Error,
    NewLocal,
    NewRemote,
}

impl std::fmt::Display for FileState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Synced => write!(f, "synced"),
            Self::LocalModified => write!(f, "local-modified"),
            Self::RemoteModified => write!(f, "remote-modified"),
            Self::Conflict => write!(f, "conflict"),
            Self::Error => write!(f, "error"),
            Self::NewLocal => write!(f, "new-local"),
            Self::NewRemote => write!(f, "new-remote"),
        }
    }
}

impl std::str::FromStr for FileState {
    type Err = crate::Error;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "synced" => Ok(Self::Synced),
            "local-modified" => Ok(Self::LocalModified),
            "remote-modified" => Ok(Self::RemoteModified),
            "conflict" => Ok(Self::Conflict),
            "error" => Ok(Self::Error),
            "new-local" => Ok(Self::NewLocal),
            "new-remote" => Ok(Self::NewRemote),
            _ => Err(crate::Error::Other(format!("Invalid file state: {s}"))),
        }
    }
}

impl FileEntry {
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let state: String = row.get("state")?;
        Ok(Self {
            id: Some(row.get("id")?),
            sync_pair_id: row.get("sync_pair_id")?,
            relative_path: row.get("relative_path")?,
            local_hash: row.get("local_hash")?,
            remote_md5: row.get("remote_md5")?,
            remote_id: row.get("remote_id")?,
            remote_rev: row.get("remote_rev")?,
            size: row.get("size")?,
            local_mtime: row.get("local_mtime")?,
            remote_mtime: row.get("remote_mtime")?,
            is_directory: row.get("is_directory")?,
            is_google_doc: row.get("is_google_doc")?,
            state: state.parse().unwrap_or(FileState::Synced),
            last_synced_at: row.get("last_synced_at")?,
        })
    }

    pub fn upsert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT INTO file_index (sync_pair_id, relative_path, local_hash, remote_md5, remote_id, remote_rev, size, local_mtime, remote_mtime, is_directory, is_google_doc, state, last_synced_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(sync_pair_id, relative_path) DO UPDATE SET
                local_hash = excluded.local_hash,
                remote_md5 = excluded.remote_md5,
                remote_id = excluded.remote_id,
                remote_rev = excluded.remote_rev,
                size = excluded.size,
                local_mtime = excluded.local_mtime,
                remote_mtime = excluded.remote_mtime,
                is_directory = excluded.is_directory,
                is_google_doc = excluded.is_google_doc,
                state = excluded.state,
                last_synced_at = excluded.last_synced_at",
            params![
                self.sync_pair_id,
                self.relative_path,
                self.local_hash,
                self.remote_md5,
                self.remote_id,
                self.remote_rev,
                self.size,
                self.local_mtime,
                self.remote_mtime,
                self.is_directory,
                self.is_google_doc,
                self.state.to_string(),
                self.last_synced_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_by_path(conn: &Connection, sync_pair_id: &str, path: &str) -> Result<Option<Self>> {
        let result = conn
            .query_row(
                "SELECT * FROM file_index WHERE sync_pair_id = ?1 AND relative_path = ?2",
                params![sync_pair_id, path],
                Self::from_row,
            )
            .optional()?;
        Ok(result)
    }

    pub fn list_by_sync_pair(conn: &Connection, sync_pair_id: &str) -> Result<Vec<Self>> {
        let mut stmt = conn
            .prepare("SELECT * FROM file_index WHERE sync_pair_id = ?1 ORDER BY relative_path")?;
        let rows = stmt.query_map(params![sync_pair_id], Self::from_row)?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    pub fn list_by_state(conn: &Connection, sync_pair_id: &str, state: &FileState) -> Result<Vec<Self>> {
        let mut stmt = conn
            .prepare("SELECT * FROM file_index WHERE sync_pair_id = ?1 AND state = ?2 ORDER BY relative_path")?;
        let rows = stmt.query_map(params![sync_pair_id, state.to_string()], Self::from_row)?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    pub fn delete_by_path(conn: &Connection, sync_pair_id: &str, path: &str) -> Result<()> {
        conn.execute(
            "DELETE FROM file_index WHERE sync_pair_id = ?1 AND relative_path = ?2",
            params![sync_pair_id, path],
        )?;
        Ok(())
    }
}

// ── ChangeLogEntry ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeLogEntry {
    pub id: Option<i64>,
    pub sync_pair_id: String,
    pub relative_path: String,
    pub action: String,
    pub detail: Option<String>,
    pub created_at: String,
    /// Bytes moved, for `upload`/`download` rows. `None` for everything
    /// else, and for transfers logged before schema v3.
    pub bytes: Option<i64>,
    /// Wall-clock duration of that transfer. Pairs with `bytes` to give a
    /// per-file speed; `None` under the same conditions.
    pub duration_ms: Option<i64>,
}

impl ChangeLogEntry {
    pub fn insert(conn: &Connection, sync_pair_id: &str, path: &str, action: &str, detail: Option<&str>) -> Result<()> {
        Self::insert_transfer(conn, sync_pair_id, path, action, detail, None, None)
    }

    /// Log an action along with what it moved and how long it took.
    pub fn insert_transfer(
        conn: &Connection,
        sync_pair_id: &str,
        path: &str,
        action: &str,
        detail: Option<&str>,
        bytes: Option<i64>,
        duration_ms: Option<i64>,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO change_log (sync_pair_id, relative_path, action, detail, bytes, duration_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![sync_pair_id, path, action, detail, bytes, duration_ms],
        )?;
        Ok(())
    }

    pub fn recent(conn: &Connection, sync_pair_id: &str, limit: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT * FROM change_log WHERE sync_pair_id = ?1 ORDER BY created_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![sync_pair_id, limit], |row| Self::from_row(row))?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get("id")?),
            sync_pair_id: row.get("sync_pair_id")?,
            relative_path: row.get("relative_path")?,
            action: row.get("action")?,
            detail: row.get("detail")?,
            created_at: row.get("created_at")?,
            bytes: row.get("bytes")?,
            duration_ms: row.get("duration_ms")?,
        })
    }
}

// ── TransferStats ────────────────────────────────────────────────────

/// One direction's totals over some window of the change log.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DirectionTotals {
    /// Transfers logged, including ones from before byte accounting existed.
    pub files: i64,
    /// Bytes moved across the transfers that were measured.
    pub bytes: i64,
    /// Milliseconds spent on those same measured transfers. Dividing
    /// `bytes` by this gives an average throughput that ignores the time
    /// between transfers, which is the number a user reads as "my speed".
    pub duration_ms: i64,
    /// How many of `files` carry a byte measurement. When this is less than
    /// `files`, the byte totals understate reality and the UI says so
    /// rather than pretending the average is complete.
    pub measured_files: i64,
}

impl DirectionTotals {
    /// Average bytes/second across measured transfers, or `None` when
    /// nothing measurable has happened yet.
    pub fn average_bps(&self) -> Option<f64> {
        if self.duration_ms <= 0 || self.bytes <= 0 {
            return None;
        }
        Some((self.bytes as f64) * 1000.0 / (self.duration_ms as f64))
    }
}

/// Aggregate transfer activity, for the statistics page.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TransferStats {
    pub uploaded: DirectionTotals,
    pub downloaded: DirectionTotals,
    pub deleted: i64,
    pub conflicts: i64,
    pub errors: i64,
    /// Timestamp of the oldest change-log row in the window — i.e. how far
    /// back these numbers actually reach.
    pub since: Option<String>,
    pub last_activity: Option<String>,
}

impl TransferStats {
    /// Totals for one sync pair, or across all pairs when `sync_pair_id` is
    /// `None`. `since` restricts to rows at or after an SQLite datetime
    /// expression (e.g. `datetime('now','-7 days')`).
    pub fn compute(
        conn: &Connection,
        sync_pair_id: Option<&str>,
        since: Option<&str>,
    ) -> Result<Self> {
        // created_at is written by SQLite's datetime('now') (UTC, no zone
        // suffix), so string comparison against another datetime() value is
        // a correct ordering — both are the same fixed-width format.
        let mut sql = String::from(
            "SELECT action, COUNT(*) AS n, \
                    COALESCE(SUM(bytes), 0) AS total_bytes, \
                    COALESCE(SUM(duration_ms), 0) AS total_ms, \
                    SUM(CASE WHEN bytes IS NOT NULL THEN 1 ELSE 0 END) AS measured \
             FROM change_log WHERE 1 = 1",
        );
        if sync_pair_id.is_some() {
            sql.push_str(" AND sync_pair_id = :pair");
        }
        if since.is_some() {
            sql.push_str(" AND created_at >= :since");
        }
        sql.push_str(" GROUP BY action");

        let mut stats = Self::default();
        {
            let mut stmt = conn.prepare(&sql)?;
            let mut params: Vec<(&str, &dyn rusqlite::ToSql)> = Vec::new();
            if let Some(id) = sync_pair_id.as_ref() {
                params.push((":pair", id));
            }
            if let Some(s) = since.as_ref() {
                params.push((":since", s));
            }
            let rows = stmt.query_map(params.as_slice(), |row| {
                Ok((
                    row.get::<_, String>("action")?,
                    row.get::<_, i64>("n")?,
                    row.get::<_, i64>("total_bytes")?,
                    row.get::<_, i64>("total_ms")?,
                    row.get::<_, i64>("measured")?,
                ))
            })?;

            for row in rows {
                let (action, n, bytes, ms, measured) = row?;
                let totals = DirectionTotals {
                    files: n,
                    bytes,
                    duration_ms: ms,
                    measured_files: measured,
                };
                match action.as_str() {
                    "upload" => stats.uploaded = totals,
                    "download" => stats.downloaded = totals,
                    "delete-local" | "delete-remote" => stats.deleted += n,
                    "conflict" => stats.conflicts += n,
                    "error" => stats.errors += n,
                    _ => {}
                }
            }
        }

        let mut range_sql =
            String::from("SELECT MIN(created_at), MAX(created_at) FROM change_log WHERE 1 = 1");
        if sync_pair_id.is_some() {
            range_sql.push_str(" AND sync_pair_id = :pair");
        }
        if since.is_some() {
            range_sql.push_str(" AND created_at >= :since");
        }
        let mut stmt = conn.prepare(&range_sql)?;
        let mut params: Vec<(&str, &dyn rusqlite::ToSql)> = Vec::new();
        if let Some(id) = sync_pair_id.as_ref() {
            params.push((":pair", id));
        }
        if let Some(s) = since.as_ref() {
            params.push((":since", s));
        }
        let (min, max) = stmt.query_row(params.as_slice(), |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        })?;
        stats.since = min;
        stats.last_activity = max;

        Ok(stats)
    }
}

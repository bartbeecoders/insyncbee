use rusqlite::Connection;

use crate::Result;

/// Run all database migrations.
pub fn run_all(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY
        );",
    )?;

    let current: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let migrations: &[(&str, i64)] = &[(MIGRATION_001, 1)];

    for (sql, version) in migrations {
        if *version > current {
            conn.execute_batch(sql)?;
            conn.execute("INSERT INTO schema_version (version) VALUES (?1)", [version])?;
            tracing::info!("Applied database migration v{version}");
        }
    }

    Ok(())
}

const MIGRATION_001: &str = "
-- Google accounts
CREATE TABLE IF NOT EXISTS accounts (
    id              TEXT PRIMARY KEY,
    email           TEXT NOT NULL UNIQUE,
    display_name    TEXT,
    access_token    TEXT NOT NULL,
    refresh_token   TEXT NOT NULL,
    token_expiry    TEXT NOT NULL,
    change_token    TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Sync pair configuration
CREATE TABLE IF NOT EXISTS sync_pairs (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    account_id      TEXT NOT NULL REFERENCES accounts(id),
    local_root      TEXT NOT NULL,
    remote_root_id  TEXT NOT NULL,
    remote_root_path TEXT NOT NULL,
    mode            TEXT NOT NULL DEFAULT 'two-way'
                    CHECK (mode IN ('two-way', 'local-to-cloud', 'cloud-to-local')),
    conflict_policy TEXT NOT NULL DEFAULT 'keep-both'
                    CHECK (conflict_policy IN ('ask', 'keep-both', 'prefer-local', 'prefer-remote', 'newest-wins')),
    status          TEXT NOT NULL DEFAULT 'active'
                    CHECK (status IN ('active', 'paused', 'error')),
    poll_interval_secs INTEGER NOT NULL DEFAULT 30,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

-- File index: the 'base state' for three-way comparison
CREATE TABLE IF NOT EXISTS file_index (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    sync_pair_id    TEXT NOT NULL REFERENCES sync_pairs(id) ON DELETE CASCADE,
    relative_path   TEXT NOT NULL,
    local_hash      TEXT,
    remote_md5      TEXT,
    remote_id       TEXT,
    remote_rev      TEXT,
    size            INTEGER,
    local_mtime     TEXT,
    remote_mtime    TEXT,
    is_directory    INTEGER NOT NULL DEFAULT 0,
    is_google_doc   INTEGER NOT NULL DEFAULT 0,
    state           TEXT NOT NULL DEFAULT 'synced'
                    CHECK (state IN ('synced', 'local-modified', 'remote-modified', 'conflict', 'error', 'new-local', 'new-remote')),
    last_synced_at  TEXT,
    UNIQUE(sync_pair_id, relative_path)
);

CREATE INDEX IF NOT EXISTS idx_file_index_pair ON file_index(sync_pair_id);
CREATE INDEX IF NOT EXISTS idx_file_index_state ON file_index(state);
CREATE INDEX IF NOT EXISTS idx_file_index_remote_id ON file_index(remote_id);

-- Change log for activity feed
CREATE TABLE IF NOT EXISTS change_log (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    sync_pair_id    TEXT NOT NULL REFERENCES sync_pairs(id) ON DELETE CASCADE,
    relative_path   TEXT NOT NULL,
    action          TEXT NOT NULL
                    CHECK (action IN ('upload', 'download', 'delete-local', 'delete-remote',
                                      'rename', 'conflict', 'error', 'resolve')),
    detail          TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_change_log_pair ON change_log(sync_pair_id);
CREATE INDEX IF NOT EXISTS idx_change_log_time ON change_log(created_at);
";

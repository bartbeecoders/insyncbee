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

    let migrations: &[(&str, i64)] = &[
        (MIGRATION_001, 1),
        (MIGRATION_002, 2),
        (MIGRATION_003, 3),
        (MIGRATION_004, 4),
    ];

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

// ── v2: per-sync-pair encryption ────────────────────────────────────
// Adds three columns to `sync_pairs`. When `encryption_enabled = 1`:
//   - `encryption_salt` is the 16-byte Argon2id salt for the user's
//     passphrase (also used to re-derive the key on a different machine).
//   - `encryption_verifier` is a small AEAD blob we can decrypt with the
//     derived key to confirm the user typed the right passphrase before
//     starting a sync (see crypto::FileCipher::make_verifier).
// The actual 32-byte derived key is **not** stored here — it lives in
// the OS keyring (see crate::keystore).
const MIGRATION_002: &str = "
ALTER TABLE sync_pairs ADD COLUMN encryption_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sync_pairs ADD COLUMN encryption_salt BLOB;
ALTER TABLE sync_pairs ADD COLUMN encryption_verifier BLOB;
";

// ── v3: transfer accounting ─────────────────────────────────────────
// `bytes` and `duration_ms` on each upload/download row are what the
// statistics page sums and what the activity feed divides to show a
// per-transfer speed. Both are nullable on purpose: rows written before
// this migration have no measurement, and inventing one (0, or a
// back-filled file size with no duration) would quietly corrupt every
// average computed over them. NULL means "not measured", and the
// aggregates say so.
const MIGRATION_003: &str = "
ALTER TABLE change_log ADD COLUMN bytes INTEGER;
ALTER TABLE change_log ADD COLUMN duration_ms INTEGER;
";

// ── v4: folder actions in the activity feed ─────────────────────────
// Folder creates were the one thing a sync did that left no trace: the
// activity feed showed the files that landed but never the directories
// made to hold them, so a sync that only created folders looked like it
// had done nothing at all.
//
// `action` is constrained by a CHECK, and SQLite cannot alter one in
// place — this is the documented table-rebuild dance. foreign_keys is
// switched off around it because `change_log` carries an ON DELETE
// CASCADE reference to `sync_pairs`, and the DROP would otherwise be
// evaluated against the constraint mid-rebuild.
const MIGRATION_004: &str = "
PRAGMA foreign_keys=OFF;

CREATE TABLE change_log_new (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    sync_pair_id    TEXT NOT NULL REFERENCES sync_pairs(id) ON DELETE CASCADE,
    relative_path   TEXT NOT NULL,
    action          TEXT NOT NULL
                    CHECK (action IN ('upload', 'download', 'delete-local', 'delete-remote',
                                      'create-local-dir', 'create-remote-dir',
                                      'rename', 'conflict', 'error', 'resolve')),
    detail          TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    bytes           INTEGER,
    duration_ms     INTEGER
);

INSERT INTO change_log_new (id, sync_pair_id, relative_path, action, detail, created_at, bytes, duration_ms)
    SELECT id, sync_pair_id, relative_path, action, detail, created_at, bytes, duration_ms
    FROM change_log;

DROP TABLE change_log;
ALTER TABLE change_log_new RENAME TO change_log;

CREATE INDEX IF NOT EXISTS idx_change_log_pair ON change_log(sync_pair_id);
CREATE INDEX IF NOT EXISTS idx_change_log_time ON change_log(created_at);

PRAGMA foreign_keys=ON;
";

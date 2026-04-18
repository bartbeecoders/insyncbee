//! Integration tests for `db::models`.
//!
//! Cover migrations, CRUD round-trips, foreign-key cascade deletes, and the
//! `UNIQUE(sync_pair_id, relative_path)` constraint on the file index.

mod common;

use common::{test_db, test_db_with_account};
use insyncbee_core::db::models::{
    Account, ChangeLogEntry, ConflictPolicy, FileEntry, FileState, SyncMode, SyncPair,
    SyncPairStatus,
};

#[test]
fn migrations_run_on_open() {
    // open_in_memory() runs migrations; if it succeeds we have a `schema_version` row.
    let db = test_db();
    let v: i64 = db
        .with_conn(|c| {
            Ok(c.query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(v, 1, "expected schema v1 after open_in_memory");
}

#[test]
fn account_crud_roundtrip() {
    let (db, account) = test_db_with_account();
    let fetched = db
        .with_conn(|c| Account::get_by_id(c, &account.id))
        .unwrap()
        .unwrap();
    assert_eq!(fetched.email, account.email);

    db.with_conn(|c| Account::update_tokens(c, &account.id, "new-access", "2030-01-01T00:00:00Z"))
        .unwrap();
    let updated = db
        .with_conn(|c| Account::get_by_id(c, &account.id))
        .unwrap()
        .unwrap();
    assert_eq!(updated.access_token, "new-access");
    assert_eq!(updated.token_expiry, "2030-01-01T00:00:00Z");

    db.with_conn(|c| Account::delete(c, &account.id)).unwrap();
    let gone = db
        .with_conn(|c| Account::get_by_id(c, &account.id))
        .unwrap();
    assert!(gone.is_none());
}

#[test]
fn sync_pair_insert_and_list() {
    let (db, account) = test_db_with_account();
    let pair = SyncPair {
        id: "pair-1".into(),
        name: "Docs".into(),
        account_id: account.id.clone(),
        local_root: "/tmp/docs".into(),
        remote_root_id: "remote-root".into(),
        remote_root_path: "/Docs".into(),
        mode: SyncMode::TwoWay,
        conflict_policy: ConflictPolicy::KeepBoth,
        status: SyncPairStatus::Active,
        poll_interval_secs: 60,
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    };
    db.with_conn(|c| pair.insert(c)).unwrap();

    let fetched = db.with_conn(|c| SyncPair::get_by_id(c, "pair-1")).unwrap().unwrap();
    assert_eq!(fetched.name, "Docs");
    assert_eq!(fetched.mode, SyncMode::TwoWay);

    let listed = db.with_conn(|c| SyncPair::list(c)).unwrap();
    assert_eq!(listed.len(), 1);
}

#[test]
fn sync_pair_status_and_settings_update() {
    let (db, account) = test_db_with_account();
    let pair = sample_pair(&account.id, "pair-2");
    db.with_conn(|c| pair.insert(c)).unwrap();

    db.with_conn(|c| SyncPair::update_status(c, "pair-2", &SyncPairStatus::Paused))
        .unwrap();
    let after = db.with_conn(|c| SyncPair::get_by_id(c, "pair-2")).unwrap().unwrap();
    assert_eq!(after.status, SyncPairStatus::Paused);

    db.with_conn(|c| {
        SyncPair::update_settings(
            c,
            "pair-2",
            "renamed",
            &SyncMode::CloudToLocal,
            &ConflictPolicy::PreferRemote,
            120,
        )
    })
    .unwrap();
    let after = db.with_conn(|c| SyncPair::get_by_id(c, "pair-2")).unwrap().unwrap();
    assert_eq!(after.name, "renamed");
    assert_eq!(after.mode, SyncMode::CloudToLocal);
    assert_eq!(after.conflict_policy, ConflictPolicy::PreferRemote);
    assert_eq!(after.poll_interval_secs, 120);
}

#[test]
fn file_entry_upsert_overwrites_existing_row() {
    let (db, account) = test_db_with_account();
    let pair = sample_pair(&account.id, "pair-3");
    db.with_conn(|c| pair.insert(c)).unwrap();

    let entry = FileEntry {
        id: None,
        sync_pair_id: pair.id.clone(),
        relative_path: "a.txt".into(),
        local_hash: Some("hash-1".into()),
        remote_md5: None,
        remote_id: None,
        remote_rev: None,
        size: Some(10),
        local_mtime: None,
        remote_mtime: None,
        is_directory: false,
        is_google_doc: false,
        state: FileState::NewLocal,
        last_synced_at: None,
    };
    db.with_conn(|c| entry.upsert(c)).unwrap();

    // Upsert with a new hash + Synced state should overwrite, not duplicate.
    let mut updated = entry.clone();
    updated.local_hash = Some("hash-2".into());
    updated.state = FileState::Synced;
    db.with_conn(|c| updated.upsert(c)).unwrap();

    let listed = db.with_conn(|c| FileEntry::list_by_sync_pair(c, &pair.id)).unwrap();
    assert_eq!(listed.len(), 1, "upsert must not duplicate (sync_pair_id, relative_path)");
    assert_eq!(listed[0].local_hash.as_deref(), Some("hash-2"));
    assert_eq!(listed[0].state, FileState::Synced);
}

#[test]
fn file_entry_delete_by_path() {
    let (db, account) = test_db_with_account();
    let pair = sample_pair(&account.id, "pair-4");
    db.with_conn(|c| pair.insert(c)).unwrap();

    for name in ["a.txt", "b.txt", "c.txt"] {
        let e = FileEntry {
            id: None,
            sync_pair_id: pair.id.clone(),
            relative_path: name.into(),
            local_hash: None,
            remote_md5: None,
            remote_id: None,
            remote_rev: None,
            size: None,
            local_mtime: None,
            remote_mtime: None,
            is_directory: false,
            is_google_doc: false,
            state: FileState::Synced,
            last_synced_at: None,
        };
        db.with_conn(|c| e.upsert(c)).unwrap();
    }
    db.with_conn(|c| FileEntry::delete_by_path(c, &pair.id, "b.txt")).unwrap();
    let listed = db.with_conn(|c| FileEntry::list_by_sync_pair(c, &pair.id)).unwrap();
    let names: Vec<_> = listed.iter().map(|e| e.relative_path.clone()).collect();
    assert_eq!(names, vec!["a.txt".to_string(), "c.txt".to_string()]);
}

#[test]
fn file_entry_list_by_state_filter() {
    let (db, account) = test_db_with_account();
    let pair = sample_pair(&account.id, "pair-5");
    db.with_conn(|c| pair.insert(c)).unwrap();

    let states = [
        ("a.txt", FileState::Synced),
        ("b.txt", FileState::Conflict),
        ("c.txt", FileState::Conflict),
        ("d.txt", FileState::NewLocal),
    ];
    for (name, state) in &states {
        let e = FileEntry {
            id: None,
            sync_pair_id: pair.id.clone(),
            relative_path: (*name).into(),
            local_hash: None,
            remote_md5: None,
            remote_id: None,
            remote_rev: None,
            size: None,
            local_mtime: None,
            remote_mtime: None,
            is_directory: false,
            is_google_doc: false,
            state: state.clone(),
            last_synced_at: None,
        };
        db.with_conn(|c| e.upsert(c)).unwrap();
    }

    let conflicts = db
        .with_conn(|c| FileEntry::list_by_state(c, &pair.id, &FileState::Conflict))
        .unwrap();
    assert_eq!(conflicts.len(), 2);
}

#[test]
fn cascade_delete_removes_file_entries_and_change_log() {
    let (db, account) = test_db_with_account();
    let pair = sample_pair(&account.id, "pair-6");
    db.with_conn(|c| pair.insert(c)).unwrap();

    let entry = FileEntry {
        id: None,
        sync_pair_id: pair.id.clone(),
        relative_path: "a.txt".into(),
        local_hash: None,
        remote_md5: None,
        remote_id: None,
        remote_rev: None,
        size: None,
        local_mtime: None,
        remote_mtime: None,
        is_directory: false,
        is_google_doc: false,
        state: FileState::Synced,
        last_synced_at: None,
    };
    db.with_conn(|c| entry.upsert(c)).unwrap();
    db.with_conn(|c| ChangeLogEntry::insert(c, &pair.id, "a.txt", "upload", None))
        .unwrap();

    db.with_conn(|c| SyncPair::delete(c, &pair.id)).unwrap();

    let entries = db.with_conn(|c| FileEntry::list_by_sync_pair(c, &pair.id)).unwrap();
    assert!(entries.is_empty(), "ON DELETE CASCADE should drop file_index rows");
    let log = db.with_conn(|c| ChangeLogEntry::recent(c, &pair.id, 100)).unwrap();
    assert!(log.is_empty(), "ON DELETE CASCADE should drop change_log rows");
}

#[test]
fn enum_string_roundtrips() {
    use std::str::FromStr;
    for m in [SyncMode::TwoWay, SyncMode::LocalToCloud, SyncMode::CloudToLocal] {
        assert_eq!(SyncMode::from_str(&m.to_string()).unwrap(), m);
    }
    for p in [
        ConflictPolicy::Ask,
        ConflictPolicy::KeepBoth,
        ConflictPolicy::PreferLocal,
        ConflictPolicy::PreferRemote,
        ConflictPolicy::NewestWins,
    ] {
        assert_eq!(ConflictPolicy::from_str(&p.to_string()).unwrap(), p);
    }
    for s in [SyncPairStatus::Active, SyncPairStatus::Paused, SyncPairStatus::Error] {
        assert_eq!(SyncPairStatus::from_str(&s.to_string()).unwrap(), s);
    }
    for fs in [
        FileState::Synced,
        FileState::LocalModified,
        FileState::RemoteModified,
        FileState::Conflict,
        FileState::Error,
        FileState::NewLocal,
        FileState::NewRemote,
    ] {
        assert_eq!(FileState::from_str(&fs.to_string()).unwrap(), fs);
    }
}

fn sample_pair(account_id: &str, id: &str) -> SyncPair {
    SyncPair {
        id: id.into(),
        name: id.into(),
        account_id: account_id.into(),
        local_root: "/tmp/x".into(),
        remote_root_id: "remote".into(),
        remote_root_path: "/".into(),
        mode: SyncMode::TwoWay,
        conflict_policy: ConflictPolicy::KeepBoth,
        status: SyncPairStatus::Active,
        poll_interval_secs: 30,
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    }
}

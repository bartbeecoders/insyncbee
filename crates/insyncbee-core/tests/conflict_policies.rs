//! One test per `ConflictPolicy` variant. Each arranges a real (true,true,true)
//! conflict, drives the engine, and asserts on the resulting state.

mod common;

use common::SyncFixture;
use insyncbee_core::db::models::{ConflictPolicy, FileEntry, FileState, SyncMode};
use insyncbee_core::drive::DriveClient;
use insyncbee_core::sync_engine::SyncEngine;

/// Build a fixture, sync once so both sides agree on `path`, then mutate
/// both sides so a conflict is produced on the next sync.
async fn arrange_conflict(policy: ConflictPolicy) -> SyncFixture {
    let mut fx = SyncFixture::new(SyncMode::TwoWay);
    fx.pair.conflict_policy = policy;
    fx.db
        .with_conn(|c| {
            insyncbee_core::db::models::SyncPair::update_settings(
                c,
                &fx.pair.id,
                &fx.pair.name,
                &fx.pair.mode,
                &fx.pair.conflict_policy,
                fx.pair.poll_interval_secs,
            )
        })
        .unwrap();

    fx.write_local("notes.md", "shared baseline");
    let engine = SyncEngine::new(fx.db.clone(), fx.pair.clone());
    engine.sync(&fx.fake).await.unwrap();

    // Diverge.
    fx.write_local("notes.md", "LOCAL EDIT");
    // Find the remote file and mutate its bytes via update.
    let remote_id = fx
        .fake
        .snapshot_by_name()
        .get("notes.md")
        .expect("baseline upload should be on the fake")
        .meta
        .id
        .clone();
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), b"REMOTE EDIT").unwrap();
    fx.fake.update_file(&remote_id, tmp.path()).await.unwrap();

    fx
}

#[tokio::test]
async fn keep_both_creates_conflicted_copy() {
    let fx = arrange_conflict(ConflictPolicy::KeepBoth).await;
    let engine = SyncEngine::new(fx.db.clone(), fx.pair.clone());
    let report = engine.sync(&fx.fake).await.unwrap();
    assert_eq!(report.conflicts, 1);

    // Local "notes.md" should still hold the local edit; a sibling
    // "notes (conflict ...).md" should have been written with REMOTE bytes.
    let local = std::fs::read(fx.local_path().join("notes.md")).unwrap();
    assert_eq!(local, b"LOCAL EDIT");
    let conflicted = std::fs::read_dir(fx.local_path())
        .unwrap()
        .filter_map(Result::ok)
        .find(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n.starts_with("notes (conflict") && n.ends_with(".md")
        })
        .expect("expected a conflicted-copy sibling");
    let cbytes = std::fs::read(conflicted.path()).unwrap();
    assert_eq!(cbytes, b"REMOTE EDIT");
}

#[tokio::test]
async fn prefer_local_overwrites_remote() {
    let fx = arrange_conflict(ConflictPolicy::PreferLocal).await;
    let engine = SyncEngine::new(fx.db.clone(), fx.pair.clone());
    engine.sync(&fx.fake).await.unwrap();

    // Local file is unchanged; remote should now hold the local bytes.
    assert_eq!(
        std::fs::read(fx.local_path().join("notes.md")).unwrap(),
        b"LOCAL EDIT"
    );
    let remote = fx
        .fake
        .snapshot_by_name()
        .get("notes.md")
        .expect("remote should still hold notes.md")
        .clone();
    assert_eq!(remote.bytes, b"LOCAL EDIT");
}

#[tokio::test]
async fn prefer_remote_overwrites_local() {
    let fx = arrange_conflict(ConflictPolicy::PreferRemote).await;
    let engine = SyncEngine::new(fx.db.clone(), fx.pair.clone());
    engine.sync(&fx.fake).await.unwrap();

    let local = std::fs::read(fx.local_path().join("notes.md")).unwrap();
    assert_eq!(local, b"REMOTE EDIT", "local should now mirror remote bytes");
}

#[tokio::test]
async fn ask_marks_entry_as_conflict_in_db() {
    let fx = arrange_conflict(ConflictPolicy::Ask).await;
    let engine = SyncEngine::new(fx.db.clone(), fx.pair.clone());
    engine.sync(&fx.fake).await.unwrap();

    let entry = fx
        .db
        .with_conn(|c| FileEntry::get_by_path(c, &fx.pair.id, "notes.md"))
        .unwrap()
        .expect("file should still be in the index after Ask");
    assert_eq!(entry.state, FileState::Conflict);

    // No silent overwrite on either side.
    assert_eq!(
        std::fs::read(fx.local_path().join("notes.md")).unwrap(),
        b"LOCAL EDIT"
    );
    let remote = fx
        .fake
        .snapshot_by_name()
        .get("notes.md")
        .unwrap()
        .clone();
    assert_eq!(remote.bytes, b"REMOTE EDIT");
}

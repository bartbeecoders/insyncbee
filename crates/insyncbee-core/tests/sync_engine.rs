//! Integration tests for `SyncEngine` against a `FakeDriveClient`.
//!
//! Walks every meaningful arm of the three-way `(local, remote, base)`
//! comparison plus the end-to-end report of a real sync cycle.

mod common;

use common::{FakeDriveClient, SyncFixture};
use insyncbee_core::db::models::{FileEntry, FileState, SyncMode};
use insyncbee_core::drive::DriveClient;
use insyncbee_core::sync_engine::{SyncAction, SyncEngine};
use std::collections::HashMap;
use std::path::Path;

// ── End-to-end sync cycles ──────────────────────────────────────────

#[tokio::test]
async fn upload_new_local_file() {
    let fx = SyncFixture::new(SyncMode::TwoWay);
    fx.write_local("hello.txt", "hi from local");

    let engine = SyncEngine::new(fx.db.clone(), fx.pair.clone());
    let report = engine.sync(&fx.fake).await.unwrap();

    assert_eq!(report.uploaded, 1, "expected the new file to be uploaded");
    assert_eq!(report.downloaded, 0);
    assert_eq!(report.errors, 0);

    // The fake should now hold one file under the remote root.
    let remote_files: Vec<_> = fx
        .fake
        .snapshot_by_name()
        .into_iter()
        .filter(|(_, f)| {
            f.meta
                .parents
                .as_deref()
                .map(|ps| ps.contains(&fx.remote_root))
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(remote_files.len(), 1);
    assert_eq!(remote_files[0].0, "hello.txt");

    // Index should now have a Synced entry for the file.
    let entries = fx
        .db
        .with_conn(|c| FileEntry::list_by_sync_pair(c, &fx.pair.id))
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].relative_path, "hello.txt");
    assert_eq!(entries[0].state, FileState::Synced);
}

#[tokio::test]
async fn download_new_remote_file() {
    let fx = SyncFixture::new(SyncMode::TwoWay);
    fx.fake
        .insert_file("from-cloud.txt", &fx.remote_root, b"cloud bytes".to_vec());

    let engine = SyncEngine::new(fx.db.clone(), fx.pair.clone());
    let report = engine.sync(&fx.fake).await.unwrap();

    assert_eq!(report.downloaded, 1);
    assert_eq!(report.uploaded, 0);

    let local_path = fx.local_path().join("from-cloud.txt");
    assert!(local_path.exists(), "remote file should have been downloaded locally");
    let read = std::fs::read(&local_path).unwrap();
    assert_eq!(read, b"cloud bytes");
}

#[tokio::test]
async fn no_changes_yields_empty_report() {
    let fx = SyncFixture::new(SyncMode::TwoWay);
    fx.write_local("a.txt", "stable");
    let engine = SyncEngine::new(fx.db.clone(), fx.pair.clone());
    // First sync uploads.
    engine.sync(&fx.fake).await.unwrap();
    // Second sync should be a no-op.
    let report = engine.sync(&fx.fake).await.unwrap();
    assert_eq!(report.uploaded, 0);
    assert_eq!(report.downloaded, 0);
    assert_eq!(report.deleted, 0);
    assert_eq!(report.conflicts, 0);
    assert!(report.skipped >= 1, "the unchanged file should be reported as skipped");
}

#[tokio::test]
async fn local_modification_uploads_update() {
    let fx = SyncFixture::new(SyncMode::TwoWay);
    fx.write_local("notes.md", "v1");
    let engine = SyncEngine::new(fx.db.clone(), fx.pair.clone());
    engine.sync(&fx.fake).await.unwrap();

    // Modify locally and re-sync.
    fx.write_local("notes.md", "v2 updated content");
    let report = engine.sync(&fx.fake).await.unwrap();
    assert_eq!(report.uploaded, 1, "modification should be uploaded once");

    let snap = fx.fake.snapshot_by_name();
    let f = snap.get("notes.md").expect("remote should still hold notes.md");
    assert_eq!(f.bytes, b"v2 updated content");
}

#[tokio::test]
async fn local_delete_propagates_to_remote_in_two_way() {
    let fx = SyncFixture::new(SyncMode::TwoWay);
    fx.write_local("ephemeral.txt", "soon to be gone");
    let engine = SyncEngine::new(fx.db.clone(), fx.pair.clone());
    engine.sync(&fx.fake).await.unwrap();

    std::fs::remove_file(fx.local_path().join("ephemeral.txt")).unwrap();
    let report = engine.sync(&fx.fake).await.unwrap();
    assert_eq!(report.deleted, 1);

    assert!(fx.fake.snapshot_by_name().get("ephemeral.txt").is_none());
}

#[tokio::test]
async fn remote_folder_delete_propagates_to_local_in_two_way() {
    // Regression test for the v0.1.5 bug: deleting a folder remotely caused
    // the folder to be re-uploaded instead of removed locally, because
    // CreateLocalDir / CreateRemoteDir never wrote a base-state entry.
    let fx = SyncFixture::new(SyncMode::TwoWay);

    // Arrange: a folder containing a file lives on the remote.
    let folder_id = fx.fake.insert_folder("photos", Some(&fx.remote_root));
    fx.fake
        .insert_file("a.jpg", &folder_id, b"image bytes".to_vec());

    let engine = SyncEngine::new(fx.db.clone(), fx.pair.clone());
    engine.sync(&fx.fake).await.unwrap();

    // After the first sync the folder + file should be local…
    let local_folder = fx.local_path().join("photos");
    assert!(local_folder.is_dir(), "remote folder should have been created locally");
    assert!(local_folder.join("a.jpg").exists());

    // …and the folder itself must be present in the index, otherwise the
    // next sync mis-categorises a remote delete as a brand-new local folder.
    let entries = fx
        .db
        .with_conn(|c| FileEntry::list_by_sync_pair(c, &fx.pair.id))
        .unwrap();
    let folder_entry = entries
        .iter()
        .find(|e| e.relative_path == "photos")
        .expect("folder must be indexed after CreateLocalDir");
    assert!(folder_entry.is_directory);

    // Act: trash both the file and the folder on the remote.
    fx.fake
        .trash_file(&fx.fake.snapshot_by_name().get("a.jpg").unwrap().meta.id)
        .await
        .unwrap();
    fx.fake.trash_file(&folder_id).await.unwrap();

    // Re-sync.
    let report = engine.sync(&fx.fake).await.unwrap();

    // Assert: folder is gone locally, no errors, no spurious re-uploads.
    assert!(!local_folder.exists(), "local folder should have been deleted");
    assert_eq!(report.uploaded, 0, "must NOT re-upload the deleted folder");
    assert_eq!(report.errors, 0);
    assert!(report.deleted >= 1);
    assert!(fx.fake.snapshot_by_name().get("photos").is_none());
}

#[tokio::test]
async fn local_folder_delete_propagates_to_remote_in_two_way() {
    // Empty folder — keeps this test focused on folder-delete propagation
    // and avoids the orthogonal parent-id-resolution bug for new local
    // folders that contain children (filed separately).
    let fx = SyncFixture::new(SyncMode::TwoWay);
    std::fs::create_dir(fx.local_path().join("empty-dir")).unwrap();

    let engine = SyncEngine::new(fx.db.clone(), fx.pair.clone());
    engine.sync(&fx.fake).await.unwrap();
    assert!(
        fx.fake.snapshot_by_name().get("empty-dir").is_some(),
        "remote should now have the folder",
    );

    // The locally-originated folder must be indexed so a later delete is
    // recognised as (false, true, true) → DeleteRemote, not (false, true,
    // false) → re-download.
    let entries = fx
        .db
        .with_conn(|c| FileEntry::list_by_sync_pair(c, &fx.pair.id))
        .unwrap();
    assert!(
        entries.iter().any(|e| e.relative_path == "empty-dir" && e.is_directory),
        "locally-created folder must be indexed after CreateRemoteDir"
    );

    std::fs::remove_dir_all(fx.local_path().join("empty-dir")).unwrap();
    let report = engine.sync(&fx.fake).await.unwrap();

    assert!(
        fx.fake.snapshot_by_name().get("empty-dir").is_none(),
        "remote folder should be trashed",
    );
    assert_eq!(report.downloaded, 0, "must NOT re-download the deleted folder");
    assert_eq!(report.errors, 0);
}

#[tokio::test]
async fn remote_delete_propagates_to_local_in_two_way() {
    let fx = SyncFixture::new(SyncMode::TwoWay);
    let id = fx
        .fake
        .insert_file("remote-doomed.txt", &fx.remote_root, b"goodbye".to_vec());
    let engine = SyncEngine::new(fx.db.clone(), fx.pair.clone());
    engine.sync(&fx.fake).await.unwrap();
    assert!(fx.local_path().join("remote-doomed.txt").exists());

    // Trash on the remote, sync again.
    fx.fake.trash_file(&id).await.unwrap();
    let report = engine.sync(&fx.fake).await.unwrap();
    assert_eq!(report.deleted, 1);
    assert!(!fx.local_path().join("remote-doomed.txt").exists());
}

// ── Mode restrictions ───────────────────────────────────────────────

#[tokio::test]
async fn local_to_cloud_ignores_new_remote_file() {
    let fx = SyncFixture::new(SyncMode::LocalToCloud);
    fx.fake
        .insert_file("cloud-only.txt", &fx.remote_root, b"don't pull me".to_vec());
    let engine = SyncEngine::new(fx.db.clone(), fx.pair.clone());
    let report = engine.sync(&fx.fake).await.unwrap();

    assert_eq!(report.downloaded, 0, "local-to-cloud must not download");
    assert!(!fx.local_path().join("cloud-only.txt").exists());
    assert!(report.skipped >= 1);
}

#[tokio::test]
async fn cloud_to_local_ignores_new_local_file() {
    let fx = SyncFixture::new(SyncMode::CloudToLocal);
    fx.write_local("local-only.txt", "don't push me");
    let engine = SyncEngine::new(fx.db.clone(), fx.pair.clone());
    let report = engine.sync(&fx.fake).await.unwrap();

    assert_eq!(report.uploaded, 0, "cloud-to-local must not upload");
    assert!(fx.fake.snapshot_by_name().get("local-only.txt").is_none());
    assert!(report.skipped >= 1);
}

#[tokio::test]
async fn cloud_to_local_does_not_propagate_local_delete() {
    let fx = SyncFixture::new(SyncMode::CloudToLocal);
    fx.fake
        .insert_file("from-cloud.txt", &fx.remote_root, b"hi".to_vec());
    let engine = SyncEngine::new(fx.db.clone(), fx.pair.clone());
    engine.sync(&fx.fake).await.unwrap();
    assert!(fx.local_path().join("from-cloud.txt").exists());

    std::fs::remove_file(fx.local_path().join("from-cloud.txt")).unwrap();
    let report = engine.sync(&fx.fake).await.unwrap();
    // CloudToLocal should re-download the file rather than delete remotely.
    assert_eq!(report.deleted, 0);
    assert!(fx.fake.snapshot_by_name().get("from-cloud.txt").is_some());
}

// ── compute_actions decision matrix ─────────────────────────────────
// These tests poke `compute_actions` directly so we can exercise every cell
// of the (in_local, in_remote, in_base) matrix without driving the engine.

#[tokio::test]
async fn compute_actions_new_local_only() {
    let fx = SyncFixture::new(SyncMode::TwoWay);
    fx.write_local("new.txt", "x");
    let actions = compute(&fx).await;
    let action = expect_one_for(&actions, "new.txt");
    assert!(matches!(action, SyncAction::Upload { .. }));
}

#[tokio::test]
async fn compute_actions_new_remote_only() {
    let fx = SyncFixture::new(SyncMode::TwoWay);
    fx.fake.insert_file("new.txt", &fx.remote_root, b"x".to_vec());
    let actions = compute(&fx).await;
    let action = expect_one_for(&actions, "new.txt");
    assert!(matches!(action, SyncAction::Download { .. }));
}

#[tokio::test]
async fn compute_actions_first_seen_both_sides_with_diff_is_conflict() {
    let fx = SyncFixture::new(SyncMode::TwoWay);
    fx.write_local("collide.txt", "local content");
    fx.fake
        .insert_file("collide.txt", &fx.remote_root, b"different cloud content".to_vec());
    let actions = compute(&fx).await;
    let action = expect_one_for(&actions, "collide.txt");
    assert!(
        matches!(action, SyncAction::Conflict { .. }),
        "first-sync divergence on the same name must produce Conflict, got {action:?}"
    );
}

#[tokio::test]
async fn compute_actions_in_base_but_deleted_both_sides_is_skipped() {
    let fx = SyncFixture::new(SyncMode::TwoWay);
    // Seed a base entry only — neither side currently has the file.
    let entry = FileEntry {
        id: None,
        sync_pair_id: fx.pair.id.clone(),
        relative_path: "ghost.txt".into(),
        local_hash: Some("h".into()),
        remote_md5: Some("m".into()),
        remote_id: Some("rid".into()),
        remote_rev: None,
        size: Some(1),
        local_mtime: None,
        remote_mtime: None,
        is_directory: false,
        is_google_doc: false,
        state: FileState::Synced,
        last_synced_at: None,
    };
    fx.db.with_conn(|c| entry.upsert(c)).unwrap();

    let actions = compute(&fx).await;
    let action = expect_one_for(&actions, "ghost.txt");
    assert!(matches!(action, SyncAction::Skip { .. }));
}

// ── Helpers ─────────────────────────────────────────────────────────

async fn compute(fx: &SyncFixture) -> Vec<SyncAction> {
    use insyncbee_core::drive::DriveFile;
    use insyncbee_core::watcher::{self, LocalFileInfo};

    let local_root = fx.local_path();
    let local: HashMap<String, LocalFileInfo> = watcher::scan_directory(&local_root)
        .unwrap()
        .into_iter()
        .map(|f| (f.relative_path.clone(), f))
        .collect();

    let remote_files = fetch_remote_tree(&fx.fake, &fx.remote_root, "").await;
    let remote: HashMap<String, DriveFile> = remote_files.into_iter().collect();

    let base: HashMap<String, FileEntry> = fx
        .db
        .with_conn(|c| FileEntry::list_by_sync_pair(c, &fx.pair.id))
        .unwrap()
        .into_iter()
        .map(|e| (e.relative_path.clone(), e))
        .collect();

    let engine = SyncEngine::new(fx.db.clone(), fx.pair.clone());
    engine.compute_actions(&local, &remote, &base, &local_root)
}

fn expect_one_for<'a>(actions: &'a [SyncAction], path: &str) -> &'a SyncAction {
    let matching: Vec<_> = actions
        .iter()
        .filter(|a| {
            // Match any variant whose relative_path equals `path`.
            match a {
                SyncAction::Upload { relative_path, .. }
                | SyncAction::UpdateRemote { relative_path, .. }
                | SyncAction::Download { relative_path, .. }
                | SyncAction::DeleteLocal { relative_path, .. }
                | SyncAction::DeleteRemote { relative_path, .. }
                | SyncAction::CreateLocalDir { relative_path, .. }
                | SyncAction::CreateRemoteDir { relative_path, .. }
                | SyncAction::Conflict { relative_path, .. }
                | SyncAction::Skip { relative_path, .. } => relative_path == path,
            }
        })
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "expected exactly one action for {path}, got: {actions:#?}"
    );
    matching[0]
}

async fn fetch_remote_tree(
    fake: &FakeDriveClient,
    folder_id: &str,
    prefix: &str,
) -> Vec<(String, insyncbee_core::drive::DriveFile)> {
    use insyncbee_core::drive::DriveClient;
    let mut out = Vec::new();
    let files = fake.list_all_files(folder_id).await.unwrap();
    for f in files {
        let path = if prefix.is_empty() {
            f.name.clone()
        } else {
            format!("{prefix}/{}", f.name)
        };
        if f.is_folder() {
            let id = f.id.clone();
            let name = path.clone();
            out.push((path, f));
            let sub = Box::pin(fetch_remote_tree(fake, &id, &name)).await;
            out.extend(sub);
        } else {
            out.push((path, f));
        }
    }
    out
}

#[allow(dead_code)]
fn _path_display(p: &Path) -> String {
    p.display().to_string()
}

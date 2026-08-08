//! Byte accounting: what a sync records about the bytes it moved, and what
//! the statistics aggregate makes of it.
//!
//! These numbers are user-facing (the activity feed divides them into a
//! speed, the statistics page sums them), and they are the kind of thing
//! that silently goes to zero when a code path stops threading metrics
//! through. Every assertion here is about a number a user would notice.

mod common;

use common::SyncFixture;
use insyncbee_core::db::models::{ChangeLogEntry, SyncMode, TransferStats};
use insyncbee_core::sync_engine::SyncEngine;

const BODY: &str = "the quick brown fox jumps over the lazy dog";

#[tokio::test]
async fn upload_records_bytes_and_duration() {
    let fx = SyncFixture::new(SyncMode::TwoWay);
    fx.write_local("notes.txt", BODY);

    let engine = SyncEngine::new(fx.db.clone(), fx.pair.clone());
    let report = engine.sync(&fx.fake).await.unwrap();

    assert_eq!(report.uploaded, 1);
    assert_eq!(
        report.bytes_uploaded,
        BODY.len() as u64,
        "the report should carry the bytes actually sent"
    );

    let entries = fx
        .db
        .with_conn(|conn| ChangeLogEntry::recent(conn, &fx.pair.id, 10))
        .unwrap();
    let upload = entries
        .iter()
        .find(|e| e.action == "upload")
        .expect("an upload row");

    assert_eq!(upload.bytes, Some(BODY.len() as i64));
    assert!(
        upload.duration_ms.map(|d| d >= 1).unwrap_or(false),
        "duration must be at least 1ms so speed never divides by zero, got {:?}",
        upload.duration_ms
    );
}

#[tokio::test]
async fn download_records_bytes() {
    let fx = SyncFixture::new(SyncMode::TwoWay);
    fx.fake
        .insert_file("remote.txt", &fx.remote_root, BODY.as_bytes().to_vec());

    let engine = SyncEngine::new(fx.db.clone(), fx.pair.clone());
    let report = engine.sync(&fx.fake).await.unwrap();

    assert_eq!(report.downloaded, 1);
    assert_eq!(report.bytes_downloaded, BODY.len() as u64);

    let entries = fx
        .db
        .with_conn(|conn| ChangeLogEntry::recent(conn, &fx.pair.id, 10))
        .unwrap();
    let download = entries
        .iter()
        .find(|e| e.action == "download")
        .expect("a download row");
    assert_eq!(download.bytes, Some(BODY.len() as i64));
}

#[tokio::test]
async fn stats_separate_the_two_directions() {
    let fx = SyncFixture::new(SyncMode::TwoWay);
    fx.write_local("up.txt", BODY);
    fx.fake
        .insert_file("down.txt", &fx.remote_root, b"short".to_vec());

    let engine = SyncEngine::new(fx.db.clone(), fx.pair.clone());
    engine.sync(&fx.fake).await.unwrap();

    let stats = fx
        .db
        .with_conn(|conn| TransferStats::compute(conn, Some(&fx.pair.id), None))
        .unwrap();

    assert_eq!(stats.uploaded.files, 1);
    assert_eq!(stats.uploaded.bytes, BODY.len() as i64);
    assert_eq!(stats.downloaded.files, 1);
    assert_eq!(stats.downloaded.bytes, 5);
    assert!(
        stats.uploaded.average_bps().is_some(),
        "an upload with bytes and duration should yield a speed"
    );
    assert!(stats.since.is_some(), "stats should report their own range");
}

#[tokio::test]
async fn stats_scope_to_one_pair() {
    let a = SyncFixture::new(SyncMode::TwoWay);
    a.write_local("a.txt", BODY);
    SyncEngine::new(a.db.clone(), a.pair.clone())
        .sync(&a.fake)
        .await
        .unwrap();

    // A second pair in the same database must not leak into the first
    // pair's totals — the statistics page offers a per-pair scope.
    let stats = a
        .db
        .with_conn(|conn| TransferStats::compute(conn, Some("some-other-pair"), None))
        .unwrap();
    assert_eq!(stats.uploaded.files, 0);
    assert_eq!(stats.uploaded.bytes, 0);
    assert!(stats.uploaded.average_bps().is_none());
}

/// Rows written before schema v3 have NULL bytes. They must still be
/// counted as transfers, and must not be silently treated as zero-byte
/// ones — `measured_files` is what tells the UI the byte total is partial.
#[tokio::test]
async fn unmeasured_rows_count_as_files_but_not_as_bytes() {
    let fx = SyncFixture::new(SyncMode::TwoWay);

    fx.db
        .with_conn(|conn| {
            ChangeLogEntry::insert(conn, &fx.pair.id, "legacy.txt", "upload", None)?;
            ChangeLogEntry::insert_transfer(
                conn,
                &fx.pair.id,
                "measured.txt",
                "upload",
                None,
                Some(1000),
                Some(500),
            )?;
            Ok(())
        })
        .unwrap();

    let stats = fx
        .db
        .with_conn(|conn| TransferStats::compute(conn, Some(&fx.pair.id), None))
        .unwrap();

    assert_eq!(stats.uploaded.files, 2, "both rows are transfers");
    assert_eq!(stats.uploaded.measured_files, 1, "only one carries bytes");
    assert_eq!(stats.uploaded.bytes, 1000);
    // 1000 bytes in 500ms = 2000 B/s. The unmeasured row must not drag
    // this toward zero by contributing a phantom 0-byte transfer.
    assert_eq!(stats.uploaded.average_bps(), Some(2000.0));
}

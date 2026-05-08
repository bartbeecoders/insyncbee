//! End-to-end tests for per-sync-pair encryption.
//!
//! These don't touch the OS keyring (see `crate::keystore` — that path
//! is integration-only). Instead they construct a `FileCipher` directly
//! and attach it to the engine via `with_cipher`, mirroring what the
//! production wiring does after pulling the key out of the keyring.

mod common;

use common::SyncFixture;
use insyncbee_core::crypto::FileCipher;
use insyncbee_core::db::models::{FileEntry, FileState, SyncMode, SyncPair};
use insyncbee_core::sync_engine::SyncEngine;
use std::sync::Arc;

const TEST_KEY: [u8; 32] = [0x77u8; 32];

fn encrypted_pair(pair: &SyncPair) -> SyncPair {
    let mut p = pair.clone();
    p.encryption_enabled = true;
    p.encryption_salt = Some(vec![0u8; 16]);
    p.encryption_verifier = Some(vec![0u8; 32]); // not consulted by the engine
    p
}

#[tokio::test]
async fn upload_encrypts_local_file() {
    let fx = SyncFixture::new(SyncMode::TwoWay);
    let pair = encrypted_pair(&fx.pair);
    fx.db
        .with_conn(|c| {
            insyncbee_core::db::models::SyncPair::update_encryption(
                c,
                &pair.id,
                true,
                Some(&[0u8; 16]),
                Some(&[0u8; 32]),
            )
        })
        .unwrap();

    let cipher = Arc::new(FileCipher::from_key(TEST_KEY));
    fx.write_local("notes.txt", "this is a secret");

    let engine = SyncEngine::new(fx.db.clone(), pair.clone()).with_cipher(cipher.clone());
    let report = engine.sync(&fx.fake).await.unwrap();
    assert_eq!(report.uploaded, 1);
    assert_eq!(report.errors, 0);

    // Find the uploaded file in the fake. Its bytes should NOT contain
    // the plaintext anywhere — they should be ciphertext + AEAD tag.
    let snap = fx.fake.snapshot_by_name();
    let uploaded = snap.get("notes.txt").expect("file uploaded under its name");
    assert!(
        !uploaded
            .bytes
            .windows("secret".len())
            .any(|w| w == b"secret"),
        "ciphertext leaked plaintext"
    );
    assert!(
        uploaded.bytes.starts_with(b"INSYNCBE"),
        "ciphertext header missing magic"
    );
}

#[tokio::test]
async fn download_decrypts_remote_file() {
    let fx = SyncFixture::new(SyncMode::TwoWay);
    let pair = encrypted_pair(&fx.pair);
    fx.db
        .with_conn(|c| {
            insyncbee_core::db::models::SyncPair::update_encryption(
                c,
                &pair.id,
                true,
                Some(&[0u8; 16]),
                Some(&[0u8; 32]),
            )
        })
        .unwrap();

    let cipher = Arc::new(FileCipher::from_key(TEST_KEY));

    // Pre-populate the fake with an *encrypted* file as if some other
    // machine had uploaded it. We do this by encrypting locally and then
    // injecting the bytes directly into the fake.
    let plain = fx.local.path().join("__staging.txt");
    let ct = fx.local.path().join("__ct");
    tokio::fs::write(&plain, b"hello from cloud").await.unwrap();
    cipher.encrypt_file(&plain, &ct).await.unwrap();
    let ct_bytes = tokio::fs::read(&ct).await.unwrap();
    tokio::fs::remove_file(&plain).await.unwrap();
    tokio::fs::remove_file(&ct).await.unwrap();
    fx.fake.insert_file("hello.txt", &fx.remote_root, ct_bytes);

    let engine = SyncEngine::new(fx.db.clone(), pair.clone()).with_cipher(cipher);
    let report = engine.sync(&fx.fake).await.unwrap();
    assert_eq!(report.downloaded, 1);
    assert_eq!(report.errors, 0);

    // The local file should be plaintext on disk.
    let local = fx.local.path().join("hello.txt");
    let recovered = tokio::fs::read(&local).await.unwrap();
    assert_eq!(recovered, b"hello from cloud");
}

#[tokio::test]
async fn round_trip_through_sync_engine() {
    // Upload-then-download cycle: we encrypt to the fake, then a fresh
    // engine on a *different* local root downloads and decrypts.
    let fx_a = SyncFixture::new(SyncMode::TwoWay);
    let pair_a = encrypted_pair(&fx_a.pair);
    fx_a.db
        .with_conn(|c| {
            insyncbee_core::db::models::SyncPair::update_encryption(
                c,
                &pair_a.id,
                true,
                Some(&[0u8; 16]),
                Some(&[0u8; 32]),
            )
        })
        .unwrap();
    let cipher = Arc::new(FileCipher::from_key(TEST_KEY));
    fx_a.write_local("doc.bin", "round-trip-payload-12345");

    let engine_a =
        SyncEngine::new(fx_a.db.clone(), pair_a.clone()).with_cipher(cipher.clone());
    let r = engine_a.sync(&fx_a.fake).await.unwrap();
    assert_eq!(r.uploaded, 1);

    // Bring up a second tempdir + DB pointed at the same fake. This
    // simulates "another machine syncs the same encrypted Drive folder".
    let fx_b_local = tempfile::TempDir::new().unwrap();
    let db_b = insyncbee_core::db::Database::open_in_memory().unwrap();
    let account = insyncbee_core::db::models::Account {
        id: "acc-b".into(),
        email: "b@example.com".into(),
        display_name: None,
        access_token: "x".into(),
        refresh_token: "x".into(),
        token_expiry: chrono::Utc::now().to_rfc3339(),
        change_token: None,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    db_b.with_conn(|c| account.insert(c)).unwrap();
    let mut pair_b = pair_a.clone();
    pair_b.id = format!("pair-{}", uuid::Uuid::new_v4());
    pair_b.account_id = account.id.clone();
    pair_b.local_root = fx_b_local.path().to_string_lossy().to_string();
    db_b.with_conn(|c| pair_b.insert(c)).unwrap();

    let engine_b = SyncEngine::new(db_b.clone(), pair_b.clone()).with_cipher(cipher);
    let r = engine_b.sync(&fx_a.fake).await.unwrap();
    assert_eq!(r.downloaded, 1);

    let recovered = tokio::fs::read(fx_b_local.path().join("doc.bin"))
        .await
        .unwrap();
    assert_eq!(recovered, b"round-trip-payload-12345");
}

#[tokio::test]
async fn sync_without_cipher_errors_per_file_not_silently_uploads() {
    // Pair is marked encrypted but no cipher attached. Upload must
    // surface as an error per file rather than leaking plaintext.
    let fx = SyncFixture::new(SyncMode::TwoWay);
    let pair = encrypted_pair(&fx.pair);
    fx.db
        .with_conn(|c| {
            insyncbee_core::db::models::SyncPair::update_encryption(
                c,
                &pair.id,
                true,
                Some(&[0u8; 16]),
                Some(&[0u8; 32]),
            )
        })
        .unwrap();

    fx.write_local("locked.txt", "do not upload as plaintext");

    let engine = SyncEngine::new(fx.db.clone(), pair); // NO with_cipher
    let report = engine.sync(&fx.fake).await.unwrap();
    assert_eq!(report.uploaded, 0);
    assert_eq!(report.errors, 1);

    // And critically, the fake should hold no file other than the root.
    let snap = fx.fake.snapshot_by_name();
    assert!(
        !snap.contains_key("locked.txt"),
        "no plaintext should have reached Drive"
    );

    // Index must NOT have a synced entry for the file — leaving one
    // would mask the failure on the next cycle.
    let entries = fx
        .db
        .with_conn(|c| FileEntry::list_by_sync_pair(c, &fx.pair.id))
        .unwrap();
    let synced_for_locked = entries
        .iter()
        .filter(|e| e.relative_path == "locked.txt" && e.state == FileState::Synced)
        .count();
    assert_eq!(synced_for_locked, 0);
}

#[tokio::test]
async fn wrong_key_at_download_surfaces_as_error() {
    let fx = SyncFixture::new(SyncMode::TwoWay);
    let pair = encrypted_pair(&fx.pair);
    fx.db
        .with_conn(|c| {
            insyncbee_core::db::models::SyncPair::update_encryption(
                c,
                &pair.id,
                true,
                Some(&[0u8; 16]),
                Some(&[0u8; 32]),
            )
        })
        .unwrap();

    // Upload encrypted with key A.
    let key_a = Arc::new(FileCipher::from_key([1u8; 32]));
    let plain = fx.local.path().join("__staging.txt");
    let ct = fx.local.path().join("__ct");
    tokio::fs::write(&plain, b"secret").await.unwrap();
    key_a.encrypt_file(&plain, &ct).await.unwrap();
    let ct_bytes = tokio::fs::read(&ct).await.unwrap();
    tokio::fs::remove_file(&plain).await.unwrap();
    tokio::fs::remove_file(&ct).await.unwrap();
    fx.fake.insert_file("doc.bin", &fx.remote_root, ct_bytes);

    // Attempt to sync down with key B → AEAD failure → reported as error.
    let key_b = Arc::new(FileCipher::from_key([2u8; 32]));
    let engine = SyncEngine::new(fx.db.clone(), pair).with_cipher(key_b);
    let report = engine.sync(&fx.fake).await.unwrap();
    assert_eq!(report.errors, 1);
    assert_eq!(report.downloaded, 0);
    assert!(
        !fx.local.path().join("doc.bin").exists()
            || tokio::fs::metadata(fx.local.path().join("doc.bin"))
                .await
                .map(|m| m.len() == 0)
                .unwrap_or(true),
        "decrypt failure must not leave a partial file the user could mistake for real data"
    );
}

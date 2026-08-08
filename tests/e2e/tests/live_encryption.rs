//! **Group I — client-side encryption**, live.
//!
//! The promise of an encrypted sync pair is narrow and absolute: Google
//! never holds plaintext, and the user never holds ciphertext. Both halves
//! are asserted here against real Drive, including the failure mode that
//! matters most — an encrypted pair whose key is not loaded must refuse to
//! sync rather than quietly upload plaintext.

use insyncbee_e2e::{skip_unless_live, Opts, E2E};

const PASSPHRASE: &str = "correct horse battery staple";

/// **I1** — upload path. Local stays plaintext, Drive receives ciphertext,
/// and the ciphertext decrypts back to exactly what we wrote.
#[tokio::test]
async fn i1_encrypted_upload_puts_ciphertext_on_drive() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup_with("i1-encrypt-up", Opts::default().encrypted(PASSPHRASE)).await?;

    let secret = "SENSITIVE-MARKER-9f3a: bank details";
    h.write("secret.txt", secret)?;

    let report = h.sync().await?;
    assert_eq!(report.uploaded, 1, "{report}");
    assert_eq!(report.errors, 0, "{report}");

    // Local side: untouched plaintext.
    assert_eq!(h.read("secret.txt")?, secret, "local file must stay plaintext");

    // Remote side: ciphertext that does not contain the marker anywhere.
    let raw = h.remote_bytes("secret.txt").await?;
    assert_ne!(raw, secret.as_bytes(), "plaintext was uploaded verbatim");
    assert!(
        !contains(&raw, b"SENSITIVE-MARKER-9f3a"),
        "plaintext marker found in the bytes stored on Drive"
    );
    assert!(
        raw.len() > secret.len(),
        "ciphertext should carry nonce/tag overhead, got {} bytes",
        raw.len()
    );

    // And it decrypts back to the original.
    assert_eq!(h.remote_text("secret.txt").await?, secret);

    h.finish().await
}

/// **I2** — download path. Deleting the local copy and re-syncing must
/// reconstruct the exact plaintext from the ciphertext on Drive.
#[tokio::test]
async fn i2_encrypted_download_restores_exact_plaintext() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup_with("i2-encrypt-down", Opts::default().encrypted(PASSPHRASE)).await?;

    // A blob big enough to span more than one cipher chunk.
    let payload: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    h.write_bytes("payload.bin", &payload)?;
    h.sync().await?;

    // Simulate a fresh machine: the file exists on Drive, not on disk. The
    // index still knows it, which is the `(false, true, true)` path — so
    // clear the index entry too, making this a genuine first download.
    h.remove("payload.bin")?;
    h.db.with_conn(|conn| {
        insyncbee_core::db::models::FileEntry::delete_by_path(conn, &h.pair.id, "payload.bin")
    })?;

    let report = h.sync().await?;
    assert_eq!(report.downloaded, 1, "{report}");
    assert!(
        h.read_bytes("payload.bin")? == payload,
        "decrypted download does not match the original bytes"
    );

    h.finish().await
}

/// **I3** — the safety property. An encrypted pair with no key loaded must
/// fail loudly and leave Drive empty. Silently uploading plaintext here
/// would defeat the entire feature.
#[tokio::test]
async fn i3_locked_pair_refuses_to_upload_plaintext() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup_with("i3-locked", Opts::default().encrypted(PASSPHRASE)).await?;

    h.write("secret.txt", "PLAINTEXT-MUST-NOT-LEAVE")?;

    let report = h.sync_without_cipher().await?;
    assert_eq!(
        report.errors, 1,
        "a locked encrypted pair must report an error, not proceed: {report}"
    );
    assert_eq!(report.uploaded, 0, "a locked pair uploaded a file: {report}");

    let remote = h.remote_paths().await?;
    assert!(
        remote.is_empty(),
        "a locked encrypted pair wrote to Drive: {remote:?}"
    );

    h.finish().await
}

/// **I4** — first sync of an encrypted pair where the file exists on both
/// sides. Local blake3 and remote MD5-over-ciphertext are not comparable,
/// so the engine defers to the conflict handler by design. This documents
/// that deliberate choice so a future "optimisation" doesn't silently
/// replace it with a wrong-but-cheap hash comparison.
#[tokio::test]
async fn i4_encrypted_first_sync_defers_to_conflict_resolution() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup_with("i4-encrypt-first", Opts::default().encrypted(PASSPHRASE)).await?;

    h.write("both.txt", "local copy")?;
    // Put ciphertext on Drive under the same name by syncing a throwaway
    // first, then re-diverging the local side.
    h.sync().await?;
    h.write("both.txt", "local copy edited")?;
    h.db.with_conn(|conn| {
        insyncbee_core::db::models::FileEntry::delete_by_path(conn, &h.pair.id, "both.txt")
    })?;

    let report = h.sync().await?;
    assert_eq!(
        report.conflicts, 1,
        "encrypted first-sync collision must be routed to conflict resolution: {report}"
    );
    assert_eq!(
        h.read("both.txt")?,
        "local copy edited",
        "KeepBoth must leave the local file alone"
    );

    h.finish().await
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

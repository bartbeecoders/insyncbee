//! **Group H — content and naming edge cases**, live.
//!
//! Everything here is a place where Google Drive and a POSIX filesystem
//! disagree, or where the transfer path branches. A fake backend cannot
//! catch any of it: the resumable-upload threshold, MD5 on empty files,
//! Unicode normalisation, Drive's tolerance for duplicate names, and the
//! scanner's dotfile rule.

use insyncbee_e2e::{filler, skip_unless_live, E2E};

/// **H1** — a file above the 4 MiB resumable threshold round-trips through
/// the chunked upload path with its bytes intact. Below the threshold the
/// client uses a completely different code path (single-shot multipart), so
/// this is the only scenario that exercises chunking at all.
#[tokio::test]
async fn h1_large_file_uses_resumable_upload_and_round_trips() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("h1-large").await?;

    // 5 MiB — one full 8 MiB chunk's worth of "final chunk" handling, and
    // comfortably over RESUMABLE_THRESHOLD_BYTES (4 MiB).
    let bytes = filler(5 * 1024 * 1024, 7);
    h.write_bytes("big.bin", &bytes)?;

    let report = h.sync().await?;
    assert_eq!(report.uploaded, 1, "{report}");
    assert_eq!(report.errors, 0, "{report}");

    let remote = h.remote_plaintext("big.bin").await?;
    assert_eq!(remote.len(), bytes.len(), "size changed in transit");
    assert!(remote == bytes, "content corrupted in transit");

    h.assert_converged().await?;
    h.finish().await
}

/// **H2** — binary content downloads byte-identically (no newline or
/// encoding mangling on the way in).
#[tokio::test]
async fn h2_binary_download_is_byte_identical() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("h2-binary").await?;

    let bytes: Vec<u8> = (0u8..=255).cycle().take(64 * 1024).collect();
    h.remote_write_bytes("blob.bin", &bytes).await?;

    h.sync().await?;

    assert!(
        h.read_bytes("blob.bin")? == bytes,
        "binary content was altered on download"
    );

    h.assert_converged().await?;
    h.finish().await
}

/// **H3** — a zero-byte file. Empty files have a real MD5 but no content,
/// and are a classic off-by-one in upload paths.
#[tokio::test]
async fn h3_zero_byte_file_round_trips() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("h3-empty").await?;

    h.write("empty.txt", "")?;
    let report = h.sync().await?;
    assert_eq!(report.errors, 0, "{report}");

    let remote = h.remote_tree().await?;
    assert_eq!(
        remote["empty.txt"].size_bytes(),
        0,
        "empty file did not stay empty"
    );

    h.assert_mirrored().await?;
    h.assert_converged().await?;
    h.finish().await
}

/// **H4** — names with spaces, Unicode, emoji, and punctuation that is
/// legal on Drive. These flow through path joining, the Drive `q` query,
/// and the conflicted-copy namer.
#[tokio::test]
async fn h4_unicode_and_punctuated_names_round_trip() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("h4-names").await?;

    let names = [
        "a file with spaces.txt",
        "café ☕ résumé.txt",
        "emoji 🐝 sync.txt",
        "it's got an apostrophe.txt",
        "ampersand & hash #1.txt",
        "dossier accentué/notes ré.txt",
    ];
    for (i, name) in names.iter().enumerate() {
        h.write(name, &format!("content {i}"))?;
    }

    let report = h.sync().await?;
    assert_eq!(report.errors, 0, "{report}");

    let remote = h.remote_paths().await?;
    for name in names {
        assert!(
            remote.contains(&name.to_string()),
            "'{name}' did not survive the round trip: {remote:?}"
        );
    }

    h.assert_mirrored().await?;
    h.assert_converged().await?;
    h.finish().await
}

/// **H5** — a deep path with a long component. Drive allows far longer
/// names than many filesystems tolerate in a full path, so this is where
/// a `File name too long` error would surface.
#[tokio::test]
async fn h5_deep_path_with_long_names_round_trips() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("h5-long-path").await?;

    let long = "l".repeat(120);
    let rel = format!("depth1/depth2/depth3/depth4/{long}.txt");
    h.write(&rel, "deep and long")?;

    let report = h.sync().await?;
    assert_eq!(report.errors, 0, "{report}");
    assert!(h.remote_exists(&rel).await?, "long deep path missing on Drive");

    h.assert_mirrored().await?;
    h.assert_converged().await?;
    h.finish().await
}

/// **H6** — Drive permits two files with the same name in one folder; a
/// POSIX directory does not. The engine keys its remote tree by path, so
/// one of the two is necessarily shadowed.
///
/// This scenario pins the *current* behaviour rather than an ideal one:
/// the sync must not error, and a version of the file must land locally.
/// Full disambiguation is tracked as a gap in `tests/SCENARIOS.md` (H6).
#[tokio::test]
async fn h6_duplicate_remote_names_do_not_break_the_cycle() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("h6-dupes").await?;

    h.remote_write("dupe.txt", "first version").await?;
    h.remote_write_duplicate("dupe.txt", "second version").await?;

    let report = h.sync().await?;
    assert_eq!(report.errors, 0, "duplicate names broke the sync: {report}");

    let got = h.read("dupe.txt")?;
    assert!(
        got == "first version" || got == "second version",
        "local copy holds neither remote version: {got:?}"
    );

    h.finish().await
}

/// **H7** — dot-prefixed files. [`watcher::scan_directory`] skips them, so
/// they are deliberately never uploaded.
#[tokio::test]
async fn h7_local_dotfiles_are_never_uploaded() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("h7-dotfile-local").await?;

    h.write(".hidden", "local only")?;
    h.write("visible.txt", "synced")?;

    let report = h.sync().await?;
    assert_eq!(report.uploaded, 1, "only the visible file should upload: {report}");
    assert!(
        !h.remote_exists(".hidden").await?,
        "a dotfile leaked to Drive"
    );
    assert!(h.exists(".hidden"), "the dotfile was disturbed locally");

    h.finish().await
}

/// **H8** — the mirror image of H7, and the sharp edge of the dotfile rule:
/// a dot-prefixed file that already exists **on Drive**.
///
/// The ignore rule has to be symmetric. When only the local scanner hid
/// dotfiles, a remote `.config` was downloaded and indexed, then became
/// invisible to the next scan — so the three-way comparison read
/// `(local=false, remote=true, base=true)`, the signature of "the user
/// deleted it", and the next cycle trashed the user's file on Drive.
///
/// Both halves are asserted: the file is never pulled down, and it is still
/// on Drive after repeated cycles.
#[tokio::test]
async fn h8_remote_dotfiles_are_ignored_not_deleted() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("h8-dotfile-remote").await?;

    h.remote_write(".config", "user data on drive").await?;

    let first = h.sync().await?;
    assert_eq!(first.downloaded, 0, "a hidden remote entry was pulled down: {first}");
    assert!(!h.exists(".config"), "hidden remote entry materialised locally");

    // The cycle that used to destroy it.
    let second = h.sync().await?;
    assert_eq!(second.deleted, 0, "a hidden remote entry was deleted: {second}");

    assert!(
        h.remote_exists(".config").await?,
        "DATA LOSS: a dot-prefixed file on Drive was trashed by a later sync cycle"
    );
    assert_eq!(
        h.remote_text(".config").await?,
        "user data on drive",
        "hidden remote entry was modified"
    );

    h.finish().await
}

//! **Group G — sync modes**, live.
//!
//! One-way modes are a data-safety feature: a user who picks
//! `local-to-cloud` for a backup folder is promising themselves that
//! nothing on Drive can ever reach in and delete their local files. These
//! scenarios assert the *negative* half of each mode — what must **not**
//! happen — because that is the half a user only finds out about after
//! losing something.

use insyncbee_core::db::models::SyncMode;
use insyncbee_e2e::{skip_unless_live, Opts, E2E};

// ── G1. local-to-cloud (backup / push-only) ─────────────────────────────

/// **G1a** — local changes are pushed.
#[tokio::test]
async fn g1a_local_to_cloud_pushes_local_changes() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup_with("g1a-push", Opts::default().mode(SyncMode::LocalToCloud)).await?;

    h.write("backup.txt", "push me")?;
    let report = h.sync().await?;

    assert_eq!(report.uploaded, 1, "{report}");
    assert_eq!(h.remote_text("backup.txt").await?, "push me");

    h.assert_converged().await?;
    h.finish().await
}

/// **G1b** — a file that appears on Drive is **not** pulled down. Nothing
/// from the cloud may materialise in a push-only folder.
#[tokio::test]
async fn g1b_local_to_cloud_ignores_new_remote_files() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup_with("g1b-no-pull", Opts::default().mode(SyncMode::LocalToCloud)).await?;

    h.remote_write("stranger.txt", "from the cloud").await?;
    let report = h.sync().await?;

    assert_eq!(report.downloaded, 0, "push-only mode downloaded a file: {report}");
    assert!(!h.exists("stranger.txt"), "remote file materialised locally");

    h.finish().await
}

/// **G1c** — the safety promise: a remote delete must never remove a local
/// file in push-only mode.
#[tokio::test]
async fn g1c_local_to_cloud_never_deletes_local_files() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup_with("g1c-no-local-delete", Opts::default().mode(SyncMode::LocalToCloud)).await?;

    h.write("precious.txt", "irreplaceable")?;
    h.sync().await?;

    h.remote_trash("precious.txt").await?;
    let report = h.sync().await?;

    assert!(
        h.exists("precious.txt"),
        "a remote delete destroyed a local file in push-only mode — {report}"
    );
    assert_eq!(
        h.read("precious.txt")?,
        "irreplaceable",
        "local content was altered"
    );

    h.finish().await
}

// ── G2. cloud-to-local (mirror / pull-only) ─────────────────────────────

/// **G2a** — remote changes are pulled.
#[tokio::test]
async fn g2a_cloud_to_local_pulls_remote_changes() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup_with("g2a-pull", Opts::default().mode(SyncMode::CloudToLocal)).await?;

    h.remote_write("mirror.txt", "pull me").await?;
    let report = h.sync().await?;

    assert_eq!(report.downloaded, 1, "{report}");
    assert_eq!(h.read("mirror.txt")?, "pull me");

    h.assert_converged().await?;
    h.finish().await
}

/// **G2b** — local files are never pushed in pull-only mode. A user's
/// scratch files must not leak into the Drive folder they are mirroring.
#[tokio::test]
async fn g2b_cloud_to_local_never_uploads_local_files() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup_with("g2b-no-push", Opts::default().mode(SyncMode::CloudToLocal)).await?;

    h.write("scratch.txt", "private note")?;
    let report = h.sync().await?;

    assert_eq!(report.uploaded, 0, "pull-only mode uploaded a file: {report}");
    assert!(
        !h.remote_exists("scratch.txt").await?,
        "local file leaked to Drive in pull-only mode"
    );

    h.finish().await
}

/// **G2c** — a local delete must not propagate to Drive in pull-only mode;
/// the next cycle simply restores the local copy from the source of truth.
#[tokio::test]
async fn g2c_cloud_to_local_never_deletes_remote_files() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup_with("g2c-no-remote-delete", Opts::default().mode(SyncMode::CloudToLocal)).await?;

    h.remote_write("source.txt", "authoritative").await?;
    h.sync().await?;
    assert!(h.exists("source.txt"), "setup did not pull the file");

    h.remove("source.txt")?;
    let report = h.sync().await?;

    assert!(
        h.remote_exists("source.txt").await?,
        "a local delete trashed the Drive copy in pull-only mode — {report}"
    );

    h.finish().await
}

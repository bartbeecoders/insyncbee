//! **Group J — auth and resilience**, live.
//!
//! Token lifetime is the one failure mode every long-running sync client
//! hits daily: an access token lasts an hour, the daemon runs for weeks.
//! These scenarios drive the refresh path against real Google endpoints,
//! and assert that an unrecoverable auth failure degrades into a clear
//! error rather than into destructive sync actions.

use insyncbee_e2e::{skip_unless_live, E2E};

/// **J1** — an expired access token is refreshed transparently mid-sync,
/// and the refreshed token is persisted so the next cycle doesn't refresh
/// again.
#[tokio::test]
async fn j1_expired_access_token_is_refreshed_transparently() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("j1-refresh").await?;

    h.write("after-refresh.txt", "uploaded with a refreshed token")?;
    h.expire_access_token()?;

    let report = h.sync().await?;
    assert_eq!(report.errors, 0, "refresh path failed: {report}");
    assert_eq!(report.uploaded, 1, "{report}");
    assert_eq!(
        h.remote_text("after-refresh.txt").await?,
        "uploaded with a refreshed token"
    );

    // The refreshed token must have been written back, so a second cycle
    // needs no refresh at all.
    h.assert_converged().await?;
    h.finish().await
}

/// **J2** — a revoked grant must fail cleanly. The critical assertion is
/// the negative one: an auth failure must never be mistaken for "the remote
/// side is empty", which would make every local file look newly created and
/// every remote file look deleted.
#[tokio::test]
async fn j2_revoked_grant_fails_without_destroying_anything() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("j2-revoked").await?;

    h.write("local.txt", "still here")?;
    h.remote_write("remote.txt", "also still here").await?;
    h.sync().await?;

    h.revoke_grant()?;
    let result = h.sync().await;

    assert!(
        result.is_err(),
        "a revoked grant must surface as an error, got {result:?}"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.to_lowercase().contains("token") || msg.to_lowercase().contains("auth") || msg.contains("grant"),
        "the error should name the auth problem, got: {msg}"
    );

    // Nothing may have been deleted on either side.
    assert!(h.exists("local.txt"), "local file removed during an auth failure");
    assert!(h.exists("remote.txt"), "downloaded file removed during an auth failure");

    // Hand the credentials back before teardown — the harness needs a live
    // grant to trash its own remote sandbox.
    h.restore_grant()?;
    h.finish().await
}

/// **J3** — a dry run touches nothing. Users reach for `--dry-run`
/// precisely when they are nervous, so it has to be provably inert on both
/// sides.
#[tokio::test]
async fn j3_dry_run_mutates_neither_side() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("j3-dry-run").await?;

    h.write("local-new.txt", "local")?;
    h.remote_write("remote-new.txt", "remote").await?;

    let local_before = h.local_tree()?;
    let remote_before = h.remote_paths().await?;

    let (actions, report) = h.dry_run().await?;
    assert_eq!(report.uploaded, 1, "dry run should plan one upload: {report}");
    assert_eq!(report.downloaded, 1, "dry run should plan one download: {report}");
    assert!(!actions.is_empty());

    assert_eq!(h.local_tree()?, local_before, "dry run changed the local tree");
    assert_eq!(
        h.remote_paths().await?,
        remote_before,
        "dry run changed the remote tree"
    );
    assert!(
        h.index()?.is_empty(),
        "dry run wrote entries into the file index"
    );

    h.finish().await
}

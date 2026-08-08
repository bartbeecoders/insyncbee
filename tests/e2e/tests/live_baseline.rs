//! **Group A — foundation** and **Group B — first sync**.
//!
//! These establish that the live rig itself is trustworthy (right account,
//! isolated sandbox) and then walk the first-sync half of the three-way
//! `(local, remote, base)` matrix against real Drive.
//!
//! See `tests/SCENARIOS.md` for the full catalogue and IDs.

use insyncbee_core::drive::DriveClient;
use insyncbee_core::watcher;
use insyncbee_e2e::{skip_unless_live, E2E};

// ── A. Foundation ───────────────────────────────────────────────────────

/// **A1** — the harness talks to the account we think it does. Every other
/// live assertion is meaningless if this is wrong, so it runs first and
/// names the account explicitly.
#[tokio::test]
async fn a1_connected_account_is_reachable() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("a1-account").await?;

    let about = h.drive.get_about().await?;
    let email = about
        .user
        .as_ref()
        .and_then(|u| u.email_address.as_deref())
        .unwrap_or_default();
    assert!(
        !email.is_empty(),
        "Drive /about returned no user — the OAuth grant is not usable"
    );
    eprintln!("[a1] authenticated as {email}");

    let quota = about.storage_quota.as_ref();
    assert!(quota.is_some(), "no storage quota returned");

    h.finish().await
}

/// **A2** — the isolation guarantee the whole safety model rests on: the
/// sandbox lives inside the user's real Drive folder but is invisible to
/// the scanner, so the user's own sync pair can never pick it up.
#[tokio::test]
async fn a2_sandbox_is_invisible_to_the_real_pair() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("a2-isolation").await?;
    h.write("canary.txt", "should never reach the user's pair")?;

    let base = h
        .local_root
        .parent()
        .and_then(|p| p.parent())
        .expect("sandbox is two levels below the base folder");

    let seen = watcher::scan_directory(base)?;
    let leaked: Vec<_> = seen
        .iter()
        .filter(|f| f.relative_path.contains("insyncbee-e2e") || f.relative_path.contains("canary"))
        .map(|f| f.relative_path.clone())
        .collect();

    assert!(
        leaked.is_empty(),
        "sandbox leaked into the user's scanned tree: {leaked:?}"
    );

    h.finish().await
}

/// **A3** — the safety net itself. A panicking scenario can't run its own
/// cleanup, so setup sweeps stale sandboxes. If that sweep ever silently
/// stopped working, test debris would accumulate in the user's real Drive
/// indefinitely — so it gets its own test.
#[tokio::test]
async fn a3_orphan_sweep_reclaims_abandoned_sandboxes() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("a3-sweep").await?;

    // A sandbox named as if it were left behind well beyond the age limit.
    let stale_epoch = 1_600_000_000u64; // 2020 — comfortably orphaned
    let stale_name = format!("e2e-{stale_epoch}-999-abandoned-probe");
    let stale = DriveClient::create_folder(&h.drive, &h.sandbox_parent, &stale_name).await?;

    // A fresh sandbox from the current run must survive the same sweep.
    let live = E2E::setup("a3-sweep-trigger").await?;

    let siblings = DriveClient::list_all_files(&h.drive, &h.sandbox_parent).await?;
    let names: Vec<_> = siblings.iter().map(|f| f.name.as_str()).collect();

    assert!(
        !names.contains(&stale_name.as_str()),
        "the orphan sweep left an abandoned sandbox behind: {names:?}"
    );
    assert!(
        siblings.iter().any(|f| f.id == live.remote_root_id),
        "the sweep destroyed a live sandbox from the current run"
    );

    // Belt and braces in case the sweep did not fire.
    let _ = h.drive.trash_file(&stale.id).await;

    live.finish().await?;
    h.finish().await
}

// ── B. First sync — the (local, remote, base) matrix ────────────────────

/// **B1** `(local, ·, ·)` — a brand-new local file is uploaded, and the
/// bytes that land on Drive are the bytes we wrote.
#[tokio::test]
async fn b1_new_local_file_uploads() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("b1-upload").await?;

    h.write("notes.txt", "hello from InSyncBee")?;
    let report = h.sync().await?;

    assert_eq!(report.uploaded, 1, "expected exactly one upload: {report}");
    assert_eq!(report.errors, 0, "{report}");
    assert_eq!(h.remote_text("notes.txt").await?, "hello from InSyncBee");

    h.assert_mirrored().await?;
    h.assert_converged().await?;
    h.finish().await
}

/// **B2** `(·, remote, ·)` — a file that appeared on Drive is downloaded
/// with byte-identical content.
#[tokio::test]
async fn b2_new_remote_file_downloads() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("b2-download").await?;

    h.remote_write("from-drive.txt", "written in the web UI").await?;
    let report = h.sync().await?;

    assert_eq!(report.downloaded, 1, "expected exactly one download: {report}");
    assert_eq!(report.errors, 0, "{report}");
    assert_eq!(h.read("from-drive.txt")?, "written in the web UI");

    h.assert_mirrored().await?;
    h.assert_converged().await?;
    h.finish().await
}

/// **B3** `(local, remote, ·)` with *identical* content — adopting a folder
/// that already matches Drive must be a no-op, not a conflict. This is the
/// single most common real-world first sync: a user points InSyncBee at a
/// folder they previously copied down by hand.
#[tokio::test]
async fn b3_identical_content_on_both_sides_is_adopted_silently() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("b3-adopt").await?;

    let content = "byte-for-byte the same on both sides";
    h.write("same.txt", content)?;
    h.remote_write("same.txt", content).await?;

    let report = h.sync().await?;

    assert_eq!(
        report.conflicts, 0,
        "identical content must not be reported as a conflict: {report}"
    );
    assert_eq!(report.uploaded, 0, "nothing to upload: {report}");
    assert_eq!(report.downloaded, 0, "nothing to download: {report}");
    assert_eq!(h.read("same.txt")?, content, "local content was disturbed");

    h.finish().await
}

/// **B4** `(local, remote, ·)` with *different* content — genuinely
/// divergent first sync is a conflict, and under `KeepBoth` neither
/// version may be lost.
#[tokio::test]
async fn b4_divergent_content_on_both_sides_conflicts() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("b4-divergent").await?;

    h.write("diverged.txt", "the local version")?;
    h.remote_write("diverged.txt", "the remote version").await?;

    let report = h.sync().await?;
    assert_eq!(report.conflicts, 1, "expected one conflict: {report}");

    // KeepBoth: the local file keeps its content and the remote arrives
    // alongside it as a timestamped copy.
    assert_eq!(h.read("diverged.txt")?, "the local version");
    let copies = conflict_copies(&h)?;
    assert_eq!(copies.len(), 1, "expected one conflicted copy, got {copies:?}");
    assert_eq!(
        std::fs::read_to_string(&copies[0])?,
        "the remote version",
        "the conflicted copy must hold the remote version"
    );

    h.finish().await
}

/// **B5** — an empty local directory is created on Drive.
#[tokio::test]
async fn b5_new_local_dir_creates_remote_dir() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("b5-mkdir-remote").await?;

    h.mkdir("reports")?;
    let report = h.sync().await?;
    assert_eq!(report.errors, 0, "{report}");

    let remote = h.remote_tree().await?;
    let entry = remote.get("reports").expect("remote 'reports' folder");
    assert!(entry.is_folder(), "'reports' must be a folder on Drive");

    h.assert_converged().await?;
    h.finish().await
}

/// **B6** — an empty Drive folder is created locally.
#[tokio::test]
async fn b6_new_remote_dir_creates_local_dir() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("b6-mkdir-local").await?;

    h.remote_mkdir("archive").await?;
    let report = h.sync().await?;
    assert_eq!(report.errors, 0, "{report}");

    assert!(
        h.local_path("archive").is_dir(),
        "expected a local 'archive' directory"
    );

    h.assert_converged().await?;
    h.finish().await
}

/// **B7** — convergence over a mixed tree. Two syncs in a row with no
/// interleaved change: the second must do nothing at all. A engine that
/// re-uploads on every cycle still passes every single-cycle assertion.
#[tokio::test]
async fn b7_mixed_tree_converges_in_one_cycle() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("b7-converge").await?;

    h.write("root.txt", "a")?;
    h.write("docs/nested.txt", "b")?;
    h.mkdir("empty-dir")?;
    h.remote_write("remote-only.txt", "c").await?;
    h.remote_mkdir("remote-dir").await?;

    let first = h.sync().await?;
    assert_eq!(first.errors, 0, "{first}");

    h.assert_converged().await?;
    h.assert_mirrored().await?;
    h.finish().await
}

// ── helpers ─────────────────────────────────────────────────────────────

/// Local files whose name carries the `(conflict …)` marker.
pub fn conflict_copies(h: &E2E) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&h.local_root)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().contains("(conflict ") {
            out.push(entry.path());
        }
    }
    out.sort();
    Ok(out)
}

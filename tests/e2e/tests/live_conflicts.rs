//! **Group F — conflict policies**, live.
//!
//! One scenario per [`ConflictPolicy`] variant. Each arranges the same real
//! conflict (both sides edited a tracked file between cycles) and asserts
//! the policy's documented outcome, plus the invariant that binds them all:
//! the losing version is never destroyed without a copy surviving somewhere.

use insyncbee_core::db::models::{ConflictPolicy, FileState};
use insyncbee_e2e::{skip_unless_live, Opts, E2E};

/// Arrange a genuine both-sides-edited conflict on `doc.txt` for `policy`.
async fn conflicted(name: &str, policy: ConflictPolicy) -> anyhow::Result<E2E> {
    let h = E2E::setup_with(name, Opts::default().policy(policy)).await?;
    h.write("doc.txt", "original")?;
    h.sync().await?;
    h.write("doc.txt", "LOCAL")?;
    h.remote_write("doc.txt", "REMOTE").await?;
    Ok(h)
}

/// **F1 `KeepBoth`** — the default. Local keeps its content; the remote
/// version lands beside it as `doc (conflict <timestamp>).txt`. Nothing is
/// overwritten, so the user can always reconcile by hand.
#[tokio::test]
async fn f1_keep_both_writes_a_timestamped_copy() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = conflicted("f1-keep-both", ConflictPolicy::KeepBoth).await?;

    let report = h.sync().await?;
    assert_eq!(report.conflicts, 1, "{report}");

    assert_eq!(h.read("doc.txt")?, "LOCAL");
    let copies = conflict_copies(&h)?;
    assert_eq!(copies.len(), 1, "expected one conflicted copy: {copies:?}");
    assert_eq!(std::fs::read_to_string(&copies[0])?, "REMOTE");

    let name = copies[0].file_name().unwrap().to_string_lossy().to_string();
    assert!(
        name.starts_with("doc (conflict ") && name.ends_with(").txt"),
        "conflicted copy is misnamed: {name}"
    );

    h.finish().await
}

/// **F2 `PreferLocal`** — local overwrites Drive. The remote version is
/// gone from the live file, but Drive keeps it in revision history, which
/// is why this policy is safe to offer.
#[tokio::test]
async fn f2_prefer_local_overwrites_remote() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = conflicted("f2-prefer-local", ConflictPolicy::PreferLocal).await?;

    let report = h.sync().await?;
    assert_eq!(report.conflicts, 1, "{report}");

    assert_eq!(h.read("doc.txt")?, "LOCAL", "local must be untouched");
    assert_eq!(
        h.remote_text("doc.txt").await?,
        "LOCAL",
        "remote must have been overwritten with the local version"
    );
    assert!(
        conflict_copies(&h)?.is_empty(),
        "PreferLocal must not leave conflicted copies behind"
    );

    h.assert_converged().await?;
    h.finish().await
}

/// **F3 `PreferRemote`** — Drive overwrites local.
#[tokio::test]
async fn f3_prefer_remote_overwrites_local() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = conflicted("f3-prefer-remote", ConflictPolicy::PreferRemote).await?;

    let report = h.sync().await?;
    assert_eq!(report.conflicts, 1, "{report}");

    assert_eq!(h.read("doc.txt")?, "REMOTE", "local must have been replaced");
    assert_eq!(h.remote_text("doc.txt").await?, "REMOTE");
    assert!(conflict_copies(&h)?.is_empty());

    h.assert_converged().await?;
    h.finish().await
}

/// **F4a `NewestWins`, local newer** — mtimes decide. The local file is
/// stamped into the future so the winner is unambiguous rather than a race
/// against whatever clock Drive stamped the upload with.
#[tokio::test]
async fn f4a_newest_wins_picks_the_newer_local_file() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = conflicted("f4a-newest-local", ConflictPolicy::NewestWins).await?;
    h.touch_offset("doc.txt", 600)?;

    let report = h.sync().await?;
    assert_eq!(report.conflicts, 1, "{report}");

    assert_eq!(h.read("doc.txt")?, "LOCAL");
    assert_eq!(
        h.remote_text("doc.txt").await?,
        "LOCAL",
        "the newer local version should have won"
    );

    h.finish().await
}

/// **F4b `NewestWins`, remote newer** — same policy, opposite outcome.
#[tokio::test]
async fn f4b_newest_wins_picks_the_newer_remote_file() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = conflicted("f4b-newest-remote", ConflictPolicy::NewestWins).await?;
    h.touch_offset("doc.txt", -86_400)?;

    let report = h.sync().await?;
    assert_eq!(report.conflicts, 1, "{report}");

    assert_eq!(
        h.read("doc.txt")?,
        "REMOTE",
        "the newer remote version should have won"
    );

    h.finish().await
}

/// **F5 `Ask`** — the queue-for-the-user policy. Neither side may be
/// touched; the conflict is recorded in `file_index` for the UI to resolve.
#[tokio::test]
async fn f5_ask_defers_without_touching_either_side() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = conflicted("f5-ask", ConflictPolicy::Ask).await?;

    let report = h.sync().await?;
    assert_eq!(report.conflicts, 1, "{report}");

    assert_eq!(h.read("doc.txt")?, "LOCAL", "Ask must not modify the local file");
    assert_eq!(
        h.remote_text("doc.txt").await?,
        "REMOTE",
        "Ask must not modify the remote file"
    );
    assert!(
        conflict_copies(&h)?.is_empty(),
        "Ask must not create conflicted copies"
    );

    assert_eq!(
        h.index_state("doc.txt")?,
        Some(FileState::Conflict),
        "the conflict must be queued in file_index for the UI"
    );
    assert_eq!(h.conflicts()?, vec!["doc.txt".to_string()]);

    h.finish().await
}

// ── helpers ─────────────────────────────────────────────────────────────

fn conflict_copies(h: &E2E) -> anyhow::Result<Vec<std::path::PathBuf>> {
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

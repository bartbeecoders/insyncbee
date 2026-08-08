//! **Group C — updates**, **Group D — deletes**, **Group E — structure**.
//!
//! The second half of the three-way matrix: what happens to files that are
//! already tracked in `file_index` when one or both sides move underneath
//! them. Deletes are where sync tools lose data, so every delete scenario
//! also asserts that nothing vanished from *both* sides at once.

use insyncbee_e2e::{skip_unless_live, E2E};

// ── C. Update propagation ───────────────────────────────────────────────

/// **C1** — a local edit to a tracked file updates the Drive copy in place
/// (same file ID, new content) rather than creating a second file.
#[tokio::test]
async fn c1_local_edit_updates_remote_in_place() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("c1-local-edit").await?;

    h.write("doc.txt", "version one")?;
    h.sync().await?;
    let first_id = h.remote_tree().await?["doc.txt"].id.clone();

    h.write("doc.txt", "version two")?;
    let report = h.sync().await?;

    assert_eq!(report.uploaded, 1, "expected one update: {report}");
    assert_eq!(h.remote_text("doc.txt").await?, "version two");

    let tree = h.remote_tree().await?;
    assert_eq!(
        tree["doc.txt"].id, first_id,
        "the edit must replace the existing Drive file, not create a new one"
    );
    assert_eq!(tree.len(), 1, "no stray duplicates: {:?}", tree.keys());

    h.assert_converged().await?;
    h.finish().await
}

/// **C2** — an edit made on Drive comes down on the next cycle.
#[tokio::test]
async fn c2_remote_edit_downloads() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("c2-remote-edit").await?;

    h.write("doc.txt", "version one")?;
    h.sync().await?;

    h.remote_write("doc.txt", "edited on the web").await?;
    let report = h.sync().await?;

    assert_eq!(report.downloaded, 1, "expected one download: {report}");
    assert_eq!(h.read("doc.txt")?, "edited on the web");

    h.assert_mirrored().await?;
    h.assert_converged().await?;
    h.finish().await
}

/// **C3** — both sides edited between cycles is a true conflict, and under
/// `KeepBoth` both versions survive on disk.
#[tokio::test]
async fn c3_simultaneous_edit_conflicts_without_losing_either_side() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("c3-both-edit").await?;

    h.write("doc.txt", "original")?;
    h.sync().await?;

    h.write("doc.txt", "local edit")?;
    h.remote_write("doc.txt", "remote edit").await?;
    let report = h.sync().await?;

    assert_eq!(report.conflicts, 1, "expected one conflict: {report}");
    assert_eq!(h.read("doc.txt")?, "local edit", "local edit must be preserved");

    let copies = conflict_copies(&h)?;
    assert_eq!(copies.len(), 1, "expected one conflicted copy, got {copies:?}");
    assert_eq!(std::fs::read_to_string(&copies[0])?, "remote edit");

    h.finish().await
}

// ── D. Deletion propagation ─────────────────────────────────────────────

/// **D1** — deleting locally trashes the Drive copy.
#[tokio::test]
async fn d1_local_delete_trashes_remote() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("d1-local-delete").await?;

    h.write("gone.txt", "delete me")?;
    h.write("kept.txt", "keep me")?;
    h.sync().await?;

    h.remove("gone.txt")?;
    let report = h.sync().await?;

    assert_eq!(report.deleted, 1, "expected one delete: {report}");
    assert!(!h.remote_exists("gone.txt").await?, "remote copy still present");
    assert!(h.remote_exists("kept.txt").await?, "unrelated file was removed");

    assert!(
        !h.index()?.contains_key("gone.txt"),
        "index entry should be dropped with the file"
    );

    h.assert_converged().await?;
    h.finish().await
}

/// **D2** — trashing on Drive removes the local copy.
#[tokio::test]
async fn d2_remote_delete_removes_local() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("d2-remote-delete").await?;

    h.write("gone.txt", "delete me")?;
    h.write("kept.txt", "keep me")?;
    h.sync().await?;

    h.remote_trash("gone.txt").await?;
    let report = h.sync().await?;

    assert_eq!(report.deleted, 1, "expected one delete: {report}");
    assert!(!h.exists("gone.txt"), "local copy still present");
    assert!(h.exists("kept.txt"), "unrelated file was removed");

    h.assert_converged().await?;
    h.finish().await
}

/// **D3** — deleted locally *and* edited remotely. The delete must not win
/// silently: the remote edit is content the user has never seen locally, so
/// losing it would be exactly the Insync failure mode this project exists
/// to avoid.
#[tokio::test]
async fn d3_local_delete_versus_remote_edit_preserves_the_remote_edit() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("d3-delete-vs-edit").await?;

    h.write("contested.txt", "original")?;
    h.sync().await?;

    h.remove("contested.txt")?;
    h.remote_write("contested.txt", "important remote edit").await?;

    let report = h.sync().await?;
    assert_eq!(report.conflicts, 1, "expected a conflict, not a silent delete: {report}");

    // Under KeepBoth the remote version is restored under a conflict name.
    let copies = conflict_copies(&h)?;
    assert_eq!(copies.len(), 1, "remote edit was not preserved locally: {copies:?}");
    assert_eq!(std::fs::read_to_string(&copies[0])?, "important remote edit");

    h.finish().await
}

/// **D4** — trashed remotely *and* edited locally. Mirror image of D3: the
/// local edit must survive on disk.
#[tokio::test]
async fn d4_remote_delete_versus_local_edit_preserves_the_local_edit() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("d4-edit-vs-delete").await?;

    h.write("contested.txt", "original")?;
    h.sync().await?;

    h.write("contested.txt", "important local edit")?;
    h.remote_trash("contested.txt").await?;

    let report = h.sync().await?;
    assert_eq!(report.conflicts, 1, "expected a conflict, not a silent delete: {report}");
    assert_eq!(
        h.read("contested.txt")?,
        "important local edit",
        "the local edit must not be destroyed by the remote delete"
    );

    h.finish().await
}

/// **D5** — deleting a folder locally cascades to Drive and, crucially, the
/// children do not come back on the next cycle. This is the regression that
/// commit f489cb3 fixed: unindexed folders were re-created forever.
#[tokio::test]
async fn d5_local_folder_delete_cascades_and_stays_deleted() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("d5-folder-delete").await?;

    h.write("project/a.txt", "one")?;
    h.write("project/sub/b.txt", "two")?;
    h.sync().await?;
    assert!(h.remote_exists("project/sub/b.txt").await?, "setup did not sync");

    h.remove("project")?;
    let report = h.sync().await?;
    assert_eq!(report.errors, 0, "cascade produced errors: {report}");

    let remote = h.remote_paths().await?;
    assert!(
        remote.is_empty(),
        "folder delete did not cascade on Drive: {remote:?}"
    );

    // The real regression: a second cycle must not resurrect anything.
    h.assert_converged().await?;
    assert!(!h.exists("project"), "the folder came back locally");
    h.finish().await
}

/// **D6** — deleting a remote folder cascades locally and stays deleted.
#[tokio::test]
async fn d6_remote_folder_delete_cascades_locally() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("d6-remote-folder-delete").await?;

    h.write("project/a.txt", "one")?;
    h.write("project/sub/b.txt", "two")?;
    h.sync().await?;

    h.remote_trash("project").await?;
    let report = h.sync().await?;
    assert_eq!(report.errors, 0, "cascade produced errors: {report}");

    assert!(!h.exists("project"), "local folder survived the remote delete");
    h.assert_converged().await?;
    h.finish().await
}

// ── E. Structure and nesting ────────────────────────────────────────────

/// **E1** — a deep tree created in one go lands in the right parents rather
/// than being flattened into the sync root (regression e3f605a: the parent
/// ID was snapshotted before the parent folder existed).
#[tokio::test]
async fn e1_deep_new_tree_lands_in_the_right_parents() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("e1-deep-tree").await?;

    h.write("a/b/c/deep.txt", "bottom")?;
    h.write("a/b/mid.txt", "middle")?;
    h.write("a/top.txt", "top")?;

    let report = h.sync().await?;
    assert_eq!(report.errors, 0, "{report}");

    let remote = h.remote_paths().await?;
    for expected in ["a", "a/b", "a/b/c", "a/b/c/deep.txt", "a/b/mid.txt", "a/top.txt"] {
        assert!(
            remote.contains(&expected.to_string()),
            "'{expected}' missing from remote tree {remote:?}"
        );
    }

    h.assert_mirrored().await?;
    h.assert_converged().await?;
    h.finish().await
}

/// **E2** — a file added to an *already synced* folder gets uploaded into
/// that folder (regression b596bc0).
#[tokio::test]
async fn e2_file_added_to_existing_folder_uploads_into_it() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("e2-add-to-folder").await?;

    h.write("folder/first.txt", "one")?;
    h.sync().await?;

    h.write("folder/second.txt", "two")?;
    let report = h.sync().await?;
    assert_eq!(report.uploaded, 1, "{report}");

    let remote = h.remote_paths().await?;
    assert!(
        remote.contains(&"folder/second.txt".to_string()),
        "new file did not land inside the folder: {remote:?}"
    );

    h.assert_mirrored().await?;
    h.assert_converged().await?;
    h.finish().await
}

/// **E3** — renaming a folder locally. InSyncBee has no move detection yet,
/// so this becomes delete-then-create; what matters is that the *content*
/// survives the round trip and the tree converges under the new name.
#[tokio::test]
async fn e3_local_folder_rename_preserves_content() -> anyhow::Result<()> {
    skip_unless_live!();
    let h = E2E::setup("e3-rename").await?;

    h.write("before/keep.txt", "precious")?;
    h.sync().await?;

    std::fs::rename(h.local_path("before"), h.local_path("after"))?;
    let report = h.sync().await?;
    assert_eq!(report.errors, 0, "{report}");

    assert_eq!(
        h.read("after/keep.txt")?,
        "precious",
        "content lost across the rename"
    );
    let remote = h.remote_paths().await?;
    assert!(
        remote.contains(&"after/keep.txt".to_string()),
        "renamed path missing on Drive: {remote:?}"
    );
    assert!(
        !remote.contains(&"before/keep.txt".to_string()),
        "old path still on Drive: {remote:?}"
    );

    h.assert_mirrored().await?;
    h.assert_converged().await?;
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

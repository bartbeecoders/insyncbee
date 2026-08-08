# InSyncBee — test scenario catalogue

The complete list of scenarios InSyncBee is tested against, grouped by the
behaviour they protect. This is the checklist; `tests/README.md` is the
strategy that explains *why* each layer exists.

**Status legend**

| | Meaning |
|---|---|
| 🟢 | Automated **live** against real Google Drive (`tests/e2e`) |
| 🔵 | Automated against the **fake** backend (`crates/insyncbee-core/tests`) |
| 🟠 | **Manual** — documented procedure, not automated |
| 🔴 | **Gap** — known limitation or unimplemented behaviour |

Live scenarios run against `bart.roelant@gmail.com`, local base
`/home/bart/Documents/Drive`, remote base `/InSyncBee`. Each one gets a
private sandbox under `.insyncbee-e2e/` on both sides — see
[Sandboxing](#sandboxing) below.

```
INSYNCBEE_E2E=1 cargo test -p insyncbee-e2e -- --test-threads=3
```

---

## A — Foundation and harness integrity

Nothing below A is meaningful if A is broken, so these run first.

| ID | Scenario | Status | Test |
|---|---|---|---|
| A1 | The connected account is reachable and `/about` returns the expected identity | 🟢 | `a1_connected_account_is_reachable` |
| A2 | The sandbox is invisible to the user's own sync pair rooted at the same folder | 🟢 | `a2_sandbox_is_invisible_to_the_real_pair` |
| A3 | The orphan sweep reclaims sandboxes abandoned by a panicking run, without touching live ones | 🟢 | `a3_orphan_sweep_reclaims_abandoned_sandboxes` |
| A4 | A scenario's DB is a throwaway copy; the developer's real database is never written | 🟢 | structural — `E2E::setup_with` opens a `TempDir` DB |

## B — First sync: the `(local, remote, base)` matrix

The arms where no base entry exists yet. B3 is the single most common
real-world first sync and the one that hid a production bug for months.

| ID | Scenario | Status | Test |
|---|---|---|---|
| B1 | `(local, ·, ·)` new local file uploads with correct bytes | 🟢 | `b1_new_local_file_uploads` |
| B2 | `(·, remote, ·)` new remote file downloads with correct bytes | 🟢 | `b2_new_remote_file_downloads` |
| B3 | `(local, remote, ·)` **identical** content is adopted silently — no conflict, no transfer | 🟢 🔵 | `b3_identical_content_on_both_sides_is_adopted_silently`, `identical_content_on_both_sides_is_adopted_not_conflicted` |
| B4 | `(local, remote, ·)` **divergent** content conflicts, both versions preserved | 🟢 | `b4_divergent_content_on_both_sides_conflicts` |
| B5 | New local directory creates the remote folder | 🟢 | `b5_new_local_dir_creates_remote_dir` |
| B6 | New remote folder creates the local directory | 🟢 | `b6_new_remote_dir_creates_local_dir` |
| B7 | A mixed tree converges in exactly one cycle | 🟢 | `b7_mixed_tree_converges_in_one_cycle` |
| B8 | `(·, ·, base)` deleted from both sides is a no-op | 🔵 | `compute_actions_in_base_but_deleted_both_sides_is_skipped` |

## C — Update propagation

| ID | Scenario | Status | Test |
|---|---|---|---|
| C1 | Local edit updates the remote **in place** (same file ID, no duplicate) | 🟢 | `c1_local_edit_updates_remote_in_place` |
| C2 | Remote edit downloads on the next cycle | 🟢 | `c2_remote_edit_downloads` |
| C3 | Both sides edited → conflict, neither version lost | 🟢 | `c3_simultaneous_edit_conflicts_without_losing_either_side` |
| C4 | Unchanged file on both sides is skipped | 🟢 🔵 | every `assert_converged`, `no_changes_yields_empty_report` |

## D — Deletion propagation

Deletion is where sync tools lose data. Every scenario here also asserts
that nothing vanished from *both* sides at once.

| ID | Scenario | Status | Test |
|---|---|---|---|
| D1 | Local delete trashes the remote; unrelated files untouched | 🟢 🔵 | `d1_local_delete_trashes_remote` |
| D2 | Remote trash removes the local copy; unrelated files untouched | 🟢 🔵 | `d2_remote_delete_removes_local` |
| D3 | Local delete vs. remote edit → conflict; **the remote edit survives** | 🟢 | `d3_local_delete_versus_remote_edit_preserves_the_remote_edit` |
| D4 | Remote delete vs. local edit → conflict; **the local edit survives** | 🟢 | `d4_remote_delete_versus_local_edit_preserves_the_local_edit` |
| D5 | Local folder delete cascades remotely and children stay deleted | 🟢 🔵 | `d5_local_folder_delete_cascades_and_stays_deleted` |
| D6 | Remote folder delete cascades locally and stays deleted | 🟢 🔵 | `d6_remote_folder_delete_cascades_locally` |
| D7 | Index rows are dropped alongside the file | 🟢 | asserted inside D1 |
| D8 | D3/D4 conflicts remain **pending** across cycles — no base state exists that wouldn't finish the deletion | 🔴 | see [Known gaps](#known-gaps) |

## E — Structure and nesting

| ID | Scenario | Status | Test |
|---|---|---|---|
| E1 | A deep new tree lands in the right parents, not flattened into the root | 🟢 🔵 | `e1_deep_new_tree_lands_in_the_right_parents` |
| E2 | A file added to an already-synced folder uploads into that folder | 🟢 🔵 | `e2_file_added_to_existing_folder_uploads_into_it` |
| E3 | Local folder rename preserves content under the new name | 🟢 | `e3_local_folder_rename_preserves_content` |
| E4 | Move detection (rename as a move, not delete+re-upload) | 🔴 | not implemented — see DESIGN.md §4.1 |

## F — Conflict policies

One scenario per `ConflictPolicy` variant, each arranging the same real
both-sides-edited conflict.

| ID | Scenario | Status | Test |
|---|---|---|---|
| F1 | `KeepBoth` writes `doc (conflict <timestamp>).txt`; neither version overwritten | 🟢 🔵 | `f1_keep_both_writes_a_timestamped_copy` |
| F2 | `PreferLocal` overwrites the remote, leaves no conflicted copy | 🟢 🔵 | `f2_prefer_local_overwrites_remote` |
| F3 | `PreferRemote` overwrites the local | 🟢 🔵 | `f3_prefer_remote_overwrites_local` |
| F4a | `NewestWins` with a newer local file → local wins | 🟢 | `f4a_newest_wins_picks_the_newer_local_file` |
| F4b | `NewestWins` with a newer remote file → remote wins | 🟢 | `f4b_newest_wins_picks_the_newer_remote_file` |
| F5 | `Ask` touches neither side and queues `FileState::Conflict` for the UI | 🟢 🔵 | `f5_ask_defers_without_touching_either_side` |
| F6 | **Resolution converges** — `KeepBoth` does not spawn a new copy every cycle | 🟢 🔵 | `keep_both_does_not_spawn_a_new_copy_on_every_cycle`, `prefer_local_converges_after_resolving` |
| F7 | Conflict resolution UI (side-by-side diff, batch resolve) | 🔴 | UI not built — DESIGN.md §4.3 |

## G — Sync modes

One-way modes are a safety promise. These assert the **negative** half —
what must never happen — because that's the half users discover too late.

| ID | Scenario | Status | Test |
|---|---|---|---|
| G1a | `local-to-cloud` pushes local changes | 🟢 | `g1a_local_to_cloud_pushes_local_changes` |
| G1b | `local-to-cloud` never pulls new remote files | 🟢 🔵 | `g1b_local_to_cloud_ignores_new_remote_files` |
| G1c | `local-to-cloud` never deletes a local file when the remote is trashed | 🟢 | `g1c_local_to_cloud_never_deletes_local_files` |
| G2a | `cloud-to-local` pulls remote changes | 🟢 | `g2a_cloud_to_local_pulls_remote_changes` |
| G2b | `cloud-to-local` never uploads local files | 🟢 🔵 | `g2b_cloud_to_local_never_uploads_local_files` |
| G2c | `cloud-to-local` never trashes a remote file when the local is deleted | 🟢 🔵 | `g2c_cloud_to_local_never_deletes_remote_files` |
| G3 | `two-way` full bidirectional behaviour | 🟢 | groups B–F |

## H — Content and naming edge cases

Everywhere Drive and a POSIX filesystem disagree, or the transfer path
branches. A fake backend cannot catch any of this.

| ID | Scenario | Status | Test |
|---|---|---|---|
| H1 | File >4 MiB takes the **resumable** upload path and round-trips intact | 🟢 | `h1_large_file_uses_resumable_upload_and_round_trips` |
| H2 | Binary download is byte-identical | 🟢 | `h2_binary_download_is_byte_identical` |
| H3 | Zero-byte file round-trips and stays zero bytes | 🟢 | `h3_zero_byte_file_round_trips` |
| H4 | Spaces, Unicode, emoji, apostrophes, `&`, `#` in names | 🟢 | `h4_unicode_and_punctuated_names_round_trip` |
| H5 | Deep path with a 120-character component | 🟢 | `h5_deep_path_with_long_names_round_trips` |
| H6 | Two Drive files with the same name in one folder do not break the cycle | 🟢 | `h6_duplicate_remote_names_do_not_break_the_cycle` |
| H7 | Local dotfiles are never uploaded | 🟢 | `h7_local_dotfiles_are_never_uploaded` |
| H8 | Remote dotfiles are ignored, **never downloaded and never trashed** | 🟢 🔵 | `h8_remote_dotfiles_are_ignored_not_deleted`, `hidden_remote_entries_are_ignored_never_deleted` |
| H9 | Duplicate-name **disambiguation** (both versions kept locally) | 🔴 | see [Known gaps](#known-gaps) |
| H10 | Google-native docs (Docs/Sheets/Slides) export to Office formats | 🔴 | not implemented — DESIGN.md §4.4 |
| H11 | Case-only rename on a case-insensitive filesystem (macOS/Windows) | 🟠 | needs a non-Linux host |
| H12 | File modified while it is being uploaded | 🔴 | no mid-upload change detection |
| H13 | Symlinks, FIFOs, and other non-regular files | 🔴 | untested; scanner treats them as regular files |

## I — Client-side encryption

The promise is narrow and absolute: Google never holds plaintext, the user
never holds ciphertext.

| ID | Scenario | Status | Test |
|---|---|---|---|
| I1 | Encrypted upload puts ciphertext on Drive; local stays plaintext; the plaintext marker appears nowhere in the uploaded bytes | 🟢 | `i1_encrypted_upload_puts_ciphertext_on_drive` |
| I2 | Encrypted download restores exact plaintext, across multiple cipher chunks | 🟢 🔵 | `i2_encrypted_download_restores_exact_plaintext` |
| I3 | A **locked** encrypted pair refuses to sync and writes nothing to Drive | 🟢 🔵 | `i3_locked_pair_refuses_to_upload_plaintext` |
| I4 | Encrypted first-sync collision defers to conflict resolution by design | 🟢 | `i4_encrypted_first_sync_defers_to_conflict_resolution` |
| I5 | Wrong passphrase is rejected by the verifier without touching files | 🔵 | `crates/insyncbee-core/tests/encryption.rs` |
| I6 | Key survives an OS keyring round-trip | 🟠 | keyring access is environment-specific |

## J — Auth and resilience

| ID | Scenario | Status | Test |
|---|---|---|---|
| J1 | An expired access token is refreshed transparently mid-sync and persisted | 🟢 | `j1_expired_access_token_is_refreshed_transparently` |
| J2 | A revoked grant fails cleanly and deletes nothing on either side | 🟢 | `j2_revoked_grant_fails_without_destroying_anything` |
| J3 | `--dry-run` mutates neither side and writes no index rows | 🟢 | `j3_dry_run_mutates_neither_side` |
| J4 | Rate limiting (429) / transient 5xx are retried with backoff | 🔴 | **no retry logic exists** — a 5xx aborts the upload |
| J5 | Network interruption mid-sync leaves a resumable state; the next cycle converges | 🔴 | resumable uploads are not resumed across process restarts |
| J6 | Killing the process mid-sync leaves the DB uncorrupted | 🟠 | manual; SQLite WAL makes this likely-safe but it is unverified |
| J7 | Drive quota exhausted produces a clear, actionable error | 🟠 | needs a full account |

## K — Watcher and daemon

| ID | Scenario | Status | Test |
|---|---|---|---|
| K1 | The watcher emits debounced events for creates/writes | 🔵 | `watcher_emits_events_for_writes` |
| K2 | The scanner skips dotfiles and returns relative paths | 🔵 | `scan_directory_skips_dotfiles_and_returns_relative_paths` |
| K3 | The daemon performs an initial sync for every active pair | 🟠 | manual — `insyncbee daemon` |
| K4 | Paused pairs are skipped by the poll loop | 🟠 | manual |
| K5 | A watcher event triggers a sync inside the debounce window | 🟠 | manual |
| K6 | Rename events are classified as renames, not delete+create | 🔴 | classified but unused by the engine |

## L — CLI surface

| ID | Scenario | Status | Test |
|---|---|---|---|
| L1 | `--help` / `--version` / empty-DB `list` and `status` | 🔵 | `crates/insyncbee-daemon/tests/cli.rs` |
| L2 | `add` → `list` → `pause` → `resume` → `remove` round-trip | 🔵 | `add_then_list_then_pause_then_remove` |
| L3 | Commands run against an isolated `XDG_DATA_HOME` | 🔵 | `cmd()` helper |
| L4 | Logs go to stdout, mixing with machine-readable command output | 🔴 | wart — see [Known gaps](#known-gaps) |

## M — Portal and GUI

| ID | Scenario | Status | Test |
|---|---|---|---|
| M1 | Portal renders hero, download cards, correct download URL shape | 🔵 | `insyncbee.portal/tests/e2e/smoke.spec.ts` |
| M2 | Tauri GUI smoke (add pair, unlock encrypted pair, resolve conflict) | 🔴 | no GUI test layer yet |

## N — Cross-cutting invariants

Asserted *inside* the scenarios above rather than as standalone tests. These
are the properties that matter more than any individual scenario.

| ID | Invariant | Where |
|---|---|---|
| N1 | **Convergence** — a repeat sync with no interleaved change does nothing at all | `E2E::assert_converged`, used by ~20 scenarios |
| N2 | **Mirror** — every file on one side exists on the other with identical content | `E2E::assert_mirrored` |
| N3 | **No silent loss** — a path never disappears from both sides at once | `E2E::assert_no_loss`, plus explicit assertions in D1–D6 |
| N4 | **No plaintext leak** — encrypted pairs never write plaintext to Drive | I1, I3 |
| N5 | **Isolation** — tests never touch the user's real files, pairs, or database | A2, A3, A4 |

---

## Known gaps

Behaviours deliberately not fixed as part of the test work, recorded so
they are decisions rather than surprises.

**D8 — delete-vs-edit conflicts stay pending.** When one side deleted a file
and the other edited it, `KeepBoth` preserves the surviving version but the
conflict re-reports on every cycle. This is intentional: there is no base
state that doesn't tell the next cycle to finish the deletion and destroy
the edit. `ConflictKind::both_sides_present` gates the write-back for
exactly this reason. The proper fix is resurrecting the surviving version
as a fresh upload/download, which is a design change, not a test change.

**H9 — duplicate remote names.** Drive allows two files with the same name
in one folder; the engine keys its remote tree by path, so one shadows the
other and is never downloaded. H6 pins that this doesn't *break* anything;
it does not claim both versions are preserved.

**J4/J5 — no retry or resume.** A 429 or 5xx aborts the transfer, and a
resumable upload session is not resumed across restarts. Worth fixing before
anyone syncs large media libraries.

**L4 — logs on stdout.** The daemon initialises `tracing` with a stdout
layer, so `insyncbee list | head -1` returns a log banner rather than data.
This already broke `add_then_list_then_pause_then_remove`, which parsed
`lines().next()`. The test now selects its row by content, but the real fix
is routing logs to stderr.

---

## Sandboxing

Live tests write to a real Drive and a real home directory, so isolation is
enforced structurally, not by convention:

* **Local** — `/home/bart/Documents/Drive/.insyncbee-e2e/e2e-<epoch>-<n>-<slug>/`.
  The leading dot is load-bearing: `watcher::scan_directory` skips
  dot-prefixed entries, so the user's own "Drive" sync pair rooted at the
  same folder cannot see, upload, or delete anything a test creates. A2
  asserts this rather than assuming it.
* **Remote** — `/InSyncBee/.insyncbee-e2e/e2e-<epoch>-<n>-<slug>/`, created
  fresh per scenario and trashed by `E2E::finish()`.
* **Database** — a throwaway SQLite file in a `TempDir`. The account row is
  *copied* out of the real database so the existing OAuth grant is reused;
  token refreshes land in the copy.
* **Orphans** — a panicking test can't run cleanup, so setup trashes any
  remote sandbox older than two hours (A3 verifies the sweep).

## Environment

| Variable | Default | Purpose |
|---|---|---|
| `INSYNCBEE_E2E` | *unset* | Set to `1` to enable live tests; otherwise every scenario skips |
| `INSYNCBEE_CLIENT_ID` / `_SECRET` | *unset* | OAuth app credentials (required) |
| `INSYNCBEE_E2E_ACCOUNT` | `bart.roelant@gmail.com` | Which connected account to borrow |
| `INSYNCBEE_E2E_LOCAL` | `/home/bart/Documents/Drive` | Local base folder |
| `INSYNCBEE_E2E_REMOTE` | `/InSyncBee` | Remote base folder |

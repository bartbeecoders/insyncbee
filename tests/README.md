# InSyncBee — testing strategy

This file is the source of truth for how (and why) InSyncBee is tested. The
zero-data-loss design goal is unforgiving: a sync bug can lose user files
silently and weeks later. Tests exist to make that effectively impossible.

`SCENARIOS.md` is the companion checklist — every scenario, its status, and
the known gaps. This file explains the shape of the suite and the rules for
adding to it.

Every test added to the project should fit into exactly one of the layers
described below — if it doesn't, the layer needs a new section before the
test does.

## Test pyramid

```
          ┌────────────────────────────────┐  ◄─ opt-in, real Google Drive,
          │  Live E2E        ~49 tests     │     real account, real bytes.
          │  tests/e2e                     │     Catches what a fake cannot.
          └────────────────────────────────┘
        ┌────────────────────────────────────┐
        │  E2E / Smoke      ~10 tests        │  ◄─ CLI smoke, portal Playwright
        └────────────────────────────────────┘
      ┌────────────────────────────────────────┐
      │  Integration      ~35 tests            │  ◄─ real I/O, fake Drive,
      │  sync_engine + watcher + migrations    │     in-memory SQLite, tempdirs
      └────────────────────────────────────────┘
    ┌────────────────────────────────────────────┐
    │  Unit             ~50 tests                │  ◄─ pure functions, parsers,
    │  helpers, models, property tests           │     property tests
    └────────────────────────────────────────────┘
```

## The central strategy: four rules

Everything below follows from four decisions. If you only read one section,
read this one.

**1. Test the invariants, not just the transitions.** Asserting that "a
local edit uploads" is necessary but weak — it passes for an engine that
uploads the same file on every cycle forever. The properties that actually
matter are cross-cutting, so they're helpers applied inside many scenarios
rather than tests of their own:

* **Convergence** (`assert_converged`) — a repeat sync with no interleaved
  change must do *nothing*. This is the single highest-yield assertion in
  the suite; it has caught more real bugs than any individual scenario.
* **Mirror** (`assert_mirrored`) — every file on one side exists on the
  other with identical content.
* **No silent loss** (`assert_no_loss`) — a path never disappears from both
  sides at once.

**2. Assert the negative half of every safety promise.** A user who picks
`local-to-cloud` is promising themselves that nothing in the cloud can
delete their local files. That promise is only tested by asserting what must
*not* happen (G1c, G2c, I3, J2). Positive-path tests pass on an engine with
no safety logic at all.

**3. A fake may be simpler than the real service, but never *different* in
a way the code under test can observe.** The `FakeDriveClient` once defined
its `md5Checksum` as a truncated blake3 — "close enough, we only need
stability". That one shortcut hid a production bug for the entire life of
the fake (see *Lessons* below). Fakes model the contract; where the contract
is a specific hash, format, or error code, the fake reproduces it exactly.

**4. Some things can only be tested against the real thing.** Hence the live
layer. Everything in `SCENARIOS.md` group H exists because Drive and POSIX
disagree somewhere: MD5 semantics, folder-trash cascades, the resumable
upload threshold, duplicate names, Unicode. No amount of fake-backend
testing reaches it.

## Layer 1 — Unit tests (`#[cfg(test)] mod tests` in source files)

Pure functions, parsers, and small data shapes. No I/O, no async, no fixtures.

* **`db::models`** — `FromStr`/`Display` round-trips for `SyncMode`,
  `ConflictPolicy`, `SyncPairStatus`, `FileState`. These enums are persisted
  as strings; if a round-trip ever breaks we corrupt the database.
* **`drive::DriveFile`** — `is_folder`, `is_google_doc`, `size_bytes` parsing.
* **`watcher::hash_file`** — blake3 invariants (deterministic, identical
  bytes → identical hash, single-bit change → different hash). Property test.

### The two hash spaces

`watcher` exposes two hashes and they are **not** interchangeable:

* `hash_file` (blake3) is the *local* content identity, stored in
  `file_index.local_hash` and compared local-to-local across cycles.
* `md5_file` is the only hash comparable to Drive's `md5Checksum`, and is
  used at exactly one site: the first sync of a file that exists on both
  sides with no base entry to route the comparison through.

Everywhere else the engine compares local-to-local or remote-to-remote and
never mixes the spaces. Mixing them is what B3 exists to catch.

## Layer 2 — Integration tests (`crates/insyncbee-core/tests/*.rs`)

These exercise real subsystems against a fake Drive backend, an in-memory
SQLite, and real files in a tempdir. Each test is hermetic: no shared state,
no network. This is the layer CI runs on every PR, so any bug the live layer
finds should be **back-ported here** with a fake-backed regression test.

* **`db_models.rs`** — open in-memory DB, run migrations, exercise CRUD on
  every model, exercise foreign-key cascades (`sync_pairs` → `file_index` /
  `change_log`), exercise `UNIQUE(sync_pair_id, relative_path)`.
* **`sync_engine.rs`** — drives `SyncEngine` against `FakeDriveClient` for
  every transition in the three-way `(local, remote, base)` matrix. Asserts
  on `SyncReport` *and* on the resulting filesystem + DB state.
* **`conflict_policies.rs`** — one test per `ConflictPolicy` variant, each
  arranging a real conflict and asserting the policy's outcome, plus that
  resolution **converges**.
* **`watcher_integration.rs`** — start a real `FileWatcher` on a tempdir,
  perform real fs ops, assert events arrive within the debounce window.
* **`hash_property.rs`** — proptest invariants for `blake3` hashing of
  arbitrary byte vectors.

### The `FakeDriveClient`

Lives in `crates/insyncbee-core/tests/common/mod.rs`. It is an in-memory
implementation of the `DriveClient` trait that:

* Stores `DriveFile` records keyed by `id`, with `parents` and contents.
* Mints synthetic IDs for new uploads/folders.
* Computes a **real MD5** `md5_checksum`, so the engine's content-equality
  checks behave exactly as in production (see rule 3).
* Records call counts so tests can assert on call patterns.

When you change the `DriveClient` trait, you change `FakeDriveClient`. CI
will catch any drift.

## Layer 3 — CLI / portal smoke

* **`crates/insyncbee-daemon/tests/cli.rs`** — `assert_cmd`-based tests of
  the CLI surface: `--help`, `--version`, `list`, `status`, exit codes.
  These run against an isolated `XDG_DATA_HOME` so they don't touch the
  developer's real DB. Note that the binary logs to stdout, so never parse
  CLI output positionally — select lines by content.
* **`insyncbee.portal/tests/e2e/smoke.spec.ts`** — Playwright loads the
  built portal, asserts the hero, the download cards, and the recommended
  download link's URL shape. This catches Vite build regressions and
  download-page wiring breaks (which previously shipped the wrong filename
  in v0.1.0–v0.1.4).

## Layer 4 — Live E2E (`tests/e2e`)

Opt-in, runs the real `SyncEngine` against the real Google Drive API using
the developer's connected account.

```
INSYNCBEE_E2E=1 cargo test -p insyncbee-e2e -- --test-threads=3
```

Without `INSYNCBEE_E2E=1` every scenario returns immediately, so
`cargo test --workspace` stays green on a machine with no Google account.

**Isolation is structural, not conventional.** Each scenario gets a private
sandbox on both sides, a throwaway database, and a cleanup path; the local
sandbox is dot-prefixed so the user's own sync pair cannot see it. The full
model, and the orphan sweep that covers panicking tests, is documented in
`SCENARIOS.md` § Sandboxing — and asserted by scenarios A2 and A3.

**Cost.** A full run is ~50 scenarios × a handful of API calls, about two
minutes wall-clock at `--test-threads=3`. Well inside Drive's quotas, but
it is not free and it is not hermetic: run it before releases and after any
change to the sync engine, not on every save.

**Why it isn't in CI.** It needs a real OAuth grant and a real Drive. The
discipline that replaces it: every bug the live layer finds gets a
fake-backed regression test in layer 2 (`identical_content_on_both_sides…`,
`hidden_remote_entries_are_ignored…`, `keep_both_does_not_spawn_a_new_copy…`
all exist for this reason).

## Lessons the live layer has already taught us

Recorded because each one is a *class* of bug, not a one-off.

**Hash-space mixing.** The engine compared a local blake3 hash to Drive's
MD5 when adopting a folder that already existed on both sides. The fake
computed "MD5" as truncated blake3, so both sides matched and the fake-backed
test passed. Against real Drive they could never match: every adopted file
was reported as a conflict, and under the default `KeepBoth` that meant a
duplicate copy of *every file* on first sync. → Fakes must not differ
observably (rule 3).

**Resolution without write-back.** Conflict handling resolved conflicts but
never recorded the outcome in `file_index`, so the next cycle recomputed the
identical conflict from the identical stale base — forever. `KeepBoth`
produced a new timestamped copy on every poll interval; the overwrite
policies re-transferred the same bytes on every cycle. Every single-cycle
assertion passed. → Test convergence, not just transitions (rule 1).

**Asymmetric ignore rules.** The local scanner skipped dot-prefixed entries;
the remote tree walker did not. A `.config` on Drive was downloaded, became
invisible to the next scan, read as `(local=false, remote=true, base=true)`
— the signature of a user delete — and was trashed on Drive on the following
cycle. Silent remote data loss. → When one side filters, both sides filter.

## What we deliberately do NOT test

* **OAuth interactive consent** — mocking a browser consent screen plus a
  loopback redirect isn't worth it for the surface involved. The *token
  refresh* path, which is what actually runs daily, is covered live (J1/J2).
* **Tauri GUI** — no GUI test layer exists yet. When one does, it becomes
  layer 3.5 here.
* **Google's own correctness** — we assert on our behaviour given Drive's
  responses, not on Drive.

## CI wiring

`.github/workflows/test.yml` runs the full unit + integration + CLI suite
on every PR and on push to `main`. Portal vitest + Playwright run in a
separate parallel job. The live E2E layer is **not** in CI (see above).
Release tagging (`release.yml`) does *not* re-run tests — it assumes `main`
is already green.

## How to add a test

1. Decide the layer. Almost always integration; reach for live E2E only when
   the behaviour depends on something real Drive does that the fake can't
   model.
2. Pick the right file (`sync_engine.rs` for sync logic,
   `conflict_policies.rs` for policy outcomes, `tests/e2e/tests/live_*.rs`
   for live). Don't create a new file for one test.
3. Write the *failing* test first. Confirm it fails. Then make it pass.
4. If it's a live scenario, end it with `assert_converged` unless the
   scenario is specifically about an unresolved state, and always call
   `finish().await` so the sandbox is reclaimed.
5. Add a row to `SCENARIOS.md`.
6. Run `cargo test --workspace` and `pnpm test` (in the portal) before
   pushing.

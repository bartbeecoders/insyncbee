# InSyncBee — testing strategy

This file is the source of truth for how (and why) InSyncBee is tested. The
zero-data-loss design goal is unforgiving: a sync bug can lose user files
silently and weeks later. Tests exist to make that effectively impossible.

Every test added to the project should fit into exactly one of the layers
described below — if it doesn't, the layer needs a new section before the
test does.

## Test pyramid

```
              ┌──────────────────────┐    ◄─ slow, brittle, full stack
              │  E2E / Smoke         │       (Playwright on portal,
              │  ~10 tests           │        CLI smoke on daemon)
              └──────────────────────┘
            ┌──────────────────────────┐
            │  Integration             │  ◄─ fast, real I/O, fakes for
            │  ~30 tests               │     external services
            │  sync_engine + watcher   │     (in-memory SQLite,
            │  + db migrations         │      tempdir, FakeDriveClient)
            └──────────────────────────┘
        ┌──────────────────────────────────┐
        │  Unit                            │  ◄─ pure functions,
        │  ~50+ tests                      │     property tests, hashing,
        │  helpers, parsers, models        │     enum round-trips
        └──────────────────────────────────┘
```

## Layer 1 — Unit tests (`#[cfg(test)] mod tests` in source files)

Pure functions, parsers, and small data shapes. No I/O, no async, no fixtures.

* **`db::models`** — `FromStr`/`Display` round-trips for `SyncMode`,
  `ConflictPolicy`, `SyncPairStatus`, `FileState`. These enums are persisted
  as strings; if a round-trip ever breaks we corrupt the database.
* **`drive::DriveFile`** — `is_folder`, `is_google_doc`, `size_bytes` parsing.
* **`watcher::hash_file`** — blake3 invariants (deterministic, identical
  bytes → identical hash, single-bit change → different hash). Property test.

## Layer 2 — Integration tests (`crates/insyncbee-core/tests/*.rs`)

These exercise real subsystems against a fake Drive backend, an in-memory
SQLite, and real files in a tempdir. Each test is hermetic: no shared state,
no network.

* **`db_models.rs`** — open in-memory DB, run migrations, exercise CRUD on
  every model, exercise foreign-key cascades (`sync_pairs` → `file_index` /
  `change_log`), exercise `UNIQUE(sync_pair_id, relative_path)`.
* **`sync_engine.rs`** — drives `SyncEngine` against `FakeDriveClient` for
  every transition in the three-way `(local, remote, base)` matrix. Asserts
  on `SyncReport` *and* on the resulting filesystem + DB state.
* **`conflict_policies.rs`** — one test per `ConflictPolicy` variant, each
  arranging a real conflict and asserting the policy's outcome.
* **`watcher_integration.rs`** — start a real `FileWatcher` on a tempdir,
  perform real fs ops, assert events arrive within the debounce window.
* **`hash_property.rs`** — proptest invariants for `blake3` hashing of
  arbitrary byte vectors.

### The `FakeDriveClient`

Lives in `crates/insyncbee-core/tests/common/mod.rs`. It is an in-memory
implementation of the `DriveClient` trait that:

* Stores `DriveFile` records keyed by `id`, with `parents` and contents.
* Mints synthetic IDs for new uploads/folders.
* Computes a real `md5_checksum` so the engine's content-equality checks
  behave exactly as in production.
* Records call counts so tests can assert on call patterns.

When you change the `DriveClient` trait, you change `FakeDriveClient`. CI
will catch any drift.

## Layer 3 — CLI / E2E smoke

* **`crates/insyncbee-daemon/tests/cli.rs`** — `assert_cmd`-based tests of
  the CLI surface: `--help`, `--version`, `list`, `status`, exit codes.
  These run against an isolated `XDG_DATA_HOME` so they don't touch the
  developer's real DB.
* **`insyncbee.portal/tests/e2e/smoke.spec.ts`** — Playwright loads the
  built portal, asserts the hero, the download cards, and the recommended
  download link's URL shape. This catches Vite build regressions and
  download-page wiring breaks (which previously shipped the wrong filename
  in v0.1.0–v0.1.4).

## What we deliberately do NOT test

* **Real Google Drive** — too slow, too rate-limited, bound to flake. The
  trait abstraction is the seam. A separate, opt-in contract test against a
  sandbox account can be added later under `tests/contract/` and gated by
  `INSYNCBEE_RUN_CONTRACT=1` + a CI secret.
* **OAuth flow** — covered by manual smoke. Mocking an interactive browser
  consent + loopback redirect isn't worth the effort given the tiny surface.
* **Tauri GUI** — no GUI exists in shipped form yet. When it does, add a
  `webdriver-tauri` smoke test layer here.

## CI wiring

`.github/workflows/test.yml` runs the full unit + integration + CLI suite
on every PR and on push to `main`. Portal vitest + Playwright run in a
separate parallel job. Release tagging (`release.yml`) does *not* re-run
tests — it assumes `main` is already green.

## How to add a test

1. Decide the layer (unit / integration / e2e) — almost always integration.
2. Pick the right file (`sync_engine.rs` for sync logic,
   `conflict_policies.rs` for policy outcomes, etc.). Don't create a new
   file for one test.
3. Write the *failing* test first. Confirm it fails. Then make it pass.
4. Run `cargo test --workspace` and `pnpm test` (in the portal) before
   pushing.

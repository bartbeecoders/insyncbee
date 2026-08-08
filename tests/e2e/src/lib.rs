//! Live end-to-end harness: drives the real [`SyncEngine`] against the real
//! Google Drive API, using the developer's already-connected account.
//!
//! # Why this exists
//!
//! `crates/insyncbee-core/tests/` proves the sync engine against a
//! `FakeDriveClient`. That catches logic bugs but is blind to everything the
//! fake gets wrong about Google: MD5 semantics, folder-trash cascades,
//! resumable-upload boundaries, name collisions, Google-native docs, token
//! refresh. This layer closes that gap.
//!
//! # Safety model
//!
//! These tests write to a real Drive and a real home directory, so isolation
//! is not optional:
//!
//! * **Local sandbox** — `<local base>/.insyncbee-e2e/e2e-<epoch>-<slug>/`.
//!   The leading dot matters: [`insyncbee_core::watcher::scan_directory`]
//!   skips dot-prefixed entries, so the user's own sync pair rooted at the
//!   same base folder never sees, uploads, or deletes anything the tests
//!   create.
//! * **Remote sandbox** — `<remote base>/.insyncbee-e2e/e2e-<epoch>-<slug>/`,
//!   created fresh per scenario and trashed by [`E2E::finish`].
//! * **Database** — a throwaway SQLite file in a `TempDir`. The account row
//!   is *copied* out of the developer's real DB so we reuse the existing
//!   OAuth grant; token refreshes land in the copy, never the original.
//! * **Orphan sweep** — a panicking test can't run its cleanup, so setup
//!   trashes any remote sandbox older than [`ORPHAN_MAX_AGE`] first.
//!
//! Nothing here ever touches the user's real sync pairs, and the only remote
//! writes happen strictly inside the per-scenario sandbox folder.
//!
//! # Running
//!
//! Live tests are opt-in and no-op without `INSYNCBEE_E2E=1`:
//!
//! ```text
//! INSYNCBEE_E2E=1 cargo test -p insyncbee-e2e -- --test-threads=4
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, ensure, Context};
use insyncbee_core::auth::{AuthManager, OAuthCredentials};
use insyncbee_core::crypto::{self, FileCipher};
use insyncbee_core::db::models::{
    Account, ConflictPolicy, FileEntry, FileState, SyncMode, SyncPair, SyncPairStatus,
};
use insyncbee_core::db::Database;
use insyncbee_core::drive::{DriveClient, DriveFile, HttpDriveClient};
use insyncbee_core::sync_engine::{SyncAction, SyncEngine, SyncReport};
use insyncbee_core::{watcher, AppPaths};
use tempfile::TempDir;

/// Directory name used for both the local and the remote sandbox root.
/// The leading dot is load-bearing locally (see the module docs).
pub const SANDBOX_DIR: &str = ".insyncbee-e2e";

/// Remote sandboxes older than this are assumed to be orphans from a
/// panicking run and get trashed on the next setup.
pub const ORPHAN_MAX_AGE_SECS: u64 = 2 * 60 * 60;

const DEFAULT_ACCOUNT: &str = "bart.roelant@gmail.com";
const DEFAULT_LOCAL_BASE: &str = "/home/bart/Documents/Drive";
const DEFAULT_REMOTE_BASE: &str = "/InSyncBee";

static SCENARIO_SEQ: AtomicU64 = AtomicU64::new(0);

/// Returns true when live tests are enabled. Every `#[tokio::test]` in this
/// crate calls [`skip_unless_live!`], which consults this.
pub fn live_enabled() -> bool {
    matches!(
        std::env::var("INSYNCBEE_E2E").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

/// Early-return from a test unless live E2E is switched on. Keeps
/// `cargo test --workspace` green on machines with no Google account.
///
/// Scenarios return `anyhow::Result<()>` so they can use `?`, so this
/// expands to an `Ok` early return.
#[macro_export]
macro_rules! skip_unless_live {
    () => {
        if !$crate::live_enabled() {
            eprintln!("skipped: set INSYNCBEE_E2E=1 to run live Drive tests");
            return Ok(());
        }
    };
}

/// Per-scenario knobs. Defaults give a two-way, keep-both, unencrypted pair.
#[derive(Debug, Clone)]
pub struct Opts {
    pub mode: SyncMode,
    pub policy: ConflictPolicy,
    /// When set, the pair is created with encryption enabled and the derived
    /// cipher is attached to every engine the harness builds.
    pub passphrase: Option<String>,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            mode: SyncMode::TwoWay,
            policy: ConflictPolicy::KeepBoth,
            passphrase: None,
        }
    }
}

impl Opts {
    pub fn mode(mut self, mode: SyncMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn policy(mut self, policy: ConflictPolicy) -> Self {
        self.policy = policy;
        self
    }

    pub fn encrypted(mut self, passphrase: &str) -> Self {
        self.passphrase = Some(passphrase.to_string());
        self
    }
}

/// One isolated live scenario: a sandbox on both sides, a throwaway DB, and
/// a sync pair wired between them.
pub struct E2E {
    pub name: String,
    pub db: Database,
    pub pair: SyncPair,
    pub drive: HttpDriveClient,
    pub local_root: PathBuf,
    pub remote_root_id: String,
    /// The shared `.insyncbee-e2e` folder that holds every scenario's
    /// sandbox. Exposed so the housekeeping scenario can verify the
    /// orphan sweep.
    pub sandbox_parent: String,
    /// The account as it was copied out of the real DB, kept so
    /// [`Self::restore_grant`] can undo a deliberate revocation.
    account: Account,
    cipher: Option<Arc<FileCipher>>,
    finished: bool,
    _tmp: TempDir,
}

impl E2E {
    /// Set up a scenario with default options.
    pub async fn setup(name: &str) -> anyhow::Result<Self> {
        Self::setup_with(name, Opts::default()).await
    }

    /// Set up a scenario, creating both sandboxes and the sync pair.
    pub async fn setup_with(name: &str, opts: Opts) -> anyhow::Result<Self> {
        let creds = OAuthCredentials::from_env().context(
            "INSYNCBEE_CLIENT_ID / INSYNCBEE_CLIENT_SECRET must be set to run live tests",
        )?;

        let email = env_or(DEFAULT_ACCOUNT, "INSYNCBEE_E2E_ACCOUNT");
        let local_base = PathBuf::from(env_or(DEFAULT_LOCAL_BASE, "INSYNCBEE_E2E_LOCAL"));
        let remote_base = env_or(DEFAULT_REMOTE_BASE, "INSYNCBEE_E2E_REMOTE");

        // Copy the connected account out of the developer's real database so
        // we reuse its OAuth grant without ever writing back to it.
        let account = load_account(&email)?;

        let tmp = TempDir::new().context("create temp dir for e2e database")?;
        let db = Database::open(&tmp.path().join("e2e.db"))?;
        db.with_conn(|conn| account.insert(conn))?;

        let auth = AuthManager::new(creds, db.clone());
        let drive = HttpDriveClient::new(auth, account.id.clone());

        // Fail fast and loudly if the grant is dead — otherwise every
        // scenario fails with a confusing per-file error instead.
        drive
            .get_about()
            .await
            .context("Drive is unreachable or the stored OAuth grant is no longer valid")?;

        let slug = slugify(name);
        let stamp = epoch_secs();
        let sandbox_name = format!("e2e-{stamp}-{}-{}", SCENARIO_SEQ.fetch_add(1, Ordering::SeqCst), slug);

        // ── remote sandbox ──────────────────────────────────────────────
        let remote_base_id = resolve_remote_path(&drive, &remote_base)
            .await
            .with_context(|| format!("resolve remote base folder '{remote_base}'"))?;
        let sandbox_parent = find_or_create_folder(&drive, &remote_base_id, SANDBOX_DIR).await?;
        sweep_orphans(&drive, &sandbox_parent).await;
        let remote_root_id = drive.create_folder(&sandbox_parent, &sandbox_name).await?.id;

        // ── local sandbox ───────────────────────────────────────────────
        ensure!(
            local_base.is_dir(),
            "local base folder does not exist: {}",
            local_base.display()
        );
        let local_root = local_base.join(SANDBOX_DIR).join(&sandbox_name);
        std::fs::create_dir_all(&local_root)
            .with_context(|| format!("create local sandbox {}", local_root.display()))?;

        // ── sync pair ───────────────────────────────────────────────────
        let (encryption_enabled, salt, verifier, cipher) = match &opts.passphrase {
            Some(pass) => {
                let salt = crypto::random_salt();
                let cipher = FileCipher::from_passphrase(pass, &salt)?;
                let verifier = cipher.make_verifier(pass.as_bytes())?;
                (true, Some(salt.to_vec()), Some(verifier), Some(Arc::new(cipher)))
            }
            None => (false, None, None, None),
        };

        let now = chrono::Utc::now().to_rfc3339();
        let pair = SyncPair {
            id: uuid::Uuid::new_v4().to_string(),
            name: format!("e2e-{slug}"),
            account_id: account.id.clone(),
            local_root: local_root.to_string_lossy().to_string(),
            remote_root_id: remote_root_id.clone(),
            remote_root_path: format!("{remote_base}/{SANDBOX_DIR}/{sandbox_name}"),
            mode: opts.mode,
            conflict_policy: opts.policy,
            status: SyncPairStatus::Active,
            poll_interval_secs: 30,
            created_at: now.clone(),
            updated_at: now,
            encryption_enabled,
            encryption_salt: salt,
            encryption_verifier: verifier,
        };
        db.with_conn(|conn| pair.insert(conn))?;

        eprintln!("[e2e:{name}] sandbox {sandbox_name} (remote {remote_root_id})");

        Ok(Self {
            name: name.to_string(),
            db,
            pair,
            drive,
            local_root,
            remote_root_id,
            sandbox_parent,
            account,
            cipher,
            finished: false,
            _tmp: tmp,
        })
    }

    // ── engine ──────────────────────────────────────────────────────────

    fn engine(&self) -> SyncEngine {
        let engine = SyncEngine::new(self.db.clone(), self.pair.clone());
        match &self.cipher {
            Some(c) => engine.with_cipher(c.clone()),
            None => engine,
        }
    }

    /// Run one full sync cycle.
    pub async fn sync(&self) -> anyhow::Result<SyncReport> {
        let report = self.engine().sync(&self.drive).await?;
        eprintln!("[e2e:{}] sync → {report}", self.name);
        Ok(report)
    }

    /// Compute actions without executing them.
    pub async fn dry_run(&self) -> anyhow::Result<(Vec<SyncAction>, SyncReport)> {
        self.engine().dry_run(&self.drive).await
    }

    /// Sync an *encrypted* pair with no cipher attached — the "user has not
    /// unlocked this pair yet" state. The engine must refuse per file rather
    /// than fall back to plaintext.
    pub async fn sync_without_cipher(&self) -> anyhow::Result<SyncReport> {
        SyncEngine::new(self.db.clone(), self.pair.clone())
            .sync(&self.drive)
            .await
    }

    // ── auth manipulation (Group J) ─────────────────────────────────────

    /// Backdate the stored access token so the next Drive call must go
    /// through the refresh path.
    pub fn expire_access_token(&self) -> anyhow::Result<()> {
        let past = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        self.db.with_conn(|conn| {
            Account::update_tokens(conn, &self.pair.account_id, "stale-access-token", &past)
        })?;
        Ok(())
    }

    /// Put the real credentials back after [`Self::revoke_grant`].
    ///
    /// Required before [`Self::finish`] in any scenario that revokes: with a
    /// dead grant the harness cannot authenticate to trash its own remote
    /// sandbox, and the cleanup failure would masquerade as a test failure.
    pub fn restore_grant(&self) -> anyhow::Result<()> {
        self.db.with_conn(|conn| {
            Account::update_credentials(
                conn,
                &self.account.id,
                &self.account.access_token,
                &self.account.refresh_token,
                &self.account.token_expiry,
                self.account.display_name.as_deref(),
            )
        })?;
        Ok(())
    }

    /// Replace both tokens with garbage, simulating a grant the user
    /// revoked from their Google account page. Only ever applied to the
    /// scenario's throwaway DB copy.
    pub fn revoke_grant(&self) -> anyhow::Result<()> {
        let past = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        self.db.with_conn(|conn| {
            Account::update_credentials(
                conn,
                &self.pair.account_id,
                "revoked-access-token",
                "revoked-refresh-token",
                &past,
                None,
            )
        })?;
        Ok(())
    }

    // ── local side ──────────────────────────────────────────────────────

    pub fn local_path(&self, rel: &str) -> PathBuf {
        self.local_root.join(rel)
    }

    /// Write a local file, creating parent directories.
    pub fn write(&self, rel: &str, contents: &str) -> anyhow::Result<()> {
        self.write_bytes(rel, contents.as_bytes())
    }

    pub fn write_bytes(&self, rel: &str, bytes: &[u8]) -> anyhow::Result<()> {
        let p = self.local_path(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, bytes).with_context(|| format!("write {}", p.display()))?;
        Ok(())
    }

    pub fn read(&self, rel: &str) -> anyhow::Result<String> {
        let p = self.local_path(rel);
        std::fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))
    }

    pub fn read_bytes(&self, rel: &str) -> anyhow::Result<Vec<u8>> {
        let p = self.local_path(rel);
        std::fs::read(&p).with_context(|| format!("read {}", p.display()))
    }

    pub fn exists(&self, rel: &str) -> bool {
        self.local_path(rel).exists()
    }

    pub fn mkdir(&self, rel: &str) -> anyhow::Result<()> {
        std::fs::create_dir_all(self.local_path(rel))?;
        Ok(())
    }

    pub fn remove(&self, rel: &str) -> anyhow::Result<()> {
        let p = self.local_path(rel);
        if p.is_dir() {
            std::fs::remove_dir_all(&p)?;
        } else {
            std::fs::remove_file(&p)?;
        }
        Ok(())
    }

    /// Set a file's mtime relative to now. Used by the newest-wins scenarios,
    /// where the winner must be unambiguous rather than a coin flip.
    pub fn touch_offset(&self, rel: &str, secs_from_now: i64) -> anyhow::Result<()> {
        let p = self.local_path(rel);
        let target = if secs_from_now >= 0 {
            SystemTime::now() + std::time::Duration::from_secs(secs_from_now as u64)
        } else {
            SystemTime::now() - std::time::Duration::from_secs((-secs_from_now) as u64)
        };
        let f = std::fs::File::options().write(true).open(&p)?;
        f.set_modified(target)?;
        Ok(())
    }

    /// Every relative path under the local root, as the sync engine sees it
    /// (so dot-files are excluded, exactly like production).
    pub fn local_tree(&self) -> anyhow::Result<BTreeMap<String, bool>> {
        Ok(watcher::scan_directory(&self.local_root)?
            .into_iter()
            .map(|f| (f.relative_path, f.is_directory))
            .collect())
    }

    // ── remote side ─────────────────────────────────────────────────────

    /// Full remote tree under the sandbox, keyed by relative path.
    pub async fn remote_tree(&self) -> anyhow::Result<BTreeMap<String, DriveFile>> {
        let mut out = BTreeMap::new();
        let mut queue = vec![(String::new(), self.remote_root_id.clone())];
        while let Some((prefix, id)) = queue.pop() {
            for f in self.drive.list_all_files(&id).await? {
                let path = if prefix.is_empty() {
                    f.name.clone()
                } else {
                    format!("{prefix}/{}", f.name)
                };
                if f.is_folder() {
                    queue.push((path.clone(), f.id.clone()));
                }
                out.insert(path, f);
            }
        }
        Ok(out)
    }

    pub async fn remote_paths(&self) -> anyhow::Result<Vec<String>> {
        Ok(self.remote_tree().await?.into_keys().collect())
    }

    pub async fn remote_exists(&self, rel: &str) -> anyhow::Result<bool> {
        Ok(self.remote_tree().await?.contains_key(rel))
    }

    async fn remote_id(&self, rel: &str) -> anyhow::Result<String> {
        self.remote_tree()
            .await?
            .get(rel)
            .map(|f| f.id.clone())
            .ok_or_else(|| anyhow!("no remote entry at '{rel}'"))
    }

    /// Create (or reuse) a remote folder chain under the sandbox root.
    pub async fn remote_mkdir(&self, rel: &str) -> anyhow::Result<String> {
        let mut current = self.remote_root_id.clone();
        for seg in rel.split('/').filter(|s| !s.is_empty()) {
            current = find_or_create_folder(&self.drive, &current, seg).await?;
        }
        Ok(current)
    }

    /// Upload raw bytes to the remote sandbox at `rel`, bypassing the sync
    /// engine — this is how scenarios simulate "somebody changed it on the
    /// web UI".
    pub async fn remote_write(&self, rel: &str, contents: &str) -> anyhow::Result<DriveFile> {
        self.remote_write_bytes(rel, contents.as_bytes()).await
    }

    pub async fn remote_write_bytes(&self, rel: &str, bytes: &[u8]) -> anyhow::Result<DriveFile> {
        let (dir, name) = split_rel(rel);
        let parent = if dir.is_empty() {
            self.remote_root_id.clone()
        } else {
            self.remote_mkdir(dir).await?
        };
        let tmp = tempfile::NamedTempFile::new()?;
        std::fs::write(tmp.path(), bytes)?;

        // Replace in place when it already exists, so callers get "edit"
        // semantics rather than a duplicate name.
        let existing = self.remote_tree().await?.get(rel).map(|f| f.id.clone());
        let file = match existing {
            Some(id) => self.drive.update_file(&id, tmp.path()).await?,
            None => self.drive.upload_file(&parent, name, tmp.path()).await?,
        };
        Ok(file)
    }

    /// Upload a *second* file with the same name in the same folder. Drive
    /// permits this; POSIX filesystems do not. Only the collision scenarios
    /// need it.
    pub async fn remote_write_duplicate(&self, rel: &str, contents: &str) -> anyhow::Result<DriveFile> {
        let (dir, name) = split_rel(rel);
        let parent = if dir.is_empty() {
            self.remote_root_id.clone()
        } else {
            self.remote_mkdir(dir).await?
        };
        let tmp = tempfile::NamedTempFile::new()?;
        std::fs::write(tmp.path(), contents.as_bytes())?;
        Ok(self.drive.upload_file(&parent, name, tmp.path()).await?)
    }

    pub async fn remote_trash(&self, rel: &str) -> anyhow::Result<()> {
        let id = self.remote_id(rel).await?;
        self.drive.trash_file(&id).await
    }

    /// Raw remote bytes — ciphertext for encrypted pairs.
    pub async fn remote_bytes(&self, rel: &str) -> anyhow::Result<Vec<u8>> {
        let id = self.remote_id(rel).await?;
        let tmp = tempfile::NamedTempFile::new()?;
        self.drive.download_file(&id, tmp.path()).await?;
        Ok(std::fs::read(tmp.path())?)
    }

    /// Remote bytes as the user would see them: decrypted when the pair is
    /// encrypted, raw otherwise.
    pub async fn remote_plaintext(&self, rel: &str) -> anyhow::Result<Vec<u8>> {
        let raw = self.remote_bytes(rel).await?;
        match &self.cipher {
            None => Ok(raw),
            Some(cipher) => {
                let src = tempfile::NamedTempFile::new()?;
                let dst = tempfile::NamedTempFile::new()?;
                std::fs::write(src.path(), &raw)?;
                cipher.decrypt_file(src.path(), dst.path()).await?;
                Ok(std::fs::read(dst.path())?)
            }
        }
    }

    pub async fn remote_text(&self, rel: &str) -> anyhow::Result<String> {
        Ok(String::from_utf8(self.remote_plaintext(rel).await?)?)
    }

    // ── index ───────────────────────────────────────────────────────────

    pub fn index(&self) -> anyhow::Result<BTreeMap<String, FileEntry>> {
        Ok(self
            .db
            .with_conn(|conn| FileEntry::list_by_sync_pair(conn, &self.pair.id))?
            .into_iter()
            .map(|e| (e.relative_path.clone(), e))
            .collect())
    }

    pub fn index_state(&self, rel: &str) -> anyhow::Result<Option<FileState>> {
        Ok(self.index()?.get(rel).map(|e| e.state.clone()))
    }

    pub fn conflicts(&self) -> anyhow::Result<Vec<String>> {
        Ok(self
            .db
            .with_conn(|conn| {
                FileEntry::list_by_state(conn, &self.pair.id, &FileState::Conflict)
            })?
            .into_iter()
            .map(|e| e.relative_path)
            .collect())
    }

    // ── invariants ──────────────────────────────────────────────────────

    /// **Convergence.** A second sync with no interleaved change must be a
    /// complete no-op. A engine that keeps re-uploading or re-downloading
    /// the same file passes every single-cycle assertion and still burns the
    /// user's bandwidth forever, so every scenario ends with this.
    pub async fn assert_converged(&self) -> anyhow::Result<()> {
        let r = self.sync().await?;
        ensure!(
            r.uploaded == 0 && r.downloaded == 0 && r.deleted == 0 && r.conflicts == 0 && r.errors == 0,
            "not converged — a repeat sync still did work: {r}"
        );
        Ok(())
    }

    /// **Mirror.** Every file present on one side is present on the other
    /// with identical content. Only meaningful for two-way pairs that have
    /// finished syncing.
    pub async fn assert_mirrored(&self) -> anyhow::Result<()> {
        let local = self.local_tree()?;
        let remote = self.remote_tree().await?;

        for (path, is_dir) in &local {
            let r = remote
                .get(path)
                .ok_or_else(|| anyhow!("local '{path}' has no remote counterpart"))?;
            ensure!(
                r.is_folder() == *is_dir,
                "'{path}' is a {} locally but a {} remotely",
                if *is_dir { "directory" } else { "file" },
                if r.is_folder() { "directory" } else { "file" }
            );
            if !is_dir {
                let want = self.read_bytes(path)?;
                let got = self.remote_plaintext(path).await?;
                ensure!(
                    want == got,
                    "content mismatch at '{path}': {} local bytes vs {} remote bytes",
                    want.len(),
                    got.len()
                );
            }
        }

        for path in remote.keys() {
            ensure!(
                local.contains_key(path),
                "remote '{path}' has no local counterpart"
            );
        }
        Ok(())
    }

    /// **No silent loss.** Every path in `expected` must still exist on at
    /// least one side. Scenarios that delete on purpose list only what they
    /// expect to survive.
    pub async fn assert_no_loss(&self, expected: &[&str]) -> anyhow::Result<()> {
        let local = self.local_tree()?;
        let remote = self.remote_tree().await?;
        for path in expected {
            ensure!(
                local.contains_key(*path) || remote.contains_key(*path),
                "DATA LOSS: '{path}' vanished from both sides"
            );
        }
        Ok(())
    }

    // ── teardown ────────────────────────────────────────────────────────

    /// Trash the remote sandbox and delete the local one. Call at the end of
    /// every scenario; [`Drop`] only cleans the local side (it cannot await).
    pub async fn finish(mut self) -> anyhow::Result<()> {
        self.finished = true;
        let remote = self.drive.trash_file(&self.remote_root_id).await;
        let local = std::fs::remove_dir_all(&self.local_root);
        remote.context("trash remote sandbox")?;
        local.context("remove local sandbox")?;
        Ok(())
    }
}

impl Drop for E2E {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        // A panicking test lands here. Local cleanup is synchronous so we can
        // still do it; the remote sandbox is left for the orphan sweep, and
        // we say so rather than failing silently.
        let _ = std::fs::remove_dir_all(&self.local_root);
        eprintln!(
            "[e2e:{}] scenario did not call finish() — remote sandbox {} left for the orphan sweep",
            self.name, self.remote_root_id
        );
    }
}

// ── module-level helpers ────────────────────────────────────────────────

fn env_or(default: &str, key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn slugify(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_lowercase()
}

/// Split `a/b/c.txt` into `("a/b", "c.txt")`.
fn split_rel(rel: &str) -> (&str, &str) {
    match rel.rsplit_once('/') {
        Some((dir, name)) => (dir, name),
        None => ("", rel),
    }
}

/// Read the connected account out of the developer's real InSyncBee database.
/// We only read; the row is then copied into the scenario's throwaway DB.
fn load_account(email: &str) -> anyhow::Result<Account> {
    let paths = AppPaths::new()?;
    ensure!(
        paths.db_path.exists(),
        "no InSyncBee database at {} — connect an account in the app first",
        paths.db_path.display()
    );
    let real = Database::open(&paths.db_path)?;
    let accounts = real.with_conn(|conn| Account::list(conn))?;
    accounts
        .into_iter()
        .find(|a| a.email.eq_ignore_ascii_case(email))
        .ok_or_else(|| {
            anyhow!("no connected account for '{email}' — run `insyncbee login` or set INSYNCBEE_E2E_ACCOUNT")
        })
}

/// Walk a display path like `/InSyncBee/Sub` down from My Drive root.
async fn resolve_remote_path(drive: &HttpDriveClient, path: &str) -> anyhow::Result<String> {
    let mut current = "root".to_string();
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        let children = DriveClient::list_all_files(drive, &current).await?;
        let found = children
            .into_iter()
            .find(|f| f.is_folder() && f.name == seg)
            .ok_or_else(|| anyhow!("remote folder '{seg}' not found (looking for '{path}')"))?;
        current = found.id;
    }
    Ok(current)
}

async fn find_or_create_folder(
    drive: &HttpDriveClient,
    parent_id: &str,
    name: &str,
) -> anyhow::Result<String> {
    let children = DriveClient::list_all_files(drive, parent_id).await?;
    if let Some(f) = children.into_iter().find(|f| f.is_folder() && f.name == name) {
        return Ok(f.id);
    }
    Ok(DriveClient::create_folder(drive, parent_id, name).await?.id)
}

/// Trash sandboxes left behind by earlier runs that panicked before
/// `finish()`. Best-effort: a failure here must not fail the scenario.
async fn sweep_orphans(drive: &HttpDriveClient, sandbox_parent: &str) {
    let now = epoch_secs();
    let children = match DriveClient::list_all_files(drive, sandbox_parent).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[e2e] orphan sweep skipped: {e}");
            return;
        }
    };
    for f in children {
        // Names look like `e2e-<epoch>-<seq>-<slug>`.
        let Some(stamp) = f
            .name
            .strip_prefix("e2e-")
            .and_then(|rest| rest.split('-').next())
            .and_then(|s| s.parse::<u64>().ok())
        else {
            continue;
        };
        if now.saturating_sub(stamp) > ORPHAN_MAX_AGE_SECS {
            eprintln!("[e2e] trashing orphan sandbox {}", f.name);
            let _ = drive.trash_file(&f.id).await;
        }
    }
}

/// Convenience for scenarios that need a deterministic blob of a given size.
pub fn filler(size: usize, seed: u8) -> Vec<u8> {
    (0..size).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
}

/// Hash a local file the same way the engine does, for content assertions.
pub fn hash(path: &Path) -> anyhow::Result<String> {
    watcher::hash_file(path)
}

/// Assert that `actions` contains at least one action matching `pred`,
/// with a message listing what was actually computed.
pub fn assert_action(actions: &[SyncAction], what: &str, pred: impl Fn(&SyncAction) -> bool) {
    if !actions.iter().any(pred) {
        let listed = actions
            .iter()
            .map(|a| a.describe())
            .collect::<Vec<_>>()
            .join("\n");
        panic!("expected a {what} action, got:\n{listed}");
    }
}

/// Fail a scenario with a message, used where `?` would swallow context.
pub fn fail<T>(msg: impl std::fmt::Display) -> anyhow::Result<T> {
    bail!("{msg}")
}

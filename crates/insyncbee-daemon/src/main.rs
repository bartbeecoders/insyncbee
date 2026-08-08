use clap::{Parser, Subcommand};
use insyncbee_core::auth::{AuthManager, OAuthCredentials};
use insyncbee_core::db::models::{ConflictPolicy, SyncMode, SyncPair, SyncPairStatus};
use insyncbee_core::db::Database;
use insyncbee_core::drive::HttpDriveClient;
use insyncbee_core::keystore;
use insyncbee_core::sync_engine::{SyncAction, SyncEngine};
use insyncbee_core::watcher::FileWatcher;
use insyncbee_core::AppPaths;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Build a sync engine for `pair`. If the pair is encrypted, the cipher
/// is loaded from the OS keyring and attached. Returns an error when the
/// keyring entry is missing — the caller surfaces that as a "needs unlock"
/// hint rather than silently skipping the pair.
fn build_engine(db: &Database, pair: &SyncPair) -> anyhow::Result<SyncEngine> {
    let mut engine = SyncEngine::new(db.clone(), pair.clone());
    if pair.encryption_enabled {
        let cipher = keystore::load_cipher(&pair.id)?.ok_or_else(|| {
            anyhow::anyhow!(
                "encryption key for '{}' is not in the keyring — run the GUI and unlock it once",
                pair.name
            )
        })?;
        engine = engine.with_cipher(Arc::new(cipher));
    }
    Ok(engine)
}

#[derive(Parser)]
#[command(
    name = "insyncbee",
    about = "InSyncBee - Google Drive sync for Linux, macOS, and Windows",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Store Google OAuth client credentials for both the CLI and the app
    Configure {
        /// Google OAuth client ID (defaults to $INSYNCBEE_CLIENT_ID)
        #[arg(long)]
        client_id: Option<String>,

        /// Google OAuth client secret (defaults to $INSYNCBEE_CLIENT_SECRET)
        #[arg(long)]
        client_secret: Option<String>,
    },

    /// Sign in with a Google account
    Login,

    /// List connected accounts
    Accounts,

    /// Remove a connected account
    Logout {
        /// Account ID or email
        account: String,
    },

    /// Add a new sync pair
    Add {
        /// Display name for this sync pair
        #[arg(long)]
        name: String,

        /// Local folder path
        #[arg(long)]
        local: String,

        /// Google Drive folder ID (use 'root' for My Drive root)
        #[arg(long)]
        remote_id: String,

        /// Remote folder display path
        #[arg(long, default_value = "/")]
        remote_path: String,

        /// Account ID to use
        #[arg(long)]
        account: String,

        /// Sync mode: two-way, local-to-cloud, cloud-to-local
        #[arg(long, default_value = "two-way")]
        mode: String,
    },

    /// List configured sync pairs
    List,

    /// Show sync status
    Status,

    /// Run a sync cycle now
    Sync {
        /// Sync pair ID (syncs all if omitted)
        pair: Option<String>,

        /// Show what would happen without making changes
        #[arg(long)]
        dry_run: bool,
    },

    /// Pause a sync pair
    Pause {
        /// Sync pair ID
        pair: String,
    },

    /// Resume a sync pair
    Resume {
        /// Sync pair ID
        pair: String,
    },

    /// Remove a sync pair
    Remove {
        /// Sync pair ID
        pair: String,
    },

    /// Run as a background daemon (file watching + remote polling)
    Daemon,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let paths = AppPaths::new()?;

    // Log to stdout AND to a daily-rotating file under paths.log_dir.
    // The non-blocking guard must outlive the program — keep it on the stack.
    let file_appender = tracing_appender::rolling::daily(&paths.log_dir, "insyncbee.log");
    let (file_writer, _guard) = tracing_appender::non_blocking(file_appender);

    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stdout))
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(file_writer)
                .with_ansi(false),
        )
        .init();

    tracing::info!("Logs: {}/insyncbee.log.<date>", paths.log_dir.display());

    let db = Database::open(&paths.db_path)?;

    match cli.command {
        Commands::Configure {
            client_id,
            client_secret,
        } => {
            // Falling back to the environment makes this a one-liner for
            // anyone who already had the exports in their shell profile:
            // `insyncbee configure` copies them into the config file the
            // desktop app can actually see.
            let creds = match (client_id, client_secret) {
                (Some(id), Some(secret)) => OAuthCredentials {
                    client_id: id,
                    client_secret: secret,
                },
                (id, secret) => {
                    let env = OAuthCredentials::from_env().map_err(|e| {
                        anyhow::anyhow!(
                            "pass --client-id and --client-secret, or set them in the \
                             environment first ({e})"
                        )
                    })?;
                    OAuthCredentials {
                        client_id: id.unwrap_or(env.client_id),
                        client_secret: secret.unwrap_or(env.client_secret),
                    }
                }
            };
            creds.save(&paths.credentials_path)?;
            println!("Credentials saved to {}", paths.credentials_path.display());
            println!("The desktop app picks these up without a restart.");
        }

        Commands::Login => {
            let creds = OAuthCredentials::load(&paths.credentials_path)?;
            let auth = AuthManager::new(creds, db);
            let account = auth.login().await?;
            println!("Logged in as: {} ({})", account.email, account.id);
        }

        Commands::Accounts => {
            let creds = OAuthCredentials::load(&paths.credentials_path)?;
            let auth = AuthManager::new(creds, db);
            let accounts = auth.list_accounts()?;
            if accounts.is_empty() {
                println!("No accounts connected. Run 'insyncbee login' to add one.");
            } else {
                println!("{:<36}  {:<30}  {}", "ID", "Email", "Name");
                println!("{}", "-".repeat(80));
                for acc in accounts {
                    println!(
                        "{:<36}  {:<30}  {}",
                        acc.id,
                        acc.email,
                        acc.display_name.unwrap_or_default()
                    );
                }
            }
        }

        Commands::Logout { account } => {
            let creds = OAuthCredentials::load(&paths.credentials_path)?;
            let auth = AuthManager::new(creds, db);
            auth.remove_account(&account)?;
            println!("Account removed.");
        }

        Commands::Add {
            name,
            local,
            remote_id,
            remote_path,
            account,
            mode,
        } => {
            let local_path = std::path::Path::new(&local);
            if !local_path.exists() {
                std::fs::create_dir_all(local_path)?;
                println!("Created local directory: {local}");
            }

            let sync_mode: SyncMode = mode.parse()?;
            let pair = SyncPair {
                id: uuid::Uuid::new_v4().to_string(),
                name: name.clone(),
                account_id: account,
                local_root: local,
                remote_root_id: remote_id,
                remote_root_path: remote_path,
                mode: sync_mode,
                conflict_policy: ConflictPolicy::KeepBoth,
                status: SyncPairStatus::Active,
                poll_interval_secs: 30,
                created_at: chrono::Utc::now().to_rfc3339(),
                updated_at: chrono::Utc::now().to_rfc3339(),
                // The CLI doesn't (yet) take a passphrase. Use the GUI to
                // create encrypted pairs.
                encryption_enabled: false,
                encryption_salt: None,
                encryption_verifier: None,
            };

            db.with_conn(|conn| pair.insert(conn))?;
            println!("Sync pair '{name}' created (ID: {})", pair.id);
        }

        Commands::List => {
            let pairs = db.with_conn(|conn| SyncPair::list(conn))?;
            if pairs.is_empty() {
                println!("No sync pairs configured. Run 'insyncbee add' to create one.");
            } else {
                for p in pairs {
                    println!(
                        "[{}] {} ({}) {} <-> {} [{}]",
                        p.status, p.name, p.id, p.local_root, p.remote_root_path, p.mode
                    );
                }
            }
        }

        Commands::Status => {
            let pairs = db.with_conn(|conn| SyncPair::list(conn))?;
            if pairs.is_empty() {
                println!("No sync pairs configured.");
            } else {
                for p in pairs {
                    let file_count = db.with_conn(|conn| {
                        insyncbee_core::db::models::FileEntry::list_by_sync_pair(conn, &p.id)
                    })?.len();
                    let conflicts = db.with_conn(|conn| {
                        insyncbee_core::db::models::FileEntry::list_by_state(
                            conn,
                            &p.id,
                            &insyncbee_core::db::models::FileState::Conflict,
                        )
                    })?.len();

                    let status_icon = match p.status {
                        SyncPairStatus::Active => "✓",
                        SyncPairStatus::Paused => "⏸",
                        SyncPairStatus::Error => "✗",
                    };

                    println!(
                        "{status_icon} {:<20} {:<12} {:>6} files  {:>3} conflicts",
                        p.name,
                        format!("[{}]", p.mode),
                        file_count,
                        conflicts,
                    );
                }
            }
        }

        Commands::Sync { pair, dry_run } => {
            let creds = OAuthCredentials::load(&paths.credentials_path)?;
            let pairs = if let Some(pair_id) = pair {
                let p = db
                    .with_conn(|conn| SyncPair::get_by_id(conn, &pair_id))?
                    .ok_or_else(|| anyhow::anyhow!("Sync pair not found: {pair_id}"))?;
                vec![p]
            } else {
                db.with_conn(|conn| SyncPair::list(conn))?
            };

            for p in pairs {
                if p.status == SyncPairStatus::Paused {
                    println!("Skipping '{}' (paused)", p.name);
                    continue;
                }

                let auth = AuthManager::new(creds.clone(), db.clone());
                let drive = HttpDriveClient::new(auth, p.account_id.clone());
                let engine = match build_engine(&db, &p) {
                    Ok(e) => e,
                    Err(e) => {
                        eprintln!("  Skipping '{}': {e}", p.name);
                        continue;
                    }
                };

                if dry_run {
                    println!("Dry run for '{}':", p.name);
                    match engine.dry_run(&drive).await {
                        Ok((actions, report)) => {
                            for action in &actions {
                                match action {
                                    SyncAction::Skip { .. } => {} // don't show skips in dry run
                                    _ => println!("{}", action.describe()),
                                }
                            }
                            println!();
                            println!(
                                "  Summary: {} upload, {} download, {} delete, {} conflict",
                                report.uploaded, report.downloaded, report.deleted, report.conflicts
                            );
                        }
                        Err(e) => eprintln!("  Error: {e}"),
                    }
                } else {
                    println!("Syncing '{}'...", p.name);
                    match engine.sync(&drive).await {
                        Ok(report) => println!("  {report}"),
                        Err(e) => {
                            eprintln!("  Error: {e}");
                            db.with_conn(|conn| {
                                SyncPair::update_status(conn, &p.id, &SyncPairStatus::Error)
                            })?;
                        }
                    }
                }
            }
        }

        Commands::Pause { pair } => {
            db.with_conn(|conn| SyncPair::update_status(conn, &pair, &SyncPairStatus::Paused))?;
            println!("Sync pair paused.");
        }

        Commands::Resume { pair } => {
            db.with_conn(|conn| SyncPair::update_status(conn, &pair, &SyncPairStatus::Active))?;
            println!("Sync pair resumed.");
        }

        Commands::Remove { pair } => {
            db.with_conn(|conn| SyncPair::delete(conn, &pair))?;
            println!("Sync pair removed.");
        }

        Commands::Daemon => {
            run_daemon(db, &paths.credentials_path).await?;
        }
    }

    Ok(())
}

/// Run the background sync daemon: watch local files + poll remote changes.
async fn run_daemon(db: Database, credentials_path: &std::path::Path) -> anyhow::Result<()> {
    let creds = OAuthCredentials::load(credentials_path)?;

    println!("InSyncBee daemon starting...");

    // Load all active sync pairs
    let pairs = db.with_conn(|conn| SyncPair::list(conn))?;
    let active_pairs: Vec<SyncPair> = pairs
        .into_iter()
        .filter(|p| p.status == SyncPairStatus::Active)
        .collect();

    if active_pairs.is_empty() {
        println!("No active sync pairs. Add one with 'insyncbee add' first.");
        return Ok(());
    }

    println!(
        "Watching {} sync pair(s). Press Ctrl+C to stop.",
        active_pairs.len()
    );

    // Set up a shutdown signal
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);

    // Start file watchers for each pair
    let mut watchers = Vec::new();
    let mut watcher_receivers = Vec::new();

    for pair in &active_pairs {
        let root = PathBuf::from(&pair.local_root);
        if !root.exists() {
            tracing::warn!("Local root for '{}' does not exist: {}", pair.name, pair.local_root);
            continue;
        }

        match FileWatcher::start(&root, 2000) {
            Ok((watcher, rx)) => {
                tracing::info!("Watching '{}' at {}", pair.name, pair.local_root);
                watchers.push(watcher);
                watcher_receivers.push((pair.id.clone(), rx));
            }
            Err(e) => {
                tracing::error!("Failed to watch '{}': {e}", pair.name);
            }
        }
    }

    // Build a map of pair ID -> poll interval for scheduling
    let mut last_poll: HashMap<String, tokio::time::Instant> = HashMap::new();
    for pair in &active_pairs {
        last_poll.insert(pair.id.clone(), tokio::time::Instant::now());
    }

    // Main daemon loop
    let mut poll_ticker = tokio::time::interval(tokio::time::Duration::from_secs(5));

    // Do an initial sync for all pairs
    for pair in &active_pairs {
        let auth = AuthManager::new(creds.clone(), db.clone());
        let drive = HttpDriveClient::new(auth, pair.account_id.clone());
        let engine = match build_engine(&db, pair) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("Skipping initial sync for '{}': {e}", pair.name);
                continue;
            }
        };

        tracing::info!("Initial sync for '{}'...", pair.name);
        match engine.sync(&drive).await {
            Ok(report) => tracing::info!("Initial sync '{}': {report}", pair.name),
            Err(e) => tracing::error!("Initial sync '{}' failed: {e}", pair.name),
        }
    }

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                println!("\nShutting down daemon...");
                break;
            }

            _ = poll_ticker.tick() => {
                let now = tokio::time::Instant::now();

                for pair in &active_pairs {
                    // Check if this pair's status is still active
                    let current_status = db.with_conn(|conn| SyncPair::get_by_id(conn, &pair.id));
                    if let Ok(Some(p)) = current_status {
                        if p.status == SyncPairStatus::Paused {
                            continue;
                        }
                    }

                    let last = last_poll.get(&pair.id).copied().unwrap_or(now);
                    let interval = tokio::time::Duration::from_secs(pair.poll_interval_secs as u64);

                    if now.duration_since(last) >= interval {
                        last_poll.insert(pair.id.clone(), now);

                        let auth = AuthManager::new(creds.clone(), db.clone());
                        let drive = HttpDriveClient::new(auth, pair.account_id.clone());
                        let engine = match build_engine(&db, pair) {
                            Ok(e) => e,
                            Err(e) => {
                                tracing::warn!("Skipping poll for '{}': {e}", pair.name);
                                continue;
                            }
                        };

                        tracing::debug!("Polling remote changes for '{}'...", pair.name);
                        match engine.sync(&drive).await {
                            Ok(report) => {
                                if report.uploaded + report.downloaded + report.deleted + report.conflicts > 0 {
                                    tracing::info!("Sync '{}': {report}", pair.name);
                                }
                            }
                            Err(e) => tracing::error!("Sync '{}' failed: {e}", pair.name),
                        }
                    }
                }
            }
        }

        // Drain any file watcher events (trigger immediate sync for affected pairs)
        for (pair_id, rx) in &mut watcher_receivers {
            while let Ok(events) = rx.try_recv() {
                if !events.is_empty() {
                    tracing::debug!("{} local change(s) detected for pair {pair_id}", events.len());

                    // Find the pair and trigger sync
                    if let Some(pair) = active_pairs.iter().find(|p| p.id == *pair_id) {
                        let auth = AuthManager::new(creds.clone(), db.clone());
                        let drive = HttpDriveClient::new(auth, pair.account_id.clone());
                        let engine = match build_engine(&db, pair) {
                            Ok(e) => e,
                            Err(e) => {
                                tracing::warn!("Skipping watcher-triggered sync for '{}': {e}", pair.name);
                                continue;
                            }
                        };

                        match engine.sync(&drive).await {
                            Ok(report) => {
                                if report.uploaded + report.downloaded + report.deleted + report.conflicts > 0 {
                                    tracing::info!("Sync '{}' (local change): {report}", pair.name);
                                }
                            }
                            Err(e) => tracing::error!("Sync '{}' failed: {e}", pair.name),
                        }

                        // Reset the poll timer for this pair since we just synced
                        last_poll.insert(pair_id.clone(), tokio::time::Instant::now());
                    }
                }
            }
        }
    }

    println!("Daemon stopped.");
    Ok(())
}

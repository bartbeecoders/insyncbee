use clap::{Parser, Subcommand};
use insyncbee_core::auth::{AuthManager, OAuthCredentials};
use insyncbee_core::db::models::{ConflictPolicy, SyncMode, SyncPair, SyncPairStatus};
use insyncbee_core::db::Database;
use insyncbee_core::drive::HttpDriveClient;
use insyncbee_core::sync_engine::{SyncAction, SyncEngine};
use insyncbee_core::watcher::FileWatcher;
use insyncbee_core::AppPaths;
use std::collections::HashMap;
use std::path::PathBuf;

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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let paths = AppPaths::new()?;
    let db = Database::open(&paths.db_path)?;

    match cli.command {
        Commands::Login => {
            let creds = OAuthCredentials::from_env()?;
            let auth = AuthManager::new(creds, db);
            let account = auth.login().await?;
            println!("Logged in as: {} ({})", account.email, account.id);
        }

        Commands::Accounts => {
            let creds = OAuthCredentials::from_env()?;
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
            let creds = OAuthCredentials::from_env()?;
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
            let creds = OAuthCredentials::from_env()?;
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
                let engine = SyncEngine::new(db.clone(), p.clone());

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
            run_daemon(db).await?;
        }
    }

    Ok(())
}

/// Run the background sync daemon: watch local files + poll remote changes.
async fn run_daemon(db: Database) -> anyhow::Result<()> {
    let creds = OAuthCredentials::from_env()?;

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
        let engine = SyncEngine::new(db.clone(), pair.clone());

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
                        let engine = SyncEngine::new(db.clone(), pair.clone());

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
                        let engine = SyncEngine::new(db.clone(), pair.clone());

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

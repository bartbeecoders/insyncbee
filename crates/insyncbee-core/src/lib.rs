pub mod auth;
pub mod crypto;
pub mod db;
pub mod drive;
pub mod error;
pub mod keystore;
pub mod sync_engine;
pub mod watcher;

pub use error::{Error, Result};

/// Application-wide configuration paths
pub struct AppPaths {
    pub data_dir: std::path::PathBuf,
    pub db_path: std::path::PathBuf,
    pub log_dir: std::path::PathBuf,
    pub config_dir: std::path::PathBuf,
    /// Where OAuth client credentials live when they aren't in the
    /// environment. A desktop launcher does not run a login shell, so an
    /// app started from the menu or a file manager inherits none of the
    /// exports in `~/.bashrc` — a file is the only thing both launch paths
    /// can see.
    pub credentials_path: std::path::PathBuf,
}

impl AppPaths {
    pub fn new() -> anyhow::Result<Self> {
        let data_dir = dirs::data_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine data directory"))?
            .join("insyncbee");

        std::fs::create_dir_all(&data_dir)?;

        let log_dir = data_dir.join("logs");
        std::fs::create_dir_all(&log_dir)?;

        let db_path = data_dir.join("insyncbee.db");

        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?
            .join("insyncbee");
        std::fs::create_dir_all(&config_dir)?;

        let credentials_path = config_dir.join("credentials.json");

        Ok(Self {
            data_dir,
            db_path,
            log_dir,
            config_dir,
            credentials_path,
        })
    }
}

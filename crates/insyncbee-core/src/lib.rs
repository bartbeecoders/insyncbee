pub mod auth;
pub mod db;
pub mod drive;
pub mod error;
pub mod sync_engine;
pub mod watcher;

pub use error::{Error, Result};

/// Application-wide configuration paths
pub struct AppPaths {
    pub data_dir: std::path::PathBuf,
    pub db_path: std::path::PathBuf,
    pub log_dir: std::path::PathBuf,
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

        Ok(Self {
            data_dir,
            db_path,
            log_dir,
        })
    }
}

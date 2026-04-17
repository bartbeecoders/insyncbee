use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Auth error: {0}")]
    Auth(String),

    #[error("Sync error: {0}")]
    Sync(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Conflict: local and remote both changed '{0}'")]
    Conflict(String),

    #[error("{0}")]
    Other(String),
}

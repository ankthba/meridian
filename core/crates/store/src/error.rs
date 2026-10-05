use thiserror::Error;

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum StoreError {
    #[error("SQLite: {0}")]
    Sqlite(String),
    #[error("DuckDB: {0}")]
    DuckDb(String),
    #[error("I/O: {0}")]
    Io(String),
    #[error("serialization: {0}")]
    Serde(String),
    #[error("migration {version} failed: {message}")]
    Migration { version: u32, message: String },
    #[error("query rejected: {0}")]
    Rejected(String),
    #[error("not found")]
    NotFound,
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Sqlite(e.to_string())
    }
}

impl From<duckdb::Error> for StoreError {
    fn from(e: duckdb::Error) -> Self {
        StoreError::DuckDb(e.to_string())
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        StoreError::Serde(e.to_string())
    }
}

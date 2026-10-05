//! Persistence: SQLite for app state, DuckDB for market data and analytics,
//! Parquet for bulk history.
//!
//! Every cached market row carries the provider that produced it so
//! [`Stores::purge_provider`] can honor terms that require deletion. Mock
//! data is never persisted (its cache policy is `NoStore`; the engine
//! enforces that before calling into this crate).

mod app;
mod error;
mod market;
mod sql_guard;

use std::path::{Path, PathBuf};

pub use app::*;
pub use error::*;
pub use market::*;
pub use sql_guard::*;

/// Both stores, opened from one data directory.
pub struct Stores {
    pub app: AppStore,
    pub market: MarketStore,
    root: PathBuf,
}

impl Stores {
    /// Opens (creating and migrating if needed) `app.sqlite`,
    /// `market.duckdb`, and the `cache/parquet` directory under `root`.
    pub fn open(root: &Path) -> StoreResult<Self> {
        std::fs::create_dir_all(root).map_err(|e| StoreError::Io(e.to_string()))?;
        let parquet = root.join("cache").join("parquet");
        std::fs::create_dir_all(&parquet).map_err(|e| StoreError::Io(e.to_string()))?;
        let app = AppStore::open(&root.join("app.sqlite"))?;
        let market = MarketStore::open(&root.join("market.duckdb"), parquet)?;
        Ok(Self { app, market, root: root.to_path_buf() })
    }

    /// In-memory stores for tests and MOCK mode.
    pub fn in_memory() -> StoreResult<Self> {
        let tmp = std::env::temp_dir().join(format!("meridian-mem-{}", std::process::id()));
        Ok(Self { app: AppStore::open_in_memory()?, market: MarketStore::open_in_memory(tmp.join("parquet"))?, root: tmp })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Deletes every row and file produced by `provider`.
    pub fn purge_provider(&self, provider: &str) -> StoreResult<PurgeReport> {
        self.market.purge_provider(provider)
    }
}

use std::path::PathBuf;

use meridian_types::UnixNanos;
use serde::{Deserialize, Serialize};

/// The app runs on mock data or real data, never both (ARCHITECTURE §4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataMode {
    Mock,
    Live,
}

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub mode: DataMode,
    /// `~/Library/Application Support/Meridian` in the app.
    pub data_dir: PathBuf,
    /// Use in-memory stores (tests, snapshot rendering).
    pub in_memory: bool,
    /// Freeze the clock (snapshot tests). `None` = wall clock.
    pub fixed_clock: Option<UnixNanos>,
    pub worker_threads: usize,
}

impl EngineConfig {
    #[must_use]
    pub fn test(mode: DataMode, fixed_clock: UnixNanos) -> Self {
        Self { mode, data_dir: std::env::temp_dir(), in_memory: true, fixed_clock: Some(fixed_clock), worker_threads: 2 }
    }
}

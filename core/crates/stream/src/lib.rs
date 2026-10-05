//! Market state owned by Rust, read by Swift at display rate.
//!
//! Providers write normalized [`StreamEvent`]s into the [`StreamHub`]'s
//! bounded channel; a dedicated apply thread merges them into per-instrument
//! [`cells`](state::MarketState). Views pull changes with
//! [`QuoteSubscription::poll`], which packs changed rows into a byte buffer
//! (layout in [`row`]). Nothing crosses the FFI per tick.

mod hub;
mod registry;
pub mod row;
mod state;

pub use hub::*;
pub use registry::*;
pub use state::*;

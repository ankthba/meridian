//! The engine wires providers (chosen by data mode), the router, the stream
//! hub, stores, and alerts, and builds every function screen as a generic
//! [`screen::Screen`]. The FFI crate is a thin facade over this crate.

// Tool failures are values returned to the model, not exceptional paths;
// `Fetched::fetched_at` reads better than a renamed field.
#![allow(clippy::result_large_err, clippy::struct_field_names)]

pub mod ask_tools;
mod cache;
mod config;
mod core;
mod error;
mod events;
pub mod screen;
pub mod screens;
mod universe;

pub use crate::core::*;
pub use config::*;
pub use error::*;
pub use events::*;
pub use universe::*;

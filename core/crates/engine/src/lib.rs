//! The engine wires providers (chosen by data mode), the router, the stream
//! hub, stores, and alerts, and builds every function screen as a generic
//! [`screen::Screen`]. The FFI crate is a thin facade over this crate.

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

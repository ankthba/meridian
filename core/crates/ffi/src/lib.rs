//! UniFFI surface for the Swift app.
//!
//! Rules (ARCHITECTURE §7.2): every export returns `Result`; sync exports
//! never block; streaming data crosses as packed byte buffers; cancellable
//! async calls take a `CancelTokenFfi`.

// UniFFI scaffolding generates `unsafe` FFI glue in this crate.
#![allow(unsafe_code)]

mod core;
mod error;
mod providers;
mod types;

pub use crate::core::*;
pub use error::*;
pub use types::*;

uniffi::setup_scaffolding!("meridian");

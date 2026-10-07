//! Normalized domain types for Meridian.
//!
//! Every provider converts vendor payloads into these types at its boundary;
//! nothing above the provider layer sees vendor formats. This crate performs
//! no I/O and reads no clocks.

pub mod calendar;
pub mod company;
pub mod error;
pub mod instrument;
pub mod key;
pub mod macro_data;
pub mod market;
pub mod news;
pub mod options;
pub mod provenance;
pub mod time;

pub use calendar::*;
pub use company::*;
pub use error::*;
pub use instrument::*;
pub use key::*;
pub use macro_data::*;
pub use market::*;
pub use news::*;
pub use options::*;
pub use provenance::*;
pub use time::*;

//! Meridian's command line: the parser, the function registry, and the
//! autocomplete index.
//!
//! Grammar (ARCHITECTURE §9.2):
//!
//! ```text
//! [<SECURITY> [<EXCHANGE>] <SECTOR>] [<FUNCTION>] [<ARGS>...] <GO>
//! ```
//!
//! For example `AAPL US <EQUITY> DES <GO>` loads Apple and opens its
//! description, while `DES <GO>` alone applies DES to the panel's loaded
//! security.
//!
//! The crate is pure: no I/O, no clocks, no randomness. The Swift shell calls
//! [`parse()`] when the user presses GO and [`SuggestIndex::suggest`] on every
//! keystroke.

mod error;
pub mod parse;
pub mod registry;
pub mod suggest;

pub use error::CommandError;
pub use parse::{
    MAX_MENU_ITEM, ParseContext, ParsedCommand, check_security_need, format_security, parse,
    validate,
};
pub use registry::{FunctionCategory, FunctionSpec, SecurityNeed, lookup, registry};
pub use suggest::{IndexedInstrument, SuggestIndex, Suggestion, SuggestionKind};

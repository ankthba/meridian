//! Meridian's command line: the parser, plain-language interpretation, the
//! function registry, and the autocomplete index.
//!
//! Plain phrases come first (`aapl`, `aapl 5y`, `aapl filings`, `aapl vs
//! msft`, `earnings this week`); see [`plain`]. The mnemonic grammar
//! (ARCHITECTURE §9.2) keeps working unchanged:
//!
//! ```text
//! [<SECURITY> [<EXCHANGE>] <SECTOR>] [<FUNCTION>] [<ARGS>...] <GO>
//! ```
//!
//! For example `AAPL US <EQUITY> DES <GO>` loads Apple and opens its
//! description, while `DES <GO>` alone applies DES to the panel's loaded
//! security.
//!
//! The crate is pure: no I/O, no clocks, no randomness. The engine owns the
//! instrument index and calls [`interpret`] when the user presses GO and
//! [`SuggestIndex::suggest`] on every keystroke.

mod error;
pub mod parse;
pub mod plain;
pub mod registry;
pub mod suggest;

pub use error::CommandError;
pub use parse::{
    MAX_MENU_ITEM, ParseContext, ParsedCommand, check_security_need, format_security, parse,
    validate,
};
pub use plain::{
    APP_ACTIONS, Action, MAJOR_COINS, MAX_COMPARE, SecurityResolver, compare_keys_of, compare_of,
    hint_for, interpret, resolve_security,
};
pub use registry::{FunctionCategory, FunctionSpec, SecurityNeed, lookup, registry};
pub use suggest::{IndexedInstrument, SuggestIndex, Suggestion, SuggestionGroup, SuggestionKind};

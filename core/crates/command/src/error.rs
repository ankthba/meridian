use meridian_types::{MarketSector, SecurityKey};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Why a parsed command can't run as typed. Messages are shown to the user
/// verbatim, so they say what to do next.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
pub enum CommandError {
    /// The function needs a security and none was typed or loaded.
    #[error("{function} requires a security. Load one first, e.g. {example} {function} <GO>")]
    SecurityRequired {
        function: String,
        /// A security in command-line form that the function accepts,
        /// e.g. `AAPL US <EQUITY>`.
        example: String,
    },

    /// The function doesn't accept securities of this sector.
    #[error(
        "{function} does not apply to {security}. {function} accepts {} securities only.",
        sector_prose(.sectors)
    )]
    SectorMismatch {
        function: String,
        security: SecurityKey,
        /// The sectors the function accepts.
        sectors: Vec<MarketSector>,
    },

    /// The mnemonic isn't in the registry.
    #[error("Unknown function {0}.")]
    UnknownFunction(String),
}

/// `Equity`, `Equity or Index`, `Equity, Index or Curncy`.
fn sector_prose(sectors: &[MarketSector]) -> String {
    match sectors {
        [] => "any".to_owned(),
        [only] => only.key_label().to_owned(),
        [init @ .., last] => {
            let init: Vec<&str> = init.iter().map(|s| s.key_label()).collect();
            format!("{} or {}", init.join(", "), last.key_label())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sector_prose_lists() {
        use MarketSector::{Curncy, Equity, Index};
        assert_eq!(sector_prose(&[]), "any");
        assert_eq!(sector_prose(&[Equity]), "Equity");
        assert_eq!(sector_prose(&[Equity, Index]), "Equity or Index");
        assert_eq!(
            sector_prose(&[Equity, Index, Curncy]),
            "Equity, Index or Curncy"
        );
    }
}

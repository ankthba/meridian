//! Security keys in terminal notation, e.g. `AAPL US Equity`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::ParseError;

/// The ten market-sector ("yellow") keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MarketSector {
    Govt,
    Corp,
    Mtge,
    MMkt,
    Muni,
    Pfd,
    Equity,
    Cmdty,
    Index,
    Curncy,
}

impl MarketSector {
    pub const ALL: [MarketSector; 10] = [
        MarketSector::Govt,
        MarketSector::Corp,
        MarketSector::Mtge,
        MarketSector::MMkt,
        MarketSector::Muni,
        MarketSector::Pfd,
        MarketSector::Equity,
        MarketSector::Cmdty,
        MarketSector::Index,
        MarketSector::Curncy,
    ];

    /// Label as written inside a security key (`AAPL US Equity`).
    #[must_use]
    pub fn key_label(self) -> &'static str {
        match self {
            MarketSector::Govt => "Govt",
            MarketSector::Corp => "Corp",
            MarketSector::Mtge => "Mtge",
            MarketSector::MMkt => "M-Mkt",
            MarketSector::Muni => "Muni",
            MarketSector::Pfd => "Pfd",
            MarketSector::Equity => "Equity",
            MarketSector::Cmdty => "Comdty",
            MarketSector::Index => "Index",
            MarketSector::Curncy => "Curncy",
        }
    }

    /// Label printed on the physical key (`<EQUITY>`).
    #[must_use]
    pub fn key_cap(self) -> &'static str {
        match self {
            MarketSector::Govt => "GOVT",
            MarketSector::Corp => "CORP",
            MarketSector::Mtge => "MTGE",
            MarketSector::MMkt => "M-MKT",
            MarketSector::Muni => "MUNI",
            MarketSector::Pfd => "PFD",
            MarketSector::Equity => "EQUITY",
            MarketSector::Cmdty => "CMDTY",
            MarketSector::Index => "INDEX",
            MarketSector::Curncy => "CRNCY",
        }
    }

    /// Parses any of the accepted spellings, case-insensitively.
    #[must_use]
    pub fn parse_label(s: &str) -> Option<MarketSector> {
        let up = s.trim().trim_start_matches('<').trim_end_matches('>').to_ascii_uppercase();
        Some(match up.as_str() {
            "GOVT" => MarketSector::Govt,
            "CORP" => MarketSector::Corp,
            "MTGE" => MarketSector::Mtge,
            "M-MKT" | "MMKT" => MarketSector::MMkt,
            "MUNI" => MarketSector::Muni,
            "PFD" => MarketSector::Pfd,
            "EQUITY" | "EQUITIES" | "EQ" => MarketSector::Equity,
            "CMDTY" | "COMDTY" => MarketSector::Cmdty,
            "INDEX" => MarketSector::Index,
            "CRNCY" | "CURNCY" | "CURRENCY" => MarketSector::Curncy,
            _ => return None,
        })
    }
}

impl fmt::Display for MarketSector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key_label())
    }
}

/// User-facing, persisted identifier for a security.
///
/// `symbol` is upper-case. `exchange` is a terminal-style exchange code
/// (`US` composite, `LN`, …) and is optional for sectors that don't use one
/// (`EURUSD Curncy`, `SPX Index`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SecurityKey {
    pub symbol: String,
    pub exchange: Option<String>,
    pub sector: MarketSector,
}

impl SecurityKey {
    #[must_use]
    pub fn new(symbol: impl Into<String>, exchange: Option<&str>, sector: MarketSector) -> Self {
        Self {
            symbol: symbol.into().to_ascii_uppercase(),
            exchange: exchange.map(str::to_ascii_uppercase),
            sector,
        }
    }

    #[must_use]
    pub fn equity(symbol: &str) -> Self {
        Self::new(symbol, Some("US"), MarketSector::Equity)
    }

    #[must_use]
    pub fn currency(pair: &str) -> Self {
        Self::new(pair, None, MarketSector::Curncy)
    }

    #[must_use]
    pub fn index(symbol: &str) -> Self {
        Self::new(symbol, None, MarketSector::Index)
    }
}

impl fmt::Display for SecurityKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.exchange {
            Some(ex) => write!(f, "{} {} {}", self.symbol, ex, self.sector),
            None => write!(f, "{} {}", self.symbol, self.sector),
        }
    }
}

impl FromStr for SecurityKey {
    type Err = ParseError;

    /// Parses `SYMBOL [EXCH] Sector`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split_whitespace().collect();
        match parts.as_slice() {
            [sym, sector] => {
                let sector = MarketSector::parse_label(sector)
                    .ok_or_else(|| ParseError::new("sector", *sector))?;
                Ok(SecurityKey::new(*sym, None, sector))
            }
            [sym, exch, sector] => {
                let sector = MarketSector::parse_label(sector)
                    .ok_or_else(|| ParseError::new("sector", *sector))?;
                Ok(SecurityKey::new(*sym, Some(exch), sector))
            }
            _ => Err(ParseError::new("security key", s)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_displays_keys() {
        let k: SecurityKey = "aapl us equity".parse().unwrap();
        assert_eq!(k, SecurityKey::equity("AAPL"));
        assert_eq!(k.to_string(), "AAPL US Equity");

        let fx: SecurityKey = "EURUSD Curncy".parse().unwrap();
        assert_eq!(fx.exchange, None);
        assert_eq!(fx.to_string(), "EURUSD Curncy");

        let fut: SecurityKey = "CLZ6 Comdty".parse().unwrap();
        assert_eq!(fut.sector, MarketSector::Cmdty);
    }

    #[test]
    fn rejects_bad_keys() {
        assert!("AAPL".parse::<SecurityKey>().is_err());
        assert!("AAPL US Stock".parse::<SecurityKey>().is_err());
    }

    #[test]
    fn sector_labels_round_trip() {
        for s in MarketSector::ALL {
            assert_eq!(MarketSector::parse_label(s.key_label()), Some(s));
            assert_eq!(MarketSector::parse_label(s.key_cap()), Some(s));
        }
    }
}

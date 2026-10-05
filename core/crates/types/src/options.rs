use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::key::SecurityKey;
use crate::provenance::Provenance;
use crate::time::UnixNanos;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum OptionRight {
    Call,
    Put,
}

impl OptionRight {
    #[must_use]
    pub fn letter(self) -> char {
        match self {
            OptionRight::Call => 'C',
            OptionRight::Put => 'P',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExerciseStyle {
    American,
    European,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GreeksSource {
    Vendor,
    /// Computed by `meridian-analytics` with the named model.
    Computed { model: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Greeks {
    pub iv: Option<f64>,
    pub delta: Option<f64>,
    pub gamma: Option<f64>,
    pub theta: Option<f64>,
    pub vega: Option<f64>,
    pub rho: Option<f64>,
    pub source: GreeksSource,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OptionContract {
    /// OCC-style symbol, e.g. `AAPL  261218C00200000`.
    pub contract_symbol: String,
    pub expiry: NaiveDate,
    pub strike: f64,
    pub right: OptionRight,
    pub style: ExerciseStyle,
    pub multiplier: f64,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub bid_size: Option<f64>,
    pub ask_size: Option<f64>,
    pub last: Option<f64>,
    pub volume: Option<f64>,
    pub open_interest: Option<f64>,
    pub greeks: Option<Greeks>,
}

impl OptionContract {
    #[must_use]
    pub fn mid(&self) -> Option<f64> {
        match (self.bid, self.ask) {
            (Some(b), Some(a)) if a >= b && a > 0.0 => Some((a + b) / 2.0),
            _ => self.last,
        }
    }

    /// Builds the 21-character OCC symbol.
    #[must_use]
    pub fn occ_symbol(root: &str, expiry: NaiveDate, right: OptionRight, strike: f64) -> String {
        let strike_milli = (strike * 1000.0).round() as u64;
        format!(
            "{:<6}{}{}{:08}",
            root.to_ascii_uppercase(),
            expiry.format("%y%m%d"),
            right.letter(),
            strike_milli
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OptionChain {
    pub underlying: SecurityKey,
    pub underlying_price: Option<f64>,
    pub as_of: UnixNanos,
    pub contracts: Vec<OptionContract>,
    pub provenance: Provenance,
}

impl OptionChain {
    /// Sorted unique expiries.
    #[must_use]
    pub fn expiries(&self) -> Vec<NaiveDate> {
        let mut v: Vec<NaiveDate> = self.contracts.iter().map(|c| c.expiry).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Sorted unique strikes for one expiry.
    #[must_use]
    pub fn strikes(&self, expiry: NaiveDate) -> Vec<f64> {
        let mut v: Vec<f64> = self.contracts.iter().filter(|c| c.expiry == expiry).map(|c| c.strike).collect();
        v.sort_by(f64::total_cmp);
        v.dedup();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn occ_symbol_format() {
        let d = NaiveDate::from_ymd_opt(2026, 12, 18).unwrap();
        assert_eq!(OptionContract::occ_symbol("AAPL", d, OptionRight::Call, 200.0), "AAPL  261218C00200000");
        assert_eq!(OptionContract::occ_symbol("SPY", d, OptionRight::Put, 512.5), "SPY   261218P00512500");
    }
}

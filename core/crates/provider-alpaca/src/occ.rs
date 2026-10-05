//! OCC option symbols.
//!
//! Alpaca sends the unpadded form, e.g. `AAPL240426C00162500`: a 1–6
//! character root, `YYMMDD` expiry, `C`/`P`, and the strike × 1000 as eight
//! digits. The padded 21-character form (`AAPL  240426C00162500`) is accepted
//! too.

use chrono::NaiveDate;
use meridian_types::OptionRight;

/// Fields of a parsed OCC symbol.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OccSymbol {
    pub root: String,
    pub expiry: NaiveDate,
    pub right: OptionRight,
    pub strike: f64,
}

/// Length of the fixed-width tail: `YYMMDD` + right + 8-digit strike.
const TAIL: usize = 15;

pub(crate) fn parse_occ(symbol: &str) -> Option<OccSymbol> {
    let s = symbol.trim();
    if !s.is_ascii() || s.len() <= TAIL {
        return None;
    }
    let (root, tail) = s.split_at(s.len() - TAIL);
    let root = root.trim_end();
    if root.is_empty() || root.len() > 6 || !root.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let (date, rest) = tail.split_at(6);
    let (right, strike) = rest.split_at(1);
    if !date.bytes().all(|b| b.is_ascii_digit()) || !strike.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let yy: i32 = date[0..2].parse().ok()?;
    let mm: u32 = date[2..4].parse().ok()?;
    let dd: u32 = date[4..6].parse().ok()?;
    let expiry = NaiveDate::from_ymd_opt(2000 + yy, mm, dd)?;
    let right = match right {
        "C" => OptionRight::Call,
        "P" => OptionRight::Put,
        _ => return None,
    };
    let strike_milli: u64 = strike.parse().ok()?;
    Some(OccSymbol { root: root.to_ascii_uppercase(), expiry, right, strike: strike_milli as f64 / 1000.0 })
}

#[cfg(test)]
mod tests {
    use meridian_types::OptionContract;

    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn parses_doc_example() {
        let o = parse_occ("AAPL240426C00162500").unwrap();
        assert_eq!(o.root, "AAPL");
        assert_eq!(o.expiry, d(2024, 4, 26));
        assert_eq!(o.right, OptionRight::Call);
        assert_eq!(o.strike, 162.5);
        assert_eq!(OptionContract::occ_symbol(&o.root, o.expiry, o.right, o.strike), "AAPL  240426C00162500");
    }

    #[test]
    fn parses_short_and_six_char_roots() {
        let f = parse_occ("F261218P00012000").unwrap();
        assert_eq!(f.root, "F");
        assert_eq!(f.right, OptionRight::Put);
        assert_eq!(f.strike, 12.0);

        // SPXW weeklies; six-character roots such as adjusted contracts.
        let w = parse_occ("SPXW240327P04925000").unwrap();
        assert_eq!(w.root, "SPXW");
        assert_eq!(w.strike, 4925.0);
        let six = parse_occ("GOOGL1270115C00150000").unwrap();
        assert_eq!(six.root, "GOOGL1");
        assert_eq!(six.expiry, d(2027, 1, 15));
    }

    #[test]
    fn parses_fractional_strikes() {
        assert_eq!(parse_occ("SPY261218P00512500").unwrap().strike, 512.5);
        assert_eq!(parse_occ("X261218C00002125").unwrap().strike, 2.125);
        assert_eq!(parse_occ("X261218C00000500").unwrap().strike, 0.5);
    }

    #[test]
    fn accepts_padded_form() {
        let o = parse_occ("SPY   261218P00512500").unwrap();
        assert_eq!(o.root, "SPY");
        assert_eq!(o.expiry, d(2026, 12, 18));
    }

    #[test]
    fn rejects_malformed() {
        for bad in [
            "",
            "240426C00162500",
            "AAPL240426X00162500",
            "AAPL241332C00162500",
            "AAPL24042AC00162500",
            "AAPL240426C0016250A",
            "TOOLONG240426C00162500",
            "AA-L240426C00162500",
        ] {
            assert_eq!(parse_occ(bad), None, "{bad}");
        }
    }
}

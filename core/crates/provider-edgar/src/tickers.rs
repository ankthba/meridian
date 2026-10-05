//! Ticker ↔ CIK reference data from `https://www.sec.gov/files/company_tickers.json`.
//!
//! The file is an object keyed by row index (`"0"`, `"1"`, …), each value
//! `{"cik_str": <number>, "ticker": "<TICKER>", "title": "<name>"}`. The SEC
//! publishes the file (https://www.sec.gov/file/company-tickers) but not a
//! field-level schema; see the crate README for how the format was checked.

use std::collections::HashMap;

use meridian_provider::{ProviderError, ProviderResult};
use meridian_types::{MarketSector, SecurityKey};
use serde::{Deserialize, Deserializer};

/// Exchange codes accepted on an equity key: `US` (composite) and the US
/// venue codes in terminal notation. Keys on other exchanges are not mapped,
/// because the same symbol can name a different company abroad.
const US_EXCHANGES: [&str; 9] = ["US", "UN", "UW", "UQ", "UR", "UA", "UP", "UF", "UV"];

/// One row of the SEC ticker file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TickerEntry {
    pub cik: u64,
    /// Ticker as the SEC writes it (class shares use `-`, e.g. `BRK-B`).
    pub ticker: String,
    pub title: String,
}

#[derive(Deserialize)]
struct TickerRowDto {
    #[serde(deserialize_with = "de_cik")]
    cik_str: u64,
    ticker: String,
    title: String,
}

/// In-memory index over the ticker file, in file order.
#[derive(Debug, Default)]
pub(crate) struct TickerIndex {
    entries: Vec<TickerEntry>,
    by_ticker: HashMap<String, usize>,
}

impl TickerIndex {
    pub(crate) fn parse(body: &str) -> ProviderResult<Self> {
        let rows: HashMap<String, TickerRowDto> =
            crate::http::parse_json(body, "company_tickers.json")?;
        let mut keyed: Vec<(u64, TickerRowDto)> = Vec::with_capacity(rows.len());
        for (k, row) in rows {
            let idx = k.parse::<u64>().map_err(|_| {
                ProviderError::parse(format!("company_tickers.json: non-numeric row key {k:?}"))
            })?;
            keyed.push((idx, row));
        }
        keyed.sort_by_key(|(i, _)| *i);
        let mut index = TickerIndex::default();
        for (_, row) in keyed {
            let ticker = row.ticker.trim().to_ascii_uppercase();
            if ticker.is_empty() {
                continue;
            }
            let norm = normalize_ticker(&ticker);
            if index.by_ticker.contains_key(&norm) {
                continue;
            }
            index.by_ticker.insert(norm, index.entries.len());
            index.entries.push(TickerEntry {
                cik: row.cik_str,
                ticker,
                title: row.title.trim().to_owned(),
            });
        }
        Ok(index)
    }

    /// Exact lookup; `.` and `/` in `symbol` match `-` in the SEC file.
    pub(crate) fn lookup(&self, symbol: &str) -> Option<&TickerEntry> {
        self.by_ticker
            .get(&normalize_ticker(symbol))
            .map(|i| &self.entries[*i])
    }

    /// Case-insensitive search: exact ticker, then ticker prefix, then name
    /// substring, each group in file order, up to `limit`.
    pub(crate) fn search(&self, text: &str, limit: usize) -> Vec<&TickerEntry> {
        let q = text.trim();
        if q.is_empty() || limit == 0 {
            return Vec::new();
        }
        let qt = normalize_ticker(q);
        let qn = q.to_lowercase();
        let mut picked: Vec<usize> = Vec::new();
        if let Some(i) = self.by_ticker.get(&qt) {
            picked.push(*i);
        }
        let prefix = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| normalize_ticker(&e.ticker).starts_with(&qt))
            .map(|(i, _)| i);
        let title = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.title.to_lowercase().contains(&qn))
            .map(|(i, _)| i);
        for i in prefix.chain(title) {
            if picked.len() >= limit {
                break;
            }
            if !picked.contains(&i) {
                picked.push(i);
            }
        }
        picked.truncate(limit);
        picked.into_iter().map(|i| &self.entries[i]).collect()
    }
}

/// Upper-cases and maps class-share separators (`.`, `/`, space) to the
/// SEC's `-` (`BRK.B` → `BRK-B`).
pub(crate) fn normalize_ticker(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| {
            if matches!(c, '.' | '/' | ' ') {
                '-'
            } else {
                c.to_ascii_uppercase()
            }
        })
        .collect()
}

/// The symbol to look up for `key`, or `None` if EDGAR can't serve it (not an
/// equity key, or listed on a non-US exchange).
pub(crate) fn equity_symbol(key: &SecurityKey) -> Option<&str> {
    if key.sector != MarketSector::Equity {
        return None;
    }
    match key.exchange.as_deref() {
        None => Some(key.symbol.as_str()),
        Some(ex) if US_EXCHANGES.iter().any(|u| u.eq_ignore_ascii_case(ex)) => {
            Some(key.symbol.as_str())
        }
        Some(_) => None,
    }
}

/// CIK as the 10-digit zero-padded string the data.sec.gov paths use.
pub(crate) fn cik10(cik: u64) -> String {
    format!("{cik:010}")
}

/// Accepts a CIK written as a JSON number or a string of digits (the
/// submissions API writes it as a string, companyfacts and the ticker file as
/// a number).
pub(crate) fn de_cik<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Cik {
        Num(u64),
        Text(String),
    }
    match Cik::deserialize(d)? {
        Cik::Num(n) => Ok(n),
        Cik::Text(s) => s
            .trim()
            .parse()
            .map_err(|_| serde::de::Error::custom(format!("invalid CIK {s:?}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> TickerIndex {
        TickerIndex::parse(include_str!("../tests/fixtures/company_tickers.json")).unwrap()
    }

    #[test]
    fn pads_cik_to_ten_digits() {
        assert_eq!(cik10(1), "0000000001");
        assert_eq!(cik10(320_193), "0000320193");
        assert_eq!(cik10(1_234_567_890), "1234567890");
    }

    #[test]
    fn normalizes_class_share_tickers() {
        assert_eq!(normalize_ticker("brk.b"), "BRK-B");
        assert_eq!(normalize_ticker("BRK/B"), "BRK-B");
        assert_eq!(normalize_ticker("BRK-B"), "BRK-B");
        assert_eq!(normalize_ticker(" aapl "), "AAPL");
    }

    #[test]
    fn parses_file_in_row_order() {
        let idx = fixture();
        assert_eq!(idx.entries.len(), 5);
        let e = idx.lookup("EXMP").unwrap();
        assert_eq!(e.cik, 1);
        assert_eq!(e.title, "Example Corp");
    }

    #[test]
    fn looks_up_class_shares_with_any_separator() {
        let idx = fixture();
        for sym in ["SMPL.B", "smpl/b", "SMPL-B"] {
            let e = idx.lookup(sym).unwrap();
            assert_eq!(e.ticker, "SMPL-B");
            assert_eq!(e.cik, 2);
        }
        assert!(idx.lookup("SMPL").is_none());
    }

    #[test]
    fn search_ranks_exact_then_prefix_then_title() {
        let idx = fixture();
        let tickers = |q: &str| {
            idx.search(q, 10)
                .iter()
                .map(|e| e.ticker.clone())
                .collect::<Vec<_>>()
        };
        // Exact ticker first, then the name match ("Example Corp").
        assert_eq!(tickers("exa"), vec!["EXA", "EXMP"]);
        // Ticker prefixes in file order, then name substrings ("Testex").
        assert_eq!(tickers("ex"), vec!["EXMP", "EXA", "TSTX"]);
        let limited = idx.search("smpl", 1);
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].ticker, "SMPL-A");
        assert!(idx.search("  ", 5).is_empty());
        let by_name: Vec<&str> = idx
            .search("holdings", 5)
            .iter()
            .map(|e| e.ticker.as_str())
            .collect();
        assert_eq!(by_name, vec!["SMPL-A", "SMPL-B"]);
    }

    #[test]
    fn maps_only_us_equity_keys() {
        assert_eq!(equity_symbol(&SecurityKey::equity("EXMP")), Some("EXMP"));
        assert_eq!(
            equity_symbol(&SecurityKey::new("EXMP", None, MarketSector::Equity)),
            Some("EXMP")
        );
        assert_eq!(
            equity_symbol(&SecurityKey::new("EXMP", Some("UN"), MarketSector::Equity)),
            Some("EXMP")
        );
        assert_eq!(
            equity_symbol(&SecurityKey::new("EXMP", Some("LN"), MarketSector::Equity)),
            None
        );
        assert_eq!(equity_symbol(&SecurityKey::currency("EURUSD")), None);
        assert_eq!(equity_symbol(&SecurityKey::index("SPX")), None);
    }

    #[test]
    fn cik_accepts_number_or_string() {
        #[derive(Deserialize)]
        struct T {
            #[serde(deserialize_with = "de_cik")]
            cik: u64,
        }
        assert_eq!(
            serde_json::from_str::<T>(r#"{"cik":320193}"#).unwrap().cik,
            320_193
        );
        assert_eq!(
            serde_json::from_str::<T>(r#"{"cik":"0000320193"}"#)
                .unwrap()
                .cik,
            320_193
        );
        assert!(serde_json::from_str::<T>(r#"{"cik":"abc"}"#).is_err());
    }

    #[test]
    fn rejects_malformed_file() {
        let err = TickerIndex::parse(r#"{"0":{"ticker":"X"}}"#).unwrap_err();
        assert!(matches!(err, ProviderError::Parse { .. }));
    }
}

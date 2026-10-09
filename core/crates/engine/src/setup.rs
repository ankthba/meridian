//! What the user still has to set up, phrased for screens: one note per
//! missing source instead of one NOT AVAILABLE per row or section.

use meridian_provider::Capability;
use meridian_types::SecurityKey;

use crate::core::Engine;

/// Symbols listed in a note before "and N more".
const LISTED: usize = 3;

/// What each keyed source adds, for the first-run checklist.
fn unlocks(provider: &str) -> &'static str {
    match provider {
        "alpaca" => "US stock prices, charts, news, dividends and options",
        "finnhub" => "earnings dates, analyst ratings and company news",
        "fred" => "economic releases and series",
        "edgar" => "SEC filings and financial statements (your name and email, no key)",
        _ => "more data",
    }
}

impl Engine {
    /// One line per source that still needs setup, e.g. "Alpaca — US stock
    /// prices, charts, …", for screens that guide a first run. Empty when
    /// everything is set up (and always in mock mode).
    pub(crate) fn setup_checklist(&self) -> Vec<(String, String)> {
        self.router()
            .needing_setup()
            .into_iter()
            .map(|(id, msg)| (format!("{} — {}", crate::screens::source_name(id.as_str()), unlocks(id.as_str())), msg))
            .collect()
    }

    /// For `keys` whose every source for `cap` still needs setup: one line
    /// per distinct message, naming the affected symbols, e.g. "Prices for
    /// AAPL, MSFT and 9 more: Alpaca API key not set — add it in Settings".
    pub(crate) fn setup_notes(&self, what: &str, cap: Capability, keys: &[SecurityKey]) -> Vec<String> {
        let router = self.router();
        let mut groups: Vec<(String, Vec<&str>)> = Vec::new();
        for k in keys {
            let Some(msg) = router.setup_needed(cap, Some(k)) else { continue };
            match groups.iter_mut().find(|(m, _)| *m == msg) {
                Some((_, syms)) => syms.push(&k.symbol),
                None => groups.push((msg, vec![&k.symbol])),
            }
        }
        groups.into_iter().map(|(msg, syms)| format!("{what} for {}: {msg}", list(&syms))).collect()
    }
}

fn list(syms: &[&str]) -> String {
    match syms.len() {
        0 => String::new(),
        1 => syms[0].to_owned(),
        n if n <= LISTED => format!("{} and {}", syms[..n - 1].join(", "), syms[n - 1]),
        n => format!("{} and {} more", syms[..LISTED].join(", "), n - LISTED),
    }
}

#[cfg(test)]
mod tests {
    use super::list;

    #[test]
    fn lists_symbols_compactly() {
        assert_eq!(list(&["AAPL"]), "AAPL");
        assert_eq!(list(&["AAPL", "MSFT"]), "AAPL and MSFT");
        assert_eq!(list(&["AAPL", "MSFT", "NVDA"]), "AAPL, MSFT and NVDA");
        assert_eq!(list(&["AAPL", "MSFT", "NVDA", "TSLA", "SPY"]), "AAPL, MSFT, NVDA and 2 more");
    }
}

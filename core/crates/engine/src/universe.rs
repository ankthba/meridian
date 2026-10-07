//! Fixed security lists used by monitor screens (WEI, CRYP, FXC). In LIVE
//! mode each list is quoted through the router; anything no provider
//! covers shows NOT AVAILABLE rather than a substitute.

use meridian_types::SecurityKey;

/// (symbol, name, region) for WEI.
pub const WORLD_INDICES: &[(&str, &str, &str)] = &[
    ("INDU", "Dow Jones Industrial Average", "Americas"),
    ("SPX", "S&P 500", "Americas"),
    ("CCMP", "Nasdaq Composite", "Americas"),
    ("NDX", "Nasdaq 100", "Americas"),
    ("RTY", "Russell 2000", "Americas"),
    ("SPTSX", "S&P/TSX Composite", "Americas"),
    ("MEXBOL", "S&P/BMV IPC", "Americas"),
    ("IBOV", "Ibovespa", "Americas"),
    ("SX5E", "Euro Stoxx 50", "EMEA"),
    ("UKX", "FTSE 100", "EMEA"),
    ("CAC", "CAC 40", "EMEA"),
    ("DAX", "DAX", "EMEA"),
    ("IBEX", "IBEX 35", "EMEA"),
    ("FTSEMIB", "FTSE MIB", "EMEA"),
    ("AEX", "AEX", "EMEA"),
    ("SMI", "Swiss Market Index", "EMEA"),
    ("NKY", "Nikkei 225", "Asia/Pacific"),
    ("HSI", "Hang Seng", "Asia/Pacific"),
    ("SHCOMP", "Shanghai Composite", "Asia/Pacific"),
    ("KOSPI", "KOSPI", "Asia/Pacific"),
    ("AS51", "S&P/ASX 200", "Asia/Pacific"),
    ("SENSEX", "BSE Sensex", "Asia/Pacific"),
    ("TWSE", "Taiwan Weighted", "Asia/Pacific"),
    ("STI", "Straits Times", "Asia/Pacific"),
];

/// US-listed ETF shown for each [`WORLD_INDICES`] entry when no configured
/// source provides index levels (LIVE mode on the free tier): (index symbol,
/// ETF ticker, ETF name). The ETF's % change approximates the index's; most
/// country funds track an MSCI country index rather than the named
/// benchmark, and none are currency-hedged.
pub const WORLD_INDEX_PROXIES: &[(&str, &str, &str)] = &[
    ("INDU", "DIA", "SPDR Dow Jones Industrial Average ETF Trust"),
    ("SPX", "SPY", "SPDR S&P 500 ETF Trust"),
    ("CCMP", "ONEQ", "Fidelity Nasdaq Composite Index ETF"),
    ("NDX", "QQQ", "Invesco QQQ Trust"),
    ("RTY", "IWM", "iShares Russell 2000 ETF"),
    ("SPTSX", "EWC", "iShares MSCI Canada ETF"),
    ("MEXBOL", "EWW", "iShares MSCI Mexico ETF"),
    ("IBOV", "EWZ", "iShares MSCI Brazil ETF"),
    ("SX5E", "FEZ", "SPDR EURO STOXX 50 ETF"),
    ("UKX", "EWU", "iShares MSCI United Kingdom ETF"),
    ("CAC", "EWQ", "iShares MSCI France ETF"),
    ("DAX", "EWG", "iShares MSCI Germany ETF"),
    ("IBEX", "EWP", "iShares MSCI Spain ETF"),
    ("FTSEMIB", "EWI", "iShares MSCI Italy ETF"),
    ("AEX", "EWN", "iShares MSCI Netherlands ETF"),
    ("SMI", "EWL", "iShares MSCI Switzerland ETF"),
    ("NKY", "EWJ", "iShares MSCI Japan ETF"),
    ("HSI", "EWH", "iShares MSCI Hong Kong ETF"),
    ("SHCOMP", "MCHI", "iShares MSCI China ETF"),
    ("KOSPI", "EWY", "iShares MSCI South Korea ETF"),
    ("AS51", "EWA", "iShares MSCI Australia ETF"),
    ("SENSEX", "INDA", "iShares MSCI India ETF"),
    ("TWSE", "EWT", "iShares MSCI Taiwan ETF"),
    ("STI", "EWS", "iShares MSCI Singapore ETF"),
];

/// The ETF proxy for a world index symbol: (ticker, name).
#[must_use]
pub fn index_proxy(index_symbol: &str) -> Option<(&'static str, &'static str)> {
    WORLD_INDEX_PROXIES.iter().find(|(i, _, _)| *i == index_symbol).map(|(_, t, n)| (*t, *n))
}

/// Currencies on the FXC matrix, in display order.
pub const FX_MATRIX: &[&str] = &["USD", "EUR", "JPY", "GBP", "CHF", "CAD", "AUD", "NZD", "CNH", "HKD", "SGD", "SEK", "NOK", "MXN"];

/// Crypto pairs on CRYP.
pub const CRYPTO: &[(&str, &str)] = &[
    ("BTCUSD", "Bitcoin"),
    ("ETHUSD", "Ethereum"),
    ("SOLUSD", "Solana"),
    ("XRPUSD", "XRP"),
    ("ADAUSD", "Cardano"),
    ("DOGEUSD", "Dogecoin"),
    ("AVAXUSD", "Avalanche"),
    ("LINKUSD", "Chainlink"),
    ("DOTUSD", "Polkadot"),
    ("LTCUSD", "Litecoin"),
    ("BCHUSD", "Bitcoin Cash"),
    ("XLMUSD", "Stellar"),
    ("UNIUSD", "Uniswap"),
    ("ATOMUSD", "Cosmos"),
    ("AAVEUSD", "Aave"),
];

/// Default contents of the first watchlist and the W worksheet.
pub const DEFAULT_WATCHLIST: &[&str] = &[
    "AAPL US Equity",
    "MSFT US Equity",
    "NVDA US Equity",
    "AMZN US Equity",
    "GOOGL US Equity",
    "META US Equity",
    "TSLA US Equity",
    "JPM US Equity",
    "XOM US Equity",
    "SPY US Equity",
    "QQQ US Equity",
    "BTCUSD Curncy",
    "ETHUSD Curncy",
    "EURUSD Curncy",
];

/// FX key for a currency vs USD in market convention (EURUSD, USDJPY …),
/// and whether the quote is USD per unit of `ccy` (`true`) or `ccy` per USD.
#[must_use]
pub fn usd_pair(ccy: &str) -> Option<(SecurityKey, bool)> {
    match ccy {
        "USD" => None,
        "EUR" | "GBP" | "AUD" | "NZD" => Some((SecurityKey::currency(&format!("{ccy}USD")), true)),
        _ => Some((SecurityKey::currency(&format!("USD{ccy}")), false)),
    }
}

#[must_use]
pub fn index_key(symbol: &str) -> SecurityKey {
    SecurityKey::index(symbol)
}

#[must_use]
pub fn crypto_key(symbol: &str) -> SecurityKey {
    SecurityKey::currency(symbol)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn every_world_index_has_one_distinct_etf_proxy() {
        let indices: HashSet<&str> = WORLD_INDICES.iter().map(|(s, _, _)| *s).collect();
        let proxied: HashSet<&str> = WORLD_INDEX_PROXIES.iter().map(|(s, _, _)| *s).collect();
        assert_eq!(indices, proxied);
        assert_eq!(WORLD_INDEX_PROXIES.len(), WORLD_INDICES.len(), "one proxy per index");
        let tickers: HashSet<&str> = WORLD_INDEX_PROXIES.iter().map(|(_, t, _)| *t).collect();
        assert_eq!(tickers.len(), WORLD_INDEX_PROXIES.len(), "distinct ETFs");
        for (_, t, name) in WORLD_INDEX_PROXIES {
            assert!(t.chars().all(|c| c.is_ascii_uppercase()) && (2..=5).contains(&t.len()), "{t}");
            assert!(name.contains("ETF") || name.contains("Trust"), "{name}");
        }
        assert_eq!(index_proxy("SPX"), Some(("SPY", "SPDR S&P 500 ETF Trust")));
        assert_eq!(index_proxy("NOPE"), None);
    }

    #[test]
    fn plain_language_coins_are_the_crypto_monitor_list() {
        let bases: HashSet<&str> = CRYPTO.iter().map(|(t, _)| t.trim_end_matches("USD")).collect();
        let coins: HashSet<&str> = meridian_command::MAJOR_COINS.into_iter().collect();
        assert_eq!(bases, coins, "keep MAJOR_COINS in the command crate equal to CRYPTO");
    }
}

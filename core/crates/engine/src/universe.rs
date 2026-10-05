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

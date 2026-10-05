//! Security key <-> Kraken pair mapping.
//!
//! `BTCUSD Curncy` is the WS v2 / display pair `BTC/USD`, whose REST name
//! (`altname`) is `XBTUSD`; both come from `AssetPairs?assetVersion=1`. A
//! terminal symbol is split by its quote suffix, 4-letter quotes first, so
//! `BTCUSDT` is `BTC/USDT` and `USDCUSD` is `USDC/USD`.

use meridian_types::{MarketSector, SecurityKey};

use crate::catalog::{Catalog, Pair};

/// Covered quote currencies, longest first. EUR is included because Kraken
/// lists most crypto against EUR.
pub(crate) const QUOTES: [&str; 4] = ["USDT", "USDC", "USD", "EUR"];

/// Fiat codes that are never a crypto base. Keeps FX pairs such as
/// `EURUSD Curncy` (which Kraken also lists) with the FX providers.
const FIAT: [&str; 24] = [
    "USD", "EUR", "GBP", "JPY", "CHF", "CAD", "AUD", "NZD", "SGD", "HKD", "CNY", "CNH", "INR",
    "BRL", "MXN", "ZAR", "SEK", "NOK", "DKK", "PLN", "TRY", "KRW", "AED", "ILS",
];

const MAX_SYMBOL_LEN: usize = 24;

/// Possible `(base, quote)` splits of a terminal symbol, longest quote first.
pub(crate) fn splits(symbol: &str) -> Vec<(&str, &str)> {
    if symbol.is_empty()
        || symbol.len() > MAX_SYMBOL_LEN
        || !symbol
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        return Vec::new();
    }
    QUOTES
        .iter()
        .filter_map(|q| {
            let base = symbol.strip_suffix(q)?;
            (!base.is_empty() && !FIAT.contains(&base)).then_some((base, *q))
        })
        .collect()
}

/// The cheap rule used before the pair list is loaded.
pub(crate) fn statically_covered(key: &SecurityKey) -> bool {
    key.sector == MarketSector::Curncy && key.exchange.is_none() && !splits(&key.symbol).is_empty()
}

/// WS v2 / display pair name.
pub(crate) fn pair_symbol(base: &str, quote: &str) -> String {
    format!("{base}/{quote}")
}

/// Resolves a key to a pair. With a catalog, only listed pairs match;
/// without one, the first plausible split is returned (WS name only).
pub(crate) fn resolve(key: &SecurityKey, catalog: Option<&Catalog>) -> Option<Pair> {
    if !statically_covered(key) {
        return None;
    }
    let candidates = splits(&key.symbol);
    match catalog {
        Some(cat) => candidates
            .iter()
            .find_map(|(b, q)| cat.get(&pair_symbol(b, q)).cloned()),
        None => candidates.first().map(|(b, q)| Pair::unlisted(b, q)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        Catalog::from_result(
            &serde_json::from_str::<crate::dto::Envelope>(include_str!(
                "../tests/fixtures/asset_pairs.json"
            ))
            .unwrap()
            .result
            .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn splits_prefer_longest_quote() {
        assert_eq!(splits("BTCUSD"), vec![("BTC", "USD")]);
        assert_eq!(splits("BTCUSDT"), vec![("BTC", "USDT")]);
        assert_eq!(splits("USDCUSD"), vec![("USDC", "USD")]);
        assert_eq!(splits("USDTUSD"), vec![("USDT", "USD")]);
        assert_eq!(splits("BTCEUR"), vec![("BTC", "EUR")]);
        assert_eq!(splits("USDTEUR"), vec![("USDT", "EUR")]);
    }

    #[test]
    fn splits_reject_fiat_bases_and_junk() {
        assert!(splits("EURUSD").is_empty());
        assert!(splits("USDEUR").is_empty());
        assert!(splits("BTCGBP").is_empty());
        assert!(splits("BTC/USD").is_empty());
        assert!(splits("USD").is_empty());
    }

    #[test]
    fn resolve_against_catalog() {
        let cat = catalog();
        let p = resolve(&SecurityKey::currency("BTCUSD"), Some(&cat)).unwrap();
        assert_eq!(p.symbol, "BTC/USD");
        assert_eq!(p.altname.as_deref(), Some("XBTUSD"));
        let p = resolve(&SecurityKey::currency("USDCUSD"), Some(&cat)).unwrap();
        assert_eq!(
            (p.base.as_str(), p.quote.as_str(), p.symbol.as_str()),
            ("USDC", "USD", "USDC/USD")
        );
        assert_eq!(
            resolve(&SecurityKey::currency("DOGEUSD"), Some(&cat))
                .unwrap()
                .altname
                .as_deref(),
            Some("XDGUSD")
        );
        assert_eq!(
            resolve(&SecurityKey::currency("BTCUSDC"), Some(&cat))
                .unwrap()
                .symbol,
            "BTC/USDC"
        );
        assert_eq!(
            resolve(&SecurityKey::currency("BTCEUR"), Some(&cat))
                .unwrap()
                .symbol,
            "BTC/EUR"
        );
        // Listed, but FX or an uncovered quote.
        assert!(resolve(&SecurityKey::currency("EURUSD"), Some(&cat)).is_none());
        assert!(resolve(&SecurityKey::currency("ETHBTC"), Some(&cat)).is_none());
        // Not in the (trimmed) pair list.
        assert!(resolve(&SecurityKey::currency("SOLUSD"), Some(&cat)).is_none());
    }

    #[test]
    fn resolve_without_catalog_uses_static_split() {
        let p = resolve(&SecurityKey::currency("SOLUSD"), None).unwrap();
        assert_eq!(p.symbol, "SOL/USD");
        assert_eq!(p.altname, None);
        assert!(
            resolve(
                &SecurityKey::new("BTCUSD", Some("US"), MarketSector::Curncy),
                None
            )
            .is_none()
        );
    }
}

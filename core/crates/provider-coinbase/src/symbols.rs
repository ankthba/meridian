//! Security key <-> Coinbase product mapping.
//!
//! `BTCUSD Curncy` is product `BTC-USD`. A terminal symbol is split into base
//! and quote by its quote-currency suffix. Covered quotes are USD and the two
//! USD stablecoins; the 4-letter ones are tried first so `BTCUSDT` is
//! `BTC-USDT`, never `BTCU-SDT`, and `USDCUSD` is `USDC-USD`.

use meridian_types::{MarketSector, SecurityKey};

use crate::catalog::{Catalog, Product};

/// Covered quote currencies, longest first.
pub(crate) const QUOTES: [&str; 3] = ["USDT", "USDC", "USD"];

/// Fiat codes that are never a crypto base. Keeps FX pairs such as
/// `EURUSD Curncy` with the FX providers.
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

/// The cheap rule used before the product list is loaded.
pub(crate) fn statically_covered(key: &SecurityKey) -> bool {
    key.sector == MarketSector::Curncy && key.exchange.is_none() && !splits(&key.symbol).is_empty()
}

/// Coinbase product id for a base/quote pair.
pub(crate) fn product_id(base: &str, quote: &str) -> String {
    format!("{base}-{quote}")
}

/// Resolves a key to a product. With a catalog, only listed products match;
/// without one, the first plausible split is returned.
pub(crate) fn resolve(key: &SecurityKey, catalog: Option<&Catalog>) -> Option<Product> {
    if !statically_covered(key) {
        return None;
    }
    let candidates = splits(&key.symbol);
    match catalog {
        Some(cat) => candidates
            .iter()
            .find_map(|(b, q)| cat.get(&product_id(b, q)).cloned()),
        None => candidates.first().map(|(b, q)| Product::unlisted(b, q)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::dto::ProductDto;

    fn catalog() -> Catalog {
        let dtos: Vec<ProductDto> =
            serde_json::from_str(include_str!("../tests/fixtures/products.json")).unwrap();
        Catalog::from_dtos(dtos)
    }

    #[test]
    fn splits_prefer_longest_quote() {
        assert_eq!(splits("BTCUSD"), vec![("BTC", "USD")]);
        assert_eq!(splits("BTCUSDT"), vec![("BTC", "USDT")]);
        assert_eq!(splits("BTCUSDC"), vec![("BTC", "USDC")]);
        assert_eq!(splits("USDCUSD"), vec![("USDC", "USD")]);
        assert_eq!(splits("USDTUSDC"), vec![("USDT", "USDC")]);
        assert_eq!(splits("USDTUSD"), vec![("USDT", "USD")]);
    }

    #[test]
    fn splits_reject_fiat_bases_and_junk() {
        assert!(splits("EURUSD").is_empty());
        assert!(splits("USD").is_empty());
        assert!(splits("USDUSDT").is_empty());
        assert!(splits("BTCEUR").is_empty());
        assert!(splits("BTC-USD").is_empty());
        assert!(splits("btcusd").is_empty());
        assert!(splits("").is_empty());
    }

    #[test]
    fn static_coverage_needs_curncy_without_exchange() {
        assert!(statically_covered(&SecurityKey::currency("BTCUSD")));
        assert!(!statically_covered(&SecurityKey::currency("EURUSD")));
        assert!(!statically_covered(&SecurityKey::equity("BTCUSD")));
        assert!(!statically_covered(&SecurityKey::new(
            "BTCUSD",
            Some("US"),
            MarketSector::Curncy
        )));
    }

    #[test]
    fn resolve_against_catalog() {
        let cat = catalog();
        let p = resolve(&SecurityKey::currency("BTCUSD"), Some(&cat)).unwrap();
        assert_eq!(p.id, "BTC-USD");
        let p = resolve(&SecurityKey::currency("USDTUSDC"), Some(&cat)).unwrap();
        assert_eq!((p.base.as_str(), p.quote.as_str()), ("USDT", "USDC"));
        // Listed on Coinbase but with a quote we don't cover.
        assert!(resolve(&SecurityKey::currency("ETHBTC"), Some(&cat)).is_none());
        // Delisted products are not in the catalog.
        assert!(resolve(&SecurityKey::currency("DNTUSDC"), Some(&cat)).is_none());
        assert!(resolve(&SecurityKey::currency("BTCUSDC"), Some(&cat)).is_none());
        // USDC-USD is not a Coinbase Exchange product.
        assert!(resolve(&SecurityKey::currency("USDCUSD"), Some(&cat)).is_none());
    }

    #[test]
    fn resolve_without_catalog_uses_static_split() {
        let p = resolve(&SecurityKey::currency("USDCUSD"), None).unwrap();
        assert_eq!(p.id, "USDC-USD");
        assert!(resolve(&SecurityKey::currency("EURUSD"), None).is_none());
    }
}

//! The product list (cached in memory) plus search and instrument building.

use std::collections::HashMap;

use meridian_types::{AssetClass, Instrument, SecurityKey};

use crate::dto::{CurrencyDto, ProductDto};
pub(crate) const EXCHANGE_NAME: &str = "Coinbase Exchange";

/// Search result order among equally good matches: USD, then the
/// stablecoins.
const QUOTE_RANK: [&str; 3] = ["USD", "USDT", "USDC"];

/// One tradable product, reduced to what we use.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Product {
    pub id: String,
    pub base: String,
    pub quote: String,
    /// Minimum price increment (`quote_increment`).
    pub tick_size: Option<f64>,
    /// Decimal places of `quote_increment`.
    pub price_decimals: Option<u8>,
}

impl Product {
    /// A product inferred from a symbol before the list was loaded.
    pub(crate) fn unlisted(base: &str, quote: &str) -> Self {
        Self {
            id: format!("{base}-{quote}"),
            base: base.to_owned(),
            quote: quote.to_owned(),
            tick_size: None,
            price_decimals: None,
        }
    }

    pub(crate) fn symbol(&self) -> String {
        format!("{}{}", self.base, self.quote)
    }

    pub(crate) fn key(&self) -> SecurityKey {
        SecurityKey::currency(&self.symbol())
    }
}

/// Non-delisted products by id (`BTC-USD`).
#[derive(Debug, Clone, Default)]
pub(crate) struct Catalog {
    products: HashMap<String, Product>,
}

impl Catalog {
    pub(crate) fn from_dtos(dtos: Vec<ProductDto>) -> Self {
        let products = dtos
            .into_iter()
            .filter(|d| d.status.as_deref() != Some("delisted"))
            .map(|d| {
                let inc = d.quote_increment.as_deref();
                let product = Product {
                    tick_size: inc
                        .and_then(|s| s.trim().parse::<f64>().ok())
                        .filter(|v| v.is_finite() && *v > 0.0),
                    price_decimals: inc.and_then(decimals_of),
                    id: d.id,
                    base: d.base_currency.to_ascii_uppercase(),
                    quote: d.quote_currency.to_ascii_uppercase(),
                };
                (product.id.clone(), product)
            })
            .collect();
        Self { products }
    }

    pub(crate) fn get(&self, id: &str) -> Option<&Product> {
        self.products.get(id)
    }

    pub(crate) fn len(&self) -> usize {
        self.products.len()
    }

    /// Products whose quote we cover.
    fn covered(&self) -> impl Iterator<Item = &Product> {
        self.products.values().filter(|p| {
            crate::symbols::splits(&p.symbol()).contains(&(p.base.as_str(), p.quote.as_str()))
        })
    }
}

/// Currency id -> full name (`BTC` -> `Bitcoin`).
pub(crate) fn names_from_dtos(dtos: Vec<CurrencyDto>) -> HashMap<String, String> {
    dtos.into_iter()
        .filter_map(|c| {
            let name = c.name?.trim().to_owned();
            (!name.is_empty()).then(|| (c.id.to_ascii_uppercase(), name))
        })
        .collect()
}

/// Decimal places of a decimal string such as `0.01000000` (2).
pub(crate) fn decimals_of(s: &str) -> Option<u8> {
    let s = s.trim();
    let v: f64 = s.parse().ok()?;
    if !v.is_finite() || v <= 0.0 {
        return None;
    }
    let d = match s.split_once('.') {
        Some((_, frac)) => frac.trim_end_matches('0').len(),
        None => 0,
    };
    u8::try_from(d).ok()
}

pub(crate) fn instrument(
    key: SecurityKey,
    p: &Product,
    names: &HashMap<String, String>,
) -> Instrument {
    let name = match names.get(&p.base) {
        Some(n) => format!("{n} / {}", p.quote),
        None => format!("{}/{}", p.base, p.quote),
    };
    let mut inst = Instrument::basic(key, name, AssetClass::Crypto, &p.quote);
    if let Some(t) = p.tick_size {
        inst.tick_size = t;
    }
    if let Some(d) = p.price_decimals {
        inst.price_decimals = d;
    }
    inst.exchange_name = Some(EXCHANGE_NAME.to_owned());
    inst
}

/// Case-insensitive match on symbol, base, quote, product id, and currency
/// name. Best matches first; at most `limit` results.
pub(crate) fn search(
    cat: &Catalog,
    names: &HashMap<String, String>,
    text: &str,
    limit: usize,
) -> Vec<Instrument> {
    let upper = text.trim().to_ascii_uppercase();
    let upper = ["CURNCY", "CRNCY", "CURRENCY"]
        .iter()
        .find_map(|s| upper.strip_suffix(s))
        .map_or(upper.as_str(), str::trim_end)
        .to_owned();
    let compact: String = upper.chars().filter(char::is_ascii_alphanumeric).collect();
    if compact.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut hits: Vec<(u8, usize, String, &Product)> = cat
        .covered()
        .filter_map(|p| {
            let sym = p.symbol();
            let name = names
                .get(&p.base)
                .map_or_else(String::new, |n| n.to_ascii_uppercase());
            let score = if sym == compact {
                0
            } else if p.base == compact {
                1
            } else if sym.starts_with(&compact) {
                2
            } else if !name.is_empty() && name == upper {
                3
            } else if !name.is_empty() && name.starts_with(&upper) {
                4
            } else if sym.contains(&compact) || (!name.is_empty() && name.contains(&upper)) {
                5
            } else {
                return None;
            };
            let quote_rank = QUOTE_RANK
                .iter()
                .position(|q| *q == p.quote)
                .unwrap_or(usize::MAX);
            Some((score, quote_rank, sym, p))
        })
        .collect();
    hits.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
    hits.into_iter()
        .take(limit)
        .map(|(_, _, _, p)| instrument(p.key(), p, names))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> (Catalog, HashMap<String, String>) {
        let dtos: Vec<ProductDto> =
            serde_json::from_str(include_str!("../tests/fixtures/products.json")).unwrap();
        let cur: Vec<CurrencyDto> =
            serde_json::from_str(include_str!("../tests/fixtures/currencies.json")).unwrap();
        (Catalog::from_dtos(dtos), names_from_dtos(cur))
    }

    #[test]
    fn catalog_drops_delisted() {
        let (cat, _) = fixtures();
        assert!(cat.get("BTC-USD").is_some());
        // DNT-USDC and BTC-USDC are delisted on Coinbase Exchange.
        assert!(cat.get("DNT-USDC").is_none());
        assert!(cat.get("BTC-USDC").is_none());
        assert_eq!(cat.len(), 7);
        let btc = cat.get("BTC-USD").unwrap();
        assert_eq!(btc.tick_size, Some(0.01));
        assert_eq!(btc.price_decimals, Some(2));
        assert_eq!(cat.get("USDT-USD").unwrap().price_decimals, Some(5));
    }

    #[test]
    fn decimals() {
        assert_eq!(decimals_of("0.01000000"), Some(2));
        assert_eq!(decimals_of("0.00001"), Some(5));
        assert_eq!(decimals_of("1"), Some(0));
        assert_eq!(decimals_of("0"), None);
        assert_eq!(decimals_of("x"), None);
    }

    #[test]
    fn instrument_fields() {
        let (cat, names) = fixtures();
        let p = cat.get("BTC-USD").unwrap();
        let i = instrument(p.key(), p, &names);
        assert_eq!(i.key.to_string(), "BTCUSD Curncy");
        assert_eq!(i.name, "Bitcoin / USD");
        assert_eq!(i.currency, "USD");
        assert_eq!(i.asset_class, AssetClass::Crypto);
        assert_eq!(i.exchange_name.as_deref(), Some(EXCHANGE_NAME));
        assert_eq!(i.price_decimals, 2);
        assert!(!i.is_synthetic);
    }

    #[test]
    fn search_ranks_and_filters() {
        let (cat, names) = fixtures();
        let r = search(&cat, &names, "btc", 10);
        let syms: Vec<String> = r.iter().map(|i| i.key.symbol.clone()).collect();
        // ETH-BTC (BTC quote) is not covered; BTC-USDC is delisted.
        assert_eq!(syms, vec!["BTCUSD", "BTCUSDT"]);
        let r = search(&cat, &names, "bitcoin", 1);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].key.symbol, "BTCUSD");
        let r = search(&cat, &names, "BTC-USDT", 5);
        assert_eq!(r[0].key.symbol, "BTCUSDT");
        let r = search(&cat, &names, "ethusd curncy", 5);
        assert_eq!(r[0].key.symbol, "ETHUSD");
        assert!(search(&cat, &names, "  ", 5).is_empty());
        assert!(search(&cat, &names, "zzzz", 5).is_empty());
    }
}

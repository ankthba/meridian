//! The pair list (cached in memory) plus search and instrument building.

use std::collections::HashMap;

use meridian_provider::{ProviderError, ProviderResult};
use meridian_types::{AssetClass, Instrument, SecurityKey};

use crate::dto::PairDto;

pub(crate) const EXCHANGE_NAME: &str = "Kraken";

/// Search result order among equally good matches.
const QUOTE_RANK: [&str; 4] = ["USD", "USDT", "USDC", "EUR"];

/// One spot pair, reduced to what we use.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Pair {
    /// WS v2 / display name, `BTC/USD`.
    pub symbol: String,
    /// REST name, `XBTUSD`. `None` only for pairs inferred before the list
    /// was loaded.
    pub altname: Option<String>,
    pub base: String,
    pub quote: String,
    pub tick_size: Option<f64>,
    /// `pair_decimals`: decimal places for prices.
    pub price_decimals: Option<u8>,
}

impl Pair {
    /// A pair inferred from a symbol before the list was loaded.
    pub(crate) fn unlisted(base: &str, quote: &str) -> Self {
        Self {
            symbol: format!("{base}/{quote}"),
            altname: None,
            base: base.to_owned(),
            quote: quote.to_owned(),
            tick_size: None,
            price_decimals: None,
        }
    }

    pub(crate) fn terminal_symbol(&self) -> String {
        format!("{}{}", self.base, self.quote)
    }

    pub(crate) fn key(&self) -> SecurityKey {
        SecurityKey::currency(&self.terminal_symbol())
    }
}

/// Pairs by WS v2 name.
#[derive(Debug, Clone, Default)]
pub(crate) struct Catalog {
    pairs: HashMap<String, Pair>,
}

impl Catalog {
    /// Builds from the `result` of `AssetPairs?assetVersion=1`, whose keys
    /// are display names (`BTC/USD`) equal to `base/quote`.
    pub(crate) fn from_result(result: &serde_json::Value) -> ProviderResult<Self> {
        let dtos: HashMap<String, PairDto> = serde_json::from_value(result.clone())
            .map_err(|e| ProviderError::parse(format!("kraken AssetPairs: {e}")))?;
        let total = dtos.len();
        let pairs: HashMap<String, Pair> = dtos
            .into_iter()
            .filter_map(|(name, d)| {
                let base = d.base.to_ascii_uppercase();
                let quote = d.quote.to_ascii_uppercase();
                if name != format!("{base}/{quote}") {
                    tracing::debug!(%name, "kraken: pair without a display name; skipping");
                    return None;
                }
                let price_decimals = d.pair_decimals.and_then(|p| u8::try_from(p).ok());
                let tick_size = d
                    .tick_size
                    .as_deref()
                    .and_then(|s| s.trim().parse::<f64>().ok())
                    .filter(|v| v.is_finite() && *v > 0.0)
                    .or_else(|| price_decimals.map(|p| 10f64.powi(-i32::from(p))));
                Some((
                    name.clone(),
                    Pair {
                        symbol: name,
                        altname: d.altname,
                        base,
                        quote,
                        tick_size,
                        price_decimals,
                    },
                ))
            })
            .collect();
        if total > 0 && pairs.is_empty() {
            return Err(ProviderError::parse(
                "kraken AssetPairs: no display-name (BASE/QUOTE) pairs; assetVersion=1 ignored?",
            ));
        }
        Ok(Self { pairs })
    }

    pub(crate) fn get(&self, symbol: &str) -> Option<&Pair> {
        self.pairs.get(symbol)
    }

    pub(crate) fn len(&self) -> usize {
        self.pairs.len()
    }

    /// Pairs whose quote and base we cover.
    fn covered(&self) -> impl Iterator<Item = &Pair> {
        self.pairs.values().filter(|p| {
            crate::symbols::splits(&p.terminal_symbol())
                .contains(&(p.base.as_str(), p.quote.as_str()))
        })
    }
}

pub(crate) fn instrument(key: SecurityKey, p: &Pair) -> Instrument {
    let mut inst = Instrument::basic(key, p.symbol.clone(), AssetClass::Crypto, &p.quote);
    if let Some(t) = p.tick_size {
        inst.tick_size = t;
    }
    if let Some(d) = p.price_decimals {
        inst.price_decimals = d;
    }
    inst.exchange_name = Some(EXCHANGE_NAME.to_owned());
    inst
}

/// Case-insensitive match on symbol, base, quote, and display name. Kraken
/// publishes no full asset names. Best matches first; at most `limit`.
pub(crate) fn search(cat: &Catalog, text: &str, limit: usize) -> Vec<Instrument> {
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
    let mut hits: Vec<(u8, usize, String, &Pair)> = cat
        .covered()
        .filter_map(|p| {
            let sym = p.terminal_symbol();
            let score = if sym == compact {
                0
            } else if p.base == compact {
                1
            } else if sym.starts_with(&compact) {
                2
            } else if sym.contains(&compact) {
                3
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
        .map(|(_, _, _, p)| instrument(p.key(), p))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::Envelope;

    fn fixture() -> Catalog {
        let env: Envelope =
            serde_json::from_str(include_str!("../tests/fixtures/asset_pairs.json")).unwrap();
        Catalog::from_result(&env.result.unwrap()).unwrap()
    }

    #[test]
    fn catalog_fields() {
        let cat = fixture();
        assert_eq!(cat.len(), 10);
        let btc = cat.get("BTC/USD").unwrap();
        assert_eq!(btc.altname.as_deref(), Some("XBTUSD"));
        assert_eq!(btc.tick_size, Some(0.1));
        assert_eq!(btc.price_decimals, Some(1));
        assert_eq!(cat.get("USDT/USD").unwrap().price_decimals, Some(5));
    }

    #[test]
    fn rejects_internal_names_only() {
        let v = serde_json::json!({"XXBTZUSD": {"altname": "XBTUSD", "base": "XXBT", "quote": "ZUSD", "pair_decimals": 1}});
        assert!(matches!(
            Catalog::from_result(&v),
            Err(ProviderError::Parse { .. })
        ));
        assert_eq!(
            Catalog::from_result(&serde_json::json!({})).unwrap().len(),
            0
        );
    }

    #[test]
    fn instrument_fields() {
        let cat = fixture();
        let p = cat.get("BTC/USD").unwrap();
        let i = instrument(p.key(), p);
        assert_eq!(i.key.to_string(), "BTCUSD Curncy");
        assert_eq!(i.name, "BTC/USD");
        assert_eq!(i.currency, "USD");
        assert_eq!(i.asset_class, AssetClass::Crypto);
        assert_eq!(i.exchange_name.as_deref(), Some("Kraken"));
        assert_eq!((i.tick_size, i.price_decimals), (0.1, 1));
    }

    #[test]
    fn search_ranks_and_filters() {
        let cat = fixture();
        let syms: Vec<String> = search(&cat, "btc", 10)
            .into_iter()
            .map(|i| i.key.symbol)
            .collect();
        // ETH/BTC (BTC quote) is not covered.
        assert_eq!(syms, vec!["BTCUSD", "BTCUSDT", "BTCUSDC", "BTCEUR"]);
        assert_eq!(search(&cat, "BTC/USDT", 5)[0].key.symbol, "BTCUSDT");
        assert_eq!(search(&cat, "dogeusd curncy", 5)[0].key.symbol, "DOGEUSD");
        assert_eq!(search(&cat, "btc", 2).len(), 2);
        // EUR/USD is FX, not crypto.
        assert!(search(&cat, "eurusd", 5).is_empty());
        assert!(search(&cat, "", 5).is_empty());
    }
}

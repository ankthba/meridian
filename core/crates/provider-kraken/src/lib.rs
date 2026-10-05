//! Kraken spot public market data: REST (`AssetPairs`, `Ticker`, `OHLC`)
//! and the WebSocket v2 `ticker` channel. No credentials.
//!
//! Built against the docs listed in `README.md`, which also documents the
//! field mapping (rolling 24 h figures), coverage rule, the 720-candle
//! history limit, and rate limits.

mod bars;
mod catalog;
mod dto;
mod http;
mod normalize;
mod stream;
mod symbols;

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use meridian_provider::{
    AiPolicy, BarsRequest, CachePolicy, Capabilities, Capability, CapabilityEntry,
    InstrumentQuery, Provider, ProviderError, ProviderResult, RateLimit, StreamHandle, StreamSink,
    StreamingProvider, TokenBucket,
};
use meridian_types::{
    Adjustment, AssetClass, BarSeries, Clock, DataDelay, FeedSource, Instrument, MarketSector,
    ProviderId, Quote, SecurityKey, SystemClock,
};
use parking_lot::RwLock;

use crate::catalog::{Catalog, Pair};
use crate::dto::{Envelope, OhlcRow, TickerDto};

/// Stable provider id.
pub const PROVIDER_ID: &str = "kraken";

const REST_BASE: &str = "https://api.kraken.com/0/public";
const DOCS_URL: &str = "https://docs.kraken.com/api/docs/guides/spot-rest-intro";
/// The pair list is refreshed after this age.
const CATALOG_TTL: Duration = Duration::from_secs(24 * 3_600);
/// Kraken: "calling the public endpoints at a frequency of 1 per second (or
/// less) would remain within the rate limits".
const DOCUMENTED_LIMIT: RateLimit = RateLimit {
    burst: 1,
    per_second: 1.0,
};

/// Kraken spot market data (public endpoints only).
pub struct KrakenProvider {
    inner: Arc<Inner>,
    caps: Capabilities,
}

struct Loaded<T> {
    value: Arc<T>,
    at: Instant,
}

/// State shared with the stream task.
pub(crate) struct Inner {
    http: reqwest::Client,
    limiter: TokenBucket,
    catalog: RwLock<Option<Loaded<Catalog>>>,
    load_lock: tokio::sync::Mutex<()>,
    /// Feed URL; a local server in tests.
    ws_url: String,
}

impl Inner {
    /// Calls a public endpoint and unwraps the `{error, result}` envelope.
    /// Kraken reports errors in `error` with HTTP 200; `W`-prefixed entries
    /// are warnings and only logged.
    async fn call(
        &self,
        endpoint: &str,
        query: &[(&str, String)],
    ) -> ProviderResult<serde_json::Value> {
        self.limiter.acquire().await;
        let body = http::send_text(
            self.http
                .get(format!("{REST_BASE}/{endpoint}"))
                .query(query),
        )
        .await?;
        let env: Envelope = http::parse_json(&body, &format!("kraken {endpoint}"))?;
        for e in &env.error {
            if e.starts_with('W') {
                tracing::debug!(endpoint, warning = %e, "kraken warning");
            } else {
                return Err(normalize::api_error(e));
            }
        }
        env.result
            .ok_or_else(|| ProviderError::parse(format!("kraken {endpoint}: missing result")))
    }

    /// The pair list if loaded, however old. Never blocks on I/O.
    pub(crate) fn catalog_now(&self) -> Option<Arc<Catalog>> {
        self.catalog.read().as_ref().map(|l| l.value.clone())
    }

    fn fresh_catalog(&self) -> Option<Arc<Catalog>> {
        self.catalog
            .read()
            .as_ref()
            .filter(|l| l.at.elapsed() < CATALOG_TTL)
            .map(|l| l.value.clone())
    }

    /// Loads (or refreshes) the pair list. A failed refresh keeps serving
    /// the previous list.
    pub(crate) async fn catalog(&self) -> ProviderResult<Arc<Catalog>> {
        if let Some(c) = self.fresh_catalog() {
            return Ok(c);
        }
        let _guard = self.load_lock.lock().await;
        if let Some(c) = self.fresh_catalog() {
            return Ok(c);
        }
        let loaded = match self
            .call("AssetPairs", &[("assetVersion", "1".to_owned())])
            .await
        {
            Ok(result) => Catalog::from_result(&result),
            Err(e) => Err(e),
        };
        match loaded {
            Ok(cat) => {
                let cat = Arc::new(cat);
                tracing::debug!(pairs = cat.len(), "kraken pair list loaded");
                *self.catalog.write() = Some(Loaded {
                    value: cat.clone(),
                    at: Instant::now(),
                });
                Ok(cat)
            }
            Err(e) => match self.catalog_now() {
                Some(stale) => {
                    tracing::warn!(error = %e, "kraken: pair list refresh failed; using the previous list");
                    Ok(stale)
                }
                None => Err(e),
            },
        }
    }
}

#[cfg(test)]
impl Inner {
    /// Offline instance: fixture pair list, feed at `ws_url`.
    pub(crate) fn for_tests(ws_url: String) -> Self {
        let env: Envelope =
            serde_json::from_str(include_str!("../tests/fixtures/asset_pairs.json")).unwrap();
        let cat = Catalog::from_result(&env.result.unwrap()).unwrap();
        Self {
            http: reqwest::Client::new(),
            limiter: TokenBucket::new(DOCUMENTED_LIMIT),
            catalog: RwLock::new(Some(Loaded {
                value: Arc::new(cat),
                at: Instant::now(),
            })),
            load_lock: tokio::sync::Mutex::new(()),
            ws_url,
        }
    }
}

fn not_found(key: &SecurityKey) -> ProviderError {
    ProviderError::NotFound(format!("{key} is not a Kraken spot pair"))
}

/// REST name for a pair (`XBTUSD`); the display name also works.
fn rest_name(p: &Pair) -> String {
    p.altname.clone().unwrap_or_else(|| p.symbol.clone())
}

/// The value under `symbol` in a per-pair result map, or the only pair
/// entry if Kraken keyed it differently.
fn pair_entry(
    result: &serde_json::Value,
    symbol: &str,
    context: &str,
) -> ProviderResult<serde_json::Value> {
    let obj = result
        .as_object()
        .ok_or_else(|| ProviderError::parse(format!("{context}: result is not an object")))?;
    if let Some(v) = obj.get(symbol) {
        return Ok(v.clone());
    }
    let mut entries = obj.iter().filter(|(k, _)| k.as_str() != "last");
    match (entries.next(), entries.next()) {
        (Some((_, v)), None) => Ok(v.clone()),
        _ => Err(ProviderError::parse(format!(
            "{context}: no entry for {symbol}"
        ))),
    }
}

fn capabilities() -> Capabilities {
    let entry = |capability, history: Option<&str>| CapabilityEntry {
        capability,
        asset_classes: vec![AssetClass::Crypto],
        delay: DataDelay::RealTime,
        source: FeedSource::Exchange(normalize::FEED.to_owned()),
        history: history.map(str::to_owned),
    };
    Capabilities {
        entries: vec![
            entry(Capability::Search, None),
            entry(Capability::Reference, None),
            entry(Capability::Quotes, None),
            entry(
                Capability::DailyBars,
                Some("last 720 candles only: daily ~2 years, weekly (native, Thursday-start) ~13 years, monthly ~23 months"),
            ),
            entry(Capability::IntradayBars, Some("last 720 candles only: 1m = 12 hours, 1h = 30 days, 4h = 120 days")),
            entry(Capability::Stream, None),
        ],
        rate_limit: Some(DOCUMENTED_LIMIT),
        max_stream_symbols: None,
        cache_policy: CachePolicy::Unrestricted,
        attribution: None,
        display_allowed: true,
        ai_policy: AiPolicy::Unreviewed,
        requires_credentials: false,
        terms_note: "Kraken Global Terms of Service (sections 8-9): Kraken content may not be distributed, sold, or \
                     made available to third parties, and data extraction other than through the provided API is \
                     prohibited. The public market-data API needs no account. No retention limit found, so caching \
                     is treated as unrestricted for personal use; the terms are general rather than API-specific, so \
                     this is a reading, not an explicit grant. Sending data to a third-party AI model is not \
                     addressed: unreviewed."
            .to_owned(),
        docs_url: DOCS_URL.to_owned(),
    }
}

impl KrakenProvider {
    pub fn new() -> ProviderResult<Self> {
        Ok(Self {
            inner: Arc::new(Inner {
                http: http::client()?,
                limiter: TokenBucket::new(DOCUMENTED_LIMIT),
                catalog: RwLock::new(None),
                load_lock: tokio::sync::Mutex::new(()),
                ws_url: stream::WS_URL.to_owned(),
            }),
            caps: capabilities(),
        })
    }

    /// Whether this provider serves `key`: `<BASE><QUOTE> Curncy` (no
    /// exchange code) with QUOTE in USD/USDT/USDC/EUR and a non-fiat base,
    /// and, once the pair list has been loaded, `BASE/QUOTE` listed on
    /// Kraken. Never blocks or does I/O.
    pub fn covers(&self, key: &SecurityKey) -> bool {
        match self.inner.catalog_now() {
            Some(cat) => symbols::resolve(key, Some(&cat)).is_some(),
            None => symbols::statically_covered(key),
        }
    }

    async fn pair(&self, key: &SecurityKey) -> ProviderResult<Pair> {
        if !symbols::statically_covered(key) {
            return Err(not_found(key));
        }
        let cat = self.inner.catalog().await?;
        symbols::resolve(key, Some(&cat)).ok_or_else(|| not_found(key))
    }
}

#[async_trait]
impl Provider for KrakenProvider {
    fn covers(&self, key: &SecurityKey) -> bool {
        KrakenProvider::covers(self, key)
    }

    fn id(&self) -> ProviderId {
        ProviderId::new(PROVIDER_ID)
    }

    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    async fn search(&self, q: &InstrumentQuery) -> ProviderResult<Vec<Instrument>> {
        if q.sector.is_some_and(|s| s != MarketSector::Curncy)
            || q.text.trim().is_empty()
            || q.limit == 0
        {
            return Ok(Vec::new());
        }
        let cat = self.inner.catalog().await?;
        Ok(catalog::search(&cat, &q.text, q.limit))
    }

    async fn instrument(&self, key: &SecurityKey) -> ProviderResult<Instrument> {
        let pair = self.pair(key).await?;
        Ok(catalog::instrument(key.clone(), &pair))
    }

    /// One `Ticker` request for all covered keys. Keys that aren't pairs are
    /// skipped (the router tries the next provider for them).
    async fn quotes(&self, keys: &[SecurityKey]) -> ProviderResult<Vec<Quote>> {
        if !keys.iter().any(symbols::statically_covered) {
            return Ok(Vec::new());
        }
        let cat = self.inner.catalog().await?;
        let pairs: Vec<(&SecurityKey, Pair)> = keys
            .iter()
            .filter_map(|k| symbols::resolve(k, Some(&cat)).map(|p| (k, p)))
            .collect();
        if pairs.is_empty() {
            return Ok(Vec::new());
        }
        let pair_param = pairs
            .iter()
            .map(|(_, p)| rest_name(p))
            .collect::<Vec<_>>()
            .join(",");
        let result = self
            .inner
            .call(
                "Ticker",
                &[
                    ("pair", pair_param.clone()),
                    ("assetVersion", "1".to_owned()),
                ],
            )
            .await?;
        let fetched_at = SystemClock.now();
        let source_ref = format!("{REST_BASE}/Ticker?pair={pair_param}&assetVersion=1");
        let mut out = Vec::with_capacity(pairs.len());
        for (key, p) in &pairs {
            let entry = if pairs.len() == 1 {
                pair_entry(&result, &p.symbol, "kraken Ticker")?
            } else if let Some(v) = result.get(&p.symbol) {
                v.clone()
            } else {
                tracing::debug!(symbol = %p.symbol, "kraken: pair missing from Ticker response");
                continue;
            };
            let dto: TickerDto = serde_json::from_value(entry)
                .map_err(|e| ProviderError::parse(format!("kraken Ticker {}: {e}", p.symbol)))?;
            out.push(normalize::quote(
                (*key).clone(),
                &dto,
                source_ref.clone(),
                fetched_at,
            )?);
        }
        Ok(out)
    }

    /// The last 720 native candles (Kraken serves no older data through
    /// REST), aggregated client-side where needed and filtered to
    /// `[from, to)`. The last native candle is the current, uncommitted one
    /// and is kept. Always `Adjustment::None`.
    async fn bars(&self, req: &BarsRequest) -> ProviderResult<BarSeries> {
        let capability = if req.interval.is_intraday() {
            Capability::IntradayBars
        } else {
            Capability::DailyBars
        };
        let plan = bars::plan(req.interval).ok_or(ProviderError::Unsupported { capability })?;
        let pair = self.pair(&req.key).await?;
        let name = rest_name(&pair);
        let query = [
            ("pair", name.clone()),
            ("interval", plan.interval_min.to_string()),
            ("assetVersion", "1".to_owned()),
        ];
        let result = self.inner.call("OHLC", &query).await?;
        let now = SystemClock.now();
        let rows: Vec<OhlcRow> =
            serde_json::from_value(pair_entry(&result, &pair.symbol, "kraken OHLC")?)
                .map_err(|e| ProviderError::parse(format!("kraken OHLC rows: {e}")))?;
        let truncated = rows.len() >= bars::MAX_CANDLES;
        let mut native = normalize::ohlc(&rows)?;
        native.sort_by_key(|b| b.ts);
        native.dedup_by_key(|b| b.ts);
        let out = match plan.aggregate {
            None => native,
            Some(bucket) => bars::aggregate(&native, bucket, truncated),
        };
        let provenance = normalize::provenance(
            now,
            format!(
                "{REST_BASE}/OHLC?pair={name}&interval={}&assetVersion=1",
                plan.interval_min
            ),
        );
        let mut series =
            BarSeries::new(req.key.clone(), req.interval, Adjustment::None, provenance);
        for b in out {
            series.push(b);
        }
        series.normalize();
        Ok(series.slice_time(req.from.unwrap_or(i64::MIN), req.to.unwrap_or(i64::MAX)))
    }

    fn streaming(&self) -> Option<&dyn StreamingProvider> {
        Some(self)
    }
}

#[async_trait]
impl StreamingProvider for KrakenProvider {
    /// Returns immediately; the task connects once there is something to
    /// subscribe to.
    async fn connect(&self, sink: StreamSink) -> ProviderResult<Box<dyn StreamHandle>> {
        Ok(Box::new(stream::spawn(self.inner.clone(), sink)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_fixture_catalog(p: &KrakenProvider) {
        let env: Envelope =
            serde_json::from_str(include_str!("../tests/fixtures/asset_pairs.json")).unwrap();
        let cat = Catalog::from_result(&env.result.unwrap()).unwrap();
        *p.inner.catalog.write() = Some(Loaded {
            value: Arc::new(cat),
            at: Instant::now(),
        });
    }

    #[test]
    fn capabilities_are_honest() {
        let c = capabilities();
        for cap in [
            Capability::Search,
            Capability::Reference,
            Capability::Quotes,
            Capability::DailyBars,
            Capability::IntradayBars,
            Capability::Stream,
        ] {
            let e = c.entry(cap, Some(AssetClass::Crypto)).unwrap();
            assert_eq!(e.delay, DataDelay::RealTime);
            assert_eq!(e.source, FeedSource::Exchange("KRAKEN".into()));
        }
        assert!(!c.supports(Capability::Quotes, Some(AssetClass::Fx)));
        assert_eq!(c.ai_policy, AiPolicy::Unreviewed);
        assert!(!c.requires_credentials);
        assert_eq!(
            c.rate_limit,
            Some(RateLimit {
                burst: 1,
                per_second: 1.0
            })
        );
    }

    #[test]
    fn pair_entry_lookup() {
        let env: Envelope =
            serde_json::from_str(include_str!("../tests/fixtures/ohlc_btc_usd_1440.json")).unwrap();
        let result = env.result.unwrap();
        assert!(pair_entry(&result, "BTC/USD", "t").unwrap().is_array());
        // Keyed differently (internal name): the single pair entry is used.
        let alt = serde_json::json!({"XXBTZUSD": [], "last": 1});
        assert!(pair_entry(&alt, "BTC/USD", "t").unwrap().is_array());
        let many = serde_json::json!({"A": [], "B": []});
        assert!(pair_entry(&many, "BTC/USD", "t").is_err());
        let env: Envelope =
            serde_json::from_str(include_str!("../tests/fixtures/ticker.json")).unwrap();
        let map: std::collections::HashMap<String, TickerDto> =
            serde_json::from_value(env.result.unwrap()).unwrap();
        assert_eq!(map.len(), 3);
    }

    #[tokio::test]
    async fn covers_uses_static_rule_until_catalog_loads() {
        let p = KrakenProvider::new().unwrap();
        assert!(p.covers(&SecurityKey::currency("SOLUSD")));
        assert!(p.covers(&SecurityKey::currency("BTCEUR")));
        assert!(!p.covers(&SecurityKey::currency("EURUSD")));
        assert!(!p.covers(&SecurityKey::equity("AAPL")));
        load_fixture_catalog(&p);
        assert!(p.covers(&SecurityKey::currency("BTCUSD")));
        assert!(Provider::covers(&p, &SecurityKey::currency("USDCUSD")));
        // Not in the (trimmed) fixture list.
        assert!(!p.covers(&SecurityKey::currency("SOLUSD")));
    }

    #[tokio::test]
    async fn uncovered_keys_never_hit_the_network() {
        let p = KrakenProvider::new().unwrap();
        assert!(matches!(
            p.instrument(&SecurityKey::equity("AAPL")).await,
            Err(ProviderError::NotFound(_))
        ));
        assert!(
            p.quotes(&[SecurityKey::currency("EURUSD")])
                .await
                .unwrap()
                .is_empty()
        );
        let q = InstrumentQuery {
            text: "btc".into(),
            sector: Some(MarketSector::Equity),
            limit: 5,
        };
        assert!(p.search(&q).await.unwrap().is_empty());
        load_fixture_catalog(&p);
        assert!(matches!(
            p.instrument(&SecurityKey::currency("SOLUSD")).await,
            Err(ProviderError::NotFound(_))
        ));
        let inst = p
            .instrument(&SecurityKey::currency("BTCUSD"))
            .await
            .unwrap();
        assert_eq!(inst.name, "BTC/USD");
        let req = BarsRequest {
            key: SecurityKey::currency("BTCUSD"),
            interval: meridian_types::BarInterval::Hour(0),
            from: None,
            to: None,
            adjustment: Adjustment::None,
        };
        assert!(matches!(
            p.bars(&req).await.unwrap_err(),
            ProviderError::Unsupported {
                capability: Capability::IntradayBars
            }
        ));
    }
}

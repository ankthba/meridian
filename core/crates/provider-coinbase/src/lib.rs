//! Coinbase Exchange public market data: REST (products, ticker, stats,
//! candles) and the WebSocket `ticker` channel. No credentials.
//!
//! Built against the docs listed in `README.md`, which also documents the
//! field mapping (all "session" figures are rolling 24 h), coverage rule,
//! history limits, and rate limits.

mod bars;
mod catalog;
mod dto;
mod http;
mod normalize;
mod stream;
mod symbols;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use meridian_provider::{
    AiPolicy, BarsRequest, CachePolicy, Capabilities, Capability, CapabilityEntry, Coverage,
    InstrumentQuery, Provider, ProviderError, ProviderResult, RateLimit, StreamHandle, StreamSink,
    StreamingProvider, TokenBucket,
};
use meridian_types::{
    Adjustment, AssetClass, Bar, BarSeries, Clock, DataDelay, FeedSource, Instrument, MarketSector,
    NANOS_PER_SEC, ProviderId, Quote, SecurityKey, SystemClock,
};
use parking_lot::RwLock;

use crate::catalog::{Catalog, Product};
use crate::dto::{CandleRow, CurrencyDto, ProductDto, StatsDto, TickerDto};

/// Stable provider id.
pub const PROVIDER_ID: &str = "coinbase";

const REST_BASE: &str = "https://api.exchange.coinbase.com";
const DOCS_URL: &str = "https://docs.cdp.coinbase.com/exchange/introduction/welcome";
/// The product list is refreshed after this age.
const CATALOG_TTL: Duration = Duration::from_secs(24 * 3_600);
/// At most this many candle requests per `bars` call (300 candles each).
const MAX_PAGES: usize = 100;
/// Documented public REST limit: 10 req/s per IP, bursts to 15.
const DOCUMENTED_LIMIT: RateLimit = RateLimit {
    burst: 15,
    per_second: 10.0,
};
/// What we actually send: a little under the documented limit, because one
/// `quotes` or `bars` call makes several requests.
const REQUEST_PACE: RateLimit = RateLimit {
    burst: 8,
    per_second: 8.0,
};

/// Coinbase Exchange spot market data (public endpoints only).
pub struct CoinbaseProvider {
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
    names: RwLock<Option<Arc<HashMap<String, String>>>>,
    load_lock: tokio::sync::Mutex<()>,
    /// Feed URL; a local server in tests.
    ws_url: String,
}

impl Inner {
    async fn get(&self, url: &str, query: &[(&str, String)]) -> ProviderResult<String> {
        self.limiter.acquire().await;
        http::send_text(self.http.get(url).query(query)).await
    }

    /// The product list if loaded, however old. Never blocks on I/O.
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

    /// Loads (or refreshes) the product list. A failed refresh keeps
    /// serving the previous list.
    pub(crate) async fn catalog(&self) -> ProviderResult<Arc<Catalog>> {
        if let Some(c) = self.fresh_catalog() {
            return Ok(c);
        }
        let _guard = self.load_lock.lock().await;
        if let Some(c) = self.fresh_catalog() {
            return Ok(c);
        }
        let loaded = async {
            let body = self.get(&format!("{REST_BASE}/products"), &[]).await?;
            let dtos: Vec<ProductDto> = http::parse_json(&body, "coinbase products")?;
            Ok::<_, ProviderError>(Catalog::from_dtos(dtos))
        }
        .await;
        match loaded {
            Ok(cat) => {
                let cat = Arc::new(cat);
                tracing::debug!(products = cat.len(), "coinbase product list loaded");
                *self.catalog.write() = Some(Loaded {
                    value: cat.clone(),
                    at: Instant::now(),
                });
                Ok(cat)
            }
            Err(e) => match self.catalog_now() {
                Some(stale) => {
                    tracing::warn!(error = %e, "coinbase: product list refresh failed; using the previous list");
                    Ok(stale)
                }
                None => Err(e),
            },
        }
    }

    /// Currency names for display. Failure is not fatal: names fall back
    /// to currency codes and the load is retried next time.
    async fn names(&self) -> Arc<HashMap<String, String>> {
        if let Some(n) = self.names.read().clone() {
            return n;
        }
        let loaded = async {
            let body = self.get(&format!("{REST_BASE}/currencies"), &[]).await?;
            let dtos: Vec<CurrencyDto> = http::parse_json(&body, "coinbase currencies")?;
            Ok::<_, ProviderError>(catalog::names_from_dtos(dtos))
        }
        .await;
        match loaded {
            Ok(n) => {
                let n = Arc::new(n);
                *self.names.write() = Some(n.clone());
                n
            }
            Err(e) => {
                tracing::debug!(error = %e, "coinbase: currency names unavailable");
                Arc::new(HashMap::new())
            }
        }
    }

    async fn quote(&self, key: &SecurityKey, product: &Product) -> ProviderResult<Quote> {
        let ticker_url = format!("{REST_BASE}/products/{}/ticker", product.id);
        let stats_url = format!("{REST_BASE}/products/{}/stats", product.id);
        let (ticker, stats) = tokio::join!(self.get(&ticker_url, &[]), self.get(&stats_url, &[]));
        let fetched_at = SystemClock.now();
        let ticker: TickerDto = http::parse_json(&ticker?, "coinbase ticker")?;
        let stats: StatsDto = http::parse_json(&stats?, "coinbase stats")?;
        normalize::quote(key.clone(), &ticker, &stats, ticker_url, fetched_at)
    }
}

#[cfg(test)]
impl Inner {
    /// Offline instance: fixture product list, feed at `ws_url`.
    pub(crate) fn for_tests(ws_url: String) -> Self {
        let dtos: Vec<ProductDto> =
            serde_json::from_str(include_str!("../tests/fixtures/products.json")).unwrap();
        Self {
            http: reqwest::Client::new(),
            limiter: TokenBucket::new(REQUEST_PACE),
            catalog: RwLock::new(Some(Loaded {
                value: Arc::new(Catalog::from_dtos(dtos)),
                at: Instant::now(),
            })),
            names: RwLock::new(None),
            load_lock: tokio::sync::Mutex::new(()),
            ws_url,
        }
    }
}

fn not_found(key: &SecurityKey) -> ProviderError {
    ProviderError::NotFound(format!("{key} is not a Coinbase Exchange product"))
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
            entry(Capability::DailyBars, Some("daily candles back to the product's Coinbase listing (BTC-USD: 2015)")),
            entry(
                Capability::IntradayBars,
                Some("1m/5m/15m/1h/6h candles back to listing; one request returns up to 300 candles and one call fetches at most 30,000"),
            ),
            entry(Capability::Stream, None),
        ],
        rate_limit: Some(DOCUMENTED_LIMIT),
        max_stream_symbols: None,
        cache_policy: CachePolicy::Unrestricted,
        attribution: None,
        display_allowed: true,
        ai_policy: AiPolicy::Unreviewed,
        requires_credentials: false,
        terms_note: "Coinbase Market Data Terms of Use: personal or research use only; no redistribution, display, \
                     or dissemination of the data or derived works to third parties without written consent. No \
                     explicit storage limit found (terms page returned 403 to automated fetches; reviewed via search \
                     summaries, 2026-10-05). Sending data to a third-party AI model is not addressed: unreviewed."
            .to_owned(),
        docs_url: DOCS_URL.to_owned(),
    }
}

impl CoinbaseProvider {
    pub fn new() -> ProviderResult<Self> {
        Ok(Self {
            inner: Arc::new(Inner {
                http: http::client()?,
                limiter: TokenBucket::new(REQUEST_PACE),
                catalog: RwLock::new(None),
                names: RwLock::new(None),
                load_lock: tokio::sync::Mutex::new(()),
                ws_url: stream::WS_URL.to_owned(),
            }),
            caps: capabilities(),
        })
    }

    /// Whether this provider serves `key`: `<BASE><QUOTE> Curncy` (no
    /// exchange code) with QUOTE in USD/USDT/USDC and a non-fiat base, and,
    /// once the product list has been loaded, `BASE-QUOTE` listed and not
    /// delisted. Never blocks or does I/O.
    pub fn covers(&self, key: &SecurityKey) -> bool {
        match self.inner.catalog_now() {
            Some(cat) => symbols::resolve(key, Some(&cat)).is_some(),
            None => symbols::statically_covered(key),
        }
    }

    async fn product(&self, key: &SecurityKey) -> ProviderResult<Product> {
        if !symbols::statically_covered(key) {
            return Err(not_found(key));
        }
        let cat = self.inner.catalog().await?;
        symbols::resolve(key, Some(&cat)).ok_or_else(|| not_found(key))
    }
}

impl Coverage for CoinbaseProvider {
    fn covers(&self, key: &SecurityKey) -> bool {
        CoinbaseProvider::covers(self, key)
    }
}

#[async_trait]
impl Provider for CoinbaseProvider {
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
        let names = self.inner.names().await;
        Ok(catalog::search(&cat, &names, &q.text, q.limit))
    }

    async fn instrument(&self, key: &SecurityKey) -> ProviderResult<Instrument> {
        let product = self.product(key).await?;
        let names = self.inner.names().await;
        Ok(catalog::instrument(key.clone(), &product, &names))
    }

    /// One `/ticker` + `/stats` pair per key. Keys that aren't products are
    /// skipped (the router tries the next provider for them). On any other
    /// error, quotes fetched so far are returned; if none, the error is.
    async fn quotes(&self, keys: &[SecurityKey]) -> ProviderResult<Vec<Quote>> {
        let wanted: Vec<&SecurityKey> = keys
            .iter()
            .filter(|k| symbols::statically_covered(k))
            .collect();
        if wanted.is_empty() {
            return Ok(Vec::new());
        }
        let cat = self.inner.catalog().await?;
        let mut out = Vec::with_capacity(wanted.len());
        for key in wanted {
            let Some(product) = symbols::resolve(key, Some(&cat)) else {
                tracing::debug!(%key, "coinbase: not a product; skipping");
                continue;
            };
            match self.inner.quote(key, &product).await {
                Ok(q) => out.push(q),
                Err(ProviderError::NotFound(msg)) => {
                    tracing::debug!(%key, %msg, "coinbase: quote not found");
                }
                Err(e) if out.is_empty() => return Err(e),
                Err(e) => {
                    tracing::warn!(%key, error = %e, "coinbase: stopping quote batch early");
                    break;
                }
            }
        }
        Ok(out)
    }

    /// Candles, paged backwards from `to` in 300-candle requests until
    /// `from`, an empty page, or [`MAX_PAGES`]. Intervals Coinbase doesn't
    /// serve are aggregated client-side. Always `Adjustment::None`.
    async fn bars(&self, req: &BarsRequest) -> ProviderResult<BarSeries> {
        let capability = if req.interval.is_intraday() {
            Capability::IntradayBars
        } else {
            Capability::DailyBars
        };
        let plan = bars::plan(req.interval).ok_or(ProviderError::Unsupported { capability })?;
        let product = self.product(&req.key).await?;
        let g = plan.granularity;
        let (lo, hi) = bars::fetch_range(plan, req.from, req.to);
        let now = SystemClock.now();
        let mut pager = bars::Pager::new(g, lo, hi, now.div_euclid(NANOS_PER_SEC));
        let url = format!("{REST_BASE}/products/{}/candles", product.id);
        let mut native: Vec<Bar> = Vec::new();
        let mut pages = 0;
        let mut truncated = false;
        while let Some((start, end)) = pager.next_window() {
            if pages == MAX_PAGES {
                truncated = true;
                tracing::debug!(product = %product.id, "coinbase: candle page limit reached");
                break;
            }
            pages += 1;
            let query = [
                ("granularity", g.to_string()),
                ("start", start.to_string()),
                ("end", end.to_string()),
            ];
            let body = self.inner.get(&url, &query).await?;
            let rows: Vec<CandleRow> = http::parse_json(&body, "coinbase candles")?;
            if rows.is_empty() {
                break;
            }
            native.extend(normalize::candles(&rows)?);
        }
        native.sort_by_key(|b| b.ts);
        native.dedup_by_key(|b| b.ts);
        let rows = match plan.aggregate {
            None => native,
            Some(bucket) => bars::aggregate(&native, bucket, truncated),
        };
        let provenance = normalize::provenance(now, format!("{url}?granularity={g}"));
        let mut series =
            BarSeries::new(req.key.clone(), req.interval, Adjustment::None, provenance);
        for b in rows {
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
impl StreamingProvider for CoinbaseProvider {
    /// Returns immediately; the task connects once there is something to
    /// subscribe to.
    async fn connect(&self, sink: StreamSink) -> ProviderResult<Box<dyn StreamHandle>> {
        Ok(Box::new(stream::spawn(self.inner.clone(), sink)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            assert_eq!(e.source, FeedSource::Exchange("COINBASE".into()));
        }
        assert!(!c.supports(Capability::Quotes, Some(AssetClass::Fx)));
        assert!(!c.supports(Capability::Fundamentals, None));
        assert_eq!(c.ai_policy, AiPolicy::Unreviewed);
        assert!(!c.requires_credentials);
        assert!(c.display_allowed);
        assert_eq!(
            c.rate_limit,
            Some(RateLimit {
                burst: 15,
                per_second: 10.0
            })
        );
    }

    #[test]
    fn not_found_status_maps_to_not_found() {
        let e = ProviderError::from_status(
            404,
            include_str!("../tests/fixtures/error_not_found.json"),
            None,
        );
        assert!(matches!(e, ProviderError::NotFound(_)));
    }

    #[tokio::test]
    async fn covers_uses_static_rule_until_catalog_loads() {
        let p = CoinbaseProvider::new().unwrap();
        assert!(p.covers(&SecurityKey::currency("BTCUSD")));
        assert!(p.covers(&SecurityKey::currency("USDCUSD")));
        assert!(!p.covers(&SecurityKey::currency("EURUSD")));
        assert!(!p.covers(&SecurityKey::equity("AAPL")));
        let dtos: Vec<ProductDto> =
            serde_json::from_str(include_str!("../tests/fixtures/products.json")).unwrap();
        *p.inner.catalog.write() = Some(Loaded {
            value: Arc::new(Catalog::from_dtos(dtos)),
            at: Instant::now(),
        });
        assert!(p.covers(&SecurityKey::currency("BTCUSD")));
        assert!(Coverage::covers(&p, &SecurityKey::currency("BTCUSDT")));
        // Not a Coinbase Exchange product once the list is known.
        assert!(!p.covers(&SecurityKey::currency("USDCUSD")));
        assert!(!p.covers(&SecurityKey::currency("DNTUSDC")));
    }

    #[tokio::test]
    async fn uncovered_keys_never_hit_the_network() {
        let p = CoinbaseProvider::new().unwrap();
        let e = p
            .instrument(&SecurityKey::equity("AAPL"))
            .await
            .unwrap_err();
        assert!(matches!(e, ProviderError::NotFound(_)));
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
        let req = BarsRequest {
            key: SecurityKey::currency("BTCUSD"),
            interval: meridian_types::BarInterval::Minute(0),
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

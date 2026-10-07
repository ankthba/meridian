//! Alpaca Market Data API: US stock snapshots, historical bars, the real-time
//! stock WebSocket stream, option chain snapshots, and news.
//!
//! Built against docs.alpaca.markets (checked 2026-10-05); see `README.md`
//! for the endpoints, plans, limits, and field mappings.
//!
//! Plans: on **Basic** (free) real-time stock data is IEX only and options
//! are the derived "indicative" feed; **Algo Trader Plus** adds SIP and OPRA.
//! [`AlpacaFeed`] tells the provider which plan the keys belong to, and the
//! capabilities and provenance it reports follow from that.

mod dto;
mod http;
mod normalize;
mod occ;
mod stream;

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{NaiveDate, Utc};
use meridian_provider::{
    AiPolicy, BarsRequest, CachePolicy, Capabilities, Capability, CapabilityEntry, ChainRequest, EventCalendarRequest,
    NewsQuery, NewsScope, Provider, ProviderError, ProviderResult, RateLimit, StreamHandle, StreamSink,
    StreamingProvider, TokenBucket,
};
use meridian_types::{
    AssetClass, BarSeries, DataDelay, DividendCalendar, DividendEvent, Dividends, FeedSource, NewsPage, OptionChain,
    ProviderId, Quote, SecurityKey, UnixNanos, datetime_to_nanos,
};
use reqwest::Url;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use secrecy::{ExposeSecret, SecretString};
use serde::de::DeserializeOwned;

use crate::normalize::{
    BarsParams, PROVIDER_ID, alpaca_symbol, bars_query, corporate_action_events, corporate_action_events_by_symbol,
    corporate_actions_query, next_page, option_contract, provenance, push_bars, rfc3339,
};

const DATA_BASE: &str = "https://data.alpaca.markets";
const STREAM_BASE: &str = "wss://stream.data.alpaca.markets/v2";
const DOCS_URL: &str = "https://docs.alpaca.markets/us/docs/about-market-data-api";

const KEY_HEADER: &str = "apca-api-key-id";
const SECRET_HEADER: &str = "apca-api-secret-key";
const MISSING_KEY: &str = "Alpaca API key not set — add it in Settings";
const PLUS_PLAN: &str = "Algo Trader Plus";

/// Symbols per snapshot request. The docs give no maximum; 100 keeps URLs
/// short.
const SNAPSHOT_CHUNK: usize = 100;
/// Documented maximum page size for option chain snapshots.
const OPTION_PAGE_LIMIT: u32 = 1000;
/// Documented maximum page size for news.
const NEWS_PAGE_LIMIT: usize = 50;
/// News pages fetched per call when a text filter or a large limit needs
/// more than one page.
const MAX_NEWS_PAGES: usize = 5;
/// Guard against a server that never stops paginating.
const MAX_PAGES: usize = 2000;
/// Basic plan: SIP history must end at least 15 minutes ago. One extra
/// minute of margin for clock skew.
const BASIC_SIP_EMBARGO: Duration = Duration::from_secs(16 * 60);
/// Corporate actions history requested for DVD (by process date).
const CORPORATE_ACTIONS_LOOKBACK_DAYS: i64 = 3653;
/// Announced actions are processed around their pay date, so the window
/// reaches this far ahead to include declared, not yet paid dividends.
const CORPORATE_ACTIONS_LOOKAHEAD_DAYS: i64 = 90;
/// Pages read per corporate actions call (1,000 records each).
const MAX_CORPORATE_ACTION_PAGES: usize = 10;
/// Symbols per corporate actions calendar request (`symbols` is a
/// comma-separated list; the docs give no maximum, 100 keeps URLs short).
const CALENDAR_SYMBOL_CHUNK: usize = 100;
/// The calendar filters on ex-date, but Alpaca's `start`/`end` filter on
/// process date, which trails the ex-date (actions are processed around
/// the pay date). The request window reaches this far past `to`…
const CALENDAR_PROCESS_LAG_DAYS: i64 = 75;
/// …and this far before `from`, for the rare action paid before its
/// ex-date (large special dividends).
const CALENDAR_PROCESS_LEAD_DAYS: i64 = 7;

/// Which Alpaca market data plan the configured keys have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlpacaFeed {
    /// Basic (free): real-time stocks from IEX only, indicative options.
    Iex,
    /// Algo Trader Plus: consolidated SIP stocks and OPRA options.
    Sip,
}

impl AlpacaFeed {
    fn stock_feed(self) -> &'static str {
        match self {
            AlpacaFeed::Iex => "iex",
            AlpacaFeed::Sip => "sip",
        }
    }

    fn option_feed(self) -> &'static str {
        match self {
            AlpacaFeed::Iex => "indicative",
            AlpacaFeed::Sip => "opra",
        }
    }

    fn label(self) -> &'static str {
        match self {
            AlpacaFeed::Iex => "IEX",
            AlpacaFeed::Sip => "SIP",
        }
    }

    fn equity_source(self) -> FeedSource {
        match self {
            AlpacaFeed::Iex => FeedSource::SingleVenue("IEX".into()),
            AlpacaFeed::Sip => FeedSource::Consolidated,
        }
    }

    /// History is always requested from SIP; Basic can't see its last 15 min.
    fn bars_delay(self) -> DataDelay {
        match self {
            AlpacaFeed::Iex => DataDelay::Delayed { minutes: 15 },
            AlpacaFeed::Sip => DataDelay::RealTime,
        }
    }

    /// Indicative quotes are derived from OPRA and its trades are delayed
    /// 15 minutes; a chain carries one delay, so the conservative one.
    fn option_delay(self) -> DataDelay {
        match self {
            AlpacaFeed::Iex => DataDelay::Delayed { minutes: 15 },
            AlpacaFeed::Sip => DataDelay::RealTime,
        }
    }

    fn option_source(self) -> FeedSource {
        match self {
            AlpacaFeed::Iex => FeedSource::Modelled,
            AlpacaFeed::Sip => FeedSource::Consolidated,
        }
    }
}

/// Provider configuration. Keys come from the Keychain via the engine.
#[derive(Debug, Clone)]
pub struct AlpacaConfig {
    pub key_id: Option<SecretString>,
    pub secret_key: Option<SecretString>,
    pub feed: AlpacaFeed,
}

struct Credentials {
    key_id: SecretString,
    secret_key: SecretString,
}

/// Alpaca Market Data provider (`alpaca`).
pub struct AlpacaProvider {
    http: reqwest::Client,
    data_base: String,
    stream_base: String,
    creds: Option<Credentials>,
    feed: AlpacaFeed,
    caps: Capabilities,
    /// Paces every HTTP request (pagination included) at the plan's limit.
    bucket: TokenBucket,
}

impl AlpacaProvider {
    pub fn new(config: AlpacaConfig) -> ProviderResult<Self> {
        let present = |s: Option<SecretString>| s.filter(|s| !s.expose_secret().trim().is_empty());
        let creds = match (present(config.key_id), present(config.secret_key)) {
            (Some(key_id), Some(secret_key)) => Some(Credentials { key_id, secret_key }),
            _ => None,
        };
        let caps = capabilities_for(config.feed);
        let bucket = TokenBucket::new(caps.rate_limit.unwrap_or_else(|| RateLimit::per_minute(200)));
        Ok(Self {
            http: http::client()?,
            data_base: DATA_BASE.to_owned(),
            stream_base: STREAM_BASE.to_owned(),
            creds,
            feed: config.feed,
            caps,
            bucket,
        })
    }

    fn creds(&self) -> ProviderResult<&Credentials> {
        self.creds.as_ref().ok_or_else(|| ProviderError::Unauthorized(MISSING_KEY.into()))
    }

    fn auth_headers(&self) -> ProviderResult<HeaderMap> {
        let c = self.creds()?;
        let value = |s: &SecretString| {
            let mut v = HeaderValue::from_str(s.expose_secret().trim())
                .map_err(|_| ProviderError::Unauthorized("Alpaca API key contains invalid characters".into()))?;
            v.set_sensitive(true);
            Ok::<_, ProviderError>(v)
        };
        let mut h = HeaderMap::new();
        h.insert(HeaderName::from_static(KEY_HEADER), value(&c.key_id)?);
        h.insert(HeaderName::from_static(SECRET_HEADER), value(&c.secret_key)?);
        Ok(h)
    }

    /// `data_base` + path segments + query. Segments are percent-encoded.
    fn endpoint(&self, segments: &[&str], query: &[(&str, String)]) -> ProviderResult<Url> {
        let mut url = Url::parse(&self.data_base).map_err(|e| ProviderError::parse(format!("request URL: {e}")))?;
        url.path_segments_mut()
            .map_err(|()| ProviderError::parse("request URL: base cannot have a path"))?
            .pop_if_empty()
            .extend(segments);
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        Ok(url)
    }

    /// Authenticated GET, paced by the provider's token bucket.
    async fn get<T: DeserializeOwned>(&self, url: &Url, context: &str, cap: Capability) -> ProviderResult<T> {
        let headers = self.auth_headers()?;
        self.bucket.acquire().await;
        let body = http::send_text(self.http.get(url.clone()).headers(headers))
            .await
            .map_err(|e| entitlement_error(e, cap))?;
        http::parse_json(&body, context)
    }

    /// `GET /v1/corporate-actions` for one symbol over `[start, end]` (by
    /// process date), all pages up to [`MAX_CORPORATE_ACTION_PAGES`].
    async fn corporate_actions(
        &self,
        key: &SecurityKey,
        symbol: &str,
        start: NaiveDate,
        end: NaiveDate,
    ) -> ProviderResult<Dividends> {
        let context = "Alpaca corporate actions";
        let first_url =
            self.endpoint(&["v1", "corporate-actions"], &corporate_actions_query(Some(symbol), start, end, None))?;
        let fetched = now();
        let mut url = first_url.clone();
        let mut events = Vec::new();
        let mut skipped = 0;
        let mut seen = HashSet::new();
        for page in 1..=MAX_CORPORATE_ACTION_PAGES {
            let resp: dto::CorporateActionsResp = self.get(&url, context, Capability::Dividends).await?;
            let (mut e, s) = corporate_action_events(&resp.corporate_actions.unwrap_or_default(), symbol);
            events.append(&mut e);
            skipped += s;
            let Some(token) = next_page(resp.next_page_token, &mut seen, context)? else { break };
            if page == MAX_CORPORATE_ACTION_PAGES {
                tracing::warn!(symbol, "Alpaca corporate actions: page limit reached; older actions not loaded");
                break;
            }
            url = self.endpoint(
                &["v1", "corporate-actions"],
                &corporate_actions_query(Some(symbol), start, end, Some(&token)),
            )?;
        }
        if skipped > 0 {
            tracing::warn!(skipped, symbol, "Alpaca corporate actions: skipped records without a valid ex-date or amount");
        }
        events.sort_by(|a, b| b.ex_date.cmp(&a.ex_date));
        Ok(Dividends {
            key: key.clone(),
            dividends: events,
            per_period: Vec::new(),
            reported_splits: Vec::new(),
            provenance: provenance(DataDelay::EndOfDay, FeedSource::Aggregated, fetched, first_url.as_str()),
        })
    }

    /// `GET /v1/corporate-actions` for many symbols (or all, `None`) over
    /// `[start, end]` by process date, all pages up to
    /// [`MAX_CORPORATE_ACTION_PAGES`]. Returns `(SYMBOL, event)` pairs and the
    /// first page's URL.
    async fn corporate_actions_many(
        &self,
        symbols: Option<&str>,
        start: NaiveDate,
        end: NaiveDate,
    ) -> ProviderResult<(Vec<(String, meridian_types::Dividend)>, String)> {
        let context = "Alpaca corporate actions";
        let path = ["v1", "corporate-actions"];
        let first_url = self.endpoint(&path, &corporate_actions_query(symbols, start, end, None))?;
        let mut url = first_url.clone();
        let mut events = Vec::new();
        let mut skipped = 0;
        let mut seen = HashSet::new();
        for page in 1..=MAX_CORPORATE_ACTION_PAGES {
            let resp: dto::CorporateActionsResp = self.get(&url, context, Capability::DividendCalendar).await?;
            let (mut e, s) = corporate_action_events_by_symbol(&resp.corporate_actions.unwrap_or_default());
            events.append(&mut e);
            skipped += s;
            let Some(token) = next_page(resp.next_page_token, &mut seen, context)? else { break };
            if page == MAX_CORPORATE_ACTION_PAGES {
                tracing::warn!("Alpaca corporate actions calendar: page limit reached; some actions not loaded");
                break;
            }
            url = self.endpoint(&path, &corporate_actions_query(symbols, start, end, Some(&token)))?;
        }
        if skipped > 0 {
            tracing::warn!(skipped, "Alpaca corporate actions: skipped records without a symbol, ex-date or amount");
        }
        Ok((events, first_url.to_string()))
    }

    /// One calendar chunk, retried once with `end` = today if a future end
    /// date is rejected (as for `dividends`).
    async fn corporate_actions_chunk(
        &self,
        symbols: Option<&str>,
        start: NaiveDate,
        end: NaiveDate,
    ) -> ProviderResult<(Vec<(String, meridian_types::Dividend)>, String)> {
        let today = Utc::now().date_naive();
        match self.corporate_actions_many(symbols, start, end).await {
            Err(ProviderError::Http { status: 400 | 422, .. }) if end > today => {
                tracing::debug!("Alpaca corporate actions: future end date rejected; retrying with end = today");
                self.corporate_actions_many(symbols, start, today.max(start)).await
            }
            other => other,
        }
    }

    /// Latest trade price of the underlying from the configured stock feed.
    async fn underlying_last(&self, symbol: &str) -> Option<f64> {
        let query = [("symbols", symbol.to_owned()), ("feed", self.feed.stock_feed().to_owned())];
        let url = self.endpoint(&["v2", "stocks", "snapshots"], &query).ok()?;
        match self.get::<dto::StockSnapshotsResp>(&url, "Alpaca stock snapshots", Capability::Quotes).await {
            Ok(resp) => resp.get(symbol)?.as_ref()?.latest_trade.as_ref()?.p,
            Err(e) => {
                tracing::debug!(error = %e, "Alpaca: underlying price unavailable for option chain");
                None
            }
        }
    }
}

fn now() -> UnixNanos {
    datetime_to_nanos(Utc::now())
}

/// A 403 whose body says the plan doesn't cover the request becomes
/// `NotEntitled`. The documented text is "subscription does not permit
/// querying recent SIP data"; the stream's "insufficient subscription" is
/// matched too in case REST uses it.
fn entitlement_error(e: ProviderError, cap: Capability) -> ProviderError {
    match &e {
        ProviderError::Unauthorized(msg) => {
            let m = msg.to_ascii_lowercase();
            if m.contains("subscription does not permit") || m.contains("insufficient subscription") {
                ProviderError::NotEntitled { capability: cap, plan: PLUS_PLAN.into() }
            } else {
                e
            }
        }
        _ => e,
    }
}

fn not_served(key: &SecurityKey) -> ProviderError {
    ProviderError::NotFound(format!("{key} is not a US equity on Alpaca"))
}

/// `[start, end]` for a bars request (Alpaca's `end` is inclusive; the
/// exclusive `to` is applied after fetching). History comes from SIP; on
/// Basic, `end` is clamped to 16 minutes ago. Without `from`, daily-or-longer
/// bars start at 2016-01-01 and intraday bars 30 days before `end`.
fn bars_window(feed: AlpacaFeed, req: &BarsRequest, now: UnixNanos) -> (UnixNanos, UnixNanos) {
    let mut end = req.to.unwrap_or(now);
    if feed == AlpacaFeed::Iex {
        end = end.min(now.saturating_sub(BASIC_SIP_EMBARGO.as_nanos() as i64));
    }
    let start = req.from.unwrap_or_else(|| {
        if req.interval.is_intraday() {
            end.saturating_sub(normalize::DEFAULT_INTRADAY_LOOKBACK_DAYS * meridian_types::NANOS_PER_DAY)
        } else {
            normalize::default_daily_start()
        }
    });
    (start, end)
}

/// Query string for `GET /v1beta1/news`.
fn news_query(
    symbols: Option<&[String]>,
    from: Option<UnixNanos>,
    to: Option<UnixNanos>,
    limit: usize,
    page_token: Option<&str>,
) -> Vec<(&'static str, String)> {
    let mut q =
        vec![("sort", "desc".to_owned()), ("include_content", "false".to_owned()), ("limit", limit.to_string())];
    if let Some(s) = symbols {
        q.push(("symbols", s.join(",")));
    }
    if let Some(f) = from {
        q.push(("start", rfc3339(f)));
    }
    if let Some(t) = to {
        q.push(("end", rfc3339(t)));
    }
    if let Some(t) = page_token {
        q.push(("page_token", t.to_owned()));
    }
    q
}

/// What Alpaca offers on each plan.
fn capabilities_for(feed: AlpacaFeed) -> Capabilities {
    let equities = vec![AssetClass::Equity, AssetClass::Etf];
    let entry = |capability, asset_classes: Vec<AssetClass>, delay, source, history: Option<&str>| CapabilityEntry {
        capability,
        asset_classes,
        delay,
        source,
        history: history.map(str::to_owned),
    };
    let (rate_limit, max_stream_symbols, plan_note) = match feed {
        AlpacaFeed::Iex => (
            RateLimit::per_minute(200),
            Some(30),
            "Basic plan: real-time stocks are IEX only (about 2.5% of volume); history is SIP up to 15 minutes ago; \
             options are the indicative feed (derived quotes, trades delayed 15 minutes); 200 requests/min; \
             30 streamed symbols.",
        ),
        AlpacaFeed::Sip => (
            RateLimit::per_minute(10_000),
            None,
            "Algo Trader Plus: consolidated SIP stocks and OPRA options in real time; 10,000 requests/min; \
             unlimited streamed stock symbols.",
        ),
    };
    Capabilities {
        entries: vec![
            entry(Capability::Quotes, equities.clone(), DataDelay::RealTime, feed.equity_source(), None),
            entry(Capability::Stream, equities.clone(), DataDelay::RealTime, feed.equity_source(), None),
            entry(
                Capability::DailyBars,
                equities.clone(),
                feed.bars_delay(),
                FeedSource::Consolidated,
                Some("since 2016"),
            ),
            entry(
                Capability::IntradayBars,
                equities.clone(),
                feed.bars_delay(),
                FeedSource::Consolidated,
                Some("since 2016"),
            ),
            // Routed by the underlying's asset class.
            entry(
                Capability::OptionChain,
                vec![AssetClass::Equity, AssetClass::Etf, AssetClass::Option],
                feed.option_delay(),
                feed.option_source(),
                None,
            ),
            entry(Capability::News, equities.clone(), DataDelay::RealTime, FeedSource::Aggregated, Some("since 2015")),
            // Corporate actions: cash dividends and splits with ex/record/pay
            // dates. Alpaca gives no guarantee on how soon an announced action
            // appears, so it is labelled end-of-day rather than real time.
            entry(
                Capability::Dividends,
                equities.clone(),
                DataDelay::EndOfDay,
                FeedSource::Aggregated,
                Some("corporate actions (cash dividends, splits): up to 10 years back, plus announced actions"),
            ),
            // The same corporate actions for many symbols at once, by ex-date.
            entry(
                Capability::DividendCalendar,
                equities,
                DataDelay::EndOfDay,
                FeedSource::Aggregated,
                Some("ex-dates of cash dividends and splits, including announced ones"),
            ),
        ],
        rate_limit: Some(rate_limit),
        max_stream_symbols,
        cache_policy: CachePolicy::Unrestricted,
        attribution: None,
        display_allowed: true,
        ai_policy: AiPolicy::Unreviewed,
        requires_credentials: true,
        terms_note: format!(
            "{plan_note} Alpaca Terms and Conditions: content is for personal, non-commercial use; no copying, \
             redistribution, or public display. No storage limit or attribution requirement found. News is from \
             Benzinga (headline and summary only)."
        ),
        docs_url: DOCS_URL.to_owned(),
    }
}

#[async_trait]
impl Provider for AlpacaProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(PROVIDER_ID)
    }

    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    /// `GET /v2/stocks/snapshots?symbols=…&feed=iex|sip`. Keys Alpaca doesn't
    /// serve, and symbols missing from the response, are left out so the
    /// router can try another provider.
    async fn quotes(&self, keys: &[SecurityKey]) -> ProviderResult<Vec<Quote>> {
        let mut by_symbol: BTreeMap<String, Vec<SecurityKey>> = BTreeMap::new();
        for k in keys {
            if let Some(sym) = alpaca_symbol(k) {
                let v = by_symbol.entry(sym).or_default();
                if !v.contains(k) {
                    v.push(k.clone());
                }
            }
        }
        if by_symbol.is_empty() {
            return Ok(Vec::new());
        }
        self.creds()?;
        let symbols: Vec<&str> = by_symbol.keys().map(String::as_str).collect();
        let mut out = Vec::new();
        for chunk in symbols.chunks(SNAPSHOT_CHUNK) {
            let query = [("symbols", chunk.join(",")), ("feed", self.feed.stock_feed().to_owned())];
            let url = self.endpoint(&["v2", "stocks", "snapshots"], &query)?;
            let resp: dto::StockSnapshotsResp = self.get(&url, "Alpaca stock snapshots", Capability::Quotes).await?;
            let fetched = now();
            for (sym, snap) in &resp {
                let (Some(snap), Some(keys)) = (snap, by_symbol.get(sym)) else { continue };
                for k in keys {
                    out.push(normalize::snapshot_to_quote(
                        k.clone(),
                        snap,
                        DataDelay::RealTime,
                        self.feed.equity_source(),
                        fetched,
                        url.as_str(),
                    ));
                }
            }
        }
        Ok(out)
    }

    /// `GET /v2/stocks/{symbol}/bars` with `feed=sip`, following
    /// `next_page_token` until it is null.
    async fn bars(&self, req: &BarsRequest) -> ProviderResult<BarSeries> {
        let cap = if req.interval.is_intraday() { Capability::IntradayBars } else { Capability::DailyBars };
        let symbol = alpaca_symbol(&req.key).ok_or_else(|| not_served(&req.key))?;
        let timeframe = normalize::timeframe(req.interval).ok_or(ProviderError::Unsupported { capability: cap })?;
        self.creds()?;

        let fetched = now();
        let (start, end) = bars_window(self.feed, req, fetched);
        let params = BarsParams { timeframe, start, end, adjustment: req.adjustment, feed: "sip" };
        let first_url = self.endpoint(&["v2", "stocks", &symbol, "bars"], &bars_query(&params, None))?;
        let mut series = BarSeries::new(
            req.key.clone(),
            req.interval,
            req.adjustment,
            provenance(self.feed.bars_delay(), FeedSource::Consolidated, fetched, first_url.as_str()),
        );
        if start > end || req.to.is_some_and(|to| to <= start) {
            return Ok(series);
        }

        let context = "Alpaca stock bars";
        let mut seen = HashSet::new();
        let mut url = first_url;
        for _ in 0..MAX_PAGES {
            let page: dto::StockBarsResp = self.get(&url, context, cap).await?;
            push_bars(&mut series, page.bars.as_deref().unwrap_or_default(), start, req.to);
            let Some(token) = next_page(page.next_page_token, &mut seen, context)? else {
                series.normalize();
                return Ok(series);
            };
            url = self.endpoint(&["v2", "stocks", &symbol, "bars"], &bars_query(&params, Some(&token)))?;
        }
        Err(ProviderError::Upstream(format!("{context}: more than {MAX_PAGES} pages")))
    }

    /// `GET /v1beta1/options/snapshots/{underlying}` with
    /// `feed=indicative|opra`, all pages. `underlying_price` is the latest
    /// trade from the stock feed (one extra request; `None` if it fails).
    async fn option_chain(&self, req: &ChainRequest) -> ProviderResult<OptionChain> {
        let symbol = alpaca_symbol(&req.underlying).ok_or_else(|| not_served(&req.underlying))?;
        self.creds()?;

        let context = "Alpaca option chain";
        let mut base_query =
            vec![("feed", self.feed.option_feed().to_owned()), ("limit", OPTION_PAGE_LIMIT.to_string())];
        if let Some(expiry) = req.expiry {
            base_query.push(("expiration_date", expiry.format("%Y-%m-%d").to_string()));
        }
        let first_url = self.endpoint(&["v1beta1", "options", "snapshots", &symbol], &base_query)?;
        let mut url = first_url.clone();
        let mut contracts = Vec::new();
        let mut newest: Option<UnixNanos> = None;
        let mut skipped = 0usize;
        let mut seen = HashSet::new();
        let mut done = false;
        for _ in 0..MAX_PAGES {
            let page: dto::OptionChainResp = self.get(&url, context, Capability::OptionChain).await?;
            for (occ, snap) in page.snapshots.unwrap_or_default() {
                let Some(snap) = snap else { continue };
                newest = newest.max(normalize::option_snapshot_time(&snap));
                match option_contract(&occ, &snap) {
                    Some(c) => contracts.push(c),
                    None => skipped += 1,
                }
            }
            let Some(token) = next_page(page.next_page_token, &mut seen, context)? else {
                done = true;
                break;
            };
            let mut q = base_query.clone();
            q.push(("page_token", token));
            url = self.endpoint(&["v1beta1", "options", "snapshots", &symbol], &q)?;
        }
        if !done {
            return Err(ProviderError::Upstream(format!("{context}: more than {MAX_PAGES} pages")));
        }
        if skipped > 0 {
            tracing::warn!(skipped, "Alpaca option chain: skipped contracts with unparseable OCC symbols");
        }
        normalize::sort_contracts(&mut contracts);
        // Newest quote/trade timestamp in the chain; fetch time if none.
        let as_of = newest.unwrap_or_else(now);
        let underlying_price = self.underlying_last(&symbol).await;
        Ok(OptionChain {
            underlying: req.underlying.clone(),
            underlying_price,
            as_of,
            contracts,
            provenance: provenance(self.feed.option_delay(), self.feed.option_source(), as_of, first_url.as_str()),
        })
    }

    /// `GET /v1beta1/news`, newest first. Company news filters by symbol;
    /// market news has no filter. Alpaca has no "top stories" or press
    /// release category, so those scopes are unsupported.
    async fn news(&self, q: &NewsQuery) -> ProviderResult<NewsPage> {
        let empty = NewsPage { items: Vec::new(), next: None };
        let symbols: Option<Vec<String>> = match q.scope {
            NewsScope::Company => {
                let mut s: Vec<String> = q.keys.iter().filter_map(alpaca_symbol).collect();
                s.sort();
                s.dedup();
                if s.is_empty() {
                    return Ok(empty);
                }
                Some(s)
            }
            // No free source curates "top" stories; TOP shows the latest
            // market-wide headlines.
            NewsScope::Market | NewsScope::Top => None,
            NewsScope::PressReleases => {
                return Err(ProviderError::Unsupported { capability: Capability::News });
            }
        };
        if q.limit == 0 {
            return Ok(empty);
        }
        self.creds()?;

        let context = "Alpaca news";
        let page_limit = q.limit.clamp(1, NEWS_PAGE_LIMIT);
        let mut items = Vec::new();
        let mut token: Option<String> = None;
        let mut seen = HashSet::new();
        for _ in 0..MAX_NEWS_PAGES {
            let query = news_query(symbols.as_deref(), q.from, q.to, page_limit, token.as_deref());
            let url = self.endpoint(&["v1beta1", "news"], &query)?;
            let resp: dto::NewsResp = self.get(&url, context, Capability::News).await?;
            let fetched = now();
            items.extend(
                resp.news
                    .unwrap_or_default()
                    .iter()
                    .map(|a| normalize::news_item(a, fetched, url.as_str()))
                    .filter(|item| normalize::matches_text(item, q.text.as_deref())),
            );
            token = next_page(resp.next_page_token, &mut seen, context)?;
            if token.is_none() || items.len() >= q.limit {
                break;
            }
        }
        items.truncate(q.limit);
        Ok(NewsPage { items, next: token })
    }

    /// Cash dividends and splits from `GET /v1/corporate-actions`, newest
    /// first: up to 10 years back (Alpaca doesn't document how far its
    /// history goes) plus actions processed up to 90 days ahead (declared,
    /// not yet paid). The docs don't say whether a future `end` is accepted;
    /// if the request is rejected as invalid (400/422), it is retried once
    /// with `end` = today.
    async fn dividends(&self, key: &SecurityKey) -> ProviderResult<Dividends> {
        let symbol = alpaca_symbol(key).ok_or_else(|| not_served(key))?;
        self.creds()?;
        let today = Utc::now().date_naive();
        let start = today - chrono::Duration::days(CORPORATE_ACTIONS_LOOKBACK_DAYS);
        let ahead = today + chrono::Duration::days(CORPORATE_ACTIONS_LOOKAHEAD_DAYS);
        match self.corporate_actions(key, &symbol, start, ahead).await {
            Err(ProviderError::Http { status: 400 | 422, .. }) => {
                tracing::debug!("Alpaca corporate actions: future end date rejected; retrying with end = today");
                self.corporate_actions(key, &symbol, start, today).await
            }
            other => other,
        }
    }

    /// Dividend and split events with an ex-date in `[from, to]` from
    /// `GET /v1/corporate-actions?symbols=A,B,…` (100 symbols per request),
    /// or for every symbol when the request has no keys. Alpaca filters on
    /// process date, so the request window is widened (see
    /// [`CALENDAR_PROCESS_LAG_DAYS`]) and the ex-date filter applied here.
    async fn dividend_calendar(&self, req: &EventCalendarRequest) -> ProviderResult<DividendCalendar> {
        self.creds()?;
        let mut wanted: Vec<(String, SecurityKey)> = Vec::new();
        for k in &req.keys {
            if let Some(sym) = alpaca_symbol(k)
                && !wanted.iter().any(|(s, _)| *s == sym)
            {
                wanted.push((sym, k.clone()));
            }
        }
        if !req.keys.is_empty() && wanted.is_empty() {
            return Err(not_served(&req.keys[0]));
        }
        let start = req.from - chrono::Duration::days(CALENDAR_PROCESS_LEAD_DAYS);
        let end = req.to + chrono::Duration::days(CALENDAR_PROCESS_LAG_DAYS);
        let fetched = now();
        let chunks: Vec<Option<String>> = if wanted.is_empty() {
            vec![None]
        } else {
            wanted
                .chunks(CALENDAR_SYMBOL_CHUNK)
                .map(|c| Some(c.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>().join(",")))
                .collect()
        };
        let mut events = Vec::new();
        let mut first_url = None;
        for chunk in &chunks {
            let (records, url) = self.corporate_actions_chunk(chunk.as_deref(), start, end).await?;
            first_url.get_or_insert(url);
            for (sym, dividend) in records {
                if dividend.ex_date < req.from || dividend.ex_date > req.to {
                    continue;
                }
                let key = if wanted.is_empty() {
                    SecurityKey::equity(&sym.replace('.', "/"))
                } else {
                    match wanted.iter().find(|(s, _)| *s == sym) {
                        Some((_, k)) => k.clone(),
                        None => continue,
                    }
                };
                events.push(DividendEvent { key, dividend });
            }
        }
        events.sort_by(|a, b| {
            a.dividend.ex_date.cmp(&b.dividend.ex_date).then_with(|| a.key.symbol.cmp(&b.key.symbol))
        });
        events.dedup_by(|a, b| {
            a.key == b.key && a.dividend.ex_date == b.dividend.ex_date && a.dividend.kind == b.dividend.kind
                && (a.dividend.amount - b.dividend.amount).abs() < 1e-12
        });
        let source_ref = first_url.unwrap_or_default();
        Ok(DividendCalendar {
            events,
            provenance: provenance(DataDelay::EndOfDay, FeedSource::Aggregated, fetched, &source_ref),
        })
    }

    fn streaming(&self) -> Option<&dyn StreamingProvider> {
        Some(self)
    }
}

#[async_trait]
impl StreamingProvider for AlpacaProvider {
    /// Opens `wss://stream.data.alpaca.markets/v2/{iex|sip}` in a background
    /// task and returns immediately. Trades and quotes are streamed for each
    /// subscribed key, up to the plan's symbol limit.
    async fn connect(&self, sink: StreamSink) -> ProviderResult<Box<dyn StreamHandle>> {
        let c = self.creds()?;
        let cfg = stream::StreamConfig::new(
            format!("{}/{}", self.stream_base, self.feed.stock_feed()),
            self.feed.label(),
            c.key_id.clone(),
            c.secret_key.clone(),
            self.caps.max_stream_symbols,
        );
        Ok(Box::new(stream::spawn(cfg, sink)))
    }
}

#[cfg(test)]
mod tests;

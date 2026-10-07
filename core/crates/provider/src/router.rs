//! Routes each request to the first configured provider that supports it.
//!
//! The router enforces rate limits, retries retryable failures with
//! exponential backoff, and opens a per-provider circuit breaker after
//! repeated failures. It never crosses the mock/live boundary: the engine
//! registers either the mock provider or real providers, never both.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use meridian_types::{
    AssetClass, BarSeries, CompanyProfile, DividendCalendar, Dividends, EarningsCalendar, EarningsHistory,
    EconomicEvent, EconomicSeries, Estimates, Filing, FilingDocument, FilingsPage, Fundamentals, Holders, Instrument,
    MarketSector, NewsPage, OptionChain, ProviderId, Quote, Recommendations, SecurityKey, Transcript, YieldCurve,
};
use parking_lot::Mutex;
use tokio::time::Instant;

use crate::capability::{Capabilities, Capability};
use crate::error::{ProviderError, ProviderResult};
use crate::rate_limit::TokenBucket;
use crate::request::{
    BarsRequest, CalendarRequest, ChainRequest, CurveRequest, EventCalendarRequest, FilingsRequest,
    FundamentalsRequest, InstrumentQuery, NewsQuery, SeriesRequest,
};
use crate::traits::Provider;

type BoxFut<T> = Pin<Box<dyn Future<Output = ProviderResult<T>> + Send>>;

const MAX_RETRIES: u32 = 2;
const BREAKER_THRESHOLD: u32 = 5;
const BREAKER_OPEN_FOR: Duration = Duration::from_secs(30);

/// Asset classes a security key may belong to, used to match capabilities.
#[must_use]
pub fn asset_hint(key: &SecurityKey) -> &'static [AssetClass] {
    match key.sector {
        MarketSector::Equity | MarketSector::Pfd => &[AssetClass::Equity, AssetClass::Etf],
        MarketSector::Curncy => &[AssetClass::Fx, AssetClass::Crypto],
        MarketSector::Index => &[AssetClass::Index],
        MarketSector::Cmdty => &[AssetClass::Future],
        MarketSector::Govt | MarketSector::MMkt => &[AssetClass::Bond, AssetClass::Rate],
        MarketSector::Corp | MarketSector::Muni | MarketSector::Mtge => &[AssetClass::Bond],
    }
}

#[derive(Debug, Default)]
struct Breaker {
    consecutive_failures: u32,
    open_until: Option<Instant>,
}

struct Registered {
    provider: Arc<dyn Provider>,
    bucket: Option<TokenBucket>,
    breaker: Mutex<Breaker>,
}

impl Registered {
    fn is_open(&self) -> bool {
        let b = self.breaker.lock();
        b.open_until.is_some_and(|t| Instant::now() < t)
    }

    fn record(&self, ok: bool) {
        let mut b = self.breaker.lock();
        if ok {
            b.consecutive_failures = 0;
            b.open_until = None;
        } else {
            b.consecutive_failures += 1;
            if b.consecutive_failures >= BREAKER_THRESHOLD {
                b.open_until = Some(Instant::now() + BREAKER_OPEN_FOR);
                tracing::warn!(provider = %self.provider.id(), "circuit breaker opened");
            }
        }
    }
}

/// Outcome of a routed call, with the provider that served it.
#[derive(Debug, Clone)]
pub struct Routed<T> {
    pub value: T,
    pub provider: ProviderId,
}

pub struct ProviderRouter {
    providers: Vec<Registered>,
}

impl ProviderRouter {
    /// Providers are tried in the given order.
    #[must_use]
    pub fn new(providers: Vec<Arc<dyn Provider>>) -> Self {
        let providers = providers
            .into_iter()
            .map(|p| {
                let bucket = p.capabilities().rate_limit.map(TokenBucket::new);
                Registered { provider: p, bucket, breaker: Mutex::new(Breaker::default()) }
            })
            .collect();
        Self { providers }
    }

    #[must_use]
    pub fn providers(&self) -> Vec<Arc<dyn Provider>> {
        self.providers.iter().map(|r| r.provider.clone()).collect()
    }

    #[must_use]
    pub fn capabilities(&self) -> Vec<(ProviderId, Capabilities)> {
        self.providers.iter().map(|r| (r.provider.id(), r.provider.capabilities().clone())).collect()
    }

    /// Whether any provider offers `cap` (optionally for `key`'s asset class).
    #[must_use]
    pub fn supports(&self, cap: Capability, key: Option<&SecurityKey>) -> bool {
        !self.candidates(cap, key).is_empty()
    }

    fn candidates(&self, cap: Capability, key: Option<&SecurityKey>) -> Vec<&Registered> {
        self.providers
            .iter()
            .filter(|r| {
                let caps = r.provider.capabilities();
                match key {
                    None => caps.supports(cap, None),
                    Some(k) => r.provider.covers(k) && asset_hint(k).iter().any(|a| caps.supports(cap, Some(*a))),
                }
            })
            .collect()
    }

    async fn call<T: Send + 'static>(
        &self,
        cap: Capability,
        key: Option<&SecurityKey>,
        f: impl Fn(Arc<dyn Provider>) -> BoxFut<T>,
    ) -> ProviderResult<Routed<T>> {
        let candidates = self.candidates(cap, key);
        if candidates.is_empty() {
            return Err(ProviderError::Unsupported { capability: cap });
        }
        let mut last_err = ProviderError::Unsupported { capability: cap };
        for reg in candidates {
            if reg.is_open() {
                last_err = Self::disabled(reg);
                continue;
            }
            match Self::attempt(reg, &f).await {
                Ok(routed) => return Ok(routed),
                // Try the next provider.
                Err(e @ (ProviderError::Unsupported { .. } | ProviderError::NotFound(_))) => last_err = e,
                Err(e) => return Err(e),
            }
        }
        Err(last_err)
    }

    fn disabled(reg: &Registered) -> ProviderError {
        ProviderError::Upstream(format!("{} temporarily disabled after repeated failures", reg.provider.id()))
    }

    /// One provider: rate limit, and retries with backoff for retryable
    /// errors. The caller checks the circuit breaker.
    async fn attempt<T: Send + 'static>(
        reg: &Registered,
        f: &impl Fn(Arc<dyn Provider>) -> BoxFut<T>,
    ) -> ProviderResult<Routed<T>> {
        let mut attempt = 0;
        loop {
            if let Some(b) = &reg.bucket {
                b.acquire().await;
            }
            match f(reg.provider.clone()).await {
                Ok(value) => {
                    reg.record(true);
                    return Ok(Routed { value, provider: reg.provider.id() });
                }
                Err(e @ (ProviderError::Unsupported { .. } | ProviderError::NotFound(_))) => return Err(e),
                Err(e) if e.is_retryable() && attempt < MAX_RETRIES => {
                    reg.record(false);
                    let backoff = match &e {
                        ProviderError::RateLimited { retry_after: Some(d) } => *d,
                        _ => Duration::from_millis(250 * 2u64.pow(attempt)),
                    };
                    tracing::debug!(provider = %reg.provider.id(), error = %e, ?backoff, "retrying");
                    tokio::time::sleep(backoff).await;
                    attempt += 1;
                }
                Err(e) => {
                    reg.record(!e.is_retryable());
                    return Err(e);
                }
            }
        }
    }

    /// Providers that offer `cap` for `key`, in routing order.
    #[must_use]
    pub fn providers_for(&self, cap: Capability, key: Option<&SecurityKey>) -> Vec<ProviderId> {
        self.candidates(cap, key).into_iter().map(|r| r.provider.id()).collect()
    }

    /// Calls one specific provider (from [`Self::providers_for`]) with the
    /// usual breaker, rate limit and retries, without falling through to
    /// another provider. For datasets where sources complement each other.
    async fn call_on<T: Send + 'static>(
        &self,
        provider: &ProviderId,
        cap: Capability,
        key: Option<&SecurityKey>,
        f: impl Fn(Arc<dyn Provider>) -> BoxFut<T>,
    ) -> ProviderResult<Routed<T>> {
        let reg = self
            .candidates(cap, key)
            .into_iter()
            .find(|r| r.provider.id() == *provider)
            .ok_or(ProviderError::Unsupported { capability: cap })?;
        if reg.is_open() {
            return Err(Self::disabled(reg));
        }
        Self::attempt(reg, &f).await
    }

    pub async fn search(&self, q: InstrumentQuery) -> ProviderResult<Vec<Instrument>> {
        // Search fans out to every provider that supports it and merges.
        let mut out = Vec::new();
        for reg in self.candidates(Capability::Search, None) {
            if let Ok(mut v) = reg.provider.search(&q).await {
                out.append(&mut v);
            }
        }
        out.truncate(q.limit);
        Ok(out)
    }

    pub async fn instrument(&self, key: &SecurityKey) -> ProviderResult<Routed<Instrument>> {
        let k = key.clone();
        self.call(Capability::Reference, Some(key), move |p| {
            let k = k.clone();
            Box::pin(async move { p.instrument(&k).await })
        })
        .await
    }

    pub async fn profile(&self, key: &SecurityKey) -> ProviderResult<Routed<CompanyProfile>> {
        let k = key.clone();
        self.call(Capability::Profile, Some(key), move |p| {
            let k = k.clone();
            Box::pin(async move { p.profile(&k).await })
        })
        .await
    }

    pub async fn quotes(&self, keys: &[SecurityKey]) -> ProviderResult<Vec<Quote>> {
        // Group keys by the first provider that covers each one.
        let mut out = Vec::with_capacity(keys.len());
        let mut remaining: Vec<SecurityKey> = keys.to_vec();
        for reg in &self.providers {
            if remaining.is_empty() {
                break;
            }
            let caps = reg.provider.capabilities();
            let (mine, rest): (Vec<_>, Vec<_>) = remaining
                .into_iter()
                .partition(|k| {
                    reg.provider.covers(k) && asset_hint(k).iter().any(|a| caps.supports(Capability::Quotes, Some(*a)))
                });
            remaining = rest;
            if mine.is_empty() || reg.is_open() {
                remaining.extend(mine);
                continue;
            }
            if let Some(b) = &reg.bucket {
                b.acquire().await;
            }
            match reg.provider.quotes(&mine).await {
                Ok(mut q) => {
                    reg.record(true);
                    let served: std::collections::HashSet<SecurityKey> = q.iter().map(|q| q.key.clone()).collect();
                    out.append(&mut q);
                    remaining.extend(mine.into_iter().filter(|k| !served.contains(k)));
                }
                Err(e) => {
                    reg.record(!e.is_retryable());
                    remaining.extend(mine);
                }
            }
        }
        Ok(out)
    }

    pub async fn bars(&self, req: BarsRequest) -> ProviderResult<Routed<BarSeries>> {
        let cap = if req.interval.is_intraday() { Capability::IntradayBars } else { Capability::DailyBars };
        let key = req.key.clone();
        self.call(cap, Some(&key), move |p| {
            let r = req.clone();
            Box::pin(async move { p.bars(&r).await })
        })
        .await
    }

    pub async fn option_chain(&self, req: ChainRequest) -> ProviderResult<Routed<OptionChain>> {
        let key = req.underlying.clone();
        self.call(Capability::OptionChain, Some(&key), move |p| {
            let r = req.clone();
            Box::pin(async move { p.option_chain(&r).await })
        })
        .await
    }

    pub async fn fundamentals(&self, req: FundamentalsRequest) -> ProviderResult<Routed<Fundamentals>> {
        let key = req.key.clone();
        self.call(Capability::Fundamentals, Some(&key), move |p| {
            let r = req.clone();
            Box::pin(async move { p.fundamentals(&r).await })
        })
        .await
    }

    pub async fn estimates(&self, key: &SecurityKey) -> ProviderResult<Routed<Estimates>> {
        let k = key.clone();
        self.call(Capability::Estimates, Some(key), move |p| {
            let k = k.clone();
            Box::pin(async move { p.estimates(&k).await })
        })
        .await
    }

    pub async fn earnings(&self, key: &SecurityKey) -> ProviderResult<Routed<EarningsHistory>> {
        let k = key.clone();
        self.call(Capability::Earnings, Some(key), move |p| {
            let k = k.clone();
            Box::pin(async move { p.earnings(&k).await })
        })
        .await
    }

    pub async fn recommendations(&self, key: &SecurityKey) -> ProviderResult<Routed<Recommendations>> {
        let k = key.clone();
        self.call(Capability::Recommendations, Some(key), move |p| {
            let k = k.clone();
            Box::pin(async move { p.recommendations(&k).await })
        })
        .await
    }

    pub async fn holders(&self, key: &SecurityKey) -> ProviderResult<Routed<Holders>> {
        let k = key.clone();
        self.call(Capability::Holders, Some(key), move |p| {
            let k = k.clone();
            Box::pin(async move { p.holders(&k).await })
        })
        .await
    }

    pub async fn dividends(&self, key: &SecurityKey) -> ProviderResult<Routed<Dividends>> {
        let k = key.clone();
        self.call(Capability::Dividends, Some(key), move |p| {
            let k = k.clone();
            Box::pin(async move { p.dividends(&k).await })
        })
        .await
    }

    /// Dividends from one specific provider. Dividend sources complement
    /// each other (event dates from one, per-period amounts from another),
    /// so DVD asks each of [`Self::providers_for`] and merges.
    pub async fn dividends_from(&self, provider: &ProviderId, key: &SecurityKey) -> ProviderResult<Routed<Dividends>> {
        let k = key.clone();
        self.call_on(provider, Capability::Dividends, Some(key), move |p| {
            let k = k.clone();
            Box::pin(async move { p.dividends(&k).await })
        })
        .await
    }

    pub async fn filings(&self, req: FilingsRequest) -> ProviderResult<Routed<FilingsPage>> {
        let key = req.key.clone();
        self.call(Capability::Filings, key.as_ref(), move |p| {
            let r = req.clone();
            Box::pin(async move { p.filings(&r).await })
        })
        .await
    }

    pub async fn filing_document(&self, filing: &Filing) -> ProviderResult<Routed<FilingDocument>> {
        let f = filing.clone();
        self.call(Capability::Filings, None, move |p| {
            let f = f.clone();
            Box::pin(async move { p.filing_document(&f).await })
        })
        .await
    }

    /// News fans out to every news provider and merges by time, newest first.
    pub async fn news(&self, q: NewsQuery) -> ProviderResult<NewsPage> {
        let candidates = self.candidates(Capability::News, None);
        if candidates.is_empty() {
            return Err(ProviderError::Unsupported { capability: Capability::News });
        }
        let mut items = Vec::new();
        let mut errors = Vec::new();
        // Whether any provider handles this query's scope; if none does the
        // answer is "not available", not an empty feed.
        let mut supported = false;
        for reg in candidates {
            if reg.is_open() {
                supported = true;
                errors.push(ProviderError::Upstream(format!("{} temporarily disabled after repeated failures", reg.provider.id())));
                continue;
            }
            if let Some(b) = &reg.bucket {
                b.acquire().await;
            }
            match reg.provider.news(&q).await {
                Ok(mut page) => {
                    supported = true;
                    reg.record(true);
                    items.append(&mut page.items);
                }
                Err(ProviderError::Unsupported { .. }) => {}
                Err(e) => {
                    supported = true;
                    reg.record(!e.is_retryable());
                    errors.push(e);
                }
            }
        }
        if !supported && errors.is_empty() && items.is_empty() {
            return Err(ProviderError::Unsupported { capability: Capability::News });
        }
        if items.is_empty()
            && let Some(e) = errors.into_iter().next()
        {
            return Err(e);
        }
        items.sort_by_key(|n| std::cmp::Reverse(n.published_at));
        items.dedup_by(|a, b| a.headline == b.headline && a.source == b.source);
        items.truncate(q.limit);
        Ok(NewsPage { items, next: None })
    }

    pub async fn transcripts(&self, key: &SecurityKey) -> ProviderResult<Routed<Vec<Transcript>>> {
        let k = key.clone();
        self.call(Capability::Transcripts, Some(key), move |p| {
            let k = k.clone();
            Box::pin(async move { p.transcripts(&k).await })
        })
        .await
    }

    pub async fn economic_series(&self, req: SeriesRequest) -> ProviderResult<Routed<EconomicSeries>> {
        self.call(Capability::EconomicSeries, None, move |p| {
            let r = req.clone();
            Box::pin(async move { p.economic_series(&r).await })
        })
        .await
    }

    pub async fn economic_calendar(&self, req: CalendarRequest) -> ProviderResult<Routed<Vec<EconomicEvent>>> {
        self.call(Capability::EconomicCalendar, None, move |p| {
            let r = req.clone();
            Box::pin(async move { p.economic_calendar(&r).await })
        })
        .await
    }

    pub async fn yield_curve(&self, req: CurveRequest) -> ProviderResult<Routed<YieldCurve>> {
        self.call(Capability::YieldCurve, None, move |p| {
            let r = req.clone();
            Box::pin(async move { p.yield_curve(&r).await })
        })
        .await
    }

    /// Earnings releases in a window from the first provider that offers an
    /// earnings calendar.
    pub async fn earnings_calendar(&self, req: EventCalendarRequest) -> ProviderResult<Routed<EarningsCalendar>> {
        self.call(Capability::EarningsCalendar, None, move |p| {
            let r = req.clone();
            Box::pin(async move { p.earnings_calendar(&r).await })
        })
        .await
    }

    /// Dividend ex-dates in a window from the first provider that offers a
    /// dividend calendar.
    pub async fn dividend_calendar(&self, req: EventCalendarRequest) -> ProviderResult<Routed<DividendCalendar>> {
        self.call(Capability::DividendCalendar, None, move |p| {
            let r = req.clone();
            Box::pin(async move { p.dividend_calendar(&r).await })
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use async_trait::async_trait;
    use meridian_types::{DataDelay, FeedSource, Provenance};

    use super::*;
    use crate::capability::{AiPolicy, CachePolicy, CapabilityEntry};

    struct Fake {
        id: &'static str,
        caps: Capabilities,
        fail_with: Option<ProviderError>,
        calls: AtomicU32,
    }

    impl Fake {
        fn new(id: &'static str, fail_with: Option<ProviderError>) -> Self {
            Self {
                id,
                caps: Capabilities {
                    entries: vec![CapabilityEntry {
                        capability: Capability::Profile,
                        asset_classes: vec![AssetClass::Equity],
                        delay: DataDelay::EndOfDay,
                        source: FeedSource::Official,
                        history: None,
                    }],
                    rate_limit: None,
                    max_stream_symbols: None,
                    cache_policy: CachePolicy::Unrestricted,
                    attribution: None,
                    display_allowed: true,
                    ai_policy: AiPolicy::Allowed,
                    requires_credentials: false,
                    terms_note: String::new(),
                    docs_url: String::new(),
                },
                fail_with,
                calls: AtomicU32::new(0),
            }
        }
    }

    #[async_trait]
    impl Provider for Fake {
        fn id(&self) -> ProviderId {
            ProviderId::new(self.id)
        }
        fn capabilities(&self) -> &Capabilities {
            &self.caps
        }
        async fn profile(&self, _key: &SecurityKey) -> ProviderResult<CompanyProfile> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match &self.fail_with {
                Some(e) => Err(e.clone()),
                None => Ok(CompanyProfile { description: Some(self.id.into()), ..Default::default() }),
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn falls_through_not_found() {
        let a = Arc::new(Fake::new("a", Some(ProviderError::NotFound("x".into()))));
        let b = Arc::new(Fake::new("b", None));
        let r = ProviderRouter::new(vec![a.clone(), b.clone()]);
        let out = r.profile(&SecurityKey::equity("AAPL")).await.unwrap();
        assert_eq!(out.provider, ProviderId::new("b"));
        assert_eq!(a.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn retries_retryable_then_fails() {
        let a = Arc::new(Fake::new("a", Some(ProviderError::Network("down".into()))));
        let r = ProviderRouter::new(vec![a.clone()]);
        let err = r.profile(&SecurityKey::equity("AAPL")).await.unwrap_err();
        assert_eq!(err, ProviderError::Network("down".into()));
        assert_eq!(a.calls.load(Ordering::SeqCst), MAX_RETRIES + 1);
    }

    #[tokio::test(start_paused = true)]
    async fn unsupported_when_no_candidate() {
        let r = ProviderRouter::new(vec![Arc::new(Fake::new("a", None))]);
        let err = r.profile(&SecurityKey::currency("EURUSD")).await.unwrap_err();
        assert!(matches!(err, ProviderError::Unsupported { capability: Capability::Profile }));
    }

    #[tokio::test(start_paused = true)]
    async fn news_unsupported_by_every_provider_is_not_an_empty_feed() {
        // Advertises News but rejects this scope (trait default).
        let mut a = Fake::new("a", None);
        a.caps.entries.push(CapabilityEntry {
            capability: Capability::News,
            asset_classes: vec![AssetClass::Equity],
            delay: DataDelay::RealTime,
            source: FeedSource::Official,
            history: None,
        });
        let r = ProviderRouter::new(vec![Arc::new(a)]);
        let q = NewsQuery { scope: crate::request::NewsScope::Top, keys: vec![], text: None, from: None, to: None, limit: 10 };
        let err = r.news(q).await.unwrap_err();
        assert!(matches!(err, ProviderError::Unsupported { capability: Capability::News }));
    }

    #[tokio::test(start_paused = true)]
    async fn breaker_opens_after_repeated_failures() {
        let a = Arc::new(Fake::new("a", Some(ProviderError::Network("down".into()))));
        let r = ProviderRouter::new(vec![a.clone()]);
        for _ in 0..2 {
            let _ = r.profile(&SecurityKey::equity("AAPL")).await;
        }
        let calls_before = a.calls.load(Ordering::SeqCst);
        let err = r.profile(&SecurityKey::equity("AAPL")).await.unwrap_err();
        assert!(matches!(err, ProviderError::Upstream(_)));
        assert_eq!(a.calls.load(Ordering::SeqCst), calls_before);
        let _ = Provenance::synthetic(0);
    }

    /// A dividends source: events or per-period rows, or an error.
    struct Divs {
        id: &'static str,
        caps: Capabilities,
        result: ProviderResult<usize>,
        calls: AtomicU32,
    }

    impl Divs {
        fn new(id: &'static str, result: ProviderResult<usize>) -> Self {
            let mut caps = Fake::new(id, None).caps;
            caps.entries[0].capability = Capability::Dividends;
            Self { id, caps, result, calls: AtomicU32::new(0) }
        }
    }

    #[async_trait]
    impl Provider for Divs {
        fn id(&self) -> ProviderId {
            ProviderId::new(self.id)
        }
        fn capabilities(&self) -> &Capabilities {
            &self.caps
        }
        async fn dividends(&self, key: &SecurityKey) -> ProviderResult<Dividends> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let n = self.result.clone()?;
            let ex_date = chrono::NaiveDate::from_ymd_opt(2026, 1, 2).unwrap();
            let d = meridian_types::Dividend {
                declared_date: None,
                ex_date,
                record_date: None,
                pay_date: None,
                amount: 0.5,
                currency: "USD".into(),
                frequency: None,
                kind: meridian_types::DividendKind::Regular,
            };
            Ok(Dividends {
                key: key.clone(),
                dividends: vec![d; n],
                per_period: Vec::new(),
                reported_splits: Vec::new(),
                provenance: Provenance::synthetic(0),
            })
        }
    }

    #[tokio::test(start_paused = true)]
    async fn dividends_from_each_provider_without_fall_through() {
        let a = Arc::new(Divs::new("a", Ok(2)));
        let b = Arc::new(Divs::new("b", Err(ProviderError::NotFound("none".into()))));
        let c = Arc::new(Fake::new("c", None)); // no dividends capability
        let r = ProviderRouter::new(vec![a.clone(), b.clone(), c]);
        let key = SecurityKey::equity("AAPL");
        assert_eq!(r.providers_for(Capability::Dividends, Some(&key)), vec![ProviderId::new("a"), ProviderId::new("b")]);
        assert!(r.providers_for(Capability::Dividends, Some(&SecurityKey::currency("EURUSD"))).is_empty());

        let got = r.dividends_from(&ProviderId::new("a"), &key).await.unwrap();
        assert_eq!(got.provider, ProviderId::new("a"));
        assert_eq!(got.value.dividends.len(), 2);
        // b's NotFound is b's answer; a is not asked again.
        let calls_a = a.calls.load(Ordering::SeqCst);
        let err = r.dividends_from(&ProviderId::new("b"), &key).await.unwrap_err();
        assert_eq!(err, ProviderError::NotFound("none".into()));
        assert_eq!(a.calls.load(Ordering::SeqCst), calls_a);
        // A provider that doesn't offer dividends (or isn't registered).
        assert!(matches!(
            r.dividends_from(&ProviderId::new("c"), &key).await,
            Err(ProviderError::Unsupported { capability: Capability::Dividends })
        ));
        assert!(matches!(
            r.dividends_from(&ProviderId::new("zz"), &key).await,
            Err(ProviderError::Unsupported { .. })
        ));
        // The routed call still falls through b's NotFound order-wise: a first.
        assert_eq!(r.dividends(&key).await.unwrap().provider, ProviderId::new("a"));
    }

    #[tokio::test(start_paused = true)]
    async fn dividends_from_retries_and_respects_the_breaker() {
        let a = Arc::new(Divs::new("a", Err(ProviderError::Network("down".into()))));
        let r = ProviderRouter::new(vec![a.clone()]);
        let key = SecurityKey::equity("AAPL");
        let id = ProviderId::new("a");
        assert_eq!(r.dividends_from(&id, &key).await.unwrap_err(), ProviderError::Network("down".into()));
        assert_eq!(a.calls.load(Ordering::SeqCst), MAX_RETRIES + 1);
        let _ = r.dividends_from(&id, &key).await;
        let before = a.calls.load(Ordering::SeqCst);
        assert!(matches!(r.dividends_from(&id, &key).await, Err(ProviderError::Upstream(_))));
        assert_eq!(a.calls.load(Ordering::SeqCst), before);
    }
}

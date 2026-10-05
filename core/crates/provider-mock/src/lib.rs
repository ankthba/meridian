//! `MockProvider`: deterministic, realistic, and clearly labeled synthetic
//! data for every capability, so all UI work can proceed without API keys.
//!
//! Everything returned is marked synthetic: `Provenance::synthetic` (provider
//! `mock`, delay and source `Synthetic`), `Instrument::is_synthetic`, the
//! `SYNTHETIC` quote flag, `[MOCK] ` headlines, and a banner on every filing
//! and transcript. Tickers and company names are real; numbers are not.
//!
//! Determinism: every series derives from the configured seed and an FNV-1a
//! hash of the security key, and is a pure function of the injected clock.
//! Price paths start at a fixed epoch (1999) and are pinned to each
//! symbol's reference price on a fixed anchor date, so history never shifts
//! as the clock advances. See `README.md` for the models used.

mod cal;
mod corp;
mod daily;
mod econ;
mod filings;
mod fundamentals;
mod hash;
mod intraday;
mod market;
mod news;
mod options;
mod stream;
mod universe;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::NaiveDate;
use meridian_provider::{
    AiPolicy, BarsRequest, CachePolicy, CalendarRequest, Capabilities, Capability, CapabilityEntry, ChainRequest,
    CurveRequest, FilingsRequest, FundamentalsRequest, InstrumentQuery, NewsQuery, Provider, ProviderError,
    ProviderResult, SeriesRequest, StreamHandle, StreamSink, StreamingProvider,
};
use meridian_types::{
    AssetClass, BarSeries, Clock, CompanyProfile, DataDelay, Dividends, EarningsHistory, EconomicEvent,
    EconomicSeries, Estimates, FeedSource, Filing, FilingDocument, FilingsPage, Fundamentals, Holders, Instrument,
    OptionChain, ProviderId, Quote, Recommendations, SecurityKey, SystemClock, Transcript, YieldCurve,
};
use parking_lot::{Mutex, RwLock};

pub use filings::FILING_BANNER;
pub use news::MOCK_PREFIX;

use crate::daily::{DayBar, FactorPaths};
use crate::econ::Macro;
use crate::universe::{Kind, Sym, Universe};

/// Terms text shown on the Data Sources screen.
pub const TERMS_NOTE: &str = "Synthetic data generated locally. Not real market data.";

/// Configuration for [`MockProvider`].
#[derive(Clone)]
pub struct MockConfig {
    /// Seed for every generated series.
    pub seed: u64,
    /// Injected clock; all outputs are a pure function of `seed` and the
    /// clock's time.
    pub clock: Arc<dyn Clock>,
    /// Filler equities for load tests, tickers `ZQ0001`, `ZQ0002`, ….
    pub extra_symbols: usize,
    /// Stream rate per subscribed symbol (Poisson arrivals). Default 2.0.
    pub updates_per_symbol_per_sec: f64,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self { seed: 42, clock: Arc::new(SystemClock), extra_symbols: 0, updates_per_symbol_per_sec: 2.0 }
    }
}

impl std::fmt::Debug for MockConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockConfig")
            .field("seed", &self.seed)
            .field("now", &self.clock.now())
            .field("extra_symbols", &self.extra_symbols)
            .field("updates_per_symbol_per_sec", &self.updates_per_symbol_per_sec)
            .finish()
    }
}

/// Recent full-day bars per (symbol hash, local date).
type RecentCache = HashMap<(u64, NaiveDate), Arc<Vec<DayBar>>>;

/// Shared generator state (the stream task holds a clone).
pub(crate) struct Inner {
    pub seed: u64,
    pub clock: Arc<dyn Clock>,
    pub universe: Universe,
    pub stream_rate: f64,
    factors: RwLock<Option<Arc<FactorPaths>>>,
    pub(crate) macro_cache: RwLock<Option<Arc<Macro>>>,
    pub(crate) recent: Mutex<RecentCache>,
}

impl Inner {
    pub(crate) fn factors(&self, until: NaiveDate) -> Arc<FactorPaths> {
        daily::ensure_factors(&self.factors, self.seed, until)
    }
}

/// Synthetic data provider. Cheap to clone handles are not provided; wrap in
/// `Arc` to share.
pub struct MockProvider {
    inner: Arc<Inner>,
    caps: Capabilities,
}

impl std::fmt::Debug for MockProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockProvider")
            .field("seed", &self.inner.seed)
            .field("symbols", &self.inner.universe.syms.len())
            .finish_non_exhaustive()
    }
}

const ALL_CAPABILITIES: [Capability; 20] = [
    Capability::Search,
    Capability::Reference,
    Capability::Profile,
    Capability::Quotes,
    Capability::DailyBars,
    Capability::IntradayBars,
    Capability::Stream,
    Capability::OptionChain,
    Capability::Fundamentals,
    Capability::Estimates,
    Capability::Earnings,
    Capability::Recommendations,
    Capability::Holders,
    Capability::Dividends,
    Capability::Filings,
    Capability::News,
    Capability::Transcripts,
    Capability::EconomicSeries,
    Capability::EconomicCalendar,
    Capability::YieldCurve,
];

const ALL_ASSETS: [AssetClass; 10] = [
    AssetClass::Equity,
    AssetClass::Etf,
    AssetClass::Index,
    AssetClass::Option,
    AssetClass::Future,
    AssetClass::Crypto,
    AssetClass::Fx,
    AssetClass::Bond,
    AssetClass::Rate,
    AssetClass::Economic,
];

fn capabilities() -> Capabilities {
    let history = |c: Capability| -> Option<String> {
        Some(
            match c {
                Capability::DailyBars => "synthetic, 20 years (from 1999 at the earliest)",
                Capability::IntradayBars => "synthetic, 60 sessions at 1m, up to 500 at 1h",
                Capability::Fundamentals => "synthetic, 10 annual / 40 quarterly periods",
                Capability::EconomicSeries => "synthetic, from 1990",
                Capability::Filings | Capability::News => "synthetic, recent",
                _ => "synthetic",
            }
            .to_string(),
        )
    };
    Capabilities {
        entries: ALL_CAPABILITIES
            .iter()
            .map(|c| CapabilityEntry {
                capability: *c,
                asset_classes: ALL_ASSETS.to_vec(),
                delay: DataDelay::Synthetic,
                source: FeedSource::Synthetic,
                history: history(*c),
            })
            .collect(),
        rate_limit: None,
        max_stream_symbols: None,
        cache_policy: CachePolicy::NoStore,
        attribution: None,
        display_allowed: true,
        ai_policy: AiPolicy::Allowed,
        requires_credentials: false,
        terms_note: TERMS_NOTE.into(),
        docs_url: "n/a (generated locally by meridian-provider-mock)".into(),
    }
}

fn not_found<T>(what: String) -> ProviderResult<T> {
    Err(ProviderError::NotFound(what))
}

impl MockProvider {
    #[must_use]
    pub fn new(cfg: MockConfig) -> Self {
        let inner = Inner {
            seed: cfg.seed,
            clock: cfg.clock,
            universe: Universe::build(cfg.extra_symbols),
            stream_rate: if cfg.updates_per_symbol_per_sec > 0.0 { cfg.updates_per_symbol_per_sec } else { 2.0 },
            factors: RwLock::new(None),
            macro_cache: RwLock::new(None),
            recent: Mutex::new(HashMap::new()),
        };
        Self { inner: Arc::new(inner), caps: capabilities() }
    }

    /// Every listed instrument (equities, ETFs, indices, FX, crypto,
    /// futures, then filler). Any pair of the 17 supported currencies also
    /// resolves on demand (e.g. `GBPNZD Curncy`).
    #[must_use]
    pub fn universe(&self) -> &[Instrument] {
        &self.inner.universe.instruments
    }

    /// Synchronous bar generation (what the async `bars` calls). Useful for
    /// benches and load generators.
    pub fn bars_sync(&self, req: &BarsRequest) -> ProviderResult<BarSeries> {
        self.inner.bars(req)
    }

    /// Synchronous quote snapshot.
    pub fn quote_sync(&self, key: &SecurityKey) -> ProviderResult<Quote> {
        let s = self.inner.resolve(key)?;
        self.inner
            .quote(&s, self.inner.clock.now())
            .ok_or_else(|| ProviderError::NotFound(format!("{key} has no synthetic price at this time")))
    }

    /// Supported economic series IDs.
    pub fn economic_series_ids(&self) -> Vec<&'static str> {
        econ::series_ids().collect()
    }

    /// Region used to group an index on a world-indices screen
    /// (`Americas`, `EMEA`, `APAC`).
    #[must_use]
    pub fn index_region(&self, key: &SecurityKey) -> Option<&'static str> {
        let s = self.inner.resolve(key).ok()?;
        if s.p.kind == Kind::Index { s.p.region } else { None }
    }

    fn equity(&self, key: &SecurityKey, what: &str) -> ProviderResult<std::borrow::Cow<'_, Sym>> {
        let s = self.inner.resolve(key)?;
        if s.p.kind != Kind::Equity {
            return not_found(format!("no {what} for {key}: not an operating company"));
        }
        Ok(s)
    }

    fn search_sync(&self, q: &InstrumentQuery) -> Vec<Instrument> {
        let text = q.text.trim().to_ascii_uppercase();
        let limit = if q.limit == 0 { 20 } else { q.limit };
        if text.is_empty() {
            // Empty query lists the universe (the engine loads reference
            // data this way for autocomplete).
            return self
                .inner
                .universe
                .instruments
                .iter()
                .filter(|i| q.sector.is_none_or(|s| s == i.key.sector))
                .take(limit)
                .cloned()
                .collect();
        }
        let mut scored: Vec<(u32, &Instrument)> = Vec::new();
        if let Ok(key) = text.parse::<SecurityKey>()
            && let Some(i) = self.inner.universe.index_of(&key)
        {
            scored.push((0, &self.inner.universe.instruments[i]));
        }
        let ticker_q = text.split_whitespace().next().unwrap_or("");
        for inst in &self.inner.universe.instruments {
            if q.sector.is_some_and(|s| s != inst.key.sector) {
                continue;
            }
            let t = inst.key.symbol.as_str();
            let score = if t == ticker_q {
                1
            } else if t.starts_with(ticker_q) {
                10 + (t.len() - ticker_q.len()) as u32
            } else if let Some(pos) = inst.name.to_ascii_uppercase().find(&text) {
                100 + pos as u32
            } else {
                continue;
            };
            scored.push((score, inst));
        }
        scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.key.symbol.cmp(&b.1.key.symbol)));
        let mut out: Vec<Instrument> = Vec::with_capacity(limit);
        for (_, inst) in scored {
            if !out.iter().any(|o| o.key == inst.key) {
                out.push(inst.clone());
            }
            if out.len() >= limit {
                return out;
            }
        }
        // Unlisted FX crosses resolve on demand.
        if out.len() < limit
            && q.sector.is_none_or(|s| s == meridian_types::MarketSector::Curncy)
            && let Some(s) = universe::fx_sym(ticker_q)
            && !out.iter().any(|o| o.key == s.inst.key)
        {
            out.push(s.inst);
        }
        out
    }
}

#[async_trait]
impl Provider for MockProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("mock")
    }

    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    async fn search(&self, q: &InstrumentQuery) -> ProviderResult<Vec<Instrument>> {
        Ok(self.search_sync(q))
    }

    async fn instrument(&self, key: &SecurityKey) -> ProviderResult<Instrument> {
        Ok(self.inner.resolve(key)?.inst.clone())
    }

    async fn profile(&self, key: &SecurityKey) -> ProviderResult<CompanyProfile> {
        let s = self.inner.resolve(key)?;
        Ok(self.inner.profile_for(&s, self.inner.clock.now()))
    }

    async fn quotes(&self, keys: &[SecurityKey]) -> ProviderResult<Vec<Quote>> {
        let now = self.inner.clock.now();
        Ok(keys
            .iter()
            .filter_map(|k| self.inner.resolve(k).ok().and_then(|s| self.inner.quote(&s, now)))
            .collect())
    }

    async fn bars(&self, req: &BarsRequest) -> ProviderResult<BarSeries> {
        self.inner.bars(req)
    }

    async fn option_chain(&self, req: &ChainRequest) -> ProviderResult<OptionChain> {
        self.inner.option_chain_for(req)
    }

    async fn fundamentals(&self, req: &FundamentalsRequest) -> ProviderResult<Fundamentals> {
        let s = self.equity(&req.key, "fundamentals")?;
        self.inner
            .fundamentals_for(&s, req.period_type, req.periods, self.inner.clock.now())
            .ok_or_else(|| ProviderError::NotFound(format!("no fundamentals for {}", req.key)))
    }

    async fn estimates(&self, key: &SecurityKey) -> ProviderResult<Estimates> {
        let s = self.equity(key, "estimates")?;
        self.inner.estimates_for(&s, self.inner.clock.now()).ok_or_else(|| ProviderError::NotFound(format!("no estimates for {key}")))
    }

    async fn earnings(&self, key: &SecurityKey) -> ProviderResult<EarningsHistory> {
        let s = self.equity(key, "earnings")?;
        self.inner.earnings_for(&s, self.inner.clock.now()).ok_or_else(|| ProviderError::NotFound(format!("no earnings for {key}")))
    }

    async fn recommendations(&self, key: &SecurityKey) -> ProviderResult<Recommendations> {
        let s = self.equity(key, "analyst recommendations")?;
        self.inner
            .recommendations_for(&s, self.inner.clock.now())
            .ok_or_else(|| ProviderError::NotFound(format!("no recommendations for {key}")))
    }

    async fn holders(&self, key: &SecurityKey) -> ProviderResult<Holders> {
        let s = self.inner.resolve(key)?;
        self.inner.holders_for(&s, self.inner.clock.now()).ok_or_else(|| ProviderError::NotFound(format!("no holders for {key}")))
    }

    async fn dividends(&self, key: &SecurityKey) -> ProviderResult<Dividends> {
        let s = self.inner.resolve(key)?;
        self.inner.dividends_for(&s, self.inner.clock.now()).ok_or_else(|| ProviderError::NotFound(format!("no dividends for {key}")))
    }

    async fn filings(&self, req: &FilingsRequest) -> ProviderResult<FilingsPage> {
        self.inner.filings_for(req)
    }

    async fn filing_document(&self, filing: &Filing) -> ProviderResult<FilingDocument> {
        self.inner.filing_document_for(filing)
    }

    async fn news(&self, q: &NewsQuery) -> ProviderResult<meridian_types::NewsPage> {
        Ok(self.inner.news_for(q))
    }

    async fn transcripts(&self, key: &SecurityKey) -> ProviderResult<Vec<Transcript>> {
        let s = self.equity(key, "transcripts")?;
        self.inner.transcripts_for(&s, self.inner.clock.now()).ok_or_else(|| ProviderError::NotFound(format!("no transcripts for {key}")))
    }

    async fn economic_series(&self, req: &SeriesRequest) -> ProviderResult<EconomicSeries> {
        self.inner.economic_series_for(req)
    }

    async fn economic_calendar(&self, req: &CalendarRequest) -> ProviderResult<Vec<EconomicEvent>> {
        Ok(self.inner.economic_calendar_for(req))
    }

    async fn yield_curve(&self, req: &CurveRequest) -> ProviderResult<YieldCurve> {
        self.inner.yield_curve_for(req)
    }

    fn streaming(&self) -> Option<&dyn StreamingProvider> {
        Some(self)
    }
}

#[async_trait]
impl StreamingProvider for MockProvider {
    async fn connect(&self, sink: StreamSink) -> ProviderResult<Box<dyn StreamHandle>> {
        let h = stream::open(self.inner.clone(), sink).map_err(ProviderError::Upstream)?;
        Ok(Box::new(h))
    }
}

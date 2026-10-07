use std::sync::Arc;

use async_trait::async_trait;
use meridian_types::{
    BarSeries, CompanyProfile, DividendCalendar, Dividends, EarningsCalendar, EarningsHistory, EconomicEvent,
    EconomicSeries, Estimates, FilingDocument, Filing, FilingsPage, Fundamentals, Holders, Instrument, NewsPage,
    OptionChain, ProviderId, Quote, Recommendations, SecurityKey, StreamEvent, Transcript, YieldCurve,
};

use crate::capability::{Capabilities, Capability};
use crate::error::{ProviderError, ProviderResult};
use crate::request::{
    BarsRequest, CalendarRequest, ChainRequest, CurveRequest, EventCalendarRequest, FilingsRequest,
    FundamentalsRequest, InstrumentQuery, NewsQuery, SeriesRequest,
};

fn unsupported<T>(capability: Capability) -> ProviderResult<T> {
    Err(ProviderError::Unsupported { capability })
}

/// Every data source implements this trait. Methods a provider doesn't
/// support keep the default, which returns `Unsupported`; routing consults
/// [`Provider::capabilities`] first, so defaults are rarely reached.
#[async_trait]
pub trait Provider: Send + Sync + 'static {
    fn id(&self) -> ProviderId;
    fn capabilities(&self) -> &Capabilities;

    /// Whether this provider can serve `key` at all (e.g. a crypto venue
    /// covers only the pairs it lists). Checked before routing and stream
    /// subscription; defaults to true.
    fn covers(&self, _key: &SecurityKey) -> bool {
        true
    }

    /// Instruments matching `q.text`. An empty text lists the provider's
    /// universe (up to `q.limit`); the engine loads reference data this way.
    async fn search(&self, _q: &InstrumentQuery) -> ProviderResult<Vec<Instrument>> {
        unsupported(Capability::Search)
    }
    async fn instrument(&self, _key: &SecurityKey) -> ProviderResult<Instrument> {
        unsupported(Capability::Reference)
    }
    async fn profile(&self, _key: &SecurityKey) -> ProviderResult<CompanyProfile> {
        unsupported(Capability::Profile)
    }
    async fn quotes(&self, _keys: &[SecurityKey]) -> ProviderResult<Vec<Quote>> {
        unsupported(Capability::Quotes)
    }
    async fn bars(&self, req: &BarsRequest) -> ProviderResult<BarSeries> {
        unsupported(if req.interval.is_intraday() { Capability::IntradayBars } else { Capability::DailyBars })
    }
    async fn option_chain(&self, _req: &ChainRequest) -> ProviderResult<OptionChain> {
        unsupported(Capability::OptionChain)
    }
    async fn fundamentals(&self, _req: &FundamentalsRequest) -> ProviderResult<Fundamentals> {
        unsupported(Capability::Fundamentals)
    }
    async fn estimates(&self, _key: &SecurityKey) -> ProviderResult<Estimates> {
        unsupported(Capability::Estimates)
    }
    async fn earnings(&self, _key: &SecurityKey) -> ProviderResult<EarningsHistory> {
        unsupported(Capability::Earnings)
    }
    async fn recommendations(&self, _key: &SecurityKey) -> ProviderResult<Recommendations> {
        unsupported(Capability::Recommendations)
    }
    async fn holders(&self, _key: &SecurityKey) -> ProviderResult<Holders> {
        unsupported(Capability::Holders)
    }
    async fn dividends(&self, _key: &SecurityKey) -> ProviderResult<Dividends> {
        unsupported(Capability::Dividends)
    }
    async fn filings(&self, _req: &FilingsRequest) -> ProviderResult<FilingsPage> {
        unsupported(Capability::Filings)
    }
    async fn filing_document(&self, _filing: &Filing) -> ProviderResult<FilingDocument> {
        unsupported(Capability::Filings)
    }
    async fn news(&self, _q: &NewsQuery) -> ProviderResult<NewsPage> {
        unsupported(Capability::News)
    }
    async fn transcripts(&self, _key: &SecurityKey) -> ProviderResult<Vec<Transcript>> {
        unsupported(Capability::Transcripts)
    }
    async fn economic_series(&self, _req: &SeriesRequest) -> ProviderResult<EconomicSeries> {
        unsupported(Capability::EconomicSeries)
    }
    async fn economic_calendar(&self, _req: &CalendarRequest) -> ProviderResult<Vec<EconomicEvent>> {
        unsupported(Capability::EconomicCalendar)
    }
    async fn yield_curve(&self, _req: &CurveRequest) -> ProviderResult<YieldCurve> {
        unsupported(Capability::YieldCurve)
    }
    /// Earnings releases dated in `[req.from, req.to]` for `req.keys` (all
    /// covered securities when empty), oldest first.
    async fn earnings_calendar(&self, _req: &EventCalendarRequest) -> ProviderResult<EarningsCalendar> {
        unsupported(Capability::EarningsCalendar)
    }
    /// Dividend and split events whose ex-date is in `[req.from, req.to]`
    /// for `req.keys` (all covered securities when empty), oldest first.
    async fn dividend_calendar(&self, _req: &EventCalendarRequest) -> ProviderResult<DividendCalendar> {
        unsupported(Capability::DividendCalendar)
    }

    /// Push feed, if the provider has one.
    fn streaming(&self) -> Option<&dyn StreamingProvider> {
        None
    }
}

/// Receives normalized events from a streaming provider. Implementations
/// must not block.
pub trait EventSink: Send + Sync {
    fn send(&self, provider: &ProviderId, event: StreamEvent);
}

pub type StreamSink = Arc<dyn EventSink>;

#[async_trait]
pub trait StreamingProvider: Send + Sync {
    /// Opens the feed and returns a handle for subscription management.
    /// The provider owns reconnects and reports state changes through the
    /// sink as `StreamEvent::Status`.
    async fn connect(&self, sink: StreamSink) -> ProviderResult<Box<dyn StreamHandle>>;
}

/// Subscription control for an open feed. Calls are non-blocking; the
/// provider applies them asynchronously.
pub trait StreamHandle: Send + Sync {
    fn subscribe(&self, keys: &[SecurityKey]);
    fn unsubscribe(&self, keys: &[SecurityKey]);
    /// Closes the feed.
    fn close(&self);
}

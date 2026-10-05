use std::time::Duration;

use meridian_types::{AssetClass, DataDelay, FeedSource};
use serde::{Deserialize, Serialize};

/// A dataset a provider may offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Capability {
    Search,
    Reference,
    Profile,
    Quotes,
    DailyBars,
    IntradayBars,
    Stream,
    OptionChain,
    Fundamentals,
    Estimates,
    Earnings,
    Recommendations,
    Holders,
    Dividends,
    Filings,
    News,
    Transcripts,
    EconomicSeries,
    EconomicCalendar,
    YieldCurve,
}

impl Capability {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Capability::Search => "security search",
            Capability::Reference => "reference data",
            Capability::Profile => "company profile",
            Capability::Quotes => "quotes",
            Capability::DailyBars => "daily history",
            Capability::IntradayBars => "intraday history",
            Capability::Stream => "streaming quotes",
            Capability::OptionChain => "option chains",
            Capability::Fundamentals => "financial statements",
            Capability::Estimates => "consensus estimates",
            Capability::Earnings => "earnings history",
            Capability::Recommendations => "analyst recommendations",
            Capability::Holders => "holders",
            Capability::Dividends => "dividends",
            Capability::Filings => "filings",
            Capability::News => "news",
            Capability::Transcripts => "earnings transcripts",
            Capability::EconomicSeries => "economic series",
            Capability::EconomicCalendar => "economic calendar",
            Capability::YieldCurve => "yield curves",
        }
    }
}

/// How long provider data may be kept, per the provider's terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CachePolicy {
    Unrestricted,
    /// Must be refreshed or discarded after this age (e.g. CoinGecko 24 h).
    MaxAge(Duration),
    /// Must not be persisted at all.
    NoStore,
    /// May be stored, but must be deleted when the subscription ends
    /// (e.g. Finnhub). `store.purge_provider` implements the deletion.
    PurgeOnUnsubscribe,
}

/// Whether the provider's terms allow sending its data to the ASK model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AiPolicy {
    Allowed,
    Forbidden,
    /// Terms not yet reviewed. Treated as forbidden.
    Unreviewed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityEntry {
    pub capability: Capability,
    pub asset_classes: Vec<AssetClass>,
    pub delay: DataDelay,
    pub source: FeedSource,
    /// Human-readable history depth, e.g. `since 2016`.
    pub history: Option<String>,
}

/// Token-bucket parameters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RateLimit {
    /// Bucket size.
    pub burst: u32,
    /// Tokens added per second.
    pub per_second: f64,
}

impl RateLimit {
    #[must_use]
    pub fn per_minute(n: u32) -> Self {
        Self { burst: n.clamp(1, 10), per_second: f64::from(n) / 60.0 }
    }

    #[must_use]
    pub fn per_second(n: u32) -> Self {
        Self { burst: n.max(1), per_second: f64::from(n) }
    }
}

/// What a provider offers and under which terms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capabilities {
    pub entries: Vec<CapabilityEntry>,
    pub rate_limit: Option<RateLimit>,
    pub max_stream_symbols: Option<usize>,
    pub cache_policy: CachePolicy,
    /// Attribution text the terms require on screen.
    pub attribution: Option<String>,
    /// Some free tiers are licensed "non-display".
    pub display_allowed: bool,
    pub ai_policy: AiPolicy,
    pub requires_credentials: bool,
    /// Terms summary shown on the Data Sources screen.
    pub terms_note: String,
    /// Official documentation the integration was built against.
    pub docs_url: String,
}

impl Capabilities {
    #[must_use]
    pub fn entry(&self, cap: Capability, asset: Option<AssetClass>) -> Option<&CapabilityEntry> {
        self.entries.iter().find(|e| {
            e.capability == cap && asset.is_none_or(|a| e.asset_classes.is_empty() || e.asset_classes.contains(&a))
        })
    }

    #[must_use]
    pub fn supports(&self, cap: Capability, asset: Option<AssetClass>) -> bool {
        self.entry(cap, asset).is_some()
    }
}

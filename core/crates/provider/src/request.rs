use chrono::NaiveDate;
use meridian_types::{Adjustment, BarInterval, MarketSector, PeriodType, SecurityKey, UnixNanos};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstrumentQuery {
    pub text: String,
    pub sector: Option<MarketSector>,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BarsRequest {
    pub key: SecurityKey,
    pub interval: BarInterval,
    /// Inclusive start. `None` = provider's earliest.
    pub from: Option<UnixNanos>,
    /// Exclusive end. `None` = now.
    pub to: Option<UnixNanos>,
    pub adjustment: Adjustment,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainRequest {
    pub underlying: SecurityKey,
    /// `None` = all listed expiries.
    pub expiry: Option<NaiveDate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FundamentalsRequest {
    pub key: SecurityKey,
    pub period_type: PeriodType,
    /// Most recent N periods.
    pub periods: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FilingsRequest {
    pub key: Option<SecurityKey>,
    /// Empty = all forms.
    pub forms: Vec<String>,
    pub limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NewsScope {
    /// Company news for the given keys (CN).
    Company,
    /// Top stories (TOP).
    Top,
    /// General market news (N).
    Market,
    /// Press releases.
    PressReleases,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewsQuery {
    pub scope: NewsScope,
    pub keys: Vec<SecurityKey>,
    /// Free-text filter applied to headline and summary.
    pub text: Option<String>,
    pub from: Option<UnixNanos>,
    pub to: Option<UnixNanos>,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeriesRequest {
    pub id: String,
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalendarRequest {
    pub from: NaiveDate,
    pub to: NaiveDate,
    /// ISO country codes; empty = all.
    pub countries: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurveRequest {
    /// e.g. `UST`.
    pub name: String,
    /// `None` = latest.
    pub date: Option<NaiveDate>,
}

/// A date window of corporate events (earnings releases, dividend
/// ex-dates) for some securities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventCalendarRequest {
    /// Inclusive.
    pub from: NaiveDate,
    /// Inclusive.
    pub to: NaiveDate,
    /// Securities to include. Empty = every security the provider covers.
    pub keys: Vec<SecurityKey>,
}

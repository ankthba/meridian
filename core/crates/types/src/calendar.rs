//! Corporate event calendars: scheduled earnings releases and dividend
//! ex-dates across many securities (CALENDAR, TODAY).

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::company::Dividend;
use crate::key::SecurityKey;
use crate::provenance::Provenance;

/// When an earnings release comes out relative to the US trading session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EarningsSession {
    BeforeOpen,
    DuringMarket,
    AfterClose,
}

impl EarningsSession {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            EarningsSession::BeforeOpen => "Before open",
            EarningsSession::DuringMarket => "During market",
            EarningsSession::AfterClose => "After close",
        }
    }
}

/// One scheduled (or recently reported) earnings release.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EarningsEvent {
    pub key: SecurityKey,
    /// Release date (US market date).
    pub date: NaiveDate,
    /// `None` when the source doesn't say.
    pub session: Option<EarningsSession>,
    /// Fiscal year and quarter the release reports, as the source labels them.
    pub fiscal_year: Option<i32>,
    pub fiscal_quarter: Option<u32>,
    /// Per-share; the source's consensus where it gives one.
    pub eps_estimate: Option<f64>,
    pub eps_actual: Option<f64>,
    pub revenue_estimate: Option<f64>,
    pub revenue_actual: Option<f64>,
}

/// Earnings releases in a date window, oldest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EarningsCalendar {
    pub events: Vec<EarningsEvent>,
    pub provenance: Provenance,
}

/// One dividend or split event for a security.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DividendEvent {
    pub key: SecurityKey,
    pub dividend: Dividend,
}

/// Dividend and split events whose ex-date falls in a window, oldest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DividendCalendar {
    pub events: Vec<DividendEvent>,
    pub provenance: Provenance,
}

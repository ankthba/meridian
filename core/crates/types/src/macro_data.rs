//! Economic series and calendar.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::provenance::Provenance;
use crate::time::UnixNanos;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub date: NaiveDate,
    pub value: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EconomicSeries {
    /// Source series ID, e.g. FRED `CPIAUCSL`.
    pub id: String,
    pub title: String,
    pub units: String,
    pub frequency: String,
    pub seasonal_adjustment: Option<String>,
    pub last_updated: Option<UnixNanos>,
    pub notes: Option<String>,
    pub observations: Vec<Observation>,
    pub provenance: Provenance,
}

impl EconomicSeries {
    #[must_use]
    pub fn latest(&self) -> Option<&Observation> {
        self.observations.iter().rev().find(|o| o.value.is_some())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Importance {
    Low,
    Medium,
    High,
}

/// One scheduled economic release (ECO).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EconomicEvent {
    pub release_time: UnixNanos,
    /// Whether `release_time` includes a known time of day.
    pub time_known: bool,
    pub country: String,
    pub event: String,
    /// Reference period, e.g. `Sep`, `Q3`.
    pub period: Option<String>,
    pub actual: Option<f64>,
    pub consensus: Option<f64>,
    pub prior: Option<f64>,
    pub unit: Option<String>,
    pub importance: Importance,
    /// Series to chart for this event, if known.
    pub series_id: Option<String>,
    pub provenance: Provenance,
}

/// One point on a yield curve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurvePoint {
    /// Tenor label as published, e.g. `3 Mo`, `10 Yr`.
    pub tenor: String,
    /// Tenor in years, for plotting.
    pub years: f64,
    pub yield_pct: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct YieldCurve {
    pub name: String,
    pub date: NaiveDate,
    pub points: Vec<CurvePoint>,
    pub provenance: Provenance,
}

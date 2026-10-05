//! Fundamentals, estimates, recommendations, holders, dividends.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::key::SecurityKey;
use crate::provenance::Provenance;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StatementKind {
    Income,
    Balance,
    CashFlow,
}

impl StatementKind {
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            StatementKind::Income => "Income Statement",
            StatementKind::Balance => "Balance Sheet",
            StatementKind::CashFlow => "Cash Flow",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PeriodType {
    Annual,
    Quarterly,
    Ttm,
}

/// One line of a financial statement. `code` is our normalized concept
/// (`revenue`, `net_income`, …) so screens and screeners don't depend on
/// vendor or XBRL tag names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatementLine {
    pub code: String,
    pub label: String,
    pub value: Option<f64>,
    /// Indentation level for display (0 = top-level).
    pub depth: u8,
    /// The source tag, e.g. `us-gaap:Revenues`, when as-reported.
    pub source_tag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Statement {
    pub kind: StatementKind,
    pub period_type: PeriodType,
    pub fiscal_year: i32,
    /// `FY`, `Q1` … `Q4`.
    pub fiscal_period: String,
    pub period_end: NaiveDate,
    pub currency: String,
    pub lines: Vec<StatementLine>,
}

impl Statement {
    #[must_use]
    pub fn value(&self, code: &str) -> Option<f64> {
        self.lines.iter().find(|l| l.code == code).and_then(|l| l.value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fundamentals {
    pub key: SecurityKey,
    pub statements: Vec<Statement>,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EstimateMetric {
    Eps,
    Revenue,
    Ebitda,
    NetIncome,
}

impl EstimateMetric {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            EstimateMetric::Eps => "EPS Adj",
            EstimateMetric::Revenue => "Revenue",
            EstimateMetric::Ebitda => "EBITDA",
            EstimateMetric::NetIncome => "Net Income",
        }
    }
}

/// Consensus estimate for one metric and period (EE).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Estimate {
    pub metric: EstimateMetric,
    pub period_type: PeriodType,
    pub fiscal_label: String,
    pub period_end: NaiveDate,
    pub mean: Option<f64>,
    pub median: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub count: Option<u32>,
    pub actual: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Estimates {
    pub key: SecurityKey,
    pub estimates: Vec<Estimate>,
    pub provenance: Provenance,
}

/// One reported earnings event (ERN).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EarningsRecord {
    pub fiscal_label: String,
    pub period_end: NaiveDate,
    pub announce_date: Option<NaiveDate>,
    pub eps_actual: Option<f64>,
    pub eps_estimate: Option<f64>,
    pub revenue_actual: Option<f64>,
    pub revenue_estimate: Option<f64>,
}

impl EarningsRecord {
    #[must_use]
    pub fn eps_surprise_pct(&self) -> Option<f64> {
        let (a, e) = (self.eps_actual?, self.eps_estimate?);
        if e == 0.0 {
            return None;
        }
        Some((a - e) / e.abs() * 100.0)
    }

    #[must_use]
    pub fn revenue_surprise_pct(&self) -> Option<f64> {
        let (a, e) = (self.revenue_actual?, self.revenue_estimate?);
        if e == 0.0 {
            return None;
        }
        Some((a - e) / e.abs() * 100.0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EarningsHistory {
    pub key: SecurityKey,
    pub records: Vec<EarningsRecord>,
    pub provenance: Provenance,
}

/// A single broker rating (ANR).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalystRating {
    pub firm: String,
    pub analyst: Option<String>,
    pub rating: String,
    /// Normalized 1 (sell) … 5 (buy).
    pub score: Option<u8>,
    pub target_price: Option<f64>,
    pub date: NaiveDate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recommendations {
    pub key: SecurityKey,
    pub as_of: NaiveDate,
    pub strong_buy: u32,
    pub buy: u32,
    pub hold: u32,
    pub sell: u32,
    pub strong_sell: u32,
    pub target_mean: Option<f64>,
    pub target_high: Option<f64>,
    pub target_low: Option<f64>,
    pub ratings: Vec<AnalystRating>,
    pub provenance: Provenance,
}

impl Recommendations {
    #[must_use]
    pub fn total(&self) -> u32 {
        self.strong_buy + self.buy + self.hold + self.sell + self.strong_sell
    }

    /// Consensus score on a 1 (sell) … 5 (buy) scale.
    #[must_use]
    pub fn consensus_score(&self) -> Option<f64> {
        let n = self.total();
        if n == 0 {
            return None;
        }
        let s = 5 * self.strong_buy + 4 * self.buy + 3 * self.hold + 2 * self.sell + self.strong_sell;
        Some(f64::from(s) / f64::from(n))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HolderKind {
    Institution,
    Fund,
    Insider,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Holder {
    pub name: String,
    pub kind: HolderKind,
    pub shares: f64,
    pub pct_outstanding: Option<f64>,
    pub market_value: Option<f64>,
    pub change_shares: Option<f64>,
    pub filing_date: Option<NaiveDate>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Holders {
    pub key: SecurityKey,
    pub holders: Vec<Holder>,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DividendKind {
    Regular,
    Special,
    /// Stock split; `amount` holds the ratio (2.0 = 2-for-1).
    Split,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dividend {
    pub declared_date: Option<NaiveDate>,
    pub ex_date: NaiveDate,
    pub record_date: Option<NaiveDate>,
    pub pay_date: Option<NaiveDate>,
    pub amount: f64,
    pub currency: String,
    pub frequency: Option<String>,
    pub kind: DividendKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dividends {
    pub key: SecurityKey,
    pub dividends: Vec<Dividend>,
    pub provenance: Provenance,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consensus_score() {
        let r = Recommendations {
            key: SecurityKey::equity("X"),
            as_of: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            strong_buy: 2,
            buy: 2,
            hold: 0,
            sell: 0,
            strong_sell: 0,
            target_mean: None,
            target_high: None,
            target_low: None,
            ratings: vec![],
            provenance: Provenance::synthetic(0),
        };
        assert_eq!(r.consensus_score(), Some(4.5));
    }

    #[test]
    fn surprise() {
        let e = EarningsRecord {
            fiscal_label: "Q1 26".into(),
            period_end: NaiveDate::from_ymd_opt(2026, 3, 31).unwrap(),
            announce_date: None,
            eps_actual: Some(1.10),
            eps_estimate: Some(1.00),
            revenue_actual: None,
            revenue_estimate: None,
        };
        assert!((e.eps_surprise_pct().unwrap() - 10.0).abs() < 1e-9);
        assert_eq!(e.revenue_surprise_pct(), None);
    }
}

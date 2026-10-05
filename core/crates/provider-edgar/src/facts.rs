//! `https://data.sec.gov/api/xbrl/companyfacts/CIK##########.json` →
//! normalized, as-reported financial statements.
//!
//! Payload shape: `{cik, entityName, facts: {<taxonomy>: {<Concept>: {label,
//! description, units: {<unit>: [{start?, end, val, accn, fy, fp, form,
//! filed, frame?}]}}}}}`. `fy`/`fp`/`form`/`filed` describe the *filing* that
//! reported the fact, not the fact's own period: a FY2025 10-K also carries
//! FY2024 and FY2023 comparatives tagged `fy: 2025`. Periods are therefore
//! classified from their own `start`/`end` dates against a fiscal calendar
//! (see [`FiscalCalendar`]).
//!
//! Rules (also in the README):
//! - Only facts from 10-K, 10-K/A, 10-Q, 10-Q/A count. For each
//!   (concept, start, end) the most recently filed value wins, so
//!   restatements replace originals.
//! - Durations of 350–380 days are fiscal years, 80–100 days are quarters;
//!   instants (no `start`) are balance-sheet values.
//! - Each line has an ordered list of us-gaap concepts; the first concept
//!   with a value for the period wins and is recorded in `source_tag`.
//!   As-reported values beat derived ones.
//! - Additive flow items may be derived: Q4 = FY − 9-month YTD (or FY −
//!   Q1 − Q2 − Q3), Q2/Q3 = YTD − prior YTD (10-Q cash-flow statements are
//!   usually YTD only). EPS and weighted-average share counts are never
//!   derived, and balance-sheet values never are.

use std::collections::HashMap;
use std::fmt;

use chrono::{Datelike, Duration, NaiveDate};
use meridian_provider::{ProviderError, ProviderResult};
use meridian_types::{PeriodType, Statement, StatementKind, StatementLine};
use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

const ALLOWED_FORMS: [&str; 4] = ["10-K", "10-K/A", "10-Q", "10-Q/A"];
const ANNUAL_DAYS: std::ops::RangeInclusive<i64> = 350..=380;
const QUARTER_DAYS: std::ops::RangeInclusive<i64> = 80..=100;
/// A 10-K filed within this many days of the fiscal year end is the
/// original report for that year, so its `fy` names the year.
const ORIGINAL_FILING_DAYS: i64 = 300;
/// Tolerance when matching a date to a fiscal year end (52/53-week years).
const FY_END_TOLERANCE_DAYS: i64 = 7;
const DAYS_PER_YEAR: f64 = 365.2425;
const DAYS_PER_QUARTER: f64 = 91.31;

pub(crate) const TAG_Q4: &str = "derived:FY-9M";
pub(crate) const TAG_YTD: &str = "derived:YTD-diff";
pub(crate) const TAG_TTM: &str = "derived:TTM";
pub(crate) const TAG_EBITDA: &str = "derived:operating_income+D&A";
pub(crate) const TAG_FCF: &str = "derived:cfo-capex";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnitKind {
    /// The reporting currency, e.g. `USD`.
    Currency,
    /// `<currency>/shares`.
    PerShare,
    Shares,
}

#[derive(Debug, Clone, Copy)]
enum Source {
    /// Duration fact. `additive` facts may be derived by subtraction or
    /// summed into TTM.
    Flow {
        tags: &'static [&'static str],
        unit: UnitKind,
        additive: bool,
    },
    /// Instant (balance-sheet) fact in the reporting currency.
    Instant { tags: &'static [&'static str] },
    /// `operating_income + d_and_a`.
    Ebitda,
    /// `cfo − capex`.
    Fcf,
}

#[derive(Debug, Clone, Copy)]
struct LineDef {
    code: &'static str,
    label: &'static str,
    kind: StatementKind,
    depth: u8,
    source: Source,
}

const fn flow(
    code: &'static str,
    label: &'static str,
    kind: StatementKind,
    depth: u8,
    tags: &'static [&'static str],
) -> LineDef {
    LineDef {
        code,
        label,
        kind,
        depth,
        source: Source::Flow {
            tags,
            unit: UnitKind::Currency,
            additive: true,
        },
    }
}

const fn instant(
    code: &'static str,
    label: &'static str,
    depth: u8,
    tags: &'static [&'static str],
) -> LineDef {
    LineDef {
        code,
        label,
        kind: StatementKind::Balance,
        depth,
        source: Source::Instant { tags },
    }
}

use StatementKind::{CashFlow, Income};

/// Every normalized line, in display order within each statement.
const LINES: &[LineDef] = &[
    // Income statement.
    flow(
        "revenue",
        "Revenue",
        Income,
        0,
        &[
            "Revenues",
            "RevenueFromContractWithCustomerExcludingAssessedTax",
            "SalesRevenueNet",
            "RevenueFromContractWithCustomerIncludingAssessedTax",
        ],
    ),
    flow(
        "cost_of_revenue",
        "Cost of Revenue",
        Income,
        1,
        &[
            "CostOfRevenue",
            "CostOfGoodsAndServicesSold",
            "CostOfGoodsSold",
        ],
    ),
    flow("gross_profit", "Gross Profit", Income, 0, &["GrossProfit"]),
    flow(
        "sga",
        "Selling, General & Administrative",
        Income,
        1,
        &["SellingGeneralAndAdministrativeExpense"],
    ),
    flow(
        "rnd",
        "Research & Development",
        Income,
        1,
        &["ResearchAndDevelopmentExpense"],
    ),
    flow(
        "operating_income",
        "Operating Income",
        Income,
        0,
        &["OperatingIncomeLoss"],
    ),
    flow(
        "interest_expense",
        "Interest Expense",
        Income,
        1,
        &["InterestExpense", "InterestExpenseNonoperating"],
    ),
    flow(
        "pretax_income",
        "Pretax Income",
        Income,
        0,
        &[
            "IncomeLossFromContinuingOperationsBeforeIncomeTaxesExtraordinaryItemsNoncontrollingInterest",
            "IncomeLossFromContinuingOperationsBeforeIncomeTaxesMinorityInterestAndIncomeLossFromEquityMethodInvestments",
        ],
    ),
    flow(
        "income_tax",
        "Income Tax",
        Income,
        1,
        &["IncomeTaxExpenseBenefit"],
    ),
    flow("net_income", "Net Income", Income, 0, &["NetIncomeLoss"]),
    LineDef {
        code: "eps_basic",
        label: "EPS (Basic)",
        kind: Income,
        depth: 1,
        source: Source::Flow {
            tags: &["EarningsPerShareBasic"],
            unit: UnitKind::PerShare,
            additive: false,
        },
    },
    LineDef {
        code: "eps_diluted",
        label: "EPS (Diluted)",
        kind: Income,
        depth: 1,
        source: Source::Flow {
            tags: &["EarningsPerShareDiluted"],
            unit: UnitKind::PerShare,
            additive: false,
        },
    },
    LineDef {
        code: "shares_diluted",
        label: "Diluted Shares (Weighted Avg)",
        kind: Income,
        depth: 1,
        source: Source::Flow {
            tags: &["WeightedAverageNumberOfDilutedSharesOutstanding"],
            unit: UnitKind::Shares,
            additive: false,
        },
    },
    LineDef {
        code: "ebitda",
        label: "EBITDA",
        kind: Income,
        depth: 0,
        source: Source::Ebitda,
    },
    // Balance sheet.
    instant(
        "cash",
        "Cash & Equivalents",
        1,
        &["CashAndCashEquivalentsAtCarryingValue"],
    ),
    instant(
        "short_term_investments",
        "Short-Term Investments",
        1,
        &[
            "ShortTermInvestments",
            "MarketableSecuritiesCurrent",
            "AvailableForSaleSecuritiesDebtSecuritiesCurrent",
        ],
    ),
    instant(
        "receivables",
        "Receivables",
        1,
        &["AccountsReceivableNetCurrent"],
    ),
    instant("inventory", "Inventory", 1, &["InventoryNet"]),
    instant(
        "total_current_assets",
        "Total Current Assets",
        0,
        &["AssetsCurrent"],
    ),
    instant(
        "ppe_net",
        "Property, Plant & Equipment (Net)",
        1,
        &["PropertyPlantAndEquipmentNet"],
    ),
    instant("goodwill", "Goodwill", 1, &["Goodwill"]),
    instant("total_assets", "Total Assets", 0, &["Assets"]),
    instant(
        "accounts_payable",
        "Accounts Payable",
        1,
        &["AccountsPayableCurrent"],
    ),
    instant(
        "short_term_debt",
        "Short-Term Debt",
        1,
        &["DebtCurrent", "LongTermDebtCurrent", "ShortTermBorrowings"],
    ),
    instant(
        "total_current_liabilities",
        "Total Current Liabilities",
        0,
        &["LiabilitiesCurrent"],
    ),
    instant(
        "long_term_debt",
        "Long-Term Debt",
        1,
        &["LongTermDebtNoncurrent", "LongTermDebt"],
    ),
    instant(
        "total_liabilities",
        "Total Liabilities",
        0,
        &["Liabilities"],
    ),
    instant(
        "total_equity",
        "Total Equity",
        0,
        &[
            "StockholdersEquity",
            "StockholdersEquityIncludingPortionAttributableToNoncontrollingInterest",
        ],
    ),
    // Cash flow statement.
    flow(
        "cfo",
        "Cash from Operations",
        CashFlow,
        0,
        &["NetCashProvidedByUsedInOperatingActivities"],
    ),
    flow(
        "d_and_a",
        "Depreciation & Amortization",
        CashFlow,
        1,
        &[
            "DepreciationDepreciationAndAmortization",
            "DepreciationAndAmortization",
        ],
    ),
    flow(
        "capex",
        "Capital Expenditures",
        CashFlow,
        1,
        &["PaymentsToAcquirePropertyPlantAndEquipment"],
    ),
    LineDef {
        code: "fcf",
        label: "Free Cash Flow",
        kind: CashFlow,
        depth: 0,
        source: Source::Fcf,
    },
    flow(
        "cfi",
        "Cash from Investing",
        CashFlow,
        0,
        &["NetCashProvidedByUsedInInvestingActivities"],
    ),
    flow(
        "dividends_paid",
        "Dividends Paid",
        CashFlow,
        1,
        &["PaymentsOfDividends", "PaymentsOfDividendsCommonStock"],
    ),
    flow(
        "buybacks",
        "Share Repurchases",
        CashFlow,
        1,
        &["PaymentsForRepurchaseOfCommonStock"],
    ),
    flow(
        "cff",
        "Cash from Financing",
        CashFlow,
        0,
        &["NetCashProvidedByUsedInFinancingActivities"],
    ),
    flow(
        "net_change_cash",
        "Net Change in Cash",
        CashFlow,
        0,
        &[
            "CashCashEquivalentsRestrictedCashAndRestrictedCashEquivalentsPeriodIncreaseDecreaseIncludingExchangeRateEffect",
            "CashAndCashEquivalentsPeriodIncreaseDecrease",
        ],
    ),
];

fn line_tags(l: &LineDef) -> &'static [&'static str] {
    match l.source {
        Source::Flow { tags, .. } | Source::Instant { tags } => tags,
        Source::Ebitda | Source::Fcf => &[],
    }
}

fn line_unit(l: &LineDef) -> UnitKind {
    match l.source {
        Source::Flow { unit, .. } => unit,
        Source::Instant { .. } | Source::Ebitda | Source::Fcf => UnitKind::Currency,
    }
}

fn is_wanted(concept: &str) -> bool {
    LINES.iter().any(|l| line_tags(l).contains(&concept))
}

// ---------------------------------------------------------------------------
// Payload DTOs. Only the us-gaap concepts we map are materialized; the rest
// of the (often multi-megabyte) payload is skipped while parsing.

#[derive(Deserialize)]
struct CompanyFactsDto {
    facts: TaxonomiesDto,
}

#[derive(Default, Deserialize)]
struct TaxonomiesDto {
    #[serde(rename = "us-gaap", default)]
    us_gaap: WantedConcepts,
}

#[derive(Default)]
struct WantedConcepts(HashMap<String, ConceptDto>);

impl<'de> Deserialize<'de> for WantedConcepts {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = WantedConcepts;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a map of us-gaap concepts")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<WantedConcepts, A::Error> {
                let mut out = HashMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    if is_wanted(&key) {
                        let c: ConceptDto = map.next_value()?;
                        out.insert(key, c);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(WantedConcepts(out))
            }
        }
        d.deserialize_map(V)
    }
}

#[derive(Deserialize)]
struct ConceptDto {
    #[serde(default)]
    units: HashMap<String, Vec<FactDto>>,
}

#[derive(Deserialize)]
struct FactDto {
    #[serde(default)]
    start: Option<String>,
    end: String,
    val: f64,
    accn: String,
    #[serde(default)]
    fy: Option<i32>,
    #[serde(default)]
    fp: Option<String>,
    form: String,
    filed: String,
}

// ---------------------------------------------------------------------------
// Normalized facts.

#[derive(Debug, Clone, Copy)]
struct Fact {
    val: f64,
    filed: NaiveDate,
}

#[derive(Debug, Default)]
struct ConceptFacts {
    /// end → [(start, fact)], one entry per distinct start.
    durations: HashMap<NaiveDate, Vec<(NaiveDate, Fact)>>,
    instants: HashMap<NaiveDate, Fact>,
}

/// An annual (10-K, `fp: FY`, ~1 year) duration seen in the payload, before
/// de-duplication. Used to build the fiscal calendar.
#[derive(Debug, Clone, Copy)]
struct AnnualObs {
    start: NaiveDate,
    end: NaiveDate,
    filed: NaiveDate,
    fy: Option<i32>,
}

/// Company facts reduced to the concepts we map, de-duplicated.
#[derive(Debug)]
pub(crate) struct FactsIndex {
    pub currency: String,
    concepts: HashMap<&'static str, ConceptFacts>,
    annual: Vec<AnnualObs>,
    /// Number of facts ending on each date, for locating quarter ends.
    end_counts: HashMap<NaiveDate, usize>,
}

fn parse_date(s: &str, concept: &str) -> ProviderResult<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").map_err(|_| {
        ProviderError::parse(format!(
            "companyfacts: invalid date {s:?} in us-gaap:{concept}"
        ))
    })
}

fn days(from: NaiveDate, to: NaiveDate) -> i64 {
    (to - from).num_days()
}

/// Reporting currency: USD if any mapped monetary concept reports in USD,
/// otherwise the most used three-letter currency unit.
fn detect_currency(concepts: &HashMap<String, ConceptDto>) -> String {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for l in LINES.iter().filter(|l| line_unit(l) == UnitKind::Currency) {
        for tag in line_tags(l) {
            let Some(c) = concepts.get(*tag) else {
                continue;
            };
            for (unit, facts) in &c.units {
                if unit.len() == 3 && unit.bytes().all(|b| b.is_ascii_uppercase()) {
                    *counts.entry(unit.as_str()).or_default() += facts.len();
                }
            }
        }
    }
    if counts.get("USD").copied().unwrap_or(0) > 0 {
        return "USD".into();
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
        .map_or_else(|| "USD".into(), |(u, _)| u.to_owned())
}

impl FactsIndex {
    pub(crate) fn parse(body: &str) -> ProviderResult<Self> {
        let dto: CompanyFactsDto = crate::http::parse_json(body, "companyfacts")?;
        Self::from_dto(&dto.facts.us_gaap.0)
    }

    fn from_dto(raw: &HashMap<String, ConceptDto>) -> ProviderResult<Self> {
        let currency = detect_currency(raw);
        let per_share = format!("{currency}/shares");
        let mut concepts = HashMap::new();
        let mut annual = Vec::new();
        let mut end_counts: HashMap<NaiveDate, usize> = HashMap::new();
        for l in LINES {
            let unit = match line_unit(l) {
                UnitKind::Currency => currency.as_str(),
                UnitKind::PerShare => per_share.as_str(),
                UnitKind::Shares => "shares",
            };
            for &tag in line_tags(l) {
                if concepts.contains_key(tag) {
                    continue;
                }
                let Some(facts) = raw.get(tag).and_then(|c| c.units.get(unit)) else {
                    continue;
                };
                let mut latest: HashMap<(Option<NaiveDate>, NaiveDate), (Fact, &str)> =
                    HashMap::new();
                for f in facts {
                    if !ALLOWED_FORMS.contains(&f.form.trim()) {
                        continue;
                    }
                    let end = parse_date(&f.end, tag)?;
                    let start = f.start.as_deref().map(|s| parse_date(s, tag)).transpose()?;
                    let filed = parse_date(&f.filed, tag)?;
                    if start.is_some_and(|s| s > end) {
                        continue;
                    }
                    if let Some(s) = start
                        && f.form.trim().starts_with("10-K")
                        && f.fp.as_deref() == Some("FY")
                        && ANNUAL_DAYS.contains(&days(s, end))
                    {
                        annual.push(AnnualObs {
                            start: s,
                            end,
                            filed,
                            fy: f.fy,
                        });
                    }
                    let fact = Fact { val: f.val, filed };
                    let newer =
                        |old: &(Fact, &str)| (filed, f.accn.as_str()) > (old.0.filed, old.1);
                    match latest.get(&(start, end)) {
                        Some(old) if !newer(old) => {}
                        _ => {
                            latest.insert((start, end), (fact, f.accn.as_str()));
                        }
                    }
                }
                let mut cf = ConceptFacts::default();
                for ((start, end), (fact, _)) in latest {
                    *end_counts.entry(end).or_default() += 1;
                    match start {
                        Some(s) => cf.durations.entry(end).or_default().push((s, fact)),
                        None => {
                            cf.instants.insert(end, fact);
                        }
                    }
                }
                concepts.insert(tag, cf);
            }
        }
        Ok(Self {
            currency,
            concepts,
            annual,
            end_counts,
        })
    }

    /// Whether any mapped concept has a fact.
    pub(crate) fn has_facts(&self) -> bool {
        !self.end_counts.is_empty()
    }

    fn date_range(&self) -> Option<(NaiveDate, NaiveDate)> {
        let min = self.end_counts.keys().min()?;
        let max = self.end_counts.keys().max()?;
        Some((*min, *max))
    }

    /// Fiscal calendar from the 10-K annual periods in the payload, or `None`
    /// if there are none (e.g. a new registrant with only 10-Qs).
    pub(crate) fn calendar_from_annual(&self) -> Option<FiscalCalendar> {
        let (min, max) = self.date_range()?;
        let years = annual_years(&self.annual)?;
        Some(FiscalCalendar::finish(years, min, max, &self.end_counts))
    }

    /// Fiscal calendar from the submissions `fiscalYearEnd` (month, day),
    /// labelling each fiscal year by the calendar year it ends in.
    pub(crate) fn calendar_from_fye(&self, month: u32, day: u32) -> Option<FiscalCalendar> {
        let (min, max) = self.date_range()?;
        let fye = |y: i32| {
            (0..4).find_map(|back| NaiveDate::from_ymd_opt(y, month, day.saturating_sub(back)))
        };
        let mut years = Vec::new();
        for y in (min.year() - 1)..=(max.year() + 1) {
            let (Some(prev), Some(end)) = (fye(y - 1), fye(y)) else {
                continue;
            };
            years.push(FiscalYear {
                label: y,
                start: prev + Duration::days(1),
                end,
                reported: false,
                quarter_ends: [None; 4],
            });
        }
        if years.is_empty() {
            return None;
        }
        Some(FiscalCalendar::finish(years, min, max, &self.end_counts))
    }

    fn concept(&self, tag: &str) -> Option<&ConceptFacts> {
        self.concepts.get(tag)
    }

    /// Latest-filed duration ending on `end` whose length is in `range`,
    /// preferring one that starts on `start` if given.
    fn duration(
        &self,
        tag: &str,
        end: NaiveDate,
        range: &std::ops::RangeInclusive<i64>,
        start: Option<NaiveDate>,
    ) -> Option<f64> {
        let list = self.concept(tag)?.durations.get(&end)?;
        let fits = || list.iter().filter(|(s, _)| range.contains(&days(*s, end)));
        if let Some(want) = start
            && let Some((_, f)) = fits().find(|(s, _)| *s == want)
        {
            return Some(f.val);
        }
        fits().max_by_key(|(_, f)| f.filed).map(|(_, f)| f.val)
    }

    fn instant(&self, tag: &str, date: NaiveDate) -> Option<f64> {
        self.concept(tag)?.instants.get(&date).map(|f| f.val)
    }

    /// Year-to-date duration starting at the fiscal year start (±3 days).
    fn ytd_fact(&self, tag: &str, fy_start: NaiveDate, end: NaiveDate) -> Option<f64> {
        let list = self.concept(tag)?.durations.get(&end)?;
        list.iter()
            .filter(|(s, _)| days(fy_start, *s).abs() <= 3)
            .max_by_key(|(_, f)| f.filed)
            .map(|(_, f)| f.val)
    }
}

// ---------------------------------------------------------------------------
// Fiscal calendar.

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FiscalYear {
    /// The company's fiscal-year label (e.g. 2025).
    pub label: i32,
    pub start: NaiveDate,
    pub end: NaiveDate,
    /// Bounds come from a 10-K annual period (not extrapolated).
    pub reported: bool,
    /// Period-end dates of Q1..Q4 found in the facts.
    pub quarter_ends: [Option<NaiveDate>; 4],
}

impl FiscalYear {
    fn period_end(&self) -> NaiveDate {
        self.quarter_ends[3].unwrap_or(self.end)
    }
}

/// Fiscal years in ascending order, covering every fact date.
#[derive(Debug, Clone)]
pub(crate) struct FiscalCalendar {
    pub years: Vec<FiscalYear>,
}

/// Fiscal years from annual 10-K periods. A year's label is the `fy` of its
/// original 10-K (the earliest one filed within 300 days of the year end);
/// years seen only as comparatives are labelled relative to the nearest
/// labelled year.
fn annual_years(obs: &[AnnualObs]) -> Option<Vec<FiscalYear>> {
    let mut by_end: HashMap<NaiveDate, Vec<&AnnualObs>> = HashMap::new();
    for o in obs {
        by_end.entry(o.end).or_default().push(o);
    }
    let mut ends: Vec<NaiveDate> = by_end.keys().copied().collect();
    ends.sort_unstable_by(|a, b| b.cmp(a));
    let mut kept: Vec<NaiveDate> = Vec::new();
    for e in ends {
        if kept.last().is_none_or(|k| days(e, *k) >= 300) {
            kept.push(e);
        }
    }
    if kept.is_empty() {
        return None;
    }
    let mut years: Vec<(FiscalYear, Option<i32>)> = kept
        .iter()
        .filter_map(|end| by_end.get(end).map(|list| (end, list)))
        .map(|(end, list)| {
            let mut starts: HashMap<NaiveDate, usize> = HashMap::new();
            for o in list {
                *starts.entry(o.start).or_default() += 1;
            }
            let start = starts
                .into_iter()
                .max_by_key(|(s, n)| (*n, *s))
                .map_or(*end, |(s, _)| s);
            let hint = list
                .iter()
                .filter(|o| (0..=ORIGINAL_FILING_DAYS).contains(&days(o.end, o.filed)))
                .min_by_key(|o| o.filed)
                .and_then(|o| o.fy);
            (
                FiscalYear {
                    label: end.year(),
                    start,
                    end: *end,
                    reported: true,
                    quarter_ends: [None; 4],
                },
                hint,
            )
        })
        .collect();
    years.sort_by_key(|(y, _)| y.end);
    let anchors: Vec<(NaiveDate, i32)> = years
        .iter()
        .filter_map(|(y, h)| h.map(|h| (y.end, h)))
        .collect();
    let from_anchor = |end: NaiveDate, a: (NaiveDate, i32)| {
        a.1 + (days(a.0, end) as f64 / DAYS_PER_YEAR).round() as i32
    };
    for (y, hint) in &mut years {
        y.label = match (*hint, anchors.iter().min_by_key(|a| days(a.0, y.end).abs())) {
            (Some(h), _) => h,
            (None, Some(a)) => from_anchor(y.end, *a),
            (None, None) => y.end.year(),
        };
    }
    // Inconsistent hints (a mislabelled filing) → relabel from the latest anchor.
    if years.windows(2).any(|w| w[1].0.label <= w[0].0.label)
        && let Some(a) = anchors.last()
    {
        for (y, _) in &mut years {
            y.label = from_anchor(y.end, *a);
        }
    }
    Some(years.into_iter().map(|(y, _)| y).collect())
}

impl FiscalCalendar {
    /// Fills gaps, extends the calendar to cover `[min, max]`, and assigns
    /// quarter-end dates from the facts.
    fn finish(
        mut years: Vec<FiscalYear>,
        min: NaiveDate,
        max: NaiveDate,
        end_counts: &HashMap<NaiveDate, usize>,
    ) -> Self {
        years.sort_by_key(|y| y.end);
        let mut filled: Vec<FiscalYear> = Vec::with_capacity(years.len());
        for y in years {
            while let Some(prev) = filled.last()
                && days(prev.end, y.start) > 40
                && filled.len() < 500
            {
                let start = prev.end + Duration::days(1);
                let end = (prev.end + Duration::days(365)).min(y.start - Duration::days(1));
                let label = prev.label + 1;
                filled.push(FiscalYear {
                    label,
                    start,
                    end,
                    reported: false,
                    quarter_ends: [None; 4],
                });
            }
            filled.push(y);
        }
        let mut years = filled;
        for _ in 0..100 {
            let Some(last) = years.last() else { break };
            if days(last.end, max) <= FY_END_TOLERANCE_DAYS {
                break;
            }
            let len = days(last.start, last.end).max(363);
            let start = last.end + Duration::days(1);
            let next = FiscalYear {
                label: last.label + 1,
                start,
                end: start + Duration::days(len),
                reported: false,
                quarter_ends: [None; 4],
            };
            years.push(next);
        }
        for _ in 0..100 {
            let Some(first) = years.first() else { break };
            if days(min, first.start) <= FY_END_TOLERANCE_DAYS {
                break;
            }
            let len = days(first.start, first.end).max(363);
            let end = first.start - Duration::days(1);
            let prev = FiscalYear {
                label: first.label - 1,
                start: end - Duration::days(len),
                end,
                reported: false,
                quarter_ends: [None; 4],
            };
            years.insert(0, prev);
        }
        let mut cal = FiscalCalendar { years };
        cal.assign_quarter_ends(end_counts);
        cal
    }

    /// Fiscal year index and quarter (1–4, 4 = fiscal year end) of `d`.
    pub(crate) fn locate(&self, d: NaiveDate) -> Option<(usize, u8)> {
        if let Some(i) = self
            .years
            .iter()
            .position(|y| days(y.end, d).abs() <= FY_END_TOLERANCE_DAYS)
        {
            return Some((i, 4));
        }
        let i = self.years.iter().position(|y| d >= y.start && d < y.end)?;
        let since_prior_end = days(self.years[i].start, d) + 1;
        let n = (since_prior_end as f64 / DAYS_PER_QUARTER).round() as i64;
        (1..=3).contains(&n).then_some((i, n as u8))
    }

    fn assign_quarter_ends(&mut self, end_counts: &HashMap<NaiveDate, usize>) {
        let mut best: HashMap<(usize, u8), (usize, NaiveDate)> = HashMap::new();
        for (&d, &n) in end_counts {
            let Some(slot) = self.locate(d) else { continue };
            let e = best.entry(slot).or_insert((0, d));
            if (n, d) > (e.0, e.1) {
                *e = (n, d);
            }
        }
        for (i, y) in self.years.iter_mut().enumerate() {
            for q in 1..=4u8 {
                y.quarter_ends[usize::from(q - 1)] = best.get(&(i, q)).map(|(_, d)| *d);
            }
            if y.reported {
                y.quarter_ends[3] = Some(y.end);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Statement assembly.

type Value = (f64, String);
type Values = HashMap<&'static str, Value>;

fn tagged(v: f64, tag: &str) -> Value {
    (v, format!("us-gaap:{tag}"))
}

/// Annual value of a base line for fiscal year `fy`.
fn annual_value(ix: &FactsIndex, l: &LineDef, fy: &FiscalYear) -> Option<Value> {
    match l.source {
        Source::Flow { tags, .. } => tags.iter().find_map(|t| {
            ix.duration(t, fy.end, &ANNUAL_DAYS, Some(fy.start))
                .map(|v| tagged(v, t))
        }),
        Source::Instant { tags } => tags
            .iter()
            .find_map(|t| ix.instant(t, fy.period_end()).map(|v| tagged(v, t))),
        Source::Ebitda | Source::Fcf => None,
    }
}

/// Year-to-date value through quarter `n` (4 = full year) for one concept.
fn ytd(ix: &FactsIndex, tag: &str, fy: &FiscalYear, n: u8) -> Option<f64> {
    if n == 4 {
        return ix.duration(tag, fy.end, &ANNUAL_DAYS, Some(fy.start));
    }
    let end = fy.quarter_ends[usize::from(n - 1)]?;
    if n == 1 {
        return ix.duration(tag, end, &QUARTER_DAYS, None);
    }
    if let Some(v) = ix.ytd_fact(tag, fy.start, end) {
        return Some(v);
    }
    let mut sum = 0.0;
    for k in 1..=n {
        sum += ix.duration(
            tag,
            fy.quarter_ends[usize::from(k - 1)]?,
            &QUARTER_DAYS,
            None,
        )?;
    }
    Some(sum)
}

/// Quarterly value of a base line for quarter `n` of `fy`.
fn quarter_value(ix: &FactsIndex, l: &LineDef, fy: &FiscalYear, n: u8) -> Option<Value> {
    let end = fy.quarter_ends[usize::from(n - 1)]?;
    match l.source {
        Source::Flow { tags, additive, .. } => {
            if let Some(v) = tags.iter().find_map(|t| {
                ix.duration(t, end, &QUARTER_DAYS, None)
                    .map(|v| tagged(v, t))
            }) {
                return Some(v);
            }
            if !additive || n == 1 {
                return None;
            }
            let tag = if n == 4 { TAG_Q4 } else { TAG_YTD };
            tags.iter()
                .find_map(|t| Some((ytd(ix, t, fy, n)? - ytd(ix, t, fy, n - 1)?, tag.to_owned())))
        }
        Source::Instant { tags } => tags
            .iter()
            .find_map(|t| ix.instant(t, end).map(|v| tagged(v, t))),
        Source::Ebitda | Source::Fcf => None,
    }
}

/// Adds the derived lines (EBITDA, FCF) computed from base lines.
fn add_derived(values: &mut Values) {
    let get = |v: &Values, c: &str| v.get(c).map(|x| x.0);
    if let (Some(oi), Some(da)) = (get(values, "operating_income"), get(values, "d_and_a")) {
        values.insert("ebitda", (oi + da, TAG_EBITDA.into()));
    }
    if let (Some(cfo), Some(capex)) = (get(values, "cfo"), get(values, "capex")) {
        values.insert("fcf", (cfo - capex, TAG_FCF.into()));
    }
}

/// One statement column: a fiscal period and its line values.
#[derive(Debug, Clone)]
struct Column {
    fiscal_year: i32,
    fiscal_period: String,
    end: NaiveDate,
    values: Values,
}

fn base_lines() -> impl Iterator<Item = &'static LineDef> {
    LINES
        .iter()
        .filter(|l| !matches!(l.source, Source::Ebitda | Source::Fcf))
}

fn annual_periods(ix: &FactsIndex, cal: &FiscalCalendar) -> Vec<Column> {
    cal.years
        .iter()
        .rev()
        .map(|fy| {
            let mut values: Values = base_lines()
                .filter_map(|l| annual_value(ix, l, fy).map(|v| (l.code, v)))
                .collect();
            add_derived(&mut values);
            Column {
                fiscal_year: fy.label,
                fiscal_period: "FY".into(),
                end: fy.period_end(),
                values,
            }
        })
        .filter(|p| !p.values.is_empty())
        .collect()
}

/// Every quarter with a known end date, oldest first, with its values.
fn quarterly_periods(ix: &FactsIndex, cal: &FiscalCalendar) -> Vec<(usize, u8, Column)> {
    let mut out = Vec::new();
    for (i, fy) in cal.years.iter().enumerate() {
        for n in 1..=4u8 {
            let Some(end) = fy.quarter_ends[usize::from(n - 1)] else {
                continue;
            };
            let mut values: Values = base_lines()
                .filter_map(|l| quarter_value(ix, l, fy, n).map(|v| (l.code, v)))
                .collect();
            add_derived(&mut values);
            out.push((
                i,
                n,
                Column {
                    fiscal_year: fy.label,
                    fiscal_period: format!("Q{n}"),
                    end,
                    values,
                },
            ));
        }
    }
    out
}

/// Trailing-twelve-month windows: additive flows summed over four
/// consecutive quarters (all four required), balance values from the last
/// quarter, EPS and share counts omitted.
fn ttm_periods(quarters: &[(usize, u8, Column)]) -> Vec<Column> {
    let mut out = Vec::new();
    for w in quarters.windows(4) {
        let consecutive = w.windows(2).all(|p| {
            let (a, b) = (&p[0], &p[1]);
            (a.0 == b.0 && b.1 == a.1 + 1) || (b.0 == a.0 + 1 && a.1 == 4 && b.1 == 1)
        });
        if !consecutive {
            continue;
        }
        let last = &w[3].2;
        let mut values = Values::new();
        for l in base_lines() {
            match l.source {
                Source::Flow { additive: true, .. } => {
                    let parts: Option<Vec<f64>> = w
                        .iter()
                        .map(|q| q.2.values.get(l.code).map(|v| v.0))
                        .collect();
                    if let Some(parts) = parts {
                        values.insert(l.code, (parts.iter().sum(), TAG_TTM.into()));
                    }
                }
                Source::Instant { .. } => {
                    if let Some(v) = last.values.get(l.code) {
                        values.insert(l.code, v.clone());
                    }
                }
                Source::Flow {
                    additive: false, ..
                }
                | Source::Ebitda
                | Source::Fcf => {}
            }
        }
        add_derived(&mut values);
        if !values.is_empty() {
            out.push(Column {
                fiscal_year: last.fiscal_year,
                fiscal_period: "TTM".into(),
                end: last.end,
                values,
            });
        }
    }
    out.reverse();
    out
}

/// Builds statements for the most recent `periods` periods with data, newest
/// first, grouped Income → Balance → Cash Flow. A statement lists the lines
/// that have a value in at least one returned period of its kind (`None`
/// where a period lacks it), so columns align.
pub(crate) fn build_statements(
    ix: &FactsIndex,
    cal: &FiscalCalendar,
    period_type: PeriodType,
    periods: usize,
) -> Vec<Statement> {
    let mut selected: Vec<Column> = match period_type {
        PeriodType::Annual => annual_periods(ix, cal),
        PeriodType::Quarterly => {
            let mut q: Vec<Column> = quarterly_periods(ix, cal)
                .into_iter()
                .map(|(_, _, p)| p)
                .filter(|p| !p.values.is_empty())
                .collect();
            q.reverse();
            q
        }
        PeriodType::Ttm => ttm_periods(&quarterly_periods(ix, cal)),
    };
    selected.truncate(periods.max(1));

    let mut out = Vec::new();
    for kind in [
        StatementKind::Income,
        StatementKind::Balance,
        StatementKind::CashFlow,
    ] {
        let defs: Vec<&LineDef> = LINES
            .iter()
            .filter(|l| l.kind == kind && selected.iter().any(|p| p.values.contains_key(l.code)))
            .collect();
        if defs.is_empty() {
            continue;
        }
        for p in &selected {
            if !defs.iter().any(|l| p.values.contains_key(l.code)) {
                continue;
            }
            let lines = defs
                .iter()
                .map(|l| {
                    let v = p.values.get(l.code);
                    StatementLine {
                        code: l.code.into(),
                        label: l.label.into(),
                        value: v.map(|v| v.0),
                        depth: l.depth,
                        source_tag: v.map(|v| v.1.clone()),
                    }
                })
                .collect();
            out.push(Statement {
                kind,
                period_type,
                fiscal_year: p.fiscal_year,
                fiscal_period: p.fiscal_period.clone(),
                period_end: p.end,
                currency: ix.currency.clone(),
                lines,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn fixture() -> (FactsIndex, FiscalCalendar) {
        let ix = FactsIndex::parse(include_str!(
            "../tests/fixtures/companyfacts_CIK0000000001.json"
        ))
        .unwrap();
        let cal = ix.calendar_from_annual().unwrap();
        (ix, cal)
    }

    fn find<'a>(s: &'a [Statement], kind: StatementKind, period: &str, fy: i32) -> &'a Statement {
        s.iter()
            .find(|s| s.kind == kind && s.fiscal_period == period && s.fiscal_year == fy)
            .unwrap()
    }

    fn line<'a>(s: &'a Statement, code: &str) -> &'a StatementLine {
        s.lines.iter().find(|l| l.code == code).unwrap()
    }

    #[test]
    fn mapping_covers_required_codes() {
        let codes: Vec<&str> = LINES.iter().map(|l| l.code).collect();
        for c in [
            "revenue",
            "cost_of_revenue",
            "gross_profit",
            "sga",
            "rnd",
            "operating_income",
            "interest_expense",
            "pretax_income",
            "income_tax",
            "net_income",
            "eps_basic",
            "eps_diluted",
            "shares_diluted",
            "ebitda",
            "cash",
            "short_term_investments",
            "receivables",
            "inventory",
            "total_current_assets",
            "ppe_net",
            "goodwill",
            "total_assets",
            "accounts_payable",
            "short_term_debt",
            "total_current_liabilities",
            "long_term_debt",
            "total_liabilities",
            "total_equity",
            "cfo",
            "capex",
            "fcf",
            "dividends_paid",
            "buybacks",
            "cfi",
            "cff",
            "net_change_cash",
        ] {
            assert!(codes.contains(&c), "missing {c}");
        }
        let concepts: usize = LINES.iter().map(|l| line_tags(l).len()).sum();
        assert!(concepts >= 25);
    }

    #[test]
    fn calendar_labels_years_from_original_filings() {
        let (_, cal) = fixture();
        let reported: Vec<(i32, NaiveDate, NaiveDate)> = cal
            .years
            .iter()
            .filter(|y| y.reported)
            .map(|y| (y.label, y.start, y.end))
            .collect();
        assert_eq!(
            reported,
            vec![
                (2023, d(2022, 10, 1), d(2023, 9, 30)),
                (2024, d(2023, 10, 1), d(2024, 9, 30)),
                (2025, d(2024, 10, 1), d(2025, 9, 30)),
            ]
        );
        // FY2023 appears only as a comparative (fy 2024/2025 filings) and is
        // labelled from its neighbours, not from the filing's fy.
        assert_eq!(
            cal.locate(d(2024, 12, 31)),
            Some((cal.years.iter().position(|y| y.label == 2025).unwrap(), 1))
        );
        // The in-progress year is extrapolated.
        let fy26 = cal.years.iter().find(|y| y.label == 2026).unwrap();
        assert!(!fy26.reported);
        assert_eq!(fy26.quarter_ends[0], Some(d(2025, 12, 31)));
    }

    #[test]
    fn annual_statements_use_period_dates_not_filing_fy() {
        let (ix, cal) = fixture();
        let s = build_statements(&ix, &cal, PeriodType::Annual, 3);
        let inc: Vec<(i32, NaiveDate)> = s
            .iter()
            .filter(|s| s.kind == StatementKind::Income)
            .map(|s| (s.fiscal_year, s.period_end))
            .collect();
        assert_eq!(
            inc,
            vec![
                (2025, d(2025, 9, 30)),
                (2024, d(2024, 9, 30)),
                (2023, d(2023, 9, 30))
            ]
        );
        let fy25 = find(&s, StatementKind::Income, "FY", 2025);
        assert_eq!(fy25.currency, "USD");
        assert_eq!(fy25.value("revenue"), Some(4600.0));
        assert_eq!(
            line(fy25, "revenue").source_tag.as_deref(),
            Some("us-gaap:Revenues")
        );
        assert_eq!(fy25.value("eps_diluted"), Some(4.6));
        // FY2024 comes from the FY2025 10-K comparative (fy=2025) and the FY2024 10-K.
        assert_eq!(
            find(&s, StatementKind::Income, "FY", 2024).value("revenue"),
            Some(4000.0)
        );
        assert_eq!(
            find(&s, StatementKind::Income, "FY", 2023).value("revenue"),
            Some(3600.0)
        );
    }

    #[test]
    fn latest_filed_value_wins_for_restatements() {
        let (ix, cal) = fixture();
        let s = build_statements(&ix, &cal, PeriodType::Annual, 3);
        // FY2024 net income: 400 in the FY2024 10-K, restated to 380 in the FY2025 10-K.
        assert_eq!(
            find(&s, StatementKind::Income, "FY", 2024).value("net_income"),
            Some(380.0)
        );
    }

    #[test]
    fn derived_ebitda_and_fcf() {
        let (ix, cal) = fixture();
        let s = build_statements(&ix, &cal, PeriodType::Annual, 1);
        let inc = find(&s, StatementKind::Income, "FY", 2025);
        assert_eq!(inc.value("ebitda"), Some(700.0));
        assert_eq!(line(inc, "ebitda").source_tag.as_deref(), Some(TAG_EBITDA));
        let cf = find(&s, StatementKind::CashFlow, "FY", 2025);
        assert_eq!(cf.value("capex"), Some(200.0));
        assert_eq!(cf.value("fcf"), Some(500.0));
        assert_eq!(line(cf, "fcf").source_tag.as_deref(), Some(TAG_FCF));
        let bal = find(&s, StatementKind::Balance, "FY", 2025);
        assert_eq!(bal.value("total_assets"), Some(10400.0));
    }

    #[test]
    fn quarterly_selection_and_q4_derivation() {
        let (ix, cal) = fixture();
        let s = build_statements(&ix, &cal, PeriodType::Quarterly, 5);
        let inc: Vec<(i32, &str)> = s
            .iter()
            .filter(|s| s.kind == StatementKind::Income)
            .map(|s| (s.fiscal_year, s.fiscal_period.as_str()))
            .collect();
        assert_eq!(
            inc,
            vec![
                (2026, "Q1"),
                (2025, "Q4"),
                (2025, "Q3"),
                (2025, "Q2"),
                (2025, "Q1")
            ]
        );

        let q4 = find(&s, StatementKind::Income, "Q4", 2025);
        assert_eq!(q4.period_end, d(2025, 9, 30));
        // 4600 − (1000 + 1100 + 1200).
        assert_eq!(q4.value("revenue"), Some(1300.0));
        assert_eq!(line(q4, "revenue").source_tag.as_deref(), Some(TAG_Q4));
        assert_eq!(q4.value("net_income"), Some(130.0));
        // EPS and weighted shares are never derived.
        assert_eq!(q4.value("eps_diluted"), None);
        assert_eq!(line(q4, "eps_diluted").source_tag, None);

        let q2 = find(&s, StatementKind::Income, "Q2", 2025);
        assert_eq!(q2.value("revenue"), Some(1100.0));
        assert_eq!(
            line(q2, "revenue").source_tag.as_deref(),
            Some("us-gaap:Revenues")
        );
        assert_eq!(q2.value("eps_diluted"), Some(1.1));

        let q1_26 = find(&s, StatementKind::Income, "Q1", 2026);
        assert_eq!(q1_26.period_end, d(2025, 12, 31));
        assert_eq!(q1_26.value("revenue"), Some(1250.0));
    }

    #[test]
    fn cash_flow_quarters_from_ytd_differences() {
        let (ix, cal) = fixture();
        let s = build_statements(&ix, &cal, PeriodType::Quarterly, 5);
        let cfo = |p: &str, fy: i32| {
            let st = find(&s, StatementKind::CashFlow, p, fy);
            (st.value("cfo"), line(st, "cfo").source_tag.clone())
        };
        assert_eq!(
            cfo("Q1", 2025),
            (
                Some(150.0),
                Some("us-gaap:NetCashProvidedByUsedInOperatingActivities".into())
            )
        );
        assert_eq!(cfo("Q2", 2025), (Some(170.0), Some(TAG_YTD.into())));
        assert_eq!(cfo("Q3", 2025), (Some(180.0), Some(TAG_YTD.into())));
        // FY 700 − 9-month YTD 500.
        assert_eq!(cfo("Q4", 2025), (Some(200.0), Some(TAG_Q4.into())));
    }

    #[test]
    fn balance_values_are_never_derived() {
        let (ix, cal) = fixture();
        let s = build_statements(&ix, &cal, PeriodType::Quarterly, 5);
        let q3 = find(&s, StatementKind::Balance, "Q3", 2025);
        assert_eq!(q3.value("total_assets"), Some(10300.0));
        // Cash is only reported at fiscal year ends in the fixture: no quarter value.
        assert_eq!(q3.value("cash"), None);
        assert_eq!(
            find(&s, StatementKind::Balance, "Q4", 2025).value("cash"),
            Some(500.0)
        );
    }

    #[test]
    fn ttm_sums_four_quarters_and_skips_eps() {
        let (ix, cal) = fixture();
        let s = build_statements(&ix, &cal, PeriodType::Ttm, 2);
        let inc: Vec<&Statement> = s
            .iter()
            .filter(|s| s.kind == StatementKind::Income)
            .collect();
        assert_eq!(inc.len(), 2);
        // Window Q2 FY25 … Q1 FY26: 1100 + 1200 + 1300 + 1250.
        assert_eq!(inc[0].fiscal_period, "TTM");
        assert_eq!(inc[0].fiscal_year, 2026);
        assert_eq!(inc[0].period_end, d(2025, 12, 31));
        assert_eq!(inc[0].value("revenue"), Some(4850.0));
        assert_eq!(line(inc[0], "revenue").source_tag.as_deref(), Some(TAG_TTM));
        assert_eq!(inc[0].value("eps_diluted"), None);
        // Window Q1 … Q4 FY25 equals the fiscal year.
        assert_eq!(inc[1].value("revenue"), Some(4600.0));
        let bal = s.iter().find(|s| s.kind == StatementKind::Balance).unwrap();
        assert_eq!(bal.value("total_assets"), Some(10500.0));
        assert_eq!(
            line(bal, "total_assets").source_tag.as_deref(),
            Some("us-gaap:Assets")
        );
    }

    #[test]
    fn statements_list_only_lines_with_data_and_align_columns() {
        let (ix, cal) = fixture();
        let s = build_statements(&ix, &cal, PeriodType::Annual, 3);
        let inc: Vec<&Statement> = s
            .iter()
            .filter(|s| s.kind == StatementKind::Income)
            .collect();
        let codes = |st: &Statement| st.lines.iter().map(|l| l.code.clone()).collect::<Vec<_>>();
        assert!(inc.windows(2).all(|w| codes(w[0]) == codes(w[1])));
        // No fixture data for gross profit → no line at all.
        assert!(!codes(inc[0]).contains(&"gross_profit".to_string()));
        // FY2023 has no EPS in the fixture → the aligned line is None.
        assert_eq!(
            find(&s, StatementKind::Income, "FY", 2023).value("eps_diluted"),
            None
        );
    }

    #[test]
    fn ignores_non_periodic_forms_and_other_units() {
        let (ix, _) = fixture();
        // The fixture's 8-K revenue fact and EUR fact must not be indexed.
        let rev = ix.concept("Revenues").unwrap();
        assert!(
            rev.durations
                .values()
                .flatten()
                .all(|(_, f)| f.val != 9999.0)
        );
        assert_eq!(ix.currency, "USD");
    }

    #[test]
    fn fiscal_year_end_fallback_calendar() {
        let body = r#"{"cik":1,"entityName":"Example Corp","facts":{"us-gaap":{"Revenues":{"label":"Revenues","units":{"USD":[
            {"start":"2025-10-01","end":"2025-12-31","val":10,"accn":"0000000001-26-000001","fy":2026,"fp":"Q1","form":"10-Q","filed":"2026-02-01"},
            {"start":"2026-01-01","end":"2026-03-31","val":11,"accn":"0000000001-26-000002","fy":2026,"fp":"Q2","form":"10-Q","filed":"2026-05-01"}
        ]}}}}}"#;
        let ix = FactsIndex::parse(body).unwrap();
        assert!(ix.calendar_from_annual().is_none());
        let cal = ix.calendar_from_fye(9, 30).unwrap();
        let s = build_statements(&ix, &cal, PeriodType::Quarterly, 4);
        let got: Vec<(i32, &str, Option<f64>)> = s
            .iter()
            .map(|s| (s.fiscal_year, s.fiscal_period.as_str(), s.value("revenue")))
            .collect();
        assert_eq!(
            got,
            vec![(2026, "Q2", Some(11.0)), (2026, "Q1", Some(10.0))]
        );
    }

    #[test]
    fn rejects_malformed_payload() {
        assert!(matches!(
            FactsIndex::parse("[]"),
            Err(ProviderError::Parse { .. })
        ));
        let bad_date = r#"{"facts":{"us-gaap":{"Revenues":{"units":{"USD":[{"end":"31/12/2025","val":1,"accn":"a","form":"10-K","filed":"2026-01-01"}]}}}}}"#;
        assert!(matches!(
            FactsIndex::parse(bad_date),
            Err(ProviderError::Parse { .. })
        ));
    }
}

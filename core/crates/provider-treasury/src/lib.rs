//! US Treasury Daily Par Yield Curve Rates.
//!
//! Source: the official XML feed
//! <https://home.treasury.gov/treasury-daily-interest-rate-xml-feed>. The
//! curve is served as `yield_curve` (name `UST`), and each tenor's daily
//! history as an economic series with ID `UST:<tenor>` (e.g. `UST:10 Yr`).
//!
//! US federal government works are not subject to copyright (17 U.S.C.
//! § 105; <https://www.usa.gov/government-copyright>), so the data may be
//! cached and passed to the ASK model. Treasury asks for appropriate
//! credit, which `ATTRIBUTION` provides.

mod feed;
mod http;

use async_trait::async_trait;
use chrono::{Datelike, Days, Months, NaiveDate, Utc};
use meridian_provider::{
    AiPolicy, CachePolicy, Capabilities, Capability, CapabilityEntry, CurveRequest, Provider, ProviderError,
    ProviderResult, RateLimit, SeriesRequest,
};
use meridian_types::{
    AssetClass, CurvePoint, DataDelay, EconomicSeries, FeedSource, Observation, ProviderId, Provenance, UnixNanos,
    YieldCurve, datetime_to_nanos,
};

use crate::feed::{CurveRow, Feed, Tenor};

/// Provider id used in provenance and routing.
pub const PROVIDER_ID: &str = "treasury";
/// The only curve this provider serves.
pub const CURVE_NAME: &str = "UST";
/// Prefix of the per-tenor series IDs (`UST:10 Yr`).
pub const SERIES_PREFIX: &str = "UST:";
/// Credit line. Treasury requests appropriate credit for its works.
pub const ATTRIBUTION: &str = "Source: U.S. Department of the Treasury, Daily Treasury Par Yield Curve Rates.";

const FEED_URL: &str = "https://home.treasury.gov/resource-center/data-chart-center/interest-rates/pages/xml";
const DOCS_URL: &str = "https://home.treasury.gov/treasury-daily-interest-rate-xml-feed";
/// The par yield curve series starts in 1990.
const FIRST_YEAR: i32 = 1990;
/// Default history window for a series request without `from`.
const DEFAULT_SERIES_DAYS: u64 = 365;

const SERIES_NOTES: &str = "Daily Treasury Par Yield Curve Rates. Treasury builds the curve with a monotone \
convex method from indicative, bid-side market quotations (not actual transactions) for the most recently \
auctioned securities, obtained by the Federal Reserve Bank of New York at or near 3:30 PM each trading day. \
Published by about 6:00 PM ET. Methodology: \
https://home.treasury.gov/policy-issues/financing-the-government/interest-rate-statistics/treasury-yield-curve-methodology";

/// Daily Treasury Par Yield Curve. No credentials.
pub struct TreasuryProvider {
    client: reqwest::Client,
    caps: Capabilities,
}

impl TreasuryProvider {
    pub fn new() -> ProviderResult<Self> {
        Ok(Self { client: http::client()?, caps: capabilities() })
    }

    async fn fetch(&self, url: &str) -> ProviderResult<Feed> {
        let body = http::send_text(self.client.get(url)).await?;
        feed::parse_feed(&body).map_err(|e| ProviderError::parse(format!("Treasury par yield feed: {e}")))
    }
}

#[async_trait]
impl Provider for TreasuryProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(PROVIDER_ID)
    }

    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    /// The curve for `req.date` (latest when `None`). If no curve was
    /// published that day (weekend, holiday, not yet published), returns the
    /// latest earlier curve; `YieldCurve.date` says which day it is.
    async fn yield_curve(&self, req: &CurveRequest) -> ProviderResult<YieldCurve> {
        if !req.name.trim().eq_ignore_ascii_case(CURVE_NAME) {
            return Err(ProviderError::NotFound(format!("curve {:?}: this provider serves {CURVE_NAME} only", req.name)));
        }
        let target = req.date.unwrap_or_else(|| Utc::now().date_naive());
        if target.year() < FIRST_YEAR {
            return Err(ProviderError::NotFound(format!("the par yield curve starts in {FIRST_YEAR}")));
        }
        // The target's month, then the month before (covers the first days
        // of a month and month-end holidays).
        let months = [Some(target), target.checked_sub_months(Months::new(1))];
        for month in months.into_iter().flatten() {
            let url = month_url(month);
            let feed = self.fetch(&url).await?;
            if let Some(row) = feed.rows.iter().rev().find(|r| r.date <= target) {
                return Ok(build_curve(row, provenance(as_of(&feed), url)));
            }
        }
        Err(ProviderError::NotFound(format!("no par yield curve published on or before {target}")))
    }

    /// Daily history of one tenor. ID `UST:<tenor>`, e.g. `UST:10 Yr`,
    /// `UST:1.5 Mo`, `UST:3M`. `from`/`to` are inclusive; the default window
    /// is the last 365 days. Days on which the tenor wasn't published (it
    /// didn't exist yet, or was suspended) have no observation.
    async fn economic_series(&self, req: &SeriesRequest) -> ProviderResult<EconomicSeries> {
        let tenor = parse_series_id(&req.id)
            .ok_or_else(|| ProviderError::NotFound(format!("series {:?}: expected {SERIES_PREFIX}<tenor>", req.id)))?;
        let today = Utc::now().date_naive();
        let to = req.to.unwrap_or(today).min(today);
        let first = NaiveDate::from_ymd_opt(FIRST_YEAR, 1, 1).unwrap_or_default();
        let from = req
            .from
            .unwrap_or_else(|| to.checked_sub_days(Days::new(DEFAULT_SERIES_DAYS)).unwrap_or(first))
            .max(first);

        let mut observations = Vec::new();
        let mut last_updated: Option<UnixNanos> = None;
        let mut urls = Vec::new();
        if from <= to {
            for year in from.year()..=to.year() {
                let url = year_url(year);
                let feed = self.fetch(&url).await?;
                if let Some(u) = feed.updated {
                    let u = datetime_to_nanos(u);
                    last_updated = Some(last_updated.map_or(u, |l: UnixNanos| l.max(u)));
                }
                observations.extend(observations_in(&feed.rows, tenor, from, to));
                urls.push(url);
            }
        }
        if observations.is_empty() && from <= to {
            return Err(ProviderError::NotFound(format!(
                "Treasury published no {} par yield between {from} and {to}",
                tenor.label()
            )));
        }
        observations.sort_by_key(|o| o.date);
        observations.dedup_by_key(|o| o.date);

        let source_ref = match urls.as_slice() {
            [one] => one.clone(),
            [first_url, .., last_url] => format!("{first_url} … {last_url}"),
            [] => DOCS_URL.to_owned(),
        };
        let as_of = last_updated.unwrap_or_else(|| datetime_to_nanos(Utc::now()));
        Ok(EconomicSeries {
            id: format!("{SERIES_PREFIX}{}", tenor.label()),
            title: format!("Treasury Par Yield Curve Rate, {}", tenor.label()),
            units: "Percent".into(),
            frequency: "Daily".into(),
            seasonal_adjustment: None,
            last_updated,
            notes: Some(SERIES_NOTES.into()),
            observations,
            provenance: provenance(as_of, source_ref),
        })
    }
}

fn capabilities() -> Capabilities {
    let entry = |capability, history: &str| CapabilityEntry {
        capability,
        asset_classes: vec![AssetClass::Rate],
        delay: DataDelay::EndOfDay,
        source: FeedSource::Official,
        history: Some(history.to_owned()),
    };
    Capabilities {
        entries: vec![
            entry(Capability::YieldCurve, "daily since 1990; the tenor set varies over time (1.5 Mo since 2025-02-18)"),
            entry(Capability::EconomicSeries, "daily per tenor since 1990 (UST:<tenor>)"),
        ],
        // Treasury documents no limit. A curve call is 1-2 requests; a
        // series call is one request per calendar year.
        rate_limit: Some(RateLimit::per_minute(60)),
        max_stream_symbols: None,
        cache_policy: CachePolicy::Unrestricted,
        attribution: Some(ATTRIBUTION.to_owned()),
        display_allowed: true,
        ai_policy: AiPolicy::Allowed,
        requires_credentials: false,
        terms_note: "Official US Treasury data, a work of the US federal government and therefore not subject \
to copyright (17 U.S.C. § 105; https://www.usa.gov/government-copyright). Treasury requests appropriate credit. \
Rates are published by about 6:00 PM ET each trading day."
            .into(),
        docs_url: DOCS_URL.into(),
    }
}

fn provenance(as_of: UnixNanos, source_ref: String) -> Provenance {
    Provenance {
        provider: ProviderId::new(PROVIDER_ID),
        synthetic: false,
        delay: DataDelay::EndOfDay,
        source: FeedSource::Official,
        as_of,
        source_ref: Some(source_ref),
        attribution: Some(ATTRIBUTION.to_owned()),
    }
}

/// The feed's own `updated` stamp, else now.
fn as_of(feed: &Feed) -> UnixNanos {
    feed.updated.map_or_else(|| datetime_to_nanos(Utc::now()), datetime_to_nanos)
}

fn month_url(d: NaiveDate) -> String {
    format!("{FEED_URL}?data=daily_treasury_yield_curve&field_tdr_date_value_month={}{:02}", d.year(), d.month())
}

fn year_url(year: i32) -> String {
    format!("{FEED_URL}?data=daily_treasury_yield_curve&field_tdr_date_value={year}")
}

/// `UST:10 Yr` → 10 Yr. The prefix is case-insensitive.
fn parse_series_id(id: &str) -> Option<Tenor> {
    let id = id.trim();
    let prefix = id.get(..SERIES_PREFIX.len())?;
    if !prefix.eq_ignore_ascii_case(SERIES_PREFIX) {
        return None;
    }
    Tenor::parse_label(&id[SERIES_PREFIX.len()..])
}

/// Points sorted by maturity. Only tenors published that day appear.
fn build_curve(row: &CurveRow, provenance: Provenance) -> YieldCurve {
    let mut points: Vec<CurvePoint> = row
        .points
        .iter()
        .map(|(t, v)| CurvePoint { tenor: t.label(), years: t.years(), yield_pct: Some(*v) })
        .collect();
    points.sort_by(|a, b| a.years.total_cmp(&b.years));
    YieldCurve { name: CURVE_NAME.into(), date: row.date, points, provenance }
}

fn observations_in(rows: &[CurveRow], tenor: Tenor, from: NaiveDate, to: NaiveDate) -> Vec<Observation> {
    rows.iter()
        .filter(|r| r.date >= from && r.date <= to)
        .filter_map(|r| r.get(tenor).map(|v| Observation { date: r.date, value: Some(v) }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OCT_2026: &str = include_str!("../tests/fixtures/par_yield_202610.xml");
    const JAN_2004: &str = include_str!("../tests/fixtures/par_yield_200401_trimmed.xml");
    const EARLY_2025: &str = include_str!("../tests/fixtures/par_yield_2025_trimmed.xml");

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn urls() {
        assert_eq!(
            month_url(d(2026, 3, 9)),
            "https://home.treasury.gov/resource-center/data-chart-center/interest-rates/pages/xml?data=daily_treasury_yield_curve&field_tdr_date_value_month=202603"
        );
        assert_eq!(
            year_url(2025),
            "https://home.treasury.gov/resource-center/data-chart-center/interest-rates/pages/xml?data=daily_treasury_yield_curve&field_tdr_date_value=2025"
        );
    }

    #[test]
    fn series_ids() {
        assert_eq!(parse_series_id("UST:10 Yr").map(Tenor::label), Some("10 Yr".into()));
        assert_eq!(parse_series_id("ust:1.5 Mo").map(Tenor::label), Some("1.5 Month".into()));
        assert_eq!(parse_series_id(" UST:3M ").map(Tenor::label), Some("3 Mo".into()));
        assert_eq!(parse_series_id("DGS10"), None);
        assert_eq!(parse_series_id("UST:"), None);
        assert_eq!(parse_series_id("UST"), None);
    }

    #[test]
    fn curve_is_sorted_and_complete() {
        let feed = feed::parse_feed(OCT_2026).unwrap();
        let curve = build_curve(&feed.rows[1], provenance(as_of(&feed), "u".into()));
        assert_eq!(curve.name, "UST");
        assert_eq!(curve.date, d(2026, 10, 2));
        let labels: Vec<&str> = curve.points.iter().map(|p| p.tenor.as_str()).collect();
        assert_eq!(
            labels,
            ["1 Mo", "1.5 Month", "2 Mo", "3 Mo", "4 Mo", "6 Mo", "1 Yr", "2 Yr", "3 Yr", "5 Yr", "7 Yr", "10 Yr", "20 Yr", "30 Yr"]
        );
        assert!(curve.points.windows(2).all(|w| w[0].years < w[1].years));
        assert_eq!(curve.points[11].yield_pct, Some(5.28));
        let p = &curve.provenance;
        assert_eq!(p.provider.as_str(), "treasury");
        assert!(!p.synthetic);
        assert_eq!(p.delay, DataDelay::EndOfDay);
        assert_eq!(p.source, FeedSource::Official);
        assert_eq!(p.attribution.as_deref(), Some(ATTRIBUTION));
    }

    #[test]
    fn old_curve_omits_unpublished_tenors() {
        let feed = feed::parse_feed(JAN_2004).unwrap();
        let curve = build_curve(&feed.rows[0], provenance(0, "u".into()));
        assert_eq!(curve.points.len(), 10);
        assert!(curve.points.iter().all(|p| p.tenor != "30 Yr" && p.yield_pct.is_some()));
        assert_eq!(curve.points.last().unwrap().tenor, "20 Yr");
    }

    #[test]
    fn series_observations_follow_tenor_availability() {
        let feed = feed::parse_feed(EARLY_2025).unwrap();
        let ten = Tenor::parse_label("10 Yr").unwrap();
        let all = observations_in(&feed.rows, ten, d(2025, 1, 1), d(2025, 12, 31));
        assert_eq!(all.len(), 32);
        assert_eq!(all[0].date, d(2025, 1, 2));

        let window = observations_in(&feed.rows, ten, d(2025, 2, 14), d(2025, 2, 18));
        // 2025-02-17 was Presidents' Day: no curve.
        assert_eq!(window.iter().map(|o| o.date).collect::<Vec<_>>(), vec![d(2025, 2, 14), d(2025, 2, 18)]);
        assert_eq!(window[0].value, Some(4.47));
        assert_eq!(window[1].value, Some(4.55));

        // The 1.5-month bill only appears from 2025-02-18.
        let short = observations_in(&feed.rows, Tenor::parse_label("1.5 Mo").unwrap(), d(2025, 1, 1), d(2025, 2, 28));
        assert_eq!(short, vec![Observation { date: d(2025, 2, 18), value: Some(4.41) }]);
    }

    #[test]
    fn capabilities_are_honest() {
        let caps = capabilities();
        assert!(caps.supports(Capability::YieldCurve, None));
        assert!(caps.supports(Capability::EconomicSeries, None));
        assert!(!caps.supports(Capability::Quotes, None));
        assert_eq!(caps.ai_policy, AiPolicy::Allowed);
        assert_eq!(caps.cache_policy, CachePolicy::Unrestricted);
        assert!(caps.terms_note.contains("usa.gov/government-copyright"));
    }

    #[tokio::test]
    async fn rejects_unknown_curves_and_series_without_http() {
        let p = TreasuryProvider::new().unwrap();
        let curve = p.yield_curve(&CurveRequest { name: "BUND".into(), date: None }).await;
        assert!(matches!(curve, Err(ProviderError::NotFound(_))));
        let old = p.yield_curve(&CurveRequest { name: "UST".into(), date: Some(d(1985, 1, 2)) }).await;
        assert!(matches!(old, Err(ProviderError::NotFound(_))));
        let series = p.economic_series(&SeriesRequest { id: "CPIAUCSL".into(), from: None, to: None }).await;
        assert!(matches!(series, Err(ProviderError::NotFound(_))));
    }
}

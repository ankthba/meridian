//! Fixture tests. Fixtures are FRED's documented example responses (see
//! `tests/fixtures/SOURCES.md`); nothing here calls the real API.

use std::sync::Arc;
use std::time::Duration;

use chrono::{NaiveDate, TimeZone, Utc};
use meridian_provider::{AiPolicy, CachePolicy, CalendarRequest, Capability, Provider, ProviderError, SeriesRequest};
use meridian_types::{AssetClass, DataDelay, FeedSource, FixedClock, Importance, date_to_nanos, datetime_to_nanos};
use secrecy::SecretString;
use serde_json::Value;

use crate::normalize::{self, ObservationDto, ReleaseDateDto};
use crate::test_server::{TestServer, query_param};
use crate::{ATTRIBUTION, FredConfig, FredProvider, PROVIDER_ID};

const SERIES: &str = include_str!("../tests/fixtures/fred_series_gnpca.json");
const OBSERVATIONS: &str = include_str!("../tests/fixtures/fred_series_observations_gnpca.json");
const MISSING_VALUES: &str = include_str!("../tests/fixtures/fred_observations_missing_values.json");
const RELEASE_DATES: &str = include_str!("../tests/fixtures/fred_releases_dates.json");
const ERROR_API_KEY: &str = include_str!("../tests/fixtures/fred_error_api_key.json");
const ERROR_NO_SERIES: &str = include_str!("../tests/fixtures/fred_error_series_does_not_exist.json");

/// Obviously fake 32-char key in FRED's documented shape.
const TEST_KEY: &str = "testkeytestkeytestkeytestkey0000";
const NOW: i64 = 1_790_000_000_000_000_000;

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn provider(base: &str, key: Option<&str>) -> FredProvider {
    let config = FredConfig { api_key: key.map(SecretString::from) };
    FredProvider::build(config, format!("{base}/fred"), Arc::new(FixedClock(NOW))).unwrap()
}

fn path_of(target: &str) -> &str {
    target.split('?').next().unwrap_or_default()
}

/// Splits a documented page into pages of `size`, keeping `count` and
/// setting each page's `offset`, the way FRED pages large results.
fn paged(fixture: &str, array: &str, size: usize, offset: usize) -> String {
    let mut v: Value = serde_json::from_str(fixture).unwrap();
    let all = v[array].as_array().unwrap().clone();
    v["count"] = Value::from(all.len());
    v["offset"] = Value::from(offset);
    v[array] = Value::from(all.into_iter().skip(offset).take(size).collect::<Vec<_>>());
    v.to_string()
}

// ---------------------------------------------------------------------------
// Pure parsing
// ---------------------------------------------------------------------------

#[test]
fn parses_documented_series_metadata() {
    let meta = normalize::parse_series_meta(SERIES, "GNPCA").unwrap();
    assert_eq!(meta.id, "GNPCA");
    assert_eq!(meta.title, "Real Gross National Product");
    assert_eq!(meta.units, "Billions of Chained 2009 Dollars");
    assert_eq!(meta.frequency, "Annual");
    assert_eq!(meta.seasonal_adjustment.as_deref(), Some("Not Seasonally Adjusted"));
    assert_eq!(meta.notes.as_deref(), Some("BEA Account Code: A001RX1"));
}

#[test]
fn last_updated_parses_with_hour_offset() {
    // "2013-07-31 09:26:16-05" is 14:26:16 UTC.
    let expected = datetime_to_nanos(Utc.with_ymd_and_hms(2013, 7, 31, 14, 26, 16).unwrap());
    assert_eq!(normalize::parse_last_updated("2013-07-31 09:26:16-05"), Some(expected));
    assert_eq!(normalize::parse_last_updated("2013-07-31 09:26:16-05:30"), Some(expected + 30 * 60 * 1_000_000_000));
    assert_eq!(normalize::parse_last_updated("31/07/2013"), None);
}

#[test]
fn empty_series_list_is_not_found() {
    let err = normalize::parse_series_meta(r#"{"realtime_start":"2026-10-05","seriess":[]}"#, "NOPE").unwrap_err();
    assert!(matches!(err, ProviderError::NotFound(_)), "{err:?}");
}

#[test]
fn parses_documented_observations() {
    let page = normalize::parse_observations_page(OBSERVATIONS).unwrap();
    assert_eq!((page.count, page.offset), (84, 0));
    assert_eq!(page.observations.len(), 84);
    let first = normalize::parse_observation(&page.observations[0]).unwrap();
    assert_eq!(first.date, date(1929, 1, 1));
    assert_eq!(first.value, Some(1065.9));
    let last = normalize::parse_observation(&page.observations[83]).unwrap();
    assert_eq!(last.date, date(2012, 1, 1));
    assert_eq!(last.value, Some(15693.1));
}

#[test]
fn dot_is_missing_not_zero() {
    let page = normalize::parse_observations_page(MISSING_VALUES).unwrap();
    let obs: Vec<_> = page.observations.iter().map(|o| normalize::parse_observation(o).unwrap()).collect();
    assert_eq!(obs[0].value, Some(1.5));
    assert_eq!(obs[1].value, None);
    assert_eq!(obs[2].value, Some(2.25));
    assert_eq!(normalize::parse_value("0").unwrap(), Some(0.0));
    assert_eq!(normalize::parse_value(" . ").unwrap(), None);
}

#[test]
fn non_numeric_value_is_a_parse_error() {
    let err = normalize::parse_value("n/a").unwrap_err();
    assert!(matches!(err, ProviderError::Parse { .. }), "{err:?}");
    let bad = ObservationDto { date: "2001-13-01".into(), value: "1".into() };
    assert!(matches!(normalize::parse_observation(&bad), Err(ProviderError::Parse { .. })));
}

#[test]
fn pagination_offsets() {
    // Single page covers everything.
    assert_eq!(normalize::next_offset(84, 0, 84), None);
    // 250k observations in pages of 100k.
    assert_eq!(normalize::next_offset(250_000, 0, 100_000), Some(100_000));
    assert_eq!(normalize::next_offset(250_000, 100_000, 100_000), Some(200_000));
    assert_eq!(normalize::next_offset(250_000, 200_000, 50_000), None);
    // Release dates: 1129 in pages of 1000.
    assert_eq!(normalize::next_offset(1129, 0, 1000), Some(1000));
    assert_eq!(normalize::next_offset(1129, 1000, 129), None);
    // An empty page always stops, even if count says otherwise.
    assert_eq!(normalize::next_offset(10, 5, 0), None);
}

#[test]
fn build_series_sorts_and_uses_last_updated_as_of() {
    let meta = normalize::parse_series_meta(SERIES, "GNPCA").unwrap();
    let mut obs = normalize::parse_observations_page(OBSERVATIONS).unwrap().observations;
    obs.reverse();
    let s = normalize::build_series(meta, obs, NOW, |as_of| {
        meridian_types::Provenance::synthetic(as_of) // provenance contents tested end-to-end below
    })
    .unwrap();
    assert_eq!(s.observations.first().unwrap().date, date(1929, 1, 1));
    assert_eq!(s.latest().unwrap().value, Some(15693.1));
    assert_eq!(s.provenance.as_of, normalize::parse_last_updated("2013-07-31 09:26:16-05").unwrap());
}

// ---------------------------------------------------------------------------
// Calendar normalization
// ---------------------------------------------------------------------------

#[test]
fn importance_classification() {
    assert_eq!(normalize::importance(50, "Employment Situation"), Importance::High);
    assert_eq!(normalize::importance(10, "Consumer Price Index"), Importance::High);
    assert_eq!(normalize::importance(101, "FOMC Press Release"), Importance::High);
    // Name match when the ID is unfamiliar.
    assert_eq!(normalize::importance(-1, "gross domestic product"), Importance::High);
    // H.15 is daily; deliberately Medium.
    assert_eq!(normalize::importance(18, "H.15 Selected Interest Rates"), Importance::Medium);
    assert_eq!(normalize::importance(262, "Failures and Assistance Transactions"), Importance::Medium);
    assert_eq!(normalize::representative_series(50, ""), Some("PAYEMS"));
    assert_eq!(normalize::representative_series(13, ""), Some("INDPRO"));
    assert_eq!(normalize::representative_series(18, "H.15 Selected Interest Rates"), None);
}

#[test]
fn documented_release_dates_become_events() {
    let page = normalize::parse_release_dates_page(RELEASE_DATES).unwrap();
    assert_eq!((page.count, page.offset, page.release_dates.len()), (1129, 0, 20));
    let prov = meridian_types::Provenance::synthetic(NOW);
    let events: Vec<_> =
        page.release_dates.iter().map(|d| normalize::release_event(d, prov.clone()).unwrap()).collect();

    let retail = &events[0];
    assert_eq!(retail.event, "Advance Monthly Sales for Retail and Food Services");
    assert_eq!(retail.release_time, date_to_nanos(date(2013, 8, 13)));
    assert!(!retail.time_known);
    assert_eq!(retail.country, "US");
    assert_eq!(retail.importance, Importance::High);
    assert_eq!(retail.series_id.as_deref(), Some("RSAFS"));
    assert_eq!((retail.actual, retail.consensus, retail.prior, retail.period.clone()), (None, None, None, None));

    let gdp = events.iter().find(|e| e.event == "Gross Domestic Product").unwrap();
    assert_eq!(gdp.importance, Importance::High);
    assert_eq!(gdp.series_id.as_deref(), Some("GDP"));

    let h15 = events.iter().find(|e| e.event == "H.15 Selected Interest Rates").unwrap();
    assert_eq!(h15.importance, Importance::Medium);
    assert_eq!(h15.series_id, None);

    let highs = events.iter().filter(|e| e.importance == Importance::High).count();
    assert_eq!(highs, 2);
}

#[test]
fn bad_release_date_is_a_parse_error() {
    let dto = ReleaseDateDto { release_id: 1, release_name: "Test".into(), date: "soon".into() };
    let err = normalize::release_event(&dto, meridian_types::Provenance::synthetic(0)).unwrap_err();
    assert!(matches!(err, ProviderError::Parse { .. }));
}

// ---------------------------------------------------------------------------
// Errors and redaction
// ---------------------------------------------------------------------------

#[test]
fn redacts_full_and_truncated_keys() {
    let body = format!("bad key {TEST_KEY} here");
    assert_eq!(normalize::redact(&body, TEST_KEY), "bad key [REDACTED] here");
    let truncated = format!("cut at {}", &TEST_KEY[..10]);
    assert_eq!(normalize::redact(&truncated, TEST_KEY), "cut at [REDACTED]");
    assert_eq!(normalize::redact("nothing", TEST_KEY), "nothing");
}

#[test]
fn maps_documented_error_bodies() {
    let err = normalize::map_error(ProviderError::from_status(400, ERROR_API_KEY, None), TEST_KEY);
    match err {
        ProviderError::Unauthorized(m) => assert!(m.contains("not registered"), "{m}"),
        other => panic!("expected Unauthorized, got {other:?}"),
    }
    let err = normalize::map_error(ProviderError::from_status(400, ERROR_NO_SERIES, None), TEST_KEY);
    assert!(matches!(err, ProviderError::NotFound(ref m) if m.contains("does not exist")), "{err:?}");
    let err = normalize::map_error(ProviderError::from_status(429, "", None), TEST_KEY);
    assert!(matches!(err, ProviderError::RateLimited { .. }));
    let err = normalize::map_error(ProviderError::from_status(500, "oops", None), TEST_KEY);
    assert!(matches!(err, ProviderError::Http { status: 500, .. }));
}

#[test]
fn echoed_key_never_survives_error_mapping() {
    let body = format!(r#"{{"error_code":400,"error_message":"Bad Request. Value {TEST_KEY} is odd."}}"#);
    for status in [400, 401, 404, 423, 500] {
        let err = normalize::map_error(ProviderError::from_status(status, &body, None), TEST_KEY);
        assert!(!format!("{err:?}").contains(TEST_KEY), "{status}: {err:?}");
        assert!(!err.to_string().contains(TEST_KEY));
    }
}

// ---------------------------------------------------------------------------
// Provider behaviour against a local server
// ---------------------------------------------------------------------------

#[tokio::test]
async fn missing_key_is_unauthorized_without_http() {
    let server = TestServer::start(|_| (200, "{}".into())).await;
    for key in [None, Some(""), Some("   ")] {
        let p = provider(&server.base, key);
        assert_eq!(p.setup_needed().as_deref(), Some("FRED API key not set — add it in Settings"));
        let req = SeriesRequest { id: "GNPCA".into(), from: None, to: None };
        let err = p.economic_series(&req).await.unwrap_err();
        assert_eq!(err, ProviderError::Unauthorized("FRED API key not set — add it in Settings".into()));
        let cal = CalendarRequest { from: date(2026, 10, 1), to: date(2026, 10, 31), countries: vec![] };
        let err = p.economic_calendar(&cal).await.unwrap_err();
        assert!(matches!(err, ProviderError::Unauthorized(_)));
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(server.connections(), 0);
}

#[tokio::test]
async fn economic_series_end_to_end_with_pagination() {
    let server = TestServer::start(|target| match path_of(target) {
        "/fred/series" => (200, SERIES.to_owned()),
        "/fred/series/observations" => {
            let offset: usize = query_param(target, "offset").unwrap().parse().unwrap();
            (200, paged(OBSERVATIONS, "observations", 50, offset))
        }
        _ => (404, String::new()),
    })
    .await;
    let p = provider(&server.base, Some(TEST_KEY));
    let req = SeriesRequest { id: " GNPCA ".into(), from: Some(date(1929, 1, 1)), to: Some(date(2012, 1, 1)) };
    let s = p.economic_series(&req).await.unwrap();

    assert_eq!(s.id, "GNPCA");
    assert_eq!(s.observations.len(), 84);
    assert_eq!(s.observations[50].date, date(1979, 1, 1));
    assert_eq!(s.last_updated, normalize::parse_last_updated("2013-07-31 09:26:16-05"));

    let prov = &s.provenance;
    assert_eq!(prov.provider.as_str(), PROVIDER_ID);
    assert!(!prov.synthetic);
    assert_eq!(prov.delay, DataDelay::EndOfDay);
    assert_eq!(prov.source, FeedSource::Official);
    assert_eq!(prov.attribution.as_deref(), Some(ATTRIBUTION));
    assert_eq!(Some(prov.as_of), s.last_updated);
    let source_ref = prov.source_ref.as_deref().unwrap();
    assert!(source_ref.contains("/fred/series/observations?series_id=GNPCA"), "{source_ref}");
    assert!(!source_ref.contains(TEST_KEY), "{source_ref}");
    assert!(!source_ref.contains("api_key"), "{source_ref}");

    let reqs = server.requests();
    let paths: Vec<_> = reqs.iter().map(|r| path_of(&r.target).to_owned()).collect();
    assert_eq!(paths, ["/fred/series", "/fred/series/observations", "/fred/series/observations"]);
    for r in &reqs {
        assert_eq!(query_param(&r.target, "api_key").as_deref(), Some(TEST_KEY));
        assert_eq!(query_param(&r.target, "file_type").as_deref(), Some("json"));
        assert_eq!(query_param(&r.target, "series_id").as_deref(), Some("GNPCA"));
        let ua = r.headers.iter().find(|(k, _)| k == "user-agent").map(|(_, v)| v.as_str());
        assert_eq!(ua, Some("Meridian/0.1 (personal use)"));
    }
    assert_eq!(query_param(&reqs[1].target, "offset").as_deref(), Some("0"));
    assert_eq!(query_param(&reqs[2].target, "offset").as_deref(), Some("50"));
    assert_eq!(query_param(&reqs[1].target, "limit").as_deref(), Some("100000"));
    assert_eq!(query_param(&reqs[1].target, "observation_start").as_deref(), Some("1929-01-01"));
    assert_eq!(query_param(&reqs[1].target, "observation_end").as_deref(), Some("2012-01-01"));
}

#[tokio::test]
async fn missing_observations_stay_missing_end_to_end() {
    let server = TestServer::start(|target| match path_of(target) {
        "/fred/series" => (200, SERIES.to_owned()),
        _ => (200, MISSING_VALUES.to_owned()),
    })
    .await;
    let p = provider(&server.base, Some(TEST_KEY));
    let s = p.economic_series(&SeriesRequest { id: "GNPCA".into(), from: None, to: None }).await.unwrap();
    let values: Vec<_> = s.observations.iter().map(|o| o.value).collect();
    assert_eq!(values, [Some(1.5), None, Some(2.25)]);
    assert_eq!(s.latest().unwrap().value, Some(2.25));
}

#[tokio::test]
async fn unknown_series_is_not_found() {
    let server = TestServer::start(|_| (400, ERROR_NO_SERIES.to_owned())).await;
    let p = provider(&server.base, Some(TEST_KEY));
    let err = p.economic_series(&SeriesRequest { id: "NOSUCH".into(), from: None, to: None }).await.unwrap_err();
    assert!(matches!(err, ProviderError::NotFound(_)), "{err:?}");
}

#[tokio::test]
async fn rejected_key_is_unauthorized_and_redacted() {
    let echo = format!(
        r#"{{"error_code":400,"error_message":"Bad Request.  The value for variable api_key ({TEST_KEY}) is not registered."}}"#
    );
    let server = TestServer::start(move |_| (400, echo.clone())).await;
    let p = provider(&server.base, Some(TEST_KEY));
    let err = p.economic_series(&SeriesRequest { id: "GNPCA".into(), from: None, to: None }).await.unwrap_err();
    assert!(matches!(err, ProviderError::Unauthorized(_)), "{err:?}");
    assert!(!err.to_string().contains(TEST_KEY), "{err}");
}

#[tokio::test]
async fn rate_limit_maps_to_rate_limited() {
    let server = TestServer::start(|_| (429, String::new())).await;
    let p = provider(&server.base, Some(TEST_KEY));
    let err = p.economic_series(&SeriesRequest { id: "GNPCA".into(), from: None, to: None }).await.unwrap_err();
    assert!(matches!(err, ProviderError::RateLimited { .. }), "{err:?}");
}

#[tokio::test]
async fn economic_calendar_end_to_end_with_pagination() {
    let server = TestServer::start(|target| {
        let offset: usize = query_param(target, "offset").unwrap().parse().unwrap();
        (200, paged(RELEASE_DATES, "release_dates", 10, offset))
    })
    .await;
    let p = provider(&server.base, Some(TEST_KEY));
    let req = CalendarRequest { from: date(2013, 8, 8), to: date(2013, 8, 13), countries: vec!["us".into()] };
    let events = p.economic_calendar(&req).await.unwrap();

    assert_eq!(events.len(), 20);
    assert!(events.windows(2).all(|w| w[0].release_time <= w[1].release_time));
    assert_eq!(events.first().unwrap().release_time, date_to_nanos(date(2013, 8, 8)));
    for e in &events {
        assert_eq!(e.provenance.attribution.as_deref(), Some(ATTRIBUTION));
        assert_eq!(e.provenance.as_of, NOW);
        let r = e.provenance.source_ref.as_deref().unwrap();
        assert!(r.contains("/fred/releases/dates?"), "{r}");
        assert!(!r.contains(TEST_KEY) && !r.contains("api_key"), "{r}");
    }

    let reqs = server.requests();
    assert_eq!(reqs.len(), 2);
    let t = &reqs[0].target;
    assert_eq!(query_param(t, "realtime_start").as_deref(), Some("2013-08-08"));
    assert_eq!(query_param(t, "realtime_end").as_deref(), Some("2013-08-13"));
    assert_eq!(query_param(t, "include_release_dates_with_no_data").as_deref(), Some("true"));
    assert_eq!(query_param(t, "order_by").as_deref(), Some("release_date"));
    assert_eq!(query_param(t, "sort_order").as_deref(), Some("asc"));
    assert_eq!(query_param(t, "limit").as_deref(), Some("1000"));
    assert_eq!(query_param(t, "api_key").as_deref(), Some(TEST_KEY));
    assert_eq!(query_param(&reqs[0].target, "offset").as_deref(), Some("0"));
    assert_eq!(query_param(&reqs[1].target, "offset").as_deref(), Some("10"));
}

#[tokio::test]
async fn calendar_drops_dates_outside_the_window() {
    let server = TestServer::start(|_| {
        let mut v: Value = serde_json::from_str(RELEASE_DATES).unwrap();
        v["count"] = Value::from(20);
        (200, v.to_string())
    })
    .await;
    let p = provider(&server.base, Some(TEST_KEY));
    let req = CalendarRequest { from: date(2013, 8, 12), to: date(2013, 8, 12), countries: vec![] };
    let events = p.economic_calendar(&req).await.unwrap();
    assert_eq!(events.len(), 4);
    assert!(events.iter().all(|e| e.release_time == date_to_nanos(date(2013, 8, 12))));
}

#[tokio::test]
async fn non_us_calendar_is_empty_without_http() {
    let server = TestServer::start(|_| (200, RELEASE_DATES.to_owned())).await;
    let p = provider(&server.base, Some(TEST_KEY));
    let req =
        CalendarRequest { from: date(2013, 8, 8), to: date(2013, 8, 13), countries: vec!["GB".into(), "JP".into()] };
    assert!(p.economic_calendar(&req).await.unwrap().is_empty());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(server.connections(), 0);
}

#[test]
fn capabilities_declare_terms() {
    let p = FredProvider::new(FredConfig::default()).unwrap();
    assert_eq!(p.id().as_str(), "fred");
    let caps = p.capabilities();
    assert!(caps.supports(Capability::EconomicSeries, Some(AssetClass::Economic)));
    assert!(caps.supports(Capability::EconomicCalendar, None));
    assert!(!caps.supports(Capability::Quotes, None));
    assert_eq!(caps.attribution.as_deref(), Some(ATTRIBUTION));
    assert_eq!(caps.cache_policy, CachePolicy::Unrestricted);
    assert_eq!(caps.ai_policy, AiPolicy::Unreviewed);
    assert!(caps.requires_credentials);
    assert!(caps.display_allowed);
    let rl = caps.rate_limit.unwrap();
    assert!((rl.per_second - 2.0).abs() < 1e-9);
    assert!(caps.entries.iter().all(|e| e.source == FeedSource::Official && e.delay == DataDelay::EndOfDay));
    assert_eq!(caps.docs_url, "https://fred.stlouisfed.org/docs/api/fred/");
}

#[test]
fn attribution_is_the_verified_wording() {
    assert_eq!(
        ATTRIBUTION,
        "This product uses the FRED\u{ae} API but is not endorsed or certified by the Federal Reserve Bank of St. Louis."
    );
}

#[test]
fn config_debug_does_not_leak_the_key() {
    let c = FredConfig { api_key: Some(SecretString::from(TEST_KEY)) };
    assert!(!format!("{c:?}").contains(TEST_KEY));
}

#[test]
fn releases_dated_daily_are_not_high_importance() {
    let prov = meridian_types::Provenance::synthetic(0);
    let ev = |id: i64, name: &str, date: &str| {
        let dto = normalize::ReleaseDateDto { release_id: id, release_name: name.to_owned(), date: date.to_owned() };
        normalize::release_event(&dto, prov.clone()).unwrap()
    };
    let mut events = vec![
        ev(101, "FOMC Press Release", "2026-10-07"),
        ev(101, "FOMC Press Release", "2026-10-08"),
        ev(101, "FOMC Press Release", "2026-10-09"),
        ev(50, "Employment Situation", "2026-10-09"),
        ev(10, "Consumer Price Index", "2026-10-14"),
        ev(10, "Consumer Price Index", "2026-11-12"),
    ];
    normalize::demote_daily_releases(&mut events);
    let rated = |name: &str| events.iter().filter(|e| e.event == name).map(|e| e.importance).collect::<Vec<_>>();
    assert_eq!(rated("FOMC Press Release"), vec![Importance::Medium; 3]);
    assert_eq!(rated("Employment Situation"), vec![Importance::High]);
    assert_eq!(rated("Consumer Price Index"), vec![Importance::High; 2]);
}

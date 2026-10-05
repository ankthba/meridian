//! Fixture tests. Fixtures are the sample responses from Finnhub's own API
//! docs (see `tests/fixtures/SOURCES.md`); nothing here calls the real API.

use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use meridian_provider::{AiPolicy, CachePolicy, Capability, NewsQuery, NewsScope, Provider, ProviderError};
use meridian_types::{
    AssetClass, DataDelay, FeedSource, FixedClock, MarketSector, SecurityKey, date_to_nanos, nanos_from_secs,
};
use secrecy::SecretString;

use crate::normalize::{self, NewsDto, ProfileDto, RecommendationDto};
use crate::test_server::{TestServer, query_param};
use crate::{FinnhubConfig, FinnhubProvider, PROVIDER_ID};

const COMPANY_NEWS: &str = include_str!("../tests/fixtures/finnhub_company_news.json");
const MARKET_NEWS: &str = include_str!("../tests/fixtures/finnhub_market_news.json");
const RECOMMENDATION: &str = include_str!("../tests/fixtures/finnhub_recommendation.json");
const PROFILE: &str = include_str!("../tests/fixtures/finnhub_profile2.json");
const EARNINGS: &str = include_str!("../tests/fixtures/finnhub_earnings.json");

/// Obviously fake key.
const TEST_KEY: &str = "test-finnhub-key-not-real-0000";
/// 2019-09-27T00:00:00Z, the day after the documented company-news sample.
const NOW: i64 = 1_569_542_400_000_000_000;

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn provider(base: &str, key: Option<&str>) -> FinnhubProvider {
    let config = FinnhubConfig { api_key: key.map(SecretString::from) };
    FinnhubProvider::build(config, format!("{base}/api/v1"), Arc::new(FixedClock(NOW))).unwrap()
}

fn query(scope: NewsScope, keys: &[&str]) -> NewsQuery {
    NewsQuery {
        scope,
        keys: keys.iter().map(|k| SecurityKey::equity(k)).collect(),
        text: None,
        from: None,
        to: None,
        limit: 0,
    }
}

fn parse_news(body: &str) -> Vec<NewsDto> {
    serde_json::from_str(body).unwrap()
}

fn path_of(target: &str) -> &str {
    target.split('?').next().unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Symbols
// ---------------------------------------------------------------------------

#[test]
fn symbol_mapping() {
    let sym = |s: &str| normalize::finnhub_symbol(&s.parse::<SecurityKey>().unwrap());
    assert_eq!(sym("AAPL US Equity").as_deref(), Some("AAPL"));
    assert_eq!(sym("aapl Equity").as_deref(), Some("AAPL"));
    assert_eq!(sym("BRK/B US Equity").as_deref(), Some("BRK.B"));
    assert_eq!(sym("BRK.B US Equity").as_deref(), Some("BRK.B"));
    assert_eq!(sym("JPM-PD US Pfd").as_deref(), Some("JPM-PD"));
    assert_eq!(sym("VOD LN Equity"), None);
    assert_eq!(sym("EURUSD Curncy"), None);
    assert_eq!(sym("SPX Index"), None);
    assert_eq!(sym("T 4 Govt"), None);
    let weird = SecurityKey::new("A$B", Some("US"), MarketSector::Equity);
    assert_eq!(normalize::finnhub_symbol(&weird), None);
}

// ---------------------------------------------------------------------------
// Text cleaning
// ---------------------------------------------------------------------------

#[test]
fn clean_text_strips_html_and_whitespace() {
    assert_eq!(normalize::clean_text("<p>Hello&nbsp;<b>world</b></p>").as_deref(), Some("Hello world"));
    assert_eq!(normalize::clean_text("  a\n\n  b\t c ").as_deref(), Some("a b c"));
    assert_eq!(normalize::clean_text("AT&amp;T &lt;up&gt; &#39;5%&#x27;").as_deref(), Some("AT&T <up> '5%'"));
    assert_eq!(normalize::clean_text("a < b and c > d").as_deref(), Some("a < b and c > d"));
    assert_eq!(normalize::clean_text("Q&A & more").as_deref(), Some("Q&A & more"));
    assert_eq!(normalize::clean_text("line<br/>break").as_deref(), Some("line break"));
    assert_eq!(normalize::clean_text("<div> </div>"), None);
    assert_eq!(normalize::clean_text(""), None);
}

// ---------------------------------------------------------------------------
// News
// ---------------------------------------------------------------------------

#[test]
fn company_news_fixture_normalizes() {
    let url = "https://finnhub.io/api/v1/company-news?symbol=AAPL&from=2019-09-20&to=2019-09-27";
    let items: Vec<_> =
        parse_news(COMPANY_NEWS).into_iter().filter_map(|d| normalize::news_item(d, Some("AAPL"), NOW, url)).collect();
    assert_eq!(items.len(), 3);
    let first = &items[0];
    assert_eq!(first.id, "25286");
    assert_eq!(first.source, "The Economic Times India");
    assert!(first.headline.starts_with("More sops needed to boost electronic manufacturing"));
    assert_eq!(first.published_at, nanos_from_secs(1_569_550_360));
    assert_eq!(first.received_at, NOW);
    assert_eq!(first.tickers, ["AAPL"]);
    assert_eq!(first.topics, ["company news"]);
    assert_eq!(first.body, None);
    assert!(first.url.as_deref().unwrap().starts_with("https://economictimes.indiatimes.com/"));
    let summary = first.summary.as_deref().unwrap();
    assert!(summary.starts_with("NEW DELHI | CHENNAI: India may have to offer"));
    assert!(!summary.contains("  "));

    let p = &first.provenance;
    assert_eq!(p.provider.as_str(), PROVIDER_ID);
    assert!(!p.synthetic);
    assert_eq!(p.delay, DataDelay::RealTime);
    assert_eq!(p.source, FeedSource::Aggregated);
    assert_eq!(p.as_of, first.published_at);
    assert_eq!(p.source_ref.as_deref(), Some(url));
    assert_eq!(p.attribution, None);
}

#[test]
fn market_news_fixture_normalizes() {
    let items: Vec<_> = parse_news(MARKET_NEWS)
        .into_iter()
        .filter_map(|d| normalize::news_item(d, None, NOW, "https://finnhub.io/api/v1/news?category=general"))
        .collect();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].id, "5085164");
    assert_eq!(items[0].source, "CNBC");
    // `related` is empty in the sample and there is no requested symbol.
    assert!(items.iter().all(|i| i.tickers.is_empty()));
    let topics: Vec<_> = items.iter().map(|i| i.topics[0].as_str()).collect();
    assert_eq!(topics, ["technology", "business", "top news"]);
    // Escaped quotes in the summary survive.
    assert!(items[1].summary.as_deref().unwrap().starts_with("\"I think post-Covid"));
}

#[test]
fn related_tickers_are_split() {
    let dto = NewsDto {
        category: Some(String::new()),
        datetime: Some(1_700_000_000),
        headline: Some("Test headline".into()),
        id: Some(1),
        related: Some("aapl, MSFT,,".into()),
        source: Some("Test".into()),
        summary: Some(String::new()),
        url: Some(" ".into()),
    };
    let item = normalize::news_item(dto, Some("ZZZZ"), NOW, "u").unwrap();
    assert_eq!(item.tickers, ["AAPL", "MSFT"]);
    assert!(item.topics.is_empty());
    assert_eq!(item.summary, None);
    assert_eq!(item.url, None);
}

#[test]
fn incomplete_articles_are_dropped() {
    let base = || NewsDto {
        category: None,
        datetime: Some(1_700_000_000),
        headline: Some("Test headline".into()),
        id: Some(1),
        related: None,
        source: None,
        summary: None,
        url: None,
    };
    assert!(normalize::news_item(base(), None, NOW, "u").is_some());
    assert!(normalize::news_item(NewsDto { id: None, ..base() }, None, NOW, "u").is_none());
    assert!(normalize::news_item(NewsDto { datetime: None, ..base() }, None, NOW, "u").is_none());
    assert!(normalize::news_item(NewsDto { datetime: Some(0), ..base() }, None, NOW, "u").is_none());
    assert!(normalize::news_item(NewsDto { headline: Some(" <b></b> ".into()), ..base() }, None, NOW, "u").is_none());
}

#[test]
fn news_filtering_sorting_and_limit() {
    let items = || -> Vec<_> {
        parse_news(MARKET_NEWS).into_iter().filter_map(|d| normalize::news_item(d, None, NOW, "u")).collect()
    };
    let mut q = query(NewsScope::Market, &[]);

    // Text filter: case-insensitive, headline or summary.
    q.text = Some("SQUARE".into());
    let out = normalize::finish_news(items(), &q);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].id, "5085164");
    q.text = Some("breakfast".into()); // only in a summary
    assert_eq!(normalize::finish_news(items(), &q)[0].id, "5085113");
    q.text = Some("   ".into()); // blank = no filter
    assert_eq!(normalize::finish_news(items(), &q).len(), 3);

    // Time bounds are inclusive.
    q.text = None;
    q.from = Some(nanos_from_secs(1_596_588_232));
    let out = normalize::finish_news(items(), &q);
    assert_eq!(out.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["5085164", "5085113"]);
    q.to = Some(nanos_from_secs(1_596_588_232));
    assert_eq!(normalize::finish_news(items(), &q).len(), 1);

    // Newest first, de-duplicated, limited.
    let mut q = query(NewsScope::Market, &[]);
    let mut twice = items();
    twice.extend(items());
    twice.reverse();
    let out = normalize::finish_news(twice, &q);
    assert_eq!(out.len(), 3);
    assert!(out.windows(2).all(|w| w[0].published_at >= w[1].published_at));
    q.limit = 2;
    assert_eq!(normalize::finish_news(items(), &q).len(), 2);
}

#[test]
fn news_window_defaults() {
    // Open range → last 7 days to today.
    assert_eq!(normalize::news_window(None, None, NOW), (date(2019, 9, 20), date(2019, 9, 27)));
    let from = date_to_nanos(date(2019, 1, 2));
    let to = date_to_nanos(date(2019, 3, 4));
    assert_eq!(normalize::news_window(Some(from), Some(to), NOW), (date(2019, 1, 2), date(2019, 3, 4)));
    assert_eq!(normalize::news_window(None, Some(to), NOW), (date(2019, 2, 25), date(2019, 3, 4)));
}

// ---------------------------------------------------------------------------
// Recommendations, profile, earnings
// ---------------------------------------------------------------------------

#[test]
fn recommendation_picks_latest_period() {
    let key = SecurityKey::equity("AAPL");
    let periods: Vec<RecommendationDto> = serde_json::from_str(RECOMMENDATION).unwrap();
    let r = normalize::recommendations(&key, periods, "src").unwrap();
    assert_eq!(r.as_of, date(2025, 3, 1));
    assert_eq!((r.strong_buy, r.buy, r.hold, r.sell, r.strong_sell), (13, 24, 7, 0, 0));
    assert_eq!(r.total(), 44);
    assert_eq!((r.target_mean, r.target_high, r.target_low), (None, None, None));
    assert!(r.ratings.is_empty());
    assert_eq!(r.provenance.as_of, date_to_nanos(date(2025, 3, 1)));
    assert_eq!(r.provenance.delay, DataDelay::EndOfDay);

    // Order doesn't matter.
    let mut periods: Vec<RecommendationDto> = serde_json::from_str(RECOMMENDATION).unwrap();
    periods.reverse();
    assert_eq!(normalize::recommendations(&key, periods, "src").unwrap().as_of, date(2025, 3, 1));

    let err = normalize::recommendations(&key, Vec::new(), "src").unwrap_err();
    assert!(matches!(err, ProviderError::NotFound(_)));
}

#[test]
fn recommendation_missing_count_is_a_parse_error() {
    let body = r#"[{"buy":1,"hold":2,"period":"2025-03-01","sell":0,"strongBuy":1,"symbol":"TEST"}]"#;
    let err = normalize::parse_body::<Vec<RecommendationDto>>(body, "test", TEST_KEY, Capability::Recommendations)
        .unwrap_err();
    assert!(matches!(err, ProviderError::Parse { .. }), "{err:?}");
}

#[test]
fn profile_fixture_converts_millions() {
    let key = SecurityKey::equity("AAPL");
    let dto: ProfileDto = serde_json::from_str(PROFILE).unwrap();
    let p = normalize::profile(&key, dto).unwrap();
    assert_eq!(p.website.as_deref(), Some("https://www.apple.com/"));
    assert_eq!(p.ipo_date.as_deref(), Some("1980-12-12"));
    assert_eq!(p.headquarters.as_deref(), Some("US"));
    assert_eq!(p.market_cap, Some(1_415_993_000_000.0));
    assert!((p.shares_outstanding.unwrap() - 4_375_479_980.468_75).abs() < 1e-3);
    assert_eq!((p.description, p.employees, p.ceo, p.sic_code), (None, None, None, None));
}

#[test]
fn empty_profile_is_not_found() {
    let dto: ProfileDto = serde_json::from_str("{}").unwrap();
    let err = normalize::profile(&SecurityKey::equity("ZZZZ"), dto).unwrap_err();
    assert!(matches!(err, ProviderError::NotFound(_)));
}

#[test]
fn non_usd_market_cap_is_dropped() {
    let dto = ProfileDto {
        name: Some("Test Co".into()),
        currency: Some("TWD".into()),
        market_capitalization: Some(10.0),
        share_outstanding: Some(2.0),
        ..ProfileDto::default()
    };
    let p = normalize::profile(&SecurityKey::equity("TEST"), dto).unwrap();
    assert_eq!(p.market_cap, None);
    assert_eq!(p.shares_outstanding, Some(2_000_000.0));
}

#[test]
fn earnings_fixture_normalizes() {
    let key = SecurityKey::equity("AAPL");
    let rows = serde_json::from_str(EARNINGS).unwrap();
    let h = normalize::earnings(&key, rows, NOW, "src").unwrap();
    let labels: Vec<_> = h.records.iter().map(|r| r.fiscal_label.as_str()).collect();
    assert_eq!(labels, ["Q1 23", "Q4 22", "Q3 22"]);
    let q1 = &h.records[0];
    assert_eq!(q1.period_end, date(2023, 3, 31));
    assert_eq!((q1.eps_actual, q1.eps_estimate), (Some(1.88), Some(1.9744)));
    assert_eq!((q1.announce_date, q1.revenue_actual, q1.revenue_estimate), (None, None, None));
    assert!((q1.eps_surprise_pct().unwrap() - (-4.7812)).abs() < 1e-3);
    assert_eq!(h.provenance.as_of, NOW);
    assert_eq!(normalize::fiscal_label(1, 2005), "Q1 05");
}

#[test]
fn earnings_edge_cases() {
    let key = SecurityKey::equity("TEST");
    let err = normalize::earnings(&key, Vec::new(), NOW, "src").unwrap_err();
    assert!(matches!(err, ProviderError::NotFound(_)));
    let rows =
        serde_json::from_str(r#"[{"actual":null,"estimate":0.5,"period":"2024-06-30","symbol":"TEST"}]"#).unwrap();
    let h = normalize::earnings(&key, rows, NOW, "src").unwrap();
    assert_eq!(h.records[0].fiscal_label, "2024-06-30");
    assert_eq!(h.records[0].eps_actual, None);
    let rows = serde_json::from_str(r#"[{"actual":1.0,"estimate":0.5,"symbol":"TEST"}]"#).unwrap();
    assert!(matches!(normalize::earnings(&key, rows, NOW, "src"), Err(ProviderError::Parse { .. })));
}

// ---------------------------------------------------------------------------
// Errors and redaction
// ---------------------------------------------------------------------------

#[test]
fn error_bodies_are_classified_and_redacted() {
    let cap = Capability::Profile;
    let premium = r#"{"error":"You don't have access to this resource."}"#;
    let err = normalize::map_error(ProviderError::from_status(403, premium, None), TEST_KEY, cap);
    assert!(matches!(err, ProviderError::NotEntitled { capability: Capability::Profile, .. }), "{err:?}");

    let bad_key = format!(r#"{{"error":"Invalid API key {TEST_KEY}"}}"#);
    let err = normalize::map_error(ProviderError::from_status(401, &bad_key, None), TEST_KEY, cap);
    assert!(matches!(err, ProviderError::Unauthorized(_)), "{err:?}");
    assert!(!err.to_string().contains(TEST_KEY), "{err}");

    let err = normalize::map_error(ProviderError::from_status(429, "", None), TEST_KEY, cap);
    assert!(matches!(err, ProviderError::RateLimited { .. }));

    let err = normalize::map_error(ProviderError::from_status(500, &bad_key, None), TEST_KEY, cap);
    assert!(!format!("{err:?}").contains(TEST_KEY), "{err:?}");

    // A 200 carrying an error object.
    let err = normalize::parse_body::<ProfileDto>(premium, "test", TEST_KEY, cap).unwrap_err();
    assert!(matches!(err, ProviderError::NotEntitled { .. }));
    let err = normalize::parse_body::<ProfileDto>(r#"{"error":"API limit reached."}"#, "t", TEST_KEY, cap).unwrap_err();
    assert!(matches!(err, ProviderError::RateLimited { .. }));
    let err = normalize::parse_body::<Vec<NewsDto>>("not json", "Finnhub test", TEST_KEY, cap).unwrap_err();
    assert!(matches!(err, ProviderError::Parse { .. }));
}

#[test]
fn redacts_full_and_truncated_keys() {
    assert_eq!(normalize::redact(&format!("x {TEST_KEY} y"), TEST_KEY), "x [REDACTED] y");
    assert_eq!(normalize::redact(&format!("x {}", &TEST_KEY[..8]), TEST_KEY), "x [REDACTED]");
}

// ---------------------------------------------------------------------------
// Provider behaviour against a local server
// ---------------------------------------------------------------------------

#[tokio::test]
async fn missing_key_is_unauthorized_without_http() {
    let server = TestServer::start(|_| (200, "[]".into())).await;
    let key = SecurityKey::equity("AAPL");
    for api_key in [None, Some(""), Some("  ")] {
        let p = provider(&server.base, api_key);
        let expected = ProviderError::Unauthorized("Finnhub API key not set — add it in Settings".into());
        assert_eq!(p.news(&query(NewsScope::Company, &["AAPL"])).await.unwrap_err(), expected);
        assert_eq!(p.news(&query(NewsScope::Market, &[])).await.unwrap_err(), expected);
        assert_eq!(p.profile(&key).await.unwrap_err(), expected);
        assert_eq!(p.recommendations(&key).await.unwrap_err(), expected);
        assert_eq!(p.earnings(&key).await.unwrap_err(), expected);
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(server.connections(), 0);
}

#[tokio::test]
async fn unsupported_scopes_and_uncovered_keys_make_no_http() {
    let server = TestServer::start(|_| (200, "[]".into())).await;
    let p = provider(&server.base, Some(TEST_KEY));
    let err = p.news(&query(NewsScope::PressReleases, &[])).await.unwrap_err();
    assert_eq!(err, ProviderError::Unsupported { capability: Capability::News });
    let vod: SecurityKey = "VOD LN Equity".parse().unwrap();
    assert!(matches!(p.profile(&vod).await, Err(ProviderError::NotFound(_))));
    let mut q = query(NewsScope::Company, &[]);
    q.keys = vec![vod];
    assert!(matches!(p.news(&q).await, Err(ProviderError::NotFound(_))));
    // No keys at all → empty page.
    assert!(p.news(&query(NewsScope::Company, &[])).await.unwrap().items.is_empty());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(server.connections(), 0);
}

#[tokio::test]
async fn company_news_end_to_end() {
    let server = TestServer::start(|target| match path_of(target) {
        "/api/v1/company-news" => (200, COMPANY_NEWS.to_owned()),
        _ => (404, String::new()),
    })
    .await;
    let p = provider(&server.base, Some(TEST_KEY));
    // Two symbols returning the same articles: one request each, items de-duplicated.
    let mut q = query(NewsScope::Company, &["AAPL", "MSFT", "AAPL"]);
    q.text = Some("iphone".into());
    let page = p.news(&q).await.unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, "25341");
    assert_eq!(page.items[0].tickers, ["AAPL"]);
    assert_eq!(page.next, None);

    let reqs = server.requests();
    assert_eq!(reqs.len(), 2);
    assert_eq!(query_param(&reqs[0].target, "symbol").as_deref(), Some("AAPL"));
    assert_eq!(query_param(&reqs[1].target, "symbol").as_deref(), Some("MSFT"));
    for r in &reqs {
        assert_eq!(query_param(&r.target, "from").as_deref(), Some("2019-09-20"));
        assert_eq!(query_param(&r.target, "to").as_deref(), Some("2019-09-27"));
        assert_eq!(query_param(&r.target, "token"), None);
        assert!(!r.target.contains(TEST_KEY));
        let token = r.headers.iter().find(|(k, _)| k == "x-finnhub-token").map(|(_, v)| v.as_str());
        assert_eq!(token, Some(TEST_KEY));
    }
    let source_ref = page.items[0].provenance.source_ref.as_deref().unwrap();
    assert!(source_ref.ends_with("/api/v1/company-news?symbol=AAPL&from=2019-09-20&to=2019-09-27"), "{source_ref}");
    assert!(!source_ref.contains(TEST_KEY));
}

#[tokio::test]
async fn market_news_end_to_end() {
    let server = TestServer::start(|_| (200, MARKET_NEWS.to_owned())).await;
    let p = provider(&server.base, Some(TEST_KEY));
    let mut q = query(NewsScope::Market, &[]);
    q.limit = 2;
    let page = p.news(&q).await.unwrap();
    assert_eq!(page.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["5085164", "5085113"]);
    let reqs = server.requests();
    assert_eq!(path_of(&reqs[0].target), "/api/v1/news");
    assert_eq!(query_param(&reqs[0].target, "category").as_deref(), Some("general"));
}

#[tokio::test]
async fn company_endpoints_end_to_end() {
    let server = TestServer::start(|target| match path_of(target) {
        "/api/v1/stock/profile2" => (200, PROFILE.to_owned()),
        "/api/v1/stock/recommendation" => (200, RECOMMENDATION.to_owned()),
        "/api/v1/stock/earnings" => (200, EARNINGS.to_owned()),
        _ => (404, String::new()),
    })
    .await;
    let p = provider(&server.base, Some(TEST_KEY));
    let key: SecurityKey = "BRK/B US Equity".parse().unwrap();

    let profile = p.profile(&key).await.unwrap();
    assert_eq!(profile.market_cap, Some(1_415_993_000_000.0));
    let recs = p.recommendations(&key).await.unwrap();
    assert_eq!(recs.key, key);
    assert_eq!(recs.as_of, date(2025, 3, 1));
    let earn = p.earnings(&key).await.unwrap();
    assert_eq!(earn.records.len(), 3);
    for prov in [&recs.provenance, &earn.provenance] {
        let r = prov.source_ref.as_deref().unwrap();
        assert!(r.contains("symbol=BRK.B"), "{r}");
        assert!(!r.contains(TEST_KEY) && !r.contains("token"), "{r}");
        assert_eq!(prov.attribution, None);
    }
    for r in server.requests() {
        assert_eq!(query_param(&r.target, "symbol").as_deref(), Some("BRK.B"));
        assert_eq!(query_param(&r.target, "token"), None);
    }
}

#[tokio::test]
async fn unknown_symbol_profile_is_not_found() {
    let server = TestServer::start(|_| (200, "{}".into())).await;
    let p = provider(&server.base, Some(TEST_KEY));
    let err = p.profile(&SecurityKey::equity("ZZZZ")).await.unwrap_err();
    assert!(matches!(err, ProviderError::NotFound(_)), "{err:?}");
}

#[tokio::test]
async fn http_errors_map_end_to_end() {
    let server = TestServer::start(|target| match query_param(target, "symbol").as_deref() {
        Some("PREM") => (403, r#"{"error":"You don't have access to this resource."}"#.into()),
        Some("BADK") => (401, format!(r#"{{"error":"Invalid API key: {TEST_KEY}"}}"#)),
        Some("SLOW") => (429, r#"{"error":"API limit reached. Please try again later."}"#.into()),
        _ => (200, "[]".into()),
    })
    .await;
    let p = provider(&server.base, Some(TEST_KEY));
    let err = p.recommendations(&SecurityKey::equity("PREM")).await.unwrap_err();
    assert!(matches!(err, ProviderError::NotEntitled { capability: Capability::Recommendations, .. }), "{err:?}");
    let err = p.recommendations(&SecurityKey::equity("BADK")).await.unwrap_err();
    assert!(matches!(err, ProviderError::Unauthorized(_)), "{err:?}");
    assert!(!err.to_string().contains(TEST_KEY));
    let err = p.recommendations(&SecurityKey::equity("SLOW")).await.unwrap_err();
    assert!(matches!(err, ProviderError::RateLimited { .. }), "{err:?}");
    // Empty array → nothing to show.
    let err = p.recommendations(&SecurityKey::equity("NONE")).await.unwrap_err();
    assert!(matches!(err, ProviderError::NotFound(_)), "{err:?}");
}

#[test]
fn capabilities_declare_terms() {
    let p = FinnhubProvider::new(FinnhubConfig::default()).unwrap();
    assert_eq!(p.id().as_str(), "finnhub");
    let caps = p.capabilities();
    assert!(caps.supports(Capability::News, Some(AssetClass::Equity)));
    assert!(caps.supports(Capability::Profile, Some(AssetClass::Equity)));
    assert!(caps.supports(Capability::Recommendations, Some(AssetClass::Equity)));
    assert!(caps.supports(Capability::Earnings, Some(AssetClass::Equity)));
    assert!(!caps.supports(Capability::Estimates, None));
    assert!(!caps.supports(Capability::Quotes, None));
    assert_eq!(caps.cache_policy, CachePolicy::PurgeOnUnsubscribe);
    assert_eq!(caps.attribution, None);
    assert_eq!(caps.ai_policy, AiPolicy::Unreviewed);
    assert!(caps.requires_credentials && caps.display_allowed);
    assert!((caps.rate_limit.unwrap().per_second - 1.0).abs() < 1e-9);
    assert!(caps.entries.iter().all(|e| e.source == FeedSource::Aggregated));
    assert_eq!(caps.entry(Capability::News, None).unwrap().delay, DataDelay::RealTime);
    assert_eq!(caps.entry(Capability::Recommendations, None).unwrap().delay, DataDelay::EndOfDay);
}

#[test]
fn config_debug_does_not_leak_the_key() {
    let c = FinnhubConfig { api_key: Some(SecretString::from(TEST_KEY)) };
    assert!(!format!("{c:?}").contains(TEST_KEY));
}

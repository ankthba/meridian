//! Provider-level tests against a scripted HTTP server on 127.0.0.1. The
//! responses are the documented examples in `tests/fixtures`; nothing here
//! talks to Alpaca.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, NaiveDate};
use meridian_provider::EventSink;
use meridian_types::{Adjustment, BarInterval, OptionRight, StreamEvent};
use parking_lot::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::*;

const SNAPSHOTS: &str = include_str!("../tests/fixtures/stock_snapshots.json");
const BARS_PAGE_1: &str = include_str!("../tests/fixtures/stock_bars_single.json");
const BARS_PAGE_2: &str = include_str!("../tests/fixtures/stock_bars_single_last_page.json");
const CHAIN_PAGE_1: &str = include_str!("../tests/fixtures/option_chain.json");
const CHAIN_PAGE_2: &str = include_str!("../tests/fixtures/option_chain_constructed.json");
const NEWS: &str = include_str!("../tests/fixtures/news.json");

#[derive(Debug, Clone)]
struct Recorded {
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
}

/// Minimal HTTP/1.1 server: answers each request with the next scripted
/// `(status, body)` and records what it received.
struct TestServer {
    base: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl TestServer {
    async fn start(responses: Vec<(u16, &'static str)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let queue = Arc::new(Mutex::new(VecDeque::from(responses)));
        let recorded = requests.clone();
        tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                tokio::spawn(serve_connection(sock, recorded.clone(), queue.clone()));
            }
        });
        Self { base, requests }
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().clone()
    }
}

async fn serve_connection(
    mut sock: TcpStream,
    requests: Arc<Mutex<Vec<Recorded>>>,
    queue: Arc<Mutex<VecDeque<(u16, &'static str)>>>,
) {
    let mut buf = Vec::new();
    loop {
        let head_end = loop {
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
            let mut chunk = [0u8; 4096];
            match sock.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
        buf.drain(..head_end);
        let mut lines = head.lines();
        let target = lines.next().unwrap_or_default().split_whitespace().nth(1).unwrap_or_default().to_owned();
        let url = Url::parse(&format!("http://local{target}")).unwrap();
        let headers = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
            .collect();
        requests.lock().push(Recorded {
            path: url.path().to_owned(),
            query: url.query_pairs().into_owned().collect(),
            headers,
        });
        let (status, body) = queue.lock().pop_front().unwrap_or((500, r#"{"message":"no scripted response"}"#));
        let resp = format!(
            "HTTP/1.1 {status} Scripted\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        );
        if sock.write_all(resp.as_bytes()).await.is_err() {
            return;
        }
    }
}

fn config(feed: AlpacaFeed) -> AlpacaConfig {
    AlpacaConfig {
        key_id: Some(SecretString::from("TEST-KEY-ID")),
        secret_key: Some(SecretString::from("TEST-SECRET")),
        feed,
    }
}

fn provider(feed: AlpacaFeed, base: &str) -> AlpacaProvider {
    let mut p = AlpacaProvider::new(config(feed)).unwrap();
    base.clone_into(&mut p.data_base);
    p
}

fn keyless(base: &str) -> AlpacaProvider {
    let mut p = AlpacaProvider::new(AlpacaConfig { key_id: None, secret_key: None, feed: AlpacaFeed::Iex }).unwrap();
    base.clone_into(&mut p.data_base);
    base.replace("http", "ws").clone_into(&mut p.stream_base);
    p
}

fn ns(s: &str) -> UnixNanos {
    datetime_to_nanos(DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc))
}

fn bars_req(interval: BarInterval, from: Option<UnixNanos>, to: Option<UnixNanos>) -> BarsRequest {
    BarsRequest { key: SecurityKey::equity("AAPL"), interval, from, to, adjustment: Adjustment::Splits }
}

fn news_q(scope: NewsScope, keys: Vec<SecurityKey>, text: Option<&str>, limit: usize) -> NewsQuery {
    NewsQuery { scope, keys, text: text.map(str::to_owned), from: None, to: None, limit }
}

struct NullSink;

impl EventSink for NullSink {
    fn send(&self, _provider: &ProviderId, _event: StreamEvent) {}
}

#[test]
fn capabilities_follow_the_plan() {
    let basic = capabilities_for(AlpacaFeed::Iex);
    let plus = capabilities_for(AlpacaFeed::Sip);

    let quotes = basic.entry(Capability::Quotes, Some(AssetClass::Equity)).unwrap();
    assert_eq!(quotes.source, FeedSource::SingleVenue("IEX".into()));
    assert_eq!(quotes.delay, DataDelay::RealTime);
    let quotes = plus.entry(Capability::Quotes, Some(AssetClass::Etf)).unwrap();
    assert_eq!(quotes.source, FeedSource::Consolidated);

    let chain = basic.entry(Capability::OptionChain, Some(AssetClass::Equity)).unwrap();
    assert_eq!(chain.source, FeedSource::Modelled);
    assert_eq!(chain.delay, DataDelay::Delayed { minutes: 15 });
    let chain = plus.entry(Capability::OptionChain, Some(AssetClass::Equity)).unwrap();
    assert_eq!(chain.source, FeedSource::Consolidated);
    assert_eq!(chain.delay, DataDelay::RealTime);

    let daily = basic.entry(Capability::DailyBars, Some(AssetClass::Equity)).unwrap();
    assert_eq!(daily.source, FeedSource::Consolidated);
    assert_eq!(daily.delay, DataDelay::Delayed { minutes: 15 });
    assert_eq!(daily.history.as_deref(), Some("since 2016"));
    assert_eq!(plus.entry(Capability::IntradayBars, None).unwrap().delay, DataDelay::RealTime);

    let news = basic.entry(Capability::News, None).unwrap();
    assert_eq!(news.source, FeedSource::Aggregated);

    assert_eq!(basic.rate_limit, Some(RateLimit::per_minute(200)));
    assert_eq!(plus.rate_limit, Some(RateLimit::per_minute(10_000)));
    assert_eq!(basic.max_stream_symbols, Some(30));
    assert_eq!(plus.max_stream_symbols, None);
    for caps in [&basic, &plus] {
        assert!(caps.requires_credentials);
        assert_eq!(caps.ai_policy, AiPolicy::Unreviewed);
        assert_eq!(caps.cache_policy, CachePolicy::Unrestricted);
        assert!(caps.supports(Capability::Stream, Some(AssetClass::Equity)));
        assert!(!caps.supports(Capability::Quotes, Some(AssetClass::Crypto)));
        assert!(!caps.supports(Capability::Fundamentals, None));
        assert!(caps.docs_url.starts_with("https://docs.alpaca.markets/"));
    }
}

#[tokio::test]
async fn missing_keys_are_unauthorized_without_http() {
    let server = TestServer::start(Vec::new()).await;
    let unauthorized = ProviderError::Unauthorized(MISSING_KEY.into());
    let half = {
        let mut p = AlpacaProvider::new(AlpacaConfig {
            key_id: Some(SecretString::from("TEST-KEY-ID")),
            secret_key: Some(SecretString::from("  ")),
            feed: AlpacaFeed::Sip,
        })
        .unwrap();
        server.base.clone_into(&mut p.data_base);
        p
    };
    for p in [keyless(&server.base), half] {
        assert_eq!(p.quotes(&[SecurityKey::equity("AAPL")]).await.unwrap_err(), unauthorized);
        assert_eq!(p.bars(&bars_req(BarInterval::Day, None, None)).await.unwrap_err(), unauthorized);
        let chain = ChainRequest { underlying: SecurityKey::equity("AAPL"), expiry: None };
        assert_eq!(p.option_chain(&chain).await.unwrap_err(), unauthorized);
        let q = news_q(NewsScope::Market, Vec::new(), None, 10);
        assert_eq!(p.news(&q).await.unwrap_err(), unauthorized);
        let sink: StreamSink = Arc::new(NullSink);
        assert_eq!(p.streaming().unwrap().connect(sink).await.err(), Some(unauthorized.clone()));
    }
    assert!(server.requests().is_empty());
}

#[test]
fn auth_headers_are_sensitive() {
    let p = AlpacaProvider::new(config(AlpacaFeed::Iex)).unwrap();
    let h = p.auth_headers().unwrap();
    let id = h.get("APCA-API-KEY-ID").unwrap();
    let secret = h.get("APCA-API-SECRET-KEY").unwrap();
    assert_eq!(id.to_str().unwrap(), "TEST-KEY-ID");
    assert!(id.is_sensitive() && secret.is_sensitive());
    // Debug output of a sensitive header value is redacted.
    assert!(!format!("{h:?}").contains("TEST-SECRET"));
}

#[tokio::test]
async fn quotes_map_snapshots_and_skip_unserved_keys() {
    let server = TestServer::start(vec![(200, SNAPSHOTS)]).await;
    let p = provider(AlpacaFeed::Iex, &server.base);
    let tsla: SecurityKey = "TSLA Equity".parse().unwrap();
    let keys =
        [SecurityKey::equity("AAPL"), tsla.clone(), SecurityKey::currency("EURUSD"), SecurityKey::equity("MSFT")];
    let quotes = p.quotes(&keys).await.unwrap();

    let reqs = server.requests();
    assert_eq!(reqs.len(), 1);
    let r = &reqs[0];
    assert_eq!(r.path, "/v2/stocks/snapshots");
    assert_eq!(r.query["symbols"], "AAPL,MSFT,TSLA");
    assert_eq!(r.query["feed"], "iex");
    assert_eq!(r.headers["apca-api-key-id"], "TEST-KEY-ID");
    assert_eq!(r.headers["apca-api-secret-key"], "TEST-SECRET");
    assert_eq!(r.headers["user-agent"], http::USER_AGENT);

    assert_eq!(quotes.len(), 2, "MSFT is missing from the response; EURUSD isn't served");
    let aapl = quotes.iter().find(|q| q.key == SecurityKey::equity("AAPL")).unwrap();
    assert_eq!(aapl.last, Some(172.61));
    assert_eq!(aapl.provenance.source, FeedSource::SingleVenue("IEX".into()));
    assert_eq!(aapl.provenance.delay, DataDelay::RealTime);
    let source_ref = aapl.provenance.source_ref.as_deref().unwrap();
    assert!(source_ref.starts_with(&format!("{}/v2/stocks/snapshots?symbols=", server.base)));
    assert!(!source_ref.contains("TEST-SECRET") && !source_ref.contains("TEST-KEY-ID"));
    // The returned key is the caller's key, not a rebuilt one.
    assert!(quotes.iter().any(|q| q.key == tsla));
}

#[tokio::test]
async fn quotes_for_unserved_keys_need_no_keys_or_http() {
    let server = TestServer::start(Vec::new()).await;
    let p = keyless(&server.base);
    let out = p.quotes(&[SecurityKey::currency("EURUSD"), "VOD LN Equity".parse().unwrap()]).await.unwrap();
    assert!(out.is_empty());
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn http_errors_map_to_provider_errors() {
    let not_permitted = r#"{"code":42210000,"message":"subscription does not permit querying recent SIP data"}"#;
    let server = TestServer::start(vec![
        (403, not_permitted),
        (403, r#"{"message":"forbidden"}"#),
        (429, r#"{"message":"too many requests"}"#),
        (200, "not json"),
    ])
    .await;
    let p = provider(AlpacaFeed::Sip, &server.base);
    let keys = [SecurityKey::equity("AAPL")];
    assert_eq!(
        p.quotes(&keys).await.unwrap_err(),
        ProviderError::NotEntitled { capability: Capability::Quotes, plan: "Algo Trader Plus".into() }
    );
    assert!(matches!(p.quotes(&keys).await.unwrap_err(), ProviderError::Unauthorized(m) if m.contains("403")));
    assert!(matches!(p.quotes(&keys).await.unwrap_err(), ProviderError::RateLimited { .. }));
    assert!(matches!(p.quotes(&keys).await.unwrap_err(), ProviderError::Parse { .. }));
    assert_eq!(server.requests()[0].query["feed"], "sip");
}

#[test]
fn bars_window_defaults_and_basic_clamp() {
    let now = ns("2026-10-05T15:00:00Z");
    let embargo_end = ns("2026-10-05T14:44:00Z");

    let (start, end) = bars_window(AlpacaFeed::Iex, &bars_req(BarInterval::Day, None, None), now);
    assert_eq!(start, ns("2016-01-01T00:00:00Z"));
    assert_eq!(end, embargo_end);

    let (start, end) = bars_window(AlpacaFeed::Sip, &bars_req(BarInterval::Minute(5), None, None), now);
    assert_eq!(end, now);
    assert_eq!(start, ns("2026-09-05T15:00:00Z"));

    // An explicit range older than the embargo is untouched.
    let from = ns("2024-01-02T00:00:00Z");
    let to = ns("2024-02-01T00:00:00Z");
    assert_eq!(bars_window(AlpacaFeed::Iex, &bars_req(BarInterval::Day, Some(from), Some(to)), now), (from, to));
    // A range reaching into the last 15 minutes is clamped on Basic only.
    let (_, end) = bars_window(AlpacaFeed::Iex, &bars_req(BarInterval::Minute(1), Some(from), Some(now)), now);
    assert_eq!(end, embargo_end);
    let (_, end) = bars_window(AlpacaFeed::Sip, &bars_req(BarInterval::Minute(1), Some(from), Some(now)), now);
    assert_eq!(end, now);
}

#[tokio::test]
async fn bars_follow_pagination() {
    let server = TestServer::start(vec![(200, BARS_PAGE_1), (200, BARS_PAGE_2)]).await;
    let p = provider(AlpacaFeed::Iex, &server.base);
    let from = ns("2022-01-01T00:00:00Z");
    let series = p.bars(&bars_req(BarInterval::Day, Some(from), None)).await.unwrap();

    let reqs = server.requests();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[0].path, "/v2/stocks/AAPL/bars");
    assert_eq!(reqs[0].query["timeframe"], "1Day");
    assert_eq!(reqs[0].query["start"], "2022-01-01T00:00:00Z");
    assert_eq!(reqs[0].query["adjustment"], "split");
    assert_eq!(reqs[0].query["feed"], "sip");
    assert_eq!(reqs[0].query["limit"], "10000");
    assert!(!reqs[0].query.contains_key("page_token"));
    assert_eq!(reqs[1].query["page_token"], "QUFQTHxNfDIwMjItMDEtMDNUMDk6MDA6MDAuMDAwMDAwMDAwWg==");
    assert_eq!(reqs[1].query["start"], reqs[0].query["start"]);

    assert_eq!(series.ts, vec![ns("2022-01-03T09:00:00Z"), ns("2023-09-29T04:00:00Z")]);
    assert_eq!(series.close, vec![178.21, 171.29]);
    assert_eq!(series.volume, vec![1118.0, 923_134.0]);
    assert_eq!(series.adjustment, Adjustment::Splits);
    assert!(series.validate().is_ok());
    assert_eq!(series.provenance.source, FeedSource::Consolidated);
    assert_eq!(series.provenance.delay, DataDelay::Delayed { minutes: 15 });
    assert!(series.provenance.source_ref.as_deref().unwrap().contains("/v2/stocks/AAPL/bars?"));
}

#[tokio::test]
async fn bars_reject_unserved_keys_and_intervals_without_http() {
    let server = TestServer::start(Vec::new()).await;
    let p = provider(AlpacaFeed::Sip, &server.base);
    let mut req = bars_req(BarInterval::Day, None, None);
    req.key = SecurityKey::currency("EURUSD");
    assert!(matches!(p.bars(&req).await.unwrap_err(), ProviderError::NotFound(_)));
    let req = bars_req(BarInterval::Minute(90), None, None);
    assert_eq!(p.bars(&req).await.unwrap_err(), ProviderError::Unsupported { capability: Capability::IntradayBars });
    // Empty window: no request.
    let t = ns("2024-01-02T00:00:00Z");
    let empty = p.bars(&bars_req(BarInterval::Day, Some(t), Some(t))).await.unwrap();
    assert!(empty.is_empty());
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn option_chain_pages_and_underlying_price() {
    let server = TestServer::start(vec![(200, CHAIN_PAGE_1), (200, CHAIN_PAGE_2), (200, SNAPSHOTS)]).await;
    let p = provider(AlpacaFeed::Iex, &server.base);
    let expiry = NaiveDate::from_ymd_opt(2024, 4, 26).unwrap();
    let chain =
        p.option_chain(&ChainRequest { underlying: SecurityKey::equity("AAPL"), expiry: Some(expiry) }).await.unwrap();

    let reqs = server.requests();
    assert_eq!(reqs.len(), 3);
    assert_eq!(reqs[0].path, "/v1beta1/options/snapshots/AAPL");
    assert_eq!(reqs[0].query["feed"], "indicative");
    assert_eq!(reqs[0].query["limit"], "1000");
    assert_eq!(reqs[0].query["expiration_date"], "2024-04-26");
    assert_eq!(reqs[1].query["page_token"], "QUFQTDI0MDYyMTAwMDIyMHwx");
    assert_eq!(reqs[1].query["expiration_date"], "2024-04-26");
    assert_eq!(reqs[2].path, "/v2/stocks/snapshots");
    assert_eq!(reqs[2].query["symbols"], "AAPL");

    // The constructed fixture holds other contracts; the server isn't
    // filtering here, the test is about pagination and mapping.
    let symbols: Vec<&str> = chain.contracts.iter().map(|c| c.contract_symbol.as_str()).collect();
    assert_eq!(symbols, ["AAPL  240426C00162500", "X     261218C00002500", "TEST  261218P00012500"]);
    let aapl = &chain.contracts[0];
    assert_eq!(aapl.right, OptionRight::Call);
    assert_eq!(aapl.greeks.as_ref().unwrap().source, meridian_types::GreeksSource::Vendor);
    assert_eq!(chain.underlying, SecurityKey::equity("AAPL"));
    assert_eq!(chain.underlying_price, Some(172.61));
    // Newest vendor timestamp across both pages (the constructed trade).
    assert_eq!(chain.as_of, ns("2026-01-02T15:00:01Z"));
    assert_eq!(chain.provenance.as_of, chain.as_of);
    assert_eq!(chain.provenance.source, FeedSource::Modelled);
    assert_eq!(chain.provenance.delay, DataDelay::Delayed { minutes: 15 });
    assert_eq!(chain.provenance.provider.as_str(), "alpaca");
}

#[tokio::test]
async fn option_chain_uses_opra_on_plus_and_survives_missing_underlying() {
    let server = TestServer::start(vec![(200, CHAIN_PAGE_2), (500, r#"{"message":"internal"}"#)]).await;
    let p = provider(AlpacaFeed::Sip, &server.base);
    let chain = p.option_chain(&ChainRequest { underlying: SecurityKey::equity("TEST"), expiry: None }).await.unwrap();
    let reqs = server.requests();
    assert_eq!(reqs[0].query["feed"], "opra");
    assert!(!reqs[0].query.contains_key("expiration_date"));
    assert_eq!(chain.contracts.len(), 2);
    assert_eq!(chain.underlying_price, None);
    assert_eq!(chain.provenance.source, FeedSource::Consolidated);
    assert_eq!(chain.provenance.delay, DataDelay::RealTime);
}

#[tokio::test]
async fn news_company_scope_and_text_filter() {
    let server = TestServer::start(vec![(200, NEWS), (200, NEWS), (200, NEWS)]).await;
    let p = provider(AlpacaFeed::Iex, &server.base);
    let keys = vec![SecurityKey::equity("AAPL"), SecurityKey::equity("AAPL"), SecurityKey::currency("EURUSD")];
    let page = p.news(&news_q(NewsScope::Company, keys, Some("china"), 1)).await.unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, "24843171");
    assert_eq!(page.next.as_deref(), Some("MTY0MDk0ODkyMzAwMDAwMDAwMHwyNDg0MzE3MQ=="));

    let reqs = server.requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].path, "/v1beta1/news");
    assert_eq!(reqs[0].query["symbols"], "AAPL");
    assert_eq!(reqs[0].query["sort"], "desc");
    assert_eq!(reqs[0].query["include_content"], "false");
    assert_eq!(reqs[0].query["limit"], "1");

    // A filter that matches nothing pages on (here: one more page, which
    // returns the same token, so pagination stops with a parse error).
    let err = p.news(&news_q(NewsScope::Market, Vec::new(), Some("no such words"), 5)).await.unwrap_err();
    assert!(matches!(err, ProviderError::Parse { .. }));
    let reqs = server.requests();
    assert!(!reqs[1].query.contains_key("symbols"));
    assert_eq!(reqs[2].query["page_token"], "MTY0MDk0ODkyMzAwMDAwMDAwMHwyNDg0MzE3MQ==");
}

#[tokio::test]
async fn news_scopes_without_http() {
    let server = TestServer::start(Vec::new()).await;
    let p = provider(AlpacaFeed::Iex, &server.base);
    assert_eq!(
        p.news(&news_q(NewsScope::PressReleases, Vec::new(), None, 10)).await.unwrap_err(),
        ProviderError::Unsupported { capability: Capability::News }
    );
    let page = p.news(&news_q(NewsScope::Top, Vec::new(), None, 0)).await.unwrap();
    assert!(page.items.is_empty());
    let page = p.news(&news_q(NewsScope::Company, vec![SecurityKey::currency("EURUSD")], None, 10)).await.unwrap();
    assert!(page.items.is_empty());
    let page = p.news(&news_q(NewsScope::Market, Vec::new(), None, 0)).await.unwrap();
    assert!(page.items.is_empty());
    assert!(server.requests().is_empty());
}

#[test]
fn news_query_shape() {
    let q = news_query(
        Some(&["AAPL".to_owned(), "BRK.B".to_owned()]),
        Some(ns("2026-10-01T00:00:00Z")),
        Some(ns("2026-10-05T12:00:00Z")),
        50,
        Some("tok"),
    );
    let m: HashMap<&str, String> = q.into_iter().collect();
    assert_eq!(m["symbols"], "AAPL,BRK.B");
    assert_eq!(m["start"], "2026-10-01T00:00:00Z");
    assert_eq!(m["end"], "2026-10-05T12:00:00Z");
    assert_eq!(m["limit"], "50");
    assert_eq!(m["page_token"], "tok");
    assert_eq!(m["sort"], "desc");
    assert_eq!(m["include_content"], "false");
}

//! SEC EDGAR: ticker ↔ CIK reference data, company profiles and filing
//! history (submissions API), filing documents rendered as text, and
//! as-reported financial statements from XBRL company facts.
//!
//! The SEC requires every automated request to declare a User-Agent naming
//! the requester. The user sets that contact ("Jane Doe jane@example.com") in
//! Settings; until they do, every call fails with
//! [`ProviderError::Unauthorized`] and no request is sent. Requests are
//! throttled internally to stay within the SEC's 10 requests/second
//! fair-access limit. See `README.md` for documentation sources and rules.

mod document;
mod facts;
mod http;
mod submissions;
mod tickers;

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use meridian_provider::{
    AiPolicy, CachePolicy, Capabilities, Capability, CapabilityEntry, FilingsRequest,
    FundamentalsRequest, InstrumentQuery, Provider, ProviderError, ProviderResult, RateLimit,
    TokenBucket,
};
use meridian_types::{
    AssetClass, Clock, CompanyProfile, DataDelay, Dividends, FeedSource, Filing, FilingDocument,
    FilingSection, FilingsPage, Fundamentals, Instrument, MarketSector, Provenance, ProviderId,
    SecurityKey, SystemClock,
};
use tokio::time::Instant;

use crate::facts::{
    FactsIndex, FiscalCalendar, build_statements, dividend_periods, reported_splits,
};
use crate::submissions::{FilingColumnsDto, SubmissionsDto, matches_forms, sort_newest_first};
use crate::tickers::{TickerEntry, TickerIndex, cik10, equity_symbol};

/// Stable provider identifier.
pub const PROVIDER_ID: &str = "edgar";
/// The documentation this integration was built against.
pub const DOCS_URL: &str =
    "https://www.sec.gov/search-filings/edgar-application-programming-interfaces";

const WWW_BASE: &str = "https://www.sec.gov";
const DATA_BASE: &str = "https://data.sec.gov";
const MISSING_CONTACT: &str = "Set your contact name and email for SEC EDGAR in Settings (required by SEC fair-access policy)";
const INVALID_CONTACT: &str = "The SEC EDGAR contact in Settings may contain only printable ASCII characters (name and email)";
/// How long the ticker file is reused before it is fetched again.
const TICKER_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// Timeout for multi-megabyte responses (company facts, filing documents).
const LARGE_TIMEOUT: Duration = Duration::from_secs(90);
/// Older submission pages fetched at most per `filings` call.
const MAX_OLDER_PAGES: usize = 5;
/// Internal throttle applied before every HTTP request. Burst 2 plus 8/s
/// admits at most 10 requests in any one-second window, under the SEC's
/// "no more than 10 requests per second".
const INTERNAL_RATE: RateLimit = RateLimit {
    burst: 2,
    per_second: 8.0,
};

/// User configuration for EDGAR.
#[derive(Clone)]
pub struct EdgarConfig {
    /// Name and email declared to the SEC in the User-Agent, e.g.
    /// `Jane Doe jane@example.com`. Not a credential, but personal, so it is
    /// never logged.
    pub contact: String,
}

impl fmt::Debug for EdgarConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EdgarConfig")
            .field("contact", &"<redacted>")
            .finish()
    }
}

enum ClientState {
    Ready(reqwest::Client),
    /// No usable contact: every call fails with this message.
    Unconfigured(&'static str),
}

struct TickerCache {
    fetched: Instant,
    index: Arc<TickerIndex>,
}

/// The SEC EDGAR provider. Cheap to share behind an `Arc`; the ticker file is
/// cached inside it.
pub struct EdgarProvider {
    caps: Capabilities,
    client: ClientState,
    bucket: TokenBucket,
    http_requests: AtomicU64,
    www_base: String,
    data_base: String,
    max_document_bytes: usize,
    tickers: tokio::sync::Mutex<Option<TickerCache>>,
}

impl fmt::Debug for EdgarProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EdgarProvider")
            .field("configured", &matches!(self.client, ClientState::Ready(_)))
            .field("http_requests", &self.http_requests.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// Checks the contact is usable in a User-Agent header. An email address is
/// required by the SEC's declared-agent format.
fn validate_contact(contact: &str) -> Result<&str, &'static str> {
    let c = contact.trim();
    if c.is_empty() || !c.contains('@') {
        return Err(MISSING_CONTACT);
    }
    if !c.chars().all(|ch| ch.is_ascii_graphic() || ch == ' ') {
        return Err(INVALID_CONTACT);
    }
    Ok(c)
}

fn capabilities() -> Capabilities {
    let entry =
        |capability, asset_classes: &[AssetClass], delay, history: Option<&str>| CapabilityEntry {
            capability,
            asset_classes: asset_classes.to_vec(),
            delay,
            source: FeedSource::Official,
            history: history.map(Into::into),
        };
    let equity = &[AssetClass::Equity][..];
    let equity_etf = &[AssetClass::Equity, AssetClass::Etf][..];
    Capabilities {
        entries: vec![
            // The ticker file's refresh cadence isn't documented; we reuse it for 24 h.
            entry(Capability::Search, equity, DataDelay::EndOfDay, None),
            entry(Capability::Reference, equity, DataDelay::EndOfDay, None),
            // Submissions: "typical processing delay of less than a second".
            entry(Capability::Profile, equity_etf, DataDelay::RealTime, None),
            entry(
                Capability::Filings,
                equity_etf,
                DataDelay::RealTime,
                Some("EDGAR filing history; recent list covers at least 1 year or 1,000 filings, older pages on demand"),
            ),
            // XBRL APIs: "typical processing delay of under a minute".
            entry(
                Capability::Fundamentals,
                equity,
                DataDelay::RealTime,
                Some("as-reported XBRL financial data (phased in from 2009); available under a minute after filing"),
            ),
            // Same company facts: dividends per share by fiscal period.
            entry(
                Capability::Dividends,
                equity,
                DataDelay::RealTime,
                Some("dividends per share declared/paid by fiscal period from XBRL financial data; no ex-, record or pay dates"),
            ),
        ],
        rate_limit: Some(RateLimit::per_second(10)),
        max_stream_symbols: None,
        cache_policy: CachePolicy::Unrestricted,
        attribution: None,
        display_allowed: true,
        ai_policy: AiPolicy::Allowed,
        requires_credentials: false,
        terms_note: "SEC EDGAR is free public US government data: \"Information presented on sec.gov is considered \
                     public information and may be copied or further distributed by users of the web site without \
                     the SEC's permission\" (https://www.sec.gov/about/privacy-information), and \"All \
                     Government-created content on sec.gov and EDGAR public filing content are free to access and \
                     reuse\" (https://www.sec.gov/os/webmaster-faq#reuse). No API key; the SEC requires a declared \
                     User-Agent with your name and email (set in Settings; configuration, not a credential) and at \
                     most 10 requests per second (https://www.sec.gov/os/webmaster-faq#code-support). Financials are \
                     as-reported XBRL facts, mapped to standard lines by Meridian."
            .into(),
        docs_url: DOCS_URL.into(),
    }
}

impl EdgarProvider {
    /// Builds the provider. A missing or unusable contact does not fail
    /// construction; instead every call returns `Unauthorized` without
    /// sending a request.
    pub fn new(config: EdgarConfig) -> ProviderResult<Self> {
        Self::with_endpoints(config, WWW_BASE, DATA_BASE)
    }

    fn with_endpoints(
        config: EdgarConfig,
        www_base: &str,
        data_base: &str,
    ) -> ProviderResult<Self> {
        let client = match validate_contact(&config.contact) {
            Ok(contact) => ClientState::Ready(http::client(contact)?),
            Err(msg) => ClientState::Unconfigured(msg),
        };
        Ok(Self {
            caps: capabilities(),
            client,
            bucket: TokenBucket::new(INTERNAL_RATE),
            http_requests: AtomicU64::new(0),
            www_base: www_base.trim_end_matches('/').to_owned(),
            data_base: data_base.trim_end_matches('/').to_owned(),
            max_document_bytes: document::MAX_DOCUMENT_BYTES,
            tickers: tokio::sync::Mutex::new(None),
        })
    }

    /// Whether a usable SEC contact is configured.
    pub fn is_configured(&self) -> bool {
        matches!(self.client, ClientState::Ready(_))
    }

    fn http(&self) -> ProviderResult<&reqwest::Client> {
        match &self.client {
            ClientState::Ready(c) => Ok(c),
            ClientState::Unconfigured(msg) => Err(ProviderError::Unauthorized((*msg).into())),
        }
    }

    /// Waits for the internal token bucket. Called before every request.
    async fn throttle(&self) {
        self.bucket.acquire().await;
        self.http_requests.fetch_add(1, Ordering::Relaxed);
    }

    async fn get_text(&self, url: &str, timeout: Option<Duration>) -> ProviderResult<String> {
        let client = self.http()?;
        self.throttle().await;
        tracing::debug!(url, "EDGAR request");
        let mut req = client.get(url);
        if let Some(t) = timeout {
            req = req.timeout(t);
        }
        http::send_text(req).await
    }

    async fn get_bytes(&self, url: &str) -> ProviderResult<Vec<u8>> {
        let client = self.http()?;
        self.throttle().await;
        tracing::debug!(url, "EDGAR document request");
        http::send_bytes_limited(
            client.get(url).timeout(LARGE_TIMEOUT),
            self.max_document_bytes,
        )
        .await
    }

    fn provenance(source_ref: String) -> Provenance {
        Provenance {
            provider: ProviderId::new(PROVIDER_ID),
            synthetic: false,
            delay: DataDelay::RealTime,
            source: FeedSource::Official,
            as_of: SystemClock.now(),
            source_ref: Some(source_ref),
            attribution: None,
        }
    }

    /// The ticker index, fetched at most once per [`TICKER_TTL`]. If a
    /// refresh fails, the previous copy is reused.
    async fn ticker_index(&self) -> ProviderResult<Arc<TickerIndex>> {
        let mut cache = self.tickers.lock().await;
        if let Some(c) = cache.as_ref()
            && c.fetched.elapsed() < TICKER_TTL
        {
            return Ok(c.index.clone());
        }
        let url = format!("{}/files/company_tickers.json", self.www_base);
        let fetched = match self.get_text(&url, None).await {
            Ok(body) => TickerIndex::parse(&body),
            Err(e) => Err(e),
        };
        match fetched {
            Ok(index) => {
                let index = Arc::new(index);
                *cache = Some(TickerCache {
                    fetched: Instant::now(),
                    index: index.clone(),
                });
                Ok(index)
            }
            Err(e) => match cache.as_ref() {
                Some(c) => {
                    tracing::warn!(error = %e, "EDGAR ticker refresh failed; reusing the previous copy");
                    Ok(c.index.clone())
                }
                None => Err(e),
            },
        }
    }

    async fn resolve(&self, key: &SecurityKey) -> ProviderResult<TickerEntry> {
        let symbol = equity_symbol(key).ok_or_else(|| {
            ProviderError::NotFound(format!("{key} is not a US equity key covered by SEC EDGAR"))
        })?;
        let index = self.ticker_index().await?;
        index.lookup(symbol).cloned().ok_or_else(|| {
            ProviderError::NotFound(format!("{symbol} is not in the SEC ticker list"))
        })
    }

    async fn submissions(&self, cik: u64) -> ProviderResult<SubmissionsDto> {
        let url = format!("{}/submissions/CIK{}.json", self.data_base, cik10(cik));
        let body = self.get_text(&url, None).await?;
        http::parse_json(&body, "submissions")
    }

    async fn parse_facts(body: String) -> ProviderResult<FactsIndex> {
        tokio::task::spawn_blocking(move || FactsIndex::parse(&body))
            .await
            .map_err(|e| ProviderError::parse(format!("companyfacts: parser task failed: {e}")))?
    }
}

#[async_trait]
impl Provider for EdgarProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(PROVIDER_ID)
    }

    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    async fn search(&self, q: &InstrumentQuery) -> ProviderResult<Vec<Instrument>> {
        self.http()?;
        if q.sector.is_some_and(|s| s != MarketSector::Equity) {
            return Ok(Vec::new());
        }
        let index = self.ticker_index().await?;
        Ok(index
            .search(&q.text, q.limit)
            .into_iter()
            .map(|e| Instrument {
                cik: Some(e.cik),
                ..Instrument::basic(
                    SecurityKey::equity(&e.ticker),
                    e.title.clone(),
                    AssetClass::Equity,
                    "USD",
                )
            })
            .collect())
    }

    async fn instrument(&self, key: &SecurityKey) -> ProviderResult<Instrument> {
        self.http()?;
        let e = self.resolve(key).await?;
        Ok(Instrument {
            cik: Some(e.cik),
            ..Instrument::basic(key.clone(), e.title, AssetClass::Equity, "USD")
        })
    }

    async fn profile(&self, key: &SecurityKey) -> ProviderResult<CompanyProfile> {
        self.http()?;
        let e = self.resolve(key).await?;
        Ok(self.submissions(e.cik).await?.to_profile())
    }

    async fn filings(&self, req: &FilingsRequest) -> ProviderResult<FilingsPage> {
        self.http()?;
        // A cross-filer "latest filings" feed is not implemented.
        let key = req.key.as_ref().ok_or(ProviderError::Unsupported {
            capability: Capability::Filings,
        })?;
        let e = self.resolve(key).await?;
        let subs = self.submissions(e.cik).await?;
        let company = subs.company_name();
        let base = Self::provenance(String::new());
        let mut filings: Vec<Filing> = match subs.recent() {
            Some(cols) => cols.to_filings(subs.cik, &company, &self.www_base, &base)?,
            None => Vec::new(),
        };
        filings.retain(|f| matches_forms(f, &req.forms));
        for page in subs.older_pages().into_iter().take(MAX_OLDER_PAGES) {
            if filings.len() >= req.limit {
                break;
            }
            let url = format!("{}/submissions/{page}", self.data_base);
            let older = match self.get_text(&url, None).await {
                Ok(body) => http::parse_json::<FilingColumnsDto>(&body, "submissions page")
                    .and_then(|cols| cols.to_filings(subs.cik, &company, &self.www_base, &base)),
                Err(e) => Err(e),
            };
            match older {
                Ok(more) => {
                    filings.extend(more.into_iter().filter(|f| matches_forms(f, &req.forms)));
                }
                Err(err) => {
                    tracing::warn!(error = %err, "EDGAR older filings page failed; returning the filings already loaded");
                    break;
                }
            }
        }
        sort_newest_first(&mut filings);
        filings.truncate(req.limit);
        Ok(FilingsPage {
            key: Some(key.clone()),
            filings,
        })
    }

    async fn filing_document(&self, filing: &Filing) -> ProviderResult<FilingDocument> {
        self.http()?;
        let url = filing.primary_doc_url.as_deref().ok_or_else(|| {
            ProviderError::NotFound(format!(
                "filing {} has no primary document",
                filing.accession
            ))
        })?;
        // Only SEC archive URLs: the declared contact must never be sent elsewhere.
        if !url.starts_with(&format!("{}/Archives/edgar/data/", self.www_base)) {
            return Err(ProviderError::Unsupported {
                capability: Capability::Filings,
            });
        }
        let name = filing.primary_document.as_deref().unwrap_or(url);
        let kind = document::doc_kind(name).ok_or_else(|| {
            ProviderError::Upstream(format!(
                "primary document {name} is not HTML or plain text and can't be shown as text"
            ))
        })?;
        let bytes = self.get_bytes(url).await?;
        let form = filing.form.clone();
        let sections =
            tokio::task::spawn_blocking(move || -> ProviderResult<Vec<FilingSection>> {
                let text = document::render_text(&bytes, kind)?;
                Ok(document::split_sections(&text, &form))
            })
            .await
            .map_err(|e| {
                ProviderError::parse(format!("filing document: conversion task failed: {e}"))
            })??;
        Ok(FilingDocument {
            filing: filing.clone(),
            sections,
        })
    }

    async fn fundamentals(&self, req: &FundamentalsRequest) -> ProviderResult<Fundamentals> {
        let facts = self.company_facts(&req.key).await?;
        let statements =
            build_statements(&facts.index, &facts.calendar, req.period_type, req.periods);
        if statements.is_empty() {
            return Err(no_statement_facts(&req.key));
        }
        Ok(Fundamentals {
            key: req.key.clone(),
            statements,
            reported_splits: reported_splits(&facts.index),
            provenance: Self::provenance(facts.url),
        })
    }

    /// Dividends per share by fiscal period, from the same company facts as
    /// the statements. Filings report per-period totals, not dividend events,
    /// so `dividends` (events with ex-dates) is always empty.
    async fn dividends(&self, key: &SecurityKey) -> ProviderResult<Dividends> {
        let facts = self.company_facts(key).await?;
        let per_period = dividend_periods(&facts.index, &facts.calendar);
        let splits = reported_splits(&facts.index);
        if per_period.is_empty() && splits.is_empty() {
            return Err(ProviderError::NotFound(format!(
                "{key} reports no dividends per share in its XBRL financial data"
            )));
        }
        Ok(Dividends {
            key: key.clone(),
            dividends: Vec::new(),
            per_period,
            reported_splits: splits,
            provenance: Self::provenance(facts.url),
        })
    }
}

fn no_statement_facts(key: &SecurityKey) -> ProviderError {
    ProviderError::NotFound(format!("no us-gaap financial statement facts for {key}"))
}

/// Parsed company facts with the fiscal calendar they were classified by.
struct CompanyFacts {
    index: FactsIndex,
    calendar: FiscalCalendar,
    url: String,
}

impl EdgarProvider {
    /// Fetches and indexes `companyfacts` for `key` and builds its fiscal
    /// calendar (from 10-K periods, else from the submissions fiscal year end).
    async fn company_facts(&self, key: &SecurityKey) -> ProviderResult<CompanyFacts> {
        self.http()?;
        let e = self.resolve(key).await?;
        let url = format!(
            "{}/api/xbrl/companyfacts/CIK{}.json",
            self.data_base,
            cik10(e.cik)
        );
        let body = self.get_text(&url, Some(LARGE_TIMEOUT)).await?;
        let index = Self::parse_facts(body).await?;
        let no_data = || no_statement_facts(key);
        if !index.has_facts() {
            return Err(no_data());
        }
        let calendar = if let Some(c) = index.calendar_from_annual() {
            c
        } else {
            // No 10-K yet: fall back to the declared fiscal year end.
            let (m, d) = self
                .submissions(e.cik)
                .await?
                .fiscal_year_end_md()
                .ok_or_else(no_data)?;
            index.calendar_from_fye(m, d).ok_or_else(no_data)?
        };
        Ok(CompanyFacts {
            index,
            calendar,
            url,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use meridian_types::{PeriodType, StatementKind};
    use parking_lot::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    const CONTACT: &str = "Jane Doe jane@example.com";

    /// (path, User-Agent) of every request the mock server received.
    type Hits = Arc<Mutex<Vec<(String, String)>>>;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn routes() -> Vec<(String, Vec<u8>)> {
        vec![
            (
                "/files/company_tickers.json".into(),
                fixture("company_tickers.json"),
            ),
            (
                "/submissions/CIK0000000001.json".into(),
                fixture("submissions_CIK0000000001.json"),
            ),
            (
                "/submissions/CIK0000000001-submissions-001.json".into(),
                fixture("CIK0000000001-submissions-001.json"),
            ),
            (
                "/api/xbrl/companyfacts/CIK0000000001.json".into(),
                fixture("companyfacts_CIK0000000001.json"),
            ),
            (
                "/api/xbrl/companyfacts/CIK0000000002.json".into(),
                fixture("companyfacts_CIK0000000002.json"),
            ),
            (
                "/Archives/edgar/data/1/000000000125000010/exmp-20250930.htm".into(),
                fixture("example_10k.htm"),
            ),
        ]
    }

    /// Minimal HTTP/1.1 server answering GETs from `routes` (404 otherwise).
    async fn serve(routes: Vec<(String, Vec<u8>)>) -> (String, Hits) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let hits: Hits = Arc::default();
        let log = hits.clone();
        let routes = Arc::new(routes);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let log = log.clone();
                let routes = routes.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 4096];
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        let n = sock.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    let head = String::from_utf8_lossy(&buf).into_owned();
                    let path = head
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .to_owned();
                    let ua = head
                        .lines()
                        .find_map(|l| {
                            let (name, value) = l.split_once(':')?;
                            name.eq_ignore_ascii_case("user-agent")
                                .then(|| value.trim().to_owned())
                        })
                        .unwrap_or_default();
                    log.lock().push((path.clone(), ua));
                    let (status, body) = routes
                        .iter()
                        .find(|(p, _)| *p == path)
                        .map_or((404, b"Not Found".to_vec()), |(_, b)| (200, b.clone()));
                    let resp = format!(
                        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.write_all(&body).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        (base, hits)
    }

    async fn provider_with(contact: &str) -> (EdgarProvider, Hits) {
        let (base, hits) = serve(routes()).await;
        let p = EdgarProvider::with_endpoints(
            EdgarConfig {
                contact: contact.into(),
            },
            &base,
            &base,
        )
        .unwrap();
        (p, hits)
    }

    fn paths(hits: &Hits) -> Vec<String> {
        hits.lock().iter().map(|h| h.0.clone()).collect()
    }

    #[test]
    fn capabilities_are_declared_honestly() {
        let c = capabilities();
        assert_eq!(c.rate_limit, Some(RateLimit::per_second(10)));
        assert_eq!(c.cache_policy, CachePolicy::Unrestricted);
        assert_eq!(c.ai_policy, AiPolicy::Allowed);
        assert!(!c.requires_credentials);
        assert_eq!(c.attribution, None);
        assert!(c.supports(Capability::Filings, Some(AssetClass::Etf)));
        assert!(c.supports(Capability::Fundamentals, Some(AssetClass::Equity)));
        assert!(!c.supports(Capability::Fundamentals, Some(AssetClass::Etf)));
        assert!(c.supports(Capability::Dividends, Some(AssetClass::Equity)));
        assert!(!c.supports(Capability::Dividends, Some(AssetClass::Etf)));
        assert!(!c.supports(Capability::Quotes, None));
        assert!(c.entries.iter().all(|e| e.source == FeedSource::Official));
        assert!(
            c.terms_note
                .contains("https://www.sec.gov/about/privacy-information")
        );
    }

    #[test]
    fn validates_contact() {
        assert_eq!(validate_contact("  "), Err(MISSING_CONTACT));
        assert_eq!(validate_contact("Jane Doe"), Err(MISSING_CONTACT));
        assert_eq!(
            validate_contact("Jöne jane@example.com"),
            Err(INVALID_CONTACT)
        );
        assert_eq!(
            validate_contact("Jane\njane@example.com"),
            Err(INVALID_CONTACT)
        );
        assert_eq!(
            validate_contact(" Jane Doe jane@example.com "),
            Ok("Jane Doe jane@example.com")
        );
        let dbg = format!(
            "{:?}",
            EdgarConfig {
                contact: CONTACT.into()
            }
        );
        assert!(!dbg.contains("jane"));
    }

    #[tokio::test]
    async fn missing_contact_is_unauthorized_without_http() {
        for contact in ["", "   ", "no email here"] {
            let (p, hits) = provider_with(contact).await;
            assert!(!p.is_configured());
            let expect = ProviderError::Unauthorized(MISSING_CONTACT.into());
            let key = SecurityKey::equity("EXMP");
            assert_eq!(p.instrument(&key).await.unwrap_err(), expect);
            assert_eq!(p.profile(&key).await.unwrap_err(), expect);
            let q = InstrumentQuery {
                text: "ex".into(),
                sector: None,
                limit: 5,
            };
            assert_eq!(p.search(&q).await.unwrap_err(), expect);
            let fr = FilingsRequest {
                key: Some(key.clone()),
                forms: vec![],
                limit: 5,
            };
            assert_eq!(p.filings(&fr).await.unwrap_err(), expect);
            let req = FundamentalsRequest {
                key: key.clone(),
                period_type: PeriodType::Annual,
                periods: 4,
            };
            assert_eq!(p.fundamentals(&req).await.unwrap_err(), expect);
            // Non-equity keys get the same answer: every call needs the contact.
            assert_eq!(
                p.instrument(&SecurityKey::currency("EURUSD"))
                    .await
                    .unwrap_err(),
                expect
            );
            assert!(
                hits.lock().is_empty(),
                "no request may be sent without a contact"
            );
            assert_eq!(p.http_requests.load(Ordering::Relaxed), 0);
        }
    }

    #[tokio::test]
    async fn declares_user_agent_and_caches_tickers() {
        let (p, hits) = provider_with(CONTACT).await;
        let inst = p.instrument(&SecurityKey::equity("SMPL.B")).await.unwrap();
        assert_eq!(inst.cik, Some(2));
        assert_eq!(inst.name, "Sample Holdings Inc.");
        assert_eq!(inst.key, SecurityKey::equity("SMPL.B"));
        assert!(!inst.is_synthetic);
        let found = p
            .search(&InstrumentQuery {
                text: "exa".into(),
                sector: None,
                limit: 10,
            })
            .await
            .unwrap();
        assert_eq!(
            found
                .iter()
                .map(|i| i.key.symbol.as_str())
                .collect::<Vec<_>>(),
            vec!["EXA", "EXMP"]
        );
        assert_eq!(found[1].cik, Some(1));
        // One fetch of the ticker file serves both calls.
        assert_eq!(paths(&hits), vec!["/files/company_tickers.json"]);
        assert_eq!(hits.lock()[0].1, "Meridian/0.1 Jane Doe jane@example.com");
    }

    #[tokio::test]
    async fn unmapped_keys_are_not_found() {
        let (p, hits) = provider_with(CONTACT).await;
        let fx = p
            .instrument(&SecurityKey::currency("EURUSD"))
            .await
            .unwrap_err();
        assert!(matches!(fx, ProviderError::NotFound(_)));
        let foreign = SecurityKey::new("EXMP", Some("LN"), MarketSector::Equity);
        assert!(matches!(
            p.profile(&foreign).await.unwrap_err(),
            ProviderError::NotFound(_)
        ));
        assert!(hits.lock().is_empty());
        let unknown = p
            .instrument(&SecurityKey::equity("NOPE"))
            .await
            .unwrap_err();
        assert!(matches!(unknown, ProviderError::NotFound(_)));
        let other = InstrumentQuery {
            text: "ex".into(),
            sector: Some(MarketSector::Curncy),
            limit: 5,
        };
        assert!(p.search(&other).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn profile_from_submissions() {
        let (p, hits) = provider_with(CONTACT).await;
        let prof = p.profile(&SecurityKey::equity("EXMP")).await.unwrap();
        assert_eq!(prof.headquarters.as_deref(), Some("ANYTOWN, CA"));
        assert_eq!(prof.fiscal_year_end.as_deref(), Some("09-30"));
        assert_eq!(
            paths(&hits),
            vec![
                "/files/company_tickers.json",
                "/submissions/CIK0000000001.json"
            ]
        );
    }

    #[tokio::test]
    async fn filings_filter_sort_limit_and_page() {
        let (p, hits) = provider_with(CONTACT).await;
        let key = SecurityKey::equity("EXMP");
        let req = FilingsRequest {
            key: Some(key.clone()),
            forms: vec!["10-K".into()],
            limit: 2,
        };
        let page = p.filings(&req).await.unwrap();
        let acc: Vec<&str> = page.filings.iter().map(|f| f.accession.as_str()).collect();
        assert_eq!(acc, vec!["0000000001-25-000010", "0000000001-24-000010"]);
        let f = &page.filings[0];
        assert_eq!(f.company, "Example Corp");
        assert_eq!(f.provenance.provider.as_str(), "edgar");
        assert!(!f.provenance.synthetic);
        assert_eq!(f.provenance.source, FeedSource::Official);
        assert!(f.provenance.as_of > 0);
        assert_eq!(
            f.primary_doc_url
                .as_deref()
                .map(|u| u.split_once("/Archives").unwrap().1),
            Some("/edgar/data/1/000000000125000010/exmp-20250930.htm")
        );
        // Limit satisfied from the recent list: no older page fetched.
        assert_eq!(paths(&hits).len(), 2);

        // Asking for more 10-Ks than the recent list holds pulls the older page.
        let req = FilingsRequest {
            key: Some(key.clone()),
            forms: vec!["10-K".into()],
            limit: 10,
        };
        let page = p.filings(&req).await.unwrap();
        let acc: Vec<&str> = page.filings.iter().map(|f| f.accession.as_str()).collect();
        assert_eq!(
            acc,
            vec![
                "0000000001-25-000010",
                "0000000001-24-000010",
                "0000000001-23-000010"
            ]
        );
        assert!(
            paths(&hits).contains(&"/submissions/CIK0000000001-submissions-001.json".to_string())
        );

        let none = FilingsRequest {
            key: None,
            forms: vec![],
            limit: 5,
        };
        assert_eq!(
            p.filings(&none).await.unwrap_err(),
            ProviderError::Unsupported {
                capability: Capability::Filings
            }
        );
    }

    #[tokio::test]
    async fn filing_document_sections() {
        let (p, _) = provider_with(CONTACT).await;
        let req = FilingsRequest {
            key: Some(SecurityKey::equity("EXMP")),
            forms: vec!["10-K".into()],
            limit: 1,
        };
        let filing = p.filings(&req).await.unwrap().filings.remove(0);
        let doc = p.filing_document(&filing).await.unwrap();
        assert_eq!(doc.filing, filing);
        assert_eq!(doc.sections[0].title, "Cover");
        assert_eq!(doc.sections[1].title, "Item 1. Business");
    }

    #[tokio::test]
    async fn filing_document_rejects_foreign_urls_and_oversize() {
        let (mut p, hits) = provider_with(CONTACT).await;
        let req = FilingsRequest {
            key: Some(SecurityKey::equity("EXMP")),
            forms: vec!["10-K".into()],
            limit: 1,
        };
        let mut filing = p.filings(&req).await.unwrap().filings.remove(0);
        let before = hits.lock().len();

        p.max_document_bytes = 100;
        let err = p.filing_document(&filing).await.unwrap_err();
        assert!(
            matches!(err, ProviderError::Upstream(ref m) if m.contains("limit")),
            "{err:?}"
        );

        filing.primary_doc_url =
            Some("https://example.com/Archives/edgar/data/1/x/exmp-20250930.htm".into());
        let err = p.filing_document(&filing).await.unwrap_err();
        assert_eq!(
            err,
            ProviderError::Unsupported {
                capability: Capability::Filings
            }
        );
        // Only the oversize attempt reached the server.
        assert_eq!(hits.lock().len(), before + 1);
    }

    #[tokio::test]
    async fn fundamentals_end_to_end() {
        let (p, hits) = provider_with(CONTACT).await;
        let req = FundamentalsRequest {
            key: SecurityKey::equity("EXMP"),
            period_type: PeriodType::Quarterly,
            periods: 4,
        };
        let f = p.fundamentals(&req).await.unwrap();
        assert_eq!(f.key, req.key);
        assert!(
            f.provenance
                .source_ref
                .as_deref()
                .unwrap()
                .ends_with("/api/xbrl/companyfacts/CIK0000000001.json")
        );
        assert!(!f.provenance.source_ref.as_deref().unwrap().contains("jane"));
        let inc: Vec<&str> = f
            .statements
            .iter()
            .filter(|s| s.kind == StatementKind::Income)
            .map(|s| s.fiscal_period.as_str())
            .collect();
        assert_eq!(inc, vec!["Q1", "Q4", "Q3", "Q2"]);
        assert_eq!(
            paths(&hits),
            vec![
                "/files/company_tickers.json",
                "/api/xbrl/companyfacts/CIK0000000001.json"
            ]
        );

        // A company without company facts → NotFound so the router can fall through.
        let missing = FundamentalsRequest {
            key: SecurityKey::equity("TSTX"),
            period_type: PeriodType::Annual,
            periods: 4,
        };
        assert!(matches!(
            p.fundamentals(&missing).await.unwrap_err(),
            ProviderError::NotFound(_)
        ));
    }

    #[tokio::test]
    async fn dividends_end_to_end() {
        let (p, hits) = provider_with(CONTACT).await;
        let key = SecurityKey::equity("SMPL.B");
        let d = p.dividends(&key).await.unwrap();
        assert_eq!(d.key, key);
        // Filings carry no dividend events, only per-period totals.
        assert!(d.dividends.is_empty());
        let fy: Vec<(i32, Option<f64>)> = d
            .per_period
            .iter()
            .filter(|x| x.period_type == PeriodType::Annual)
            .map(|x| (x.fiscal_year, x.declared_per_share))
            .collect();
        assert_eq!(fy, vec![(2025, Some(0.5)), (2024, Some(0.44))]);
        assert_eq!(d.reported_splits.len(), 1);
        assert_eq!(d.reported_splits[0].ratio, 2.0);
        assert_eq!(d.provenance.provider.as_str(), "edgar");
        assert!(!d.provenance.synthetic);
        assert!(
            d.provenance
                .source_ref
                .as_deref()
                .unwrap()
                .ends_with("/api/xbrl/companyfacts/CIK0000000002.json")
        );
        assert_eq!(
            paths(&hits),
            vec![
                "/files/company_tickers.json",
                "/api/xbrl/companyfacts/CIK0000000002.json"
            ]
        );

        // Fundamentals from the same facts carry the split disclosure.
        let f = p
            .fundamentals(&FundamentalsRequest {
                key: key.clone(),
                period_type: PeriodType::Annual,
                periods: 3,
            })
            .await
            .unwrap();
        assert_eq!(f.reported_splits, d.reported_splits);

        // A company that reports no dividends per share → NotFound, with a reason.
        let none = p.dividends(&SecurityKey::equity("EXMP")).await.unwrap_err();
        assert!(
            matches!(none, ProviderError::NotFound(ref m) if m.contains("no dividends per share")),
            "{none:?}"
        );
        let exmp = p
            .fundamentals(&FundamentalsRequest {
                key: SecurityKey::equity("EXMP"),
                period_type: PeriodType::Annual,
                periods: 3,
            })
            .await
            .unwrap();
        assert!(exmp.reported_splits.is_empty());
    }

    #[tokio::test]
    async fn every_request_takes_a_token() {
        let (p, hits) = provider_with(CONTACT).await;
        let key = SecurityKey::equity("EXMP");
        p.profile(&key).await.unwrap();
        p.filings(&FilingsRequest {
            key: Some(key.clone()),
            forms: vec![],
            limit: 50,
        })
        .await
        .unwrap();
        let _ = p
            .fundamentals(&FundamentalsRequest {
                key,
                period_type: PeriodType::Annual,
                periods: 2,
            })
            .await
            .unwrap();
        let sent = hits.lock().len() as u64;
        assert!(sent >= 4);
        assert_eq!(p.http_requests.load(Ordering::Relaxed), sent);
    }

    #[tokio::test(start_paused = true)]
    async fn internal_bucket_stays_under_ten_per_second() {
        let p = EdgarProvider::with_endpoints(
            EdgarConfig {
                contact: CONTACT.into(),
            },
            "http://127.0.0.1:9",
            "http://127.0.0.1:9",
        )
        .unwrap();
        let start = Instant::now();
        for _ in 0..11 {
            p.throttle().await;
        }
        // 11 requests can never fit in one second.
        assert!(
            start.elapsed() >= Duration::from_secs(1),
            "{:?}",
            start.elapsed()
        );
    }
}

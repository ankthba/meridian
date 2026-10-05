//! Press-release wires over public RSS/Atom feeds (GlobeNewswire, PR
//! Newswire, Business Wire).
//!
//! Headlines and summaries only, for personal reading; see `README.md` for
//! the verified feed URLs, each publisher's terms, and the ticker rules.
//! Every `news()` call reads all configured feeds concurrently (each feed's
//! response is reused for [`FEED_CACHE_TTL`] in memory) and filters locally.

#[allow(dead_code)] // Verbatim per-crate copy; `parse_json` is unused here.
mod http;
mod parse;
mod tickers;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use meridian_provider::{
    AiPolicy, CachePolicy, Capabilities, Capability, CapabilityEntry, NewsQuery, NewsScope,
    Provider, ProviderError, ProviderResult, RateLimit,
};
use meridian_types::{
    AssetClass, Clock, DataDelay, FeedSource, MarketSector, NewsItem, NewsPage, ProviderId,
    SecurityKey, SystemClock,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::time::Instant;

/// Stable provider id.
pub const PROVIDER_ID: &str = "rss";

/// How long one feed response is reused before it is fetched again. Keeps
/// repeated company-news lookups from re-downloading every feed, so each
/// publisher sees at most one request per feed per minute. In memory only;
/// nothing is persisted.
pub const FEED_CACHE_TTL: Duration = Duration::from_secs(60);

/// Attempts per feed download (first try plus retries) for transient errors.
const FETCH_ATTEMPTS: u32 = 3;
/// How long a news request waits for feeds before answering with those that
/// have arrived.
const RESPONSE_DEADLINE: Duration = Duration::from_secs(4);
/// Base delay between attempts; attempt `n` waits `n × RETRY_DELAY`.
const RETRY_DELAY: Duration = Duration::from_millis(250);

const DOCS_URL: &str = "https://www.globenewswire.com/rss/list";

/// Terminal-style exchange codes that denote a US listing. Company-scope
/// filtering only matches keys with no exchange or one of these.
const US_EXCHANGE_CODES: [&str; 9] = ["US", "UN", "UW", "UQ", "UR", "UA", "UP", "UF", "UV"];

/// One feed: a publisher label (shown as the item's `source`) and its URL.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RssFeed {
    /// Publisher label, e.g. `GlobeNewswire`.
    pub name: String,
    pub url: String,
}

impl RssFeed {
    #[must_use]
    pub fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
        }
    }

    /// GlobeNewswire, releases from public companies (last 20).
    #[must_use]
    pub fn globenewswire_public_companies() -> Self {
        Self::new(
            "GlobeNewswire",
            "https://www.globenewswire.com/RssFeed/orgclass/1/feedTitle/GlobeNewswire%20-%20News%20about%20Public%20Companies",
        )
    }

    /// GlobeNewswire, earnings releases and operating results (last 20).
    #[must_use]
    pub fn globenewswire_earnings() -> Self {
        Self::new(
            "GlobeNewswire",
            "https://www.globenewswire.com/RssFeed/subjectcode/13-Earnings%20Releases%20And%20Operating%20Results/feedTitle/GlobeNewswire%20-%20Earnings%20Releases%20And%20Operating%20Results",
        )
    }

    /// PR Newswire, all news releases (last 20).
    #[must_use]
    pub fn prnewswire_all() -> Self {
        Self::new(
            "PR Newswire",
            "https://www.prnewswire.com/rss/news-releases-list.rss",
        )
    }

    /// Business Wire, all-news channel via the media-RSS path (`/mrss/`), the
    /// one its robots.txt permits (`/rss/` is disallowed for all agents). It
    /// carries only releases with multimedia: about 76 items over the last
    /// week, about 100 KB.
    #[must_use]
    pub fn businesswire_all() -> Self {
        Self::new(
            "Business Wire",
            "https://feed.businesswire.com/mrss/home/?rss=G1QFDERJXkJcFVJYWQ==",
        )
    }
}

/// Which feeds to read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RssConfig {
    pub feeds: Vec<RssFeed>,
}

impl Default for RssConfig {
    /// The feeds verified on 2026-10-05 (see `README.md`).
    fn default() -> Self {
        Self {
            feeds: vec![
                RssFeed::globenewswire_public_companies(),
                RssFeed::globenewswire_earnings(),
                RssFeed::prnewswire_all(),
                RssFeed::businesswire_all(),
            ],
        }
    }
}

struct CachedFeed {
    fetched: Instant,
    items: Arc<Vec<NewsItem>>,
}

/// One configured feed and its short-lived response cache. The mutex is held
/// across a fetch so concurrent callers share one download.
struct FeedSlot {
    feed: RssFeed,
    cache: Mutex<Option<CachedFeed>>,
}

pub struct RssProvider {
    client: reqwest::Client,
    slots: Vec<Arc<FeedSlot>>,
    caps: Capabilities,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for RssProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RssProvider")
            .field(
                "feeds",
                &self.slots.iter().map(|s| &s.feed).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

fn capabilities() -> Capabilities {
    Capabilities {
        entries: vec![CapabilityEntry {
            capability: Capability::News,
            asset_classes: vec![AssetClass::Equity, AssetClass::Etf],
            delay: DataDelay::RealTime,
            source: FeedSource::Aggregated,
            history: Some(
                "latest items in each feed only (20 per feed; Business Wire's multimedia feed about 76 over a week)"
                    .into(),
            ),
        }],
        rate_limit: Some(RateLimit::per_minute(30)),
        max_stream_symbols: None,
        // Business Wire's terms forbid storing or aggregating site content and
        // PR Newswire's forbid database storage; display live, keep nothing.
        cache_policy: CachePolicy::NoStore,
        attribution: None,
        display_allowed: true,
        ai_policy: AiPolicy::Unreviewed,
        requires_credentials: false,
        terms_note: "Public press-release RSS feeds, headlines and summaries only, for personal non-commercial \
                     reading; each item links to the publisher's page. PR Newswire's terms limit use to personal, \
                     noncommercial use and forbid redistribution, scraping, database storage and AI training; \
                     Business Wire's terms permit retrieving RSS feeds and reading releases but forbid storing, \
                     aggregating or redistributing them; GlobeNewswire publishes no separate RSS terms. Not \
                     persisted."
            .into(),
        docs_url: DOCS_URL.into(),
    }
}

fn validate_feed(feed: &RssFeed) -> ProviderResult<()> {
    let bad = |why: &str| {
        ProviderError::parse(format!(
            "RSS feed configuration: {why} ({:?}, {:?})",
            feed.name, feed.url
        ))
    };
    if feed.name.trim().is_empty() {
        return Err(bad("empty feed name"));
    }
    let url = reqwest::Url::parse(feed.url.trim()).map_err(|_| bad("invalid URL"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(bad("URL must be http(s) with a host"));
    }
    Ok(())
}

impl RssProvider {
    /// Builds the provider. Fails if the feed list is empty or any feed has a
    /// blank name or a non-http(s) URL (`ProviderError::Parse` naming the
    /// feed).
    pub fn new(config: RssConfig) -> ProviderResult<Self> {
        if config.feeds.is_empty() {
            return Err(ProviderError::parse(
                "RSS feed configuration: no feeds configured",
            ));
        }
        for feed in &config.feeds {
            validate_feed(feed)?;
        }
        let slots = config
            .feeds
            .into_iter()
            .map(|feed| {
                Arc::new(FeedSlot {
                    feed: RssFeed::new(feed.name.trim(), feed.url.trim()),
                    cache: Mutex::new(None),
                })
            })
            .collect();
        Ok(Self {
            client: http::client()?,
            slots,
            caps: capabilities(),
            clock: Arc::new(SystemClock),
        })
    }

    /// Replaces the clock used for `received_at` / `as_of` (tests).
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// The configured feeds, in order.
    pub fn feeds(&self) -> impl Iterator<Item = &RssFeed> {
        self.slots.iter().map(|s| &s.feed)
    }

    /// Reads every feed concurrently. Returns the items of the feeds that
    /// answered within [`RESPONSE_DEADLINE`]; fails only if none did (with
    /// the first feed's error, in configuration order). A feed still loading
    /// at the deadline keeps going in the background and fills its cache for
    /// the next request, so one slow publisher never stalls the screen.
    async fn fetch_all(&self) -> ProviderResult<Vec<NewsItem>> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(self.slots.len().max(1));
        for (idx, slot) in self.slots.iter().enumerate() {
            let (client, slot, clock, tx) = (
                self.client.clone(),
                Arc::clone(slot),
                Arc::clone(&self.clock),
                tx.clone(),
            );
            tokio::spawn(async move {
                let res = load_feed(&client, &slot, clock.as_ref()).await;
                let _ = tx.send((idx, res)).await;
            });
        }
        drop(tx);
        let mut results: Vec<Option<ProviderResult<Arc<Vec<NewsItem>>>>> =
            vec![None; self.slots.len()];
        let deadline = Instant::now() + RESPONSE_DEADLINE;
        while let Ok(Some((idx, res))) = tokio::time::timeout_at(deadline, rx.recv()).await {
            results[idx] = Some(res);
        }
        merge_feed_results(self.slots.iter().zip(results).map(|(slot, r)| {
            let r = r.unwrap_or_else(|| {
                Err(ProviderError::Upstream(format!(
                    "{}: no response within {} s",
                    slot.feed.name,
                    RESPONSE_DEADLINE.as_secs()
                )))
            });
            (slot.feed.name.as_str(), r)
        }))
    }
}

/// Whether one fetch failure is worth an immediate retry. 404 is included
/// because PR Newswire's edge intermittently 301-redirects its feed to a
/// trailing-slash URL that 404s (about one request in three on 2026-10-05);
/// the next request normally succeeds. Rate limiting is never retried here.
fn retry_fetch(e: &ProviderError) -> bool {
    match e {
        ProviderError::NotFound(_) => true,
        ProviderError::RateLimited { .. } => false,
        other => other.is_retryable(),
    }
}

/// Downloads a feed body, retrying transient failures up to
/// [`FETCH_ATTEMPTS`] times in total.
async fn fetch_body(client: &reqwest::Client, feed: &RssFeed) -> ProviderResult<String> {
    let mut attempt = 1;
    loop {
        let req = client.get(&feed.url).header(
            reqwest::header::ACCEPT,
            "application/rss+xml, application/atom+xml, application/xml;q=0.9, text/xml;q=0.8, */*;q=0.5",
        );
        match http::send_text(req).await {
            Ok(body) => return Ok(body),
            Err(e) if attempt < FETCH_ATTEMPTS && retry_fetch(&e) => {
                tracing::debug!(feed = %feed.name, attempt, error = %e, "retrying feed fetch");
                tokio::time::sleep(RETRY_DELAY * attempt).await;
                attempt += 1;
            }
            Err(e) => return Err(with_feed(&feed.name, e)),
        }
    }
}

/// Returns a feed's items, from the in-memory cache if fresh.
async fn load_feed(
    client: &reqwest::Client,
    slot: &FeedSlot,
    clock: &dyn Clock,
) -> ProviderResult<Arc<Vec<NewsItem>>> {
    let mut cache = slot.cache.lock().await;
    if let Some(c) = cache.as_ref()
        && c.fetched.elapsed() < FEED_CACHE_TTL
    {
        return Ok(Arc::clone(&c.items));
    }
    let body = fetch_body(client, &slot.feed).await?;
    let fetched_at = clock.now();
    let feed = slot.feed.clone();
    // A large feed takes tens of milliseconds to parse; keep it off the
    // async workers.
    let items = tokio::task::spawn_blocking(move || parse::parse_items(&body, &feed, fetched_at))
        .await
        .map_err(|e| {
            ProviderError::Upstream(format!("{}: parser task failed: {e}", slot.feed.name))
        })??;
    let items = Arc::new(items);
    *cache = Some(CachedFeed {
        fetched: Instant::now(),
        items: Arc::clone(&items),
    });
    Ok(items)
}

/// Prefixes an error message with the feed name so logs and the all-failed
/// error say which feed broke.
fn with_feed(name: &str, e: ProviderError) -> ProviderError {
    match e {
        ProviderError::Network(m) => ProviderError::Network(format!("{name}: {m}")),
        ProviderError::Unauthorized(m) => ProviderError::Unauthorized(format!("{name}: {m}")),
        ProviderError::NotFound(m) => ProviderError::NotFound(format!("{name}: {m}")),
        ProviderError::Upstream(m) => ProviderError::Upstream(format!("{name}: {m}")),
        ProviderError::Http {
            status,
            body_snippet,
        } => ProviderError::Http {
            status,
            body_snippet: format!("{name}: {body_snippet}"),
        },
        other => other,
    }
}

/// Combines per-feed results: successful feeds' items, failures logged; if
/// every feed failed, the first failure.
fn merge_feed_results<'a>(
    results: impl IntoIterator<Item = (&'a str, ProviderResult<Arc<Vec<NewsItem>>>)>,
) -> ProviderResult<Vec<NewsItem>> {
    let mut items = Vec::new();
    let mut any_ok = false;
    let mut first_err = None;
    for (name, res) in results {
        match res {
            Ok(feed_items) => {
                any_ok = true;
                items.extend(feed_items.iter().cloned());
            }
            Err(e) => {
                tracing::warn!(feed = name, error = %e, "RSS feed failed");
                first_err.get_or_insert(e);
            }
        }
    }
    match (any_ok, first_err) {
        (false, Some(e)) => Err(e),
        _ => Ok(items),
    }
}

/// Normalised US symbols of the equity keys a company query can match.
fn company_symbols(keys: &[SecurityKey]) -> HashSet<String> {
    keys.iter()
        .filter(|k| k.sector == MarketSector::Equity)
        .filter(|k| {
            k.exchange
                .as_deref()
                .is_none_or(|ex| US_EXCHANGE_CODES.contains(&ex))
        })
        .map(|k| tickers::normalize_symbol(&k.symbol))
        .collect()
}

/// Applies the query to fetched items: company tickers, text, time window
/// (`from` inclusive, `to` exclusive), newest first, de-duplicated by URL and
/// then by (headline, source), truncated to `limit`.
fn select_items(
    mut items: Vec<NewsItem>,
    q: &NewsQuery,
    symbols: Option<&HashSet<String>>,
) -> Vec<NewsItem> {
    let needle = q
        .text
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase);
    items.retain(|it| {
        symbols.is_none_or(|wanted| it.tickers.iter().any(|t| wanted.contains(t)))
            && q.from.is_none_or(|from| it.published_at >= from)
            && q.to.is_none_or(|to| it.published_at < to)
            && needle.as_deref().is_none_or(|n| {
                it.headline.to_lowercase().contains(n)
                    || it
                        .summary
                        .as_deref()
                        .is_some_and(|s| s.to_lowercase().contains(n))
            })
    });
    items.sort_by(|a, b| {
        b.published_at
            .cmp(&a.published_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut seen_urls = HashSet::new();
    let mut seen_heads = HashSet::new();
    items.retain(|it| {
        if let Some(u) = &it.url
            && !seen_urls.insert(u.clone())
        {
            return false;
        }
        seen_heads.insert((it.headline.clone(), it.source.clone()))
    });
    items.truncate(q.limit);
    items
}

#[async_trait]
impl Provider for RssProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(PROVIDER_ID)
    }

    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    async fn news(&self, q: &NewsQuery) -> ProviderResult<NewsPage> {
        let symbols = match q.scope {
            NewsScope::Top | NewsScope::Market => {
                return Err(ProviderError::Unsupported {
                    capability: Capability::News,
                });
            }
            NewsScope::PressReleases => None,
            NewsScope::Company => {
                let symbols = company_symbols(&q.keys);
                if symbols.is_empty() {
                    // Only US equity listings can be matched; nothing to fetch.
                    return Ok(NewsPage {
                        items: Vec::new(),
                        next: None,
                    });
                }
                Some(symbols)
            }
        };
        let items = self.fetch_all().await?;
        Ok(NewsPage {
            items: select_items(items, q, symbols.as_ref()),
            next: None,
        })
    }
}

#[cfg(test)]
mod tests;

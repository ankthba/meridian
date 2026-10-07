//! FRED® (Federal Reserve Economic Data, Federal Reserve Bank of St. Louis):
//! economic series and the release calendar.
//!
//! Built against the FRED API v1 docs (checked 2026-10-05); see `README.md`
//! for the exact pages, the terms notes, and the mapping decisions.
//!
//! The API key travels as the `api_key` query parameter (the only method v1
//! documents). It is added to the request last, never to the URL kept for
//! provenance, and it is redacted from every error message.

mod http;
mod normalize;
#[cfg(test)]
mod test_server;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use async_trait::async_trait;
use meridian_provider::{
    AiPolicy, CachePolicy, CalendarRequest, Capabilities, Capability, CapabilityEntry, Provider, ProviderError,
    ProviderResult, RateLimit, SeriesRequest, TokenBucket,
};
use meridian_types::{
    AssetClass, Clock, DataDelay, EconomicEvent, EconomicSeries, FeedSource, Provenance, ProviderId, SystemClock,
    UnixNanos,
};
use secrecy::{ExposeSecret, SecretString};

/// Stable provider ID.
pub const PROVIDER_ID: &str = "fred";

/// The notice FRED's terms require "prominently on your application"
/// (verbatim from <https://fred.stlouisfed.org/docs/api/terms_of_use.html>,
/// checked 2026-10-05).
pub const ATTRIBUTION: &str =
    "This product uses the FRED® API but is not endorsed or certified by the Federal Reserve Bank of St. Louis.";

/// API documentation this crate was built against.
pub const DOCS_URL: &str = "https://fred.stlouisfed.org/docs/api/fred/";

const BASE_URL: &str = "https://api.stlouisfed.org/fred";
const MISSING_KEY: &str = "FRED API key not set — add it in Settings";

/// Documented on `fred/errors.html`: "Up to 120 requests per minute".
const REQUESTS_PER_MINUTE: u32 = 120;

/// `fred/series/observations` `limit` maximum (documented 1–100000).
pub(crate) const OBSERVATIONS_PAGE_LIMIT: u64 = 100_000;
/// `fred/releases/dates` `limit` maximum (documented 1–1000).
pub(crate) const RELEASE_DATES_PAGE_LIMIT: u64 = 1_000;
/// Safety caps; hitting one returns an error rather than partial data.
const MAX_OBSERVATION_PAGES: usize = 10;
const MAX_RELEASE_DATE_PAGES: usize = 50;

const TERMS_NOTE: &str = "Free with a FRED API key. The FRED notice must be shown wherever FRED data appears. \
Some series are owned by third parties and copyrighted (their notes contain 'Copyright'); the terms allow \
those only for your own personal use. Values update when each source publishes a release; release dates are \
the sources' schedules and may precede availability on FRED. Rate limit 120 requests/minute. The terms say \
nothing about AI processing, so ASK use is unreviewed.";

/// Configuration. The key comes from the Keychain via the engine.
#[derive(Debug, Clone, Default)]
pub struct FredConfig {
    pub api_key: Option<SecretString>,
}

/// FRED provider: `EconomicSeries` and `EconomicCalendar`.
pub struct FredProvider {
    client: reqwest::Client,
    api_key: Option<SecretString>,
    base_url: String,
    caps: Capabilities,
    /// Paces individual HTTP requests (one routed call can make several).
    bucket: TokenBucket,
    clock: Arc<dyn Clock>,
}

impl FredProvider {
    pub fn new(config: FredConfig) -> ProviderResult<Self> {
        Self::build(config, BASE_URL.to_owned(), Arc::new(SystemClock))
    }

    fn build(config: FredConfig, base_url: String, clock: Arc<dyn Clock>) -> ProviderResult<Self> {
        let rate_limit = RateLimit::per_minute(REQUESTS_PER_MINUTE);
        Ok(Self {
            client: http::client()?,
            api_key: config.api_key,
            base_url,
            caps: capabilities(rate_limit),
            bucket: TokenBucket::new(rate_limit),
            clock,
        })
    }

    /// The configured key, or `Unauthorized` without touching the network.
    fn key(&self) -> ProviderResult<&str> {
        self.api_key
            .as_ref()
            .map(|k| k.expose_secret().trim())
            .filter(|k| !k.is_empty())
            .ok_or_else(|| ProviderError::Unauthorized(MISSING_KEY.into()))
    }

    /// GETs `{base}/{path}?{params}&api_key=…`. Returns the body and the
    /// request URL *without* the key, for provenance.
    async fn get(&self, path: &str, params: &[(&str, String)]) -> ProviderResult<(String, String)> {
        let key = self.key()?;
        let url = reqwest::Url::parse_with_params(&format!("{}/{path}", self.base_url), params)
            .map_err(|e| ProviderError::Upstream(format!("invalid FRED request URL: {e}")))?;
        let source_ref = url.to_string();
        self.bucket.acquire().await;
        let request = self.client.get(url).query(&[("api_key", key)]);
        let body = http::send_text(request).await.map_err(|e| normalize::map_error(e, key))?;
        Ok((body, source_ref))
    }

    async fn fetch_series(&self, req: &SeriesRequest) -> ProviderResult<EconomicSeries> {
        let id = req.id.trim();
        if id.is_empty() {
            return Err(ProviderError::NotFound("empty FRED series id".into()));
        }
        let fetched_at = self.clock.now();
        let (meta_body, _) =
            self.get("series", &[("series_id", id.to_owned()), ("file_type", "json".to_owned())]).await?;
        let meta = normalize::parse_series_meta(&meta_body, id)?;

        let mut params = vec![
            ("series_id", id.to_owned()),
            ("file_type", "json".to_owned()),
            ("sort_order", "asc".to_owned()),
            ("limit", OBSERVATIONS_PAGE_LIMIT.to_string()),
        ];
        if let Some(from) = req.from {
            params.push(("observation_start", from.format("%Y-%m-%d").to_string()));
        }
        if let Some(to) = req.to {
            params.push(("observation_end", to.format("%Y-%m-%d").to_string()));
        }

        let mut observations = Vec::new();
        let mut source_ref = None;
        let mut offset = 0_u64;
        for page_no in 1..=MAX_OBSERVATION_PAGES {
            let mut page_params = params.clone();
            page_params.push(("offset", offset.to_string()));
            let (body, url) = self.get("series/observations", &page_params).await?;
            source_ref.get_or_insert(url);
            let page = normalize::parse_observations_page(&body)?;
            let received = page.observations.len() as u64;
            observations.extend(page.observations);
            match normalize::next_offset(page.count, page.offset, received) {
                None => {
                    let source_ref = source_ref.unwrap_or_default();
                    return normalize::build_series(meta, observations, fetched_at, |as_of| {
                        provenance(as_of, source_ref)
                    });
                }
                Some(next) if page_no < MAX_OBSERVATION_PAGES => offset = next,
                Some(_) => break,
            }
        }
        Err(ProviderError::Upstream(format!(
            "FRED series {id} has more than {MAX_OBSERVATION_PAGES} pages of observations; narrow the date range"
        )))
    }

    async fn fetch_calendar(&self, req: &CalendarRequest) -> ProviderResult<Vec<EconomicEvent>> {
        let fetched_at = self.clock.now();
        let params = vec![
            ("realtime_start", req.from.format("%Y-%m-%d").to_string()),
            ("realtime_end", req.to.format("%Y-%m-%d").to_string()),
            ("include_release_dates_with_no_data", "true".to_owned()),
            ("file_type", "json".to_owned()),
            ("limit", RELEASE_DATES_PAGE_LIMIT.to_string()),
            ("order_by", "release_date".to_owned()),
            ("sort_order", "asc".to_owned()),
        ];

        let mut events = Vec::new();
        let mut offset = 0_u64;
        for page_no in 1..=MAX_RELEASE_DATE_PAGES {
            let mut page_params = params.clone();
            page_params.push(("offset", offset.to_string()));
            let (body, url) = self.get("releases/dates", &page_params).await?;
            let page = normalize::parse_release_dates_page(&body)?;
            let received = page.release_dates.len() as u64;
            for dto in &page.release_dates {
                let event = normalize::release_event(dto, provenance(fetched_at, url.clone()))?;
                // Guard: keep only dates inside the requested window.
                let date = meridian_types::nanos_to_date(event.release_time);
                if date >= req.from && date <= req.to {
                    events.push(event);
                }
            }
            match normalize::next_offset(page.count, page.offset, received) {
                None => {
                    normalize::demote_daily_releases(&mut events);
                    events.sort_by(|a, b| a.release_time.cmp(&b.release_time).then_with(|| a.event.cmp(&b.event)));
                    return Ok(events);
                }
                Some(next) if page_no < MAX_RELEASE_DATE_PAGES => offset = next,
                Some(_) => break,
            }
        }
        Err(ProviderError::Upstream(format!(
            "FRED release calendar has more than {} entries for this range; narrow the date range",
            MAX_RELEASE_DATE_PAGES as u64 * RELEASE_DATES_PAGE_LIMIT
        )))
    }
}

/// Provenance for FRED payloads: official, updated on release, with the
/// mandatory notice. `source_ref` is the request URL without the key.
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

fn capabilities(rate_limit: RateLimit) -> Capabilities {
    Capabilities {
        entries: vec![
            CapabilityEntry {
                capability: Capability::EconomicSeries,
                asset_classes: vec![AssetClass::Economic],
                delay: DataDelay::EndOfDay,
                source: FeedSource::Official,
                history: Some("full series history; updated on each source release".into()),
            },
            CapabilityEntry {
                capability: Capability::EconomicCalendar,
                asset_classes: vec![AssetClass::Economic],
                delay: DataDelay::EndOfDay,
                source: FeedSource::Official,
                history: Some("release dates incl. scheduled future dates; dates only, no times or consensus".into()),
            },
        ],
        rate_limit: Some(rate_limit),
        max_stream_symbols: None,
        cache_policy: CachePolicy::Unrestricted,
        attribution: Some(ATTRIBUTION.to_owned()),
        display_allowed: true,
        ai_policy: AiPolicy::Unreviewed,
        requires_credentials: true,
        terms_note: TERMS_NOTE.to_owned(),
        docs_url: DOCS_URL.to_owned(),
    }
}

#[async_trait]
impl Provider for FredProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(PROVIDER_ID)
    }

    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    async fn economic_series(&self, req: &SeriesRequest) -> ProviderResult<EconomicSeries> {
        self.key()?;
        self.fetch_series(req).await
    }

    /// FRED's calendar is US-centric: a request whose country filter is
    /// non-empty and excludes `US` gets an empty list.
    async fn economic_calendar(&self, req: &CalendarRequest) -> ProviderResult<Vec<EconomicEvent>> {
        self.key()?;
        if !req.countries.is_empty() && !req.countries.iter().any(|c| c.trim().eq_ignore_ascii_case("US")) {
            return Ok(Vec::new());
        }
        if req.from > req.to {
            return Ok(Vec::new());
        }
        self.fetch_calendar(req).await
    }
}

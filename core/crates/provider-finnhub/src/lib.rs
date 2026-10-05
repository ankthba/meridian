//! Finnhub free tier: company and market news, recommendation trends,
//! company profile, and EPS surprises.
//!
//! Built against the Finnhub API docs (checked 2026-10-05); see `README.md`
//! for the exact sources, what is free, the terms, and mapping decisions.
//!
//! The API key travels in the `X-Finnhub-Token` header (documented as an
//! alternative to the `token` query parameter), marked sensitive, so request
//! URLs never contain it. It is still redacted from every error message.

mod http;
mod normalize;
#[cfg(test)]
mod test_server;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use async_trait::async_trait;
use meridian_provider::{
    AiPolicy, CachePolicy, Capabilities, Capability, CapabilityEntry, NewsQuery, NewsScope, Provider, ProviderError,
    ProviderResult, RateLimit, TokenBucket,
};
use meridian_types::{
    AssetClass, Clock, CompanyProfile, DataDelay, EarningsHistory, FeedSource, NewsPage, ProviderId, Recommendations,
    SecurityKey, SystemClock,
};
use reqwest::header::HeaderValue;
use secrecy::{ExposeSecret, SecretString};

/// Stable provider ID.
pub const PROVIDER_ID: &str = "finnhub";

/// API documentation this crate was built against.
pub const DOCS_URL: &str = "https://finnhub.io/docs/api";

const BASE_URL: &str = "https://finnhub.io/api/v1";
const MISSING_KEY: &str = "Finnhub API key not set — add it in Settings";
const TOKEN_HEADER: &str = "X-Finnhub-Token";

/// Free plan: "60 API calls/minute" (finnhub.io/pricing); every plan also
/// has a 30 calls/second cap, which this is well under.
const REQUESTS_PER_MINUTE: u32 = 60;

/// `/news` category for general market news.
const MARKET_NEWS_CATEGORY: &str = "general";

const TERMS_NOTE: &str = "Free plan, personal use only: the terms forbid redistributing or sharing data \"or \
derived results\" with anyone without Finnhub's written approval, and all data must be deleted when the \
subscription ends (Meridian purges it). Free tier: 60 calls/minute (plus a 30 calls/second cap on every plan); \
company news covers North American companies with 1 year of history; EPS surprises cover the last 4 quarters; \
no price targets. No attribution requirement. AI use is unreviewed: the no-sharing clause may cover sending data \
to an AI service.";

/// Configuration. The key comes from the Keychain via the engine.
#[derive(Debug, Clone, Default)]
pub struct FinnhubConfig {
    pub api_key: Option<SecretString>,
}

/// Finnhub provider (free-tier endpoints only).
pub struct FinnhubProvider {
    client: reqwest::Client,
    api_key: Option<SecretString>,
    base_url: String,
    caps: Capabilities,
    /// Paces individual HTTP requests (company news makes one per symbol).
    bucket: TokenBucket,
    clock: Arc<dyn Clock>,
}

impl FinnhubProvider {
    pub fn new(config: FinnhubConfig) -> ProviderResult<Self> {
        Self::build(config, BASE_URL.to_owned(), Arc::new(SystemClock))
    }

    fn build(config: FinnhubConfig, base_url: String, clock: Arc<dyn Clock>) -> ProviderResult<Self> {
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

    /// GETs `{base}/{path}?{params}` with the key in a sensitive header.
    /// Returns the body and the request URL (which never holds the key).
    async fn get(
        &self,
        path: &str,
        params: &[(&str, String)],
        capability: Capability,
    ) -> ProviderResult<(String, String, &str)> {
        let key = self.key()?;
        let url = reqwest::Url::parse_with_params(&format!("{}/{path}", self.base_url), params)
            .map_err(|e| ProviderError::Upstream(format!("invalid Finnhub request URL: {e}")))?;
        let source_ref = url.to_string();
        let mut token = HeaderValue::from_str(key).map_err(|_| {
            ProviderError::Unauthorized("Finnhub API key contains characters not allowed in an HTTP header".into())
        })?;
        token.set_sensitive(true);
        self.bucket.acquire().await;
        let request = self.client.get(url).header(TOKEN_HEADER, token);
        let body = http::send_text(request).await.map_err(|e| normalize::map_error(e, key, capability))?;
        Ok((body, source_ref, key))
    }

    async fn company_news(&self, q: &NewsQuery) -> ProviderResult<NewsPage> {
        self.key()?;
        if q.keys.is_empty() {
            return Ok(NewsPage { items: Vec::new(), next: None });
        }
        let mut symbols: Vec<String> = Vec::new();
        for k in &q.keys {
            if let Some(sym) = normalize::finnhub_symbol(k)
                && !symbols.contains(&sym)
            {
                symbols.push(sym);
            }
        }
        if symbols.is_empty() {
            return Err(normalize::not_covered(&q.keys[0]));
        }
        let now = self.clock.now();
        let (from, to) = normalize::news_window(q.from, q.to, now);
        let mut items = Vec::new();
        for sym in &symbols {
            let params = [
                ("symbol", sym.clone()),
                ("from", from.format("%Y-%m-%d").to_string()),
                ("to", to.format("%Y-%m-%d").to_string()),
            ];
            let (body, url, key) = self.get("company-news", &params, Capability::News).await?;
            let dtos: Vec<normalize::NewsDto> =
                normalize::parse_body(&body, "Finnhub /company-news response", key, Capability::News)?;
            items.extend(dtos.into_iter().filter_map(|d| normalize::news_item(d, Some(sym), now, &url)));
        }
        Ok(NewsPage { items: normalize::finish_news(items, q), next: None })
    }

    async fn market_news(&self, q: &NewsQuery) -> ProviderResult<NewsPage> {
        self.key()?;
        let now = self.clock.now();
        let params = [("category", MARKET_NEWS_CATEGORY.to_owned())];
        let (body, url, key) = self.get("news", &params, Capability::News).await?;
        let dtos: Vec<normalize::NewsDto> =
            normalize::parse_body(&body, "Finnhub /news response", key, Capability::News)?;
        let items = dtos.into_iter().filter_map(|d| normalize::news_item(d, None, now, &url)).collect();
        Ok(NewsPage { items: normalize::finish_news(items, q), next: None })
    }

    fn symbol(&self, key: &SecurityKey) -> ProviderResult<String> {
        self.key()?;
        normalize::finnhub_symbol(key).ok_or_else(|| normalize::not_covered(key))
    }
}

fn capabilities(rate_limit: RateLimit) -> Capabilities {
    let entry = |capability, asset_classes, delay, history: Option<&str>| CapabilityEntry {
        capability,
        asset_classes,
        delay,
        source: FeedSource::Aggregated,
        history: history.map(str::to_owned),
    };
    Capabilities {
        entries: vec![
            entry(
                Capability::News,
                vec![AssetClass::Equity, AssetClass::Etf],
                // Finnhub labels these endpoints real-time; latency is unverified.
                DataDelay::RealTime,
                Some("company news: 1 year, North American companies (free tier); market news: latest only"),
            ),
            entry(Capability::Profile, vec![AssetClass::Equity, AssetClass::Etf], DataDelay::EndOfDay, None),
            entry(
                Capability::Recommendations,
                vec![AssetClass::Equity],
                DataDelay::EndOfDay,
                Some("monthly recommendation-count periods; no price targets on the free tier"),
            ),
            entry(
                Capability::Earnings,
                vec![AssetClass::Equity],
                DataDelay::EndOfDay,
                Some("EPS actual vs estimate, last 4 quarters (free tier)"),
            ),
        ],
        rate_limit: Some(rate_limit),
        max_stream_symbols: None,
        cache_policy: CachePolicy::PurgeOnUnsubscribe,
        attribution: None,
        display_allowed: true,
        ai_policy: AiPolicy::Unreviewed,
        requires_credentials: true,
        terms_note: TERMS_NOTE.to_owned(),
        docs_url: DOCS_URL.to_owned(),
    }
}

#[async_trait]
impl Provider for FinnhubProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(PROVIDER_ID)
    }

    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    /// Company news (`/company-news`, one call per US symbol) and general
    /// market news (`/news?category=general`). Top stories and press
    /// releases are not offered on the free tier.
    async fn news(&self, q: &NewsQuery) -> ProviderResult<NewsPage> {
        match q.scope {
            NewsScope::Company => self.company_news(q).await,
            NewsScope::Market => self.market_news(q).await,
            NewsScope::Top | NewsScope::PressReleases => {
                Err(ProviderError::Unsupported { capability: Capability::News })
            }
        }
    }

    async fn profile(&self, key: &SecurityKey) -> ProviderResult<CompanyProfile> {
        let sym = self.symbol(key)?;
        let (body, _, api_key) = self.get("stock/profile2", &[("symbol", sym)], Capability::Profile).await?;
        let dto = normalize::parse_body(&body, "Finnhub /stock/profile2 response", api_key, Capability::Profile)?;
        normalize::profile(key, dto)
    }

    async fn recommendations(&self, key: &SecurityKey) -> ProviderResult<Recommendations> {
        let sym = self.symbol(key)?;
        let cap = Capability::Recommendations;
        let (body, url, api_key) = self.get("stock/recommendation", &[("symbol", sym)], cap).await?;
        let periods = normalize::parse_body(&body, "Finnhub /stock/recommendation response", api_key, cap)?;
        normalize::recommendations(key, periods, &url)
    }

    async fn earnings(&self, key: &SecurityKey) -> ProviderResult<EarningsHistory> {
        let sym = self.symbol(key)?;
        let fetched_at = self.clock.now();
        let cap = Capability::Earnings;
        let (body, url, api_key) = self.get("stock/earnings", &[("symbol", sym)], cap).await?;
        let rows = normalize::parse_body(&body, "Finnhub /stock/earnings response", api_key, cap)?;
        normalize::earnings(key, rows, fetched_at, &url)
    }
}

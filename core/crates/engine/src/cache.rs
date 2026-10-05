//! Read-through cache that honors each provider's `CachePolicy`, and serves
//! stale data (clearly flagged) when the network is down — the basis of
//! offline mode.

use std::time::Duration;

use meridian_provider::{CachePolicy, ProviderError, ProviderResult, Routed};
use meridian_types::{ProviderId, UnixNanos, NANOS_PER_SEC};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::core::Engine;
use crate::error::{EngineError, EngineResult};

/// Value plus whether it came from cache and is past its TTL.
#[derive(Debug, Clone)]
pub struct Fetched<T> {
    pub value: T,
    pub provider: String,
    pub from_cache: bool,
    pub stale: bool,
    pub fetched_at: UnixNanos,
}

impl Engine {
    fn cache_policy(&self, provider: &ProviderId) -> CachePolicy {
        self.router()
            .capabilities()
            .into_iter()
            .find(|(id, _)| id == provider)
            .map_or(CachePolicy::NoStore, |(_, c)| c.cache_policy)
    }

    /// Fetches through the router, caching JSON under `(dataset, key)`.
    ///
    /// - Fresh cache hit → returned without a network call.
    /// - Miss/expired → fetch; store according to the provider's policy.
    /// - Fetch fails with a network/server error and a stale copy exists →
    ///   the stale copy is returned with `stale = true`.
    pub(crate) async fn cached<T, F>(&self, dataset: &str, key: &str, ttl: Duration, fetch: F) -> EngineResult<Fetched<T>>
    where
        T: Serialize + DeserializeOwned + Clone,
        F: std::future::Future<Output = ProviderResult<Routed<T>>>,
    {
        let now = self.now();
        let cached = self.stores().market.get_blob(dataset, key, now).ok().flatten();
        if let Some(c) = &cached
            && !c.expired
            && let Ok(v) = serde_json::from_str::<T>(&c.json)
        {
            return Ok(Fetched { value: v, provider: c.provider.clone(), from_cache: true, stale: false, fetched_at: c.fetched_at });
        }
        match fetch.await {
            Ok(routed) => {
                let policy = self.cache_policy(&routed.provider);
                let ttl_ns = i64::try_from(ttl.as_nanos()).unwrap_or(i64::MAX);
                let expires = match policy {
                    CachePolicy::NoStore => None,
                    CachePolicy::MaxAge(d) => Some(now + ttl_ns.min(i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))),
                    CachePolicy::Unrestricted | CachePolicy::PurgeOnUnsubscribe => Some(now + ttl_ns),
                };
                if policy != CachePolicy::NoStore
                    && let Ok(json) = serde_json::to_string(&routed.value)
                    && let Err(e) = self.stores().market.put_blob(dataset, routed.provider.as_str(), key, &json, now, expires)
                {
                    tracing::warn!(error = %e, dataset, "cache write failed");
                }
                Ok(Fetched { value: routed.value, provider: routed.provider.to_string(), from_cache: false, stale: false, fetched_at: now })
            }
            Err(e) => {
                let offline = matches!(e, ProviderError::Network(_) | ProviderError::Http { .. } | ProviderError::RateLimited { .. });
                if offline
                    && let Some(c) = cached
                    && let Ok(v) = serde_json::from_str::<T>(&c.json)
                {
                    tracing::info!(dataset, key, "serving stale cache after fetch error");
                    return Ok(Fetched { value: v, provider: c.provider, from_cache: true, stale: true, fetched_at: c.fetched_at });
                }
                Err(EngineError::Provider(e))
            }
        }
    }
}

/// Standard TTLs.
#[allow(dead_code)] // some TTLs are used by screens added in later phases
pub(crate) mod ttl {
    use std::time::Duration;

    pub const PROFILE: Duration = Duration::from_secs(7 * 86_400);
    pub const FUNDAMENTALS: Duration = Duration::from_secs(86_400);
    pub const ESTIMATES: Duration = Duration::from_secs(6 * 3_600);
    pub const HOLDERS: Duration = Duration::from_secs(86_400);
    pub const FILINGS: Duration = Duration::from_secs(15 * 60);
    pub const FILING_DOC: Duration = Duration::from_secs(30 * 86_400);
    pub const NEWS: Duration = Duration::from_secs(60);
    pub const SERIES: Duration = Duration::from_secs(6 * 3_600);
    pub const CALENDAR: Duration = Duration::from_secs(3_600);
    pub const CURVE: Duration = Duration::from_secs(3_600);
    pub const CHAIN: Duration = Duration::from_secs(60);
}

/// Seconds → nanos helper for TTL math in tests.
#[allow(dead_code)]
pub(crate) fn secs(n: i64) -> UnixNanos {
    n * NANOS_PER_SEC
}

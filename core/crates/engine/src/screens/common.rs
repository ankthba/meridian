//! Helpers shared by screens: security checks, error screens, cached bars,
//! current quotes, and range parsing.

use chrono::{Datelike, NaiveDate};
use meridian_provider::{BarsRequest, CachePolicy};
use meridian_stream::row::QuoteRow;
use meridian_types::{
    Adjustment, BarInterval, BarSeries, NANOS_PER_DAY, Provenance, Quote, SecurityKey, UnixNanos, date_to_nanos,
    nanos_to_date,
};

use super::ScreenRequest;
use crate::cache::Fetched;
use crate::core::Engine;
use crate::error::{EngineError, EngineResult};
use crate::screen::{Block, NoticeLevel, Screen, ScreenStatus};

/// The request's security, or a screen explaining that one is required.
pub(crate) fn require_security(function: &str, title: &str, req: &ScreenRequest) -> Result<SecurityKey, Box<Screen>> {
    req.security.clone().ok_or_else(|| {
        Box::new(Screen::not_available(
            function,
            title,
            None,
            format!("{function} needs a security. Example: AAPL US <EQUITY> {function} <GO>"),
        ))
    })
}

/// Screen for an engine error: NOT AVAILABLE for capability gaps, error
/// otherwise.
pub(crate) fn error_screen(function: &str, title: &str, security: Option<&SecurityKey>, err: &EngineError) -> Screen {
    let mut s = Screen::new(function, title, security.map(ToString::to_string));
    let msg = err.user_message();
    s.status = if err.is_unavailable() { ScreenStatus::NotAvailable { reason: msg } } else { ScreenStatus::Error { message: msg } };
    s
}

/// Adds a notice when data came from a stale cache.
pub(crate) fn stale_notice<T>(screen: &mut Screen, f: &Fetched<T>) {
    if f.stale {
        screen.push(Block::Notice {
            level: NoticeLevel::Warning,
            text: format!("OFFLINE — showing cached data from {}", fmt_datetime(f.fetched_at)),
        });
    }
}

#[must_use]
pub fn fmt_date(ns: UnixNanos) -> String {
    nanos_to_date(ns).format("%m/%d/%Y").to_string()
}

#[must_use]
pub fn fmt_datetime(ns: UnixNanos) -> String {
    meridian_types::nanos_to_datetime(ns).format("%m/%d/%Y %H:%M UTC").to_string()
}

/// Parses a range preset relative to `now`: `1D 5D 1M 3M 6M YTD 1Y 2Y 5Y
/// 10Y 20Y MAX`. Returns `(from, to)`; `from = None` means all history.
#[must_use]
pub fn parse_range(range: &str, now: UnixNanos) -> (Option<UnixNanos>, UnixNanos) {
    let today = nanos_to_date(now);
    let r = range.trim().to_ascii_uppercase();
    let days = |n: i64| Some(now - n * NANOS_PER_DAY);
    let from = match r.as_str() {
        "1D" => days(1),
        "5D" => days(7),
        "1M" => days(31),
        "3M" => days(92),
        "6M" => days(183),
        "YTD" => NaiveDate::from_ymd_opt(today.year(), 1, 1).map(date_to_nanos),
        "2Y" => days(731),
        "3Y" => days(1_096),
        "5Y" => days(1_827),
        "10Y" => days(3_653),
        "20Y" => days(7_306),
        "MAX" => None,
        _ => days(366), // 1Y and unrecognized presets
    };
    (from, now + NANOS_PER_DAY)
}

/// Parses `MM/DD/YYYY` or `YYYY-MM-DD`.
#[must_use]
pub fn parse_date(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    NaiveDate::parse_from_str(s, "%m/%d/%Y").or_else(|_| NaiveDate::parse_from_str(s, "%Y-%m-%d")).ok()
}

impl Engine {
    /// Bars with read-through caching in DuckDB (honoring cache policy).
    /// Falls back to cached bars, flagged stale, when the fetch fails.
    pub async fn bars(&self, key: &SecurityKey, interval: BarInterval, from: Option<UnixNanos>, to: UnixNanos) -> EngineResult<Fetched<BarSeries>> {
        let now = self.now();
        let cached = self.stores().market.get_bars(key, interval, from, Some(to), None).ok().flatten();
        if let Some(c) = &cached {
            // Fresh enough if it reaches within ~4 days of `to` for daily
            // data (weekends/holidays) or ~2 intervals for intraday.
            let slack = if interval.is_intraday() { 2 * interval.seconds() * 1_000_000_000 } else { 4 * NANOS_PER_DAY };
            let last = c.ts.last().copied().unwrap_or(0);
            let first = c.ts.first().copied().unwrap_or(i64::MAX);
            let covers_start = from.is_none_or(|f| first <= f + 7 * NANOS_PER_DAY);
            if last + slack >= to.min(now) && covers_start {
                return Ok(Fetched { value: c.clone(), provider: c.provenance.provider.to_string(), from_cache: true, stale: false, fetched_at: now });
            }
        }
        let req = BarsRequest { key: key.clone(), interval, from, to: Some(to), adjustment: Adjustment::Splits };
        match self.router().bars(req).await {
            Ok(routed) => {
                let mut series = routed.value;
                series.normalize();
                let policy = self
                    .router()
                    .capabilities()
                    .into_iter()
                    .find(|(id, _)| *id == routed.provider)
                    .map_or(CachePolicy::NoStore, |(_, c)| c.cache_policy);
                if !matches!(policy, CachePolicy::NoStore | CachePolicy::MaxAge(_))
                    && let Err(e) = self.stores().market.put_bars(&series)
                {
                    tracing::warn!(error = %e, "bar cache write failed");
                }
                Ok(Fetched { value: series, provider: routed.provider.to_string(), from_cache: false, stale: false, fetched_at: now })
            }
            Err(e) => match cached {
                Some(c) if matches!(e, meridian_provider::ProviderError::Network(_) | meridian_provider::ProviderError::Http { .. }) => {
                    Ok(Fetched { value: c, provider: String::new(), from_cache: true, stale: true, fetched_at: now })
                }
                _ => Err(EngineError::Provider(e)),
            },
        }
    }

    /// Latest quote row from stream state, or a REST snapshot written into
    /// state if the instrument has none yet.
    pub async fn quote_row(&self, key: &SecurityKey) -> Option<QuoteRow> {
        let id = self.hub().registry().intern(key);
        if let Some(r) = self.hub().state().snapshot(id) {
            return Some(r);
        }
        let quotes = self.router().quotes(std::slice::from_ref(key)).await.ok()?;
        let q = quotes.into_iter().next()?;
        self.hub().apply_snapshot(&q);
        self.hub().state().snapshot(id)
    }

    /// REST quotes for many keys (monitors), also seeding stream state.
    pub async fn quotes(&self, keys: &[SecurityKey]) -> Vec<Quote> {
        let mut out = Vec::new();
        for chunk in keys.chunks(100) {
            if let Ok(q) = self.router().quotes(chunk).await {
                for x in &q {
                    self.hub().apply_snapshot(x);
                }
                out.extend(q);
            }
        }
        out
    }

    /// Provenance for quotes on `key` (first quote provider that covers it).
    #[must_use]
    pub fn quote_provenance(&self, key: &SecurityKey) -> Option<Provenance> {
        let caps = self.router().capabilities();
        let providers = self.router().providers();
        for (p, (id, c)) in providers.iter().zip(caps) {
            if !p.covers(key) {
                continue;
            }
            for a in meridian_provider::asset_hint(key) {
                if let Some(e) = c.entry(meridian_provider::Capability::Quotes, Some(*a)) {
                    return Some(Provenance {
                        provider: id,
                        synthetic: e.delay == meridian_types::DataDelay::Synthetic,
                        delay: e.delay,
                        source: e.source.clone(),
                        as_of: self.now(),
                        source_ref: None,
                        attribution: c.attribution.clone(),
                    });
                }
            }
        }
        None
    }
}

/// Rebuilds derived indexes after the universe loads. Overridden once the
/// autocomplete index exists.
pub(crate) fn rebuild_suggest_index(engine: &Engine) {
    crate::screens::suggest_hook(engine);
}

/// Placeholder until analytics-backed screens are registered.
pub(crate) fn extra_builder_for(function: &str) -> Option<super::Builder> {
    crate::screens::analytics_builder_for(function)
}


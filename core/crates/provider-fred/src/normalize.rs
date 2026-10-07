//! FRED wire formats (private DTOs) and their conversion into Meridian types.
//! Everything here is pure: no I/O, no clocks.

use chrono::{DateTime, NaiveDate};
use meridian_provider::{ProviderError, ProviderResult};
use meridian_types::{
    EconomicEvent, EconomicSeries, Importance, Observation, Provenance, UnixNanos, date_to_nanos, datetime_to_nanos,
};
use serde::Deserialize;

use crate::http;

/// FRED's marker for a missing observation (documented on
/// `fred/v2/release_observations.html`: "values for dates with missing
/// observations are reported as a period '.'").
pub(crate) const MISSING_VALUE: &str = ".";

/// FRED's `last_updated` format, e.g. `2013-07-31 09:26:16-05`. `%#z`
/// accepts an offset with or without minutes.
const LAST_UPDATED_FORMAT: &str = "%Y-%m-%d %H:%M:%S%#z";

/// Releases rated `High` on the calendar, with a representative series to
/// chart. Matched by FRED release ID, falling back to the exact release name
/// (case-insensitive). IDs, names, and series were checked against
/// fred.stlouisfed.org on 2026-10-05.
pub(crate) const HIGH_IMPORTANCE_RELEASES: [(i64, &str, &str); 10] = [
    (50, "Employment Situation", "PAYEMS"),
    (10, "Consumer Price Index", "CPIAUCSL"),
    (53, "Gross Domestic Product", "GDP"),
    (54, "Personal Income and Outlays", "PCEPI"),
    (9, "Advance Monthly Sales for Retail and Food Services", "RSAFS"),
    (46, "Producer Price Index", "PPIFIS"),
    (13, "G.17 Industrial Production and Capacity Utilization", "INDPRO"),
    (192, "Job Openings and Labor Turnover Survey", "JTSJOL"),
    (27, "New Residential Construction", "HOUST"),
    (101, "FOMC Press Release", "DFEDTARU"),
];

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

/// `fred/series` response. (`seriess` is FRED's spelling.)
#[derive(Debug, Deserialize)]
struct SeriesEnvelope {
    #[serde(default)]
    seriess: Vec<SeriesDto>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SeriesDto {
    pub id: String,
    pub title: String,
    pub frequency: String,
    pub units: String,
    #[serde(default)]
    pub seasonal_adjustment: Option<String>,
    #[serde(default)]
    pub last_updated: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// One page of `fred/series/observations`.
#[derive(Debug, Deserialize)]
pub(crate) struct ObservationsPage {
    pub count: u64,
    pub offset: u64,
    pub observations: Vec<ObservationDto>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ObservationDto {
    pub date: String,
    pub value: String,
}

/// One page of `fred/releases/dates`.
#[derive(Debug, Deserialize)]
pub(crate) struct ReleaseDatesPage {
    pub count: u64,
    pub offset: u64,
    pub release_dates: Vec<ReleaseDateDto>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReleaseDateDto {
    pub release_id: i64,
    pub release_name: String,
    pub date: String,
}

/// FRED's JSON error body: `{"error_code":400,"error_message":"..."}`.
#[derive(Debug, Deserialize)]
struct ErrorBody {
    error_message: String,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parses `fred/series`; an empty `seriess` array means the series is unknown.
pub(crate) fn parse_series_meta(body: &str, requested_id: &str) -> ProviderResult<SeriesDto> {
    let env: SeriesEnvelope = http::parse_json(body, "FRED fred/series response")?;
    env.seriess
        .into_iter()
        .next()
        .ok_or_else(|| ProviderError::NotFound(format!("FRED series {requested_id} does not exist")))
}

pub(crate) fn parse_observations_page(body: &str) -> ProviderResult<ObservationsPage> {
    http::parse_json(body, "FRED fred/series/observations response")
}

pub(crate) fn parse_release_dates_page(body: &str) -> ProviderResult<ReleaseDatesPage> {
    http::parse_json(body, "FRED fred/releases/dates response")
}

pub(crate) fn parse_date(s: &str, context: &str) -> ProviderResult<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
        .map_err(|e| ProviderError::parse(format!("{context}: bad date {s:?}: {e}")))
}

/// `"."` is FRED's missing-value marker and becomes `None`. Anything else
/// must be a number; a non-numeric value is a format change, not data.
pub(crate) fn parse_value(raw: &str) -> ProviderResult<Option<f64>> {
    let v = raw.trim();
    if v == MISSING_VALUE {
        return Ok(None);
    }
    v.parse::<f64>().map(Some).map_err(|e| ProviderError::parse(format!("FRED observation value {raw:?}: {e}")))
}

pub(crate) fn parse_observation(dto: &ObservationDto) -> ProviderResult<Observation> {
    Ok(Observation { date: parse_date(&dto.date, "FRED observation")?, value: parse_value(&dto.value)? })
}

/// Parses `last_updated` (`2013-07-31 09:26:16-05`) into UTC nanos. Returns
/// `None` if FRED ever changes the format; the series is still usable.
pub(crate) fn parse_last_updated(s: &str) -> Option<UnixNanos> {
    match DateTime::parse_from_str(s.trim(), LAST_UPDATED_FORMAT) {
        Ok(dt) => Some(datetime_to_nanos(dt.to_utc())),
        Err(e) => {
            tracing::debug!(value = s, error = %e, "unparseable FRED last_updated");
            None
        }
    }
}

/// Offset of the next page, or `None` when `offset + received` covers
/// `count`. An empty page always ends pagination.
pub(crate) fn next_offset(count: u64, offset: u64, received: u64) -> Option<u64> {
    if received == 0 {
        return None;
    }
    let next = offset.saturating_add(received);
    (next < count).then_some(next)
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

/// Builds the normalized series. Observations are sorted by date ascending.
pub(crate) fn build_series(
    meta: SeriesDto,
    pages: Vec<ObservationDto>,
    fetched_at: UnixNanos,
    provenance: impl FnOnce(UnixNanos) -> Provenance,
) -> ProviderResult<EconomicSeries> {
    let mut observations = pages.iter().map(parse_observation).collect::<ProviderResult<Vec<_>>>()?;
    observations.sort_by_key(|o| o.date);
    let last_updated = meta.last_updated.as_deref().and_then(parse_last_updated);
    Ok(EconomicSeries {
        id: meta.id,
        title: meta.title,
        units: meta.units,
        frequency: meta.frequency,
        seasonal_adjustment: non_empty(meta.seasonal_adjustment),
        last_updated,
        notes: non_empty(meta.notes),
        observations,
        provenance: provenance(last_updated.unwrap_or(fetched_at)),
    })
}

// ---------------------------------------------------------------------------
// Calendar
// ---------------------------------------------------------------------------

fn high_importance_entry(release_id: i64, name: &str) -> Option<&'static (i64, &'static str, &'static str)> {
    let name = name.trim();
    HIGH_IMPORTANCE_RELEASES.iter().find(|(id, n, _)| *id == release_id || n.eq_ignore_ascii_case(name))
}

/// `High` for the curated list in [`HIGH_IMPORTANCE_RELEASES`], `Medium`
/// otherwise. FRED publishes no importance rating of its own.
pub(crate) fn importance(release_id: i64, name: &str) -> Importance {
    if high_importance_entry(release_id, name).is_some() { Importance::High } else { Importance::Medium }
}

/// A representative FRED series for a high-importance release.
pub(crate) fn representative_series(release_id: i64, name: &str) -> Option<&'static str> {
    high_importance_entry(release_id, name).map(|(_, _, series)| *series)
}

/// One release date → one event. FRED gives a date only, so `release_time`
/// is midnight UTC of that date and `time_known` is false. FRED has no
/// consensus, actual, or prior values on this endpoint.
/// Rates a release `Medium` when FRED dates it on three or more consecutive
/// days in the window: that is a daily data feed, not a scheduled
/// announcement. FRED dates the FOMC Press Release (101) daily because its
/// target-range series update every day, so its dates don't mark meetings.
pub(crate) fn demote_daily_releases(events: &mut [EconomicEvent]) {
    let mut days: std::collections::HashMap<&str, Vec<i64>> = std::collections::HashMap::new();
    for e in events.iter().filter(|e| e.importance == Importance::High) {
        days.entry(e.event.as_str()).or_default().push(e.release_time.div_euclid(meridian_types::NANOS_PER_DAY));
    }
    let daily: std::collections::HashSet<String> = days
        .into_iter()
        .filter_map(|(name, mut d)| {
            d.sort_unstable();
            d.dedup();
            d.windows(3).any(|w| w[1] == w[0] + 1 && w[2] == w[1] + 1).then(|| name.to_owned())
        })
        .collect();
    for e in events.iter_mut().filter(|e| daily.contains(&e.event)) {
        e.importance = Importance::Medium;
    }
}

pub(crate) fn release_event(dto: &ReleaseDateDto, provenance: Provenance) -> ProviderResult<EconomicEvent> {
    let date = parse_date(&dto.date, "FRED release date")?;
    Ok(EconomicEvent {
        release_time: date_to_nanos(date),
        time_known: false,
        country: "US".to_owned(),
        event: dto.release_name.trim().to_owned(),
        period: None,
        actual: None,
        consensus: None,
        prior: None,
        unit: None,
        importance: importance(dto.release_id, &dto.release_name),
        series_id: representative_series(dto.release_id, &dto.release_name).map(str::to_owned),
        provenance,
    })
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Removes every occurrence of `secret` from `text`, plus a trailing partial
/// copy (bodies are truncated to a snippet, which could cut a key in half).
pub(crate) fn redact(text: &str, secret: &str) -> String {
    const MASK: &str = "[REDACTED]";
    if secret.is_empty() {
        return text.to_owned();
    }
    let mut out = text.replace(secret, MASK);
    // A truncated tail of at least 4 chars that is a prefix of the secret.
    for len in (4..secret.len()).rev() {
        if let Some(prefix) = secret.get(..len)
            && out.ends_with(prefix)
        {
            out.truncate(out.len() - len);
            out.push_str(MASK);
            break;
        }
    }
    out
}

/// Pulls `error_message` out of a FRED JSON error body.
pub(crate) fn error_message(body: &str) -> Option<String> {
    serde_json::from_str::<ErrorBody>(body)
        .ok()
        .map(|b| b.error_message.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Re-maps an error from [`http::send_text`] using FRED's error body, and
/// redacts the API key from every message.
///
/// FRED answers most client errors with HTTP 400 and a message:
/// "The series does not exist." → `NotFound`; anything about `api_key` →
/// `Unauthorized`.
pub(crate) fn map_error(err: ProviderError, key: &str) -> ProviderError {
    match err {
        ProviderError::Http { status, body_snippet } => {
            let body = redact(&body_snippet, key);
            let msg = error_message(&body).unwrap_or(body);
            let lower = msg.to_ascii_lowercase();
            if lower.contains("does not exist") {
                ProviderError::NotFound(format!("FRED: {msg}"))
            } else if lower.contains("api_key") {
                ProviderError::Unauthorized(format!("FRED rejected the API key: {msg}"))
            } else {
                ProviderError::Http { status, body_snippet: msg }
            }
        }
        ProviderError::Unauthorized(m) => ProviderError::Unauthorized(redact(&m, key)),
        ProviderError::NotFound(m) => ProviderError::NotFound(redact(&m, key)),
        ProviderError::Network(m) => ProviderError::Network(redact(&m, key)),
        ProviderError::Upstream(m) => ProviderError::Upstream(redact(&m, key)),
        other => other,
    }
}

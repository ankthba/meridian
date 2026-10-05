//! Finnhub wire formats (private DTOs) and their conversion into Meridian
//! types. Everything here is pure: no I/O, no clocks.

use std::collections::HashSet;

use chrono::{Days, NaiveDate};
use meridian_provider::{Capability, NewsQuery, ProviderError, ProviderResult};
use meridian_types::{
    CompanyProfile, DataDelay, EarningsHistory, EarningsRecord, FeedSource, MarketSector, NewsItem, Provenance,
    ProviderId, Recommendations, SecurityKey, UnixNanos, date_to_nanos, nanos_from_secs, nanos_to_date,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::{PROVIDER_ID, http};

/// `marketCapitalization` and `shareOutstanding` in `/stock/profile2` are in
/// millions. The docs don't state the unit; it follows from the documented
/// sample (Apple: 1,415,993 and 4,375.48 ≈ $1.4T and 4.38B shares).
pub(crate) const PROFILE_UNIT: f64 = 1_000_000.0;

/// Company news window when the query leaves `from` open.
pub(crate) const DEFAULT_NEWS_DAYS: u64 = 7;

// ---------------------------------------------------------------------------
// DTOs (fields per https://finnhub.io/static/swagger.json definitions)
// ---------------------------------------------------------------------------

/// `CompanyNews` / `MarketNews` (identical schemas).
#[derive(Debug, Deserialize)]
pub(crate) struct NewsDto {
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub datetime: Option<i64>,
    #[serde(default)]
    pub headline: Option<String>,
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub related: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

/// `RecommendationTrend`. Counts are required: a missing count is a format
/// change, not zero analysts.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecommendationDto {
    pub buy: u32,
    pub hold: u32,
    pub period: String,
    pub sell: u32,
    pub strong_buy: u32,
    pub strong_sell: u32,
}

/// `CompanyProfile2`. Finnhub answers `{}` for symbols it doesn't know.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProfileDto {
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub ticker: Option<String>,
    #[serde(default)]
    pub ipo: Option<String>,
    #[serde(default)]
    pub market_capitalization: Option<f64>,
    #[serde(default)]
    pub share_outstanding: Option<f64>,
    #[serde(default)]
    pub weburl: Option<String>,
}

/// `EarningResult`.
#[derive(Debug, Deserialize)]
pub(crate) struct EarningsDto {
    #[serde(default)]
    pub actual: Option<f64>,
    #[serde(default)]
    pub estimate: Option<f64>,
    #[serde(default)]
    pub period: Option<String>,
    #[serde(default)]
    pub quarter: Option<u32>,
    #[serde(default)]
    pub year: Option<i32>,
}

/// Finnhub's error body: `{"error": "..."}`.
#[derive(Debug, Deserialize)]
struct ErrorBody {
    error: String,
}

// ---------------------------------------------------------------------------
// Symbols
// ---------------------------------------------------------------------------

/// Finnhub's symbol for a key, or `None` if this integration doesn't cover
/// it. Only US listings (`AAPL US Equity`, or no exchange): Finnhub's free
/// company news is North-America-only and non-US symbols use a different
/// symbology (`TICKER.EXCHANGE`). Class shares use a dot (`BRK/B` → `BRK.B`).
pub(crate) fn finnhub_symbol(key: &SecurityKey) -> Option<String> {
    if !matches!(key.sector, MarketSector::Equity | MarketSector::Pfd) {
        return None;
    }
    if !key.exchange.as_deref().is_none_or(|e| e.eq_ignore_ascii_case("US")) {
        return None;
    }
    let sym = key.symbol.trim().to_ascii_uppercase().replace('/', ".");
    let valid = !sym.is_empty() && sym.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    valid.then_some(sym)
}

pub(crate) fn not_covered(key: &SecurityKey) -> ProviderError {
    ProviderError::NotFound(format!("{key} is not covered by Finnhub (US-listed equities only)"))
}

// ---------------------------------------------------------------------------
// Provenance
// ---------------------------------------------------------------------------

pub(crate) fn provenance(delay: DataDelay, as_of: UnixNanos, source_ref: &str) -> Provenance {
    Provenance {
        provider: ProviderId::new(PROVIDER_ID),
        synthetic: false,
        delay,
        source: FeedSource::Aggregated,
        as_of,
        source_ref: Some(source_ref.to_owned()),
        // Finnhub's terms of service contain no attribution requirement.
        attribution: None,
    }
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

fn decode_entity(entity: &str) -> Option<char> {
    Some(match entity {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        _ => {
            let num = entity.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse().ok()?,
            };
            char::from_u32(code)?
        }
    })
}

/// Strips HTML tags, decodes common entities, and collapses whitespace.
/// Empty results become `None`.
pub(crate) fn clean_text(raw: &str) -> Option<String> {
    // 1. Tags → spaces. A '<' only starts a tag when followed by a letter,
    //    '/', '!' or '?', so "a < b" survives.
    let mut no_tags = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(i) = rest.find('<') {
        no_tags.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let starts_tag = after.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?'));
        match after.find('>') {
            Some(end) if starts_tag => {
                no_tags.push(' ');
                rest = &after[end + 1..];
            }
            _ => {
                no_tags.push('<');
                rest = after;
            }
        }
    }
    no_tags.push_str(rest);

    // 2. Entities.
    let mut decoded = String::with_capacity(no_tags.len());
    let mut rest = no_tags.as_str();
    while let Some(i) = rest.find('&') {
        decoded.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let entity =
            after.find(';').filter(|&end| end <= 10).and_then(|end| Some((decode_entity(&after[..end])?, end)));
        if let Some((c, end)) = entity {
            decoded.push(c);
            rest = &after[end + 1..];
        } else {
            decoded.push('&');
            rest = after;
        }
    }
    decoded.push_str(rest);

    // 3. Whitespace.
    let out = decoded.split_whitespace().collect::<Vec<_>>().join(" ");
    (!out.is_empty()).then_some(out)
}

fn non_empty(s: Option<&str>) -> Option<String> {
    s.map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned)
}

// ---------------------------------------------------------------------------
// Parsing and errors
// ---------------------------------------------------------------------------

/// Removes every occurrence of `secret` from `text`, plus a trailing partial
/// copy (error bodies are truncated to a snippet).
pub(crate) fn redact(text: &str, secret: &str) -> String {
    const MASK: &str = "[REDACTED]";
    if secret.is_empty() {
        return text.to_owned();
    }
    let mut out = text.replace(secret, MASK);
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

fn error_message(body: &str) -> Option<String> {
    serde_json::from_str::<ErrorBody>(body).ok().map(|b| b.error.trim().to_owned())
}

/// Classifies a Finnhub `{"error": ...}` message.
fn classify(msg: String, capability: Capability) -> ProviderError {
    let lower = msg.to_ascii_lowercase();
    if lower.contains("access") {
        // "You don't have access to this resource." = premium endpoint.
        ProviderError::NotEntitled { capability, plan: "a paid Finnhub plan".into() }
    } else if lower.contains("api key") || lower.contains("token") {
        ProviderError::Unauthorized(format!("Finnhub rejected the API key: {msg}"))
    } else if lower.contains("limit") {
        ProviderError::RateLimited { retry_after: None }
    } else {
        ProviderError::Upstream(format!("Finnhub: {msg}"))
    }
}

/// Re-maps an error from [`http::send_text`]: uses Finnhub's error body and
/// redacts the key from every message.
pub(crate) fn map_error(err: ProviderError, key: &str, capability: Capability) -> ProviderError {
    match err {
        ProviderError::Unauthorized(m) => {
            let m = redact(&m, key);
            if m.to_ascii_lowercase().contains("access") {
                ProviderError::NotEntitled { capability, plan: "a paid Finnhub plan".into() }
            } else {
                ProviderError::Unauthorized(m)
            }
        }
        ProviderError::Http { status, body_snippet } => {
            let body = redact(&body_snippet, key);
            match error_message(&body) {
                Some(msg) => match classify(msg, capability) {
                    ProviderError::Upstream(m) => ProviderError::Http { status, body_snippet: m },
                    other => other,
                },
                None => ProviderError::Http { status, body_snippet: body },
            }
        }
        ProviderError::NotFound(m) => ProviderError::NotFound(redact(&m, key)),
        ProviderError::Network(m) => ProviderError::Network(redact(&m, key)),
        ProviderError::Upstream(m) => ProviderError::Upstream(redact(&m, key)),
        other => other,
    }
}

/// Parses a 2xx body, treating a `{"error": ...}` body as an error.
pub(crate) fn parse_body<T: DeserializeOwned>(
    body: &str,
    context: &str,
    key: &str,
    capability: Capability,
) -> ProviderResult<T> {
    if let Some(msg) = error_message(body) {
        return Err(classify(redact(&msg, key), capability));
    }
    http::parse_json(body, context)
}

fn parse_date(s: &str, context: &str) -> ProviderResult<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
        .map_err(|e| ProviderError::parse(format!("{context}: bad date {s:?}: {e}")))
}

// ---------------------------------------------------------------------------
// News
// ---------------------------------------------------------------------------

/// `from`/`to` dates for `/company-news` (both required by Finnhub). An open
/// `to` is today; an open `from` is [`DEFAULT_NEWS_DAYS`] before `to`.
pub(crate) fn news_window(from: Option<UnixNanos>, to: Option<UnixNanos>, now: UnixNanos) -> (NaiveDate, NaiveDate) {
    let to_date = nanos_to_date(to.unwrap_or(now));
    let from_date =
        from.map_or_else(|| to_date.checked_sub_days(Days::new(DEFAULT_NEWS_DAYS)).unwrap_or(to_date), nanos_to_date);
    (from_date, to_date)
}

/// One Finnhub article → `NewsItem`. Items without an ID, a timestamp, or a
/// headline are dropped (nothing is invented for them). `fallback_symbol`
/// (the requested symbol, for company news) fills `tickers` when `related`
/// is empty.
pub(crate) fn news_item(
    dto: NewsDto,
    fallback_symbol: Option<&str>,
    received_at: UnixNanos,
    source_ref: &str,
) -> Option<NewsItem> {
    let id = dto.id?.to_string();
    let published_at = dto.datetime.filter(|&s| s > 0).map(nanos_from_secs)?;
    let headline = clean_text(dto.headline.as_deref().unwrap_or_default())?;
    let mut tickers: Vec<String> = dto
        .related
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(|t| t.trim().to_ascii_uppercase())
        .filter(|t| !t.is_empty())
        .collect();
    if tickers.is_empty()
        && let Some(sym) = fallback_symbol
    {
        tickers.push(sym.to_owned());
    }
    Some(NewsItem {
        id,
        source: dto.source.as_deref().map(str::trim).unwrap_or_default().to_owned(),
        headline,
        summary: dto.summary.as_deref().and_then(clean_text),
        body: None,
        url: non_empty(dto.url.as_deref()),
        published_at,
        received_at,
        tickers,
        topics: non_empty(dto.category.as_deref()).into_iter().collect(),
        provenance: provenance(DataDelay::RealTime, published_at, source_ref),
    })
}

/// Applies the query's text filter (case-insensitive, headline + summary)
/// and time bounds (inclusive), de-duplicates by ID, sorts newest first,
/// and truncates to `limit` (0 = no limit).
pub(crate) fn finish_news(items: Vec<NewsItem>, q: &NewsQuery) -> Vec<NewsItem> {
    let needle = q.text.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_lowercase);
    let mut seen = HashSet::new();
    let mut out: Vec<NewsItem> = items
        .into_iter()
        .filter(|i| q.from.is_none_or(|f| i.published_at >= f) && q.to.is_none_or(|t| i.published_at <= t))
        .filter(|i| {
            needle.as_deref().is_none_or(|n| {
                i.headline.to_lowercase().contains(n)
                    || i.summary.as_deref().is_some_and(|s| s.to_lowercase().contains(n))
            })
        })
        .filter(|i| seen.insert(i.id.clone()))
        .collect();
    out.sort_by(|a, b| b.published_at.cmp(&a.published_at).then_with(|| a.id.cmp(&b.id)));
    if q.limit > 0 {
        out.truncate(q.limit);
    }
    out
}

// ---------------------------------------------------------------------------
// Recommendations
// ---------------------------------------------------------------------------

/// Counts from the most recent period (by date, regardless of order). No
/// per-broker ratings or price targets on the free tier.
pub(crate) fn recommendations(
    key: &SecurityKey,
    periods: Vec<RecommendationDto>,
    source_ref: &str,
) -> ProviderResult<Recommendations> {
    let mut latest: Option<(NaiveDate, RecommendationDto)> = None;
    for p in periods {
        let date = parse_date(&p.period, "Finnhub recommendation period")?;
        if latest.as_ref().is_none_or(|(d, _)| date > *d) {
            latest = Some((date, p));
        }
    }
    let (as_of, p) =
        latest.ok_or_else(|| ProviderError::NotFound(format!("no Finnhub recommendation trends for {key}")))?;
    Ok(Recommendations {
        key: key.clone(),
        as_of,
        strong_buy: p.strong_buy,
        buy: p.buy,
        hold: p.hold,
        sell: p.sell,
        strong_sell: p.strong_sell,
        target_mean: None,
        target_high: None,
        target_low: None,
        ratings: Vec::new(),
        provenance: provenance(DataDelay::EndOfDay, date_to_nanos(as_of), source_ref),
    })
}

// ---------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------

/// `/stock/profile2` → `CompanyProfile`. Market cap and shares are converted
/// from millions to absolute units. Market cap is only kept when the
/// profile's currency is USD (or absent), since `CompanyProfile` carries no
/// currency.
pub(crate) fn profile(key: &SecurityKey, dto: ProfileDto) -> ProviderResult<CompanyProfile> {
    let known = non_empty(dto.name.as_deref()).is_some() || non_empty(dto.ticker.as_deref()).is_some();
    if !known {
        return Err(ProviderError::NotFound(format!("no Finnhub profile for {key}")));
    }
    let usd = dto.currency.as_deref().map(str::trim).is_none_or(|c| c.is_empty() || c.eq_ignore_ascii_case("USD"));
    if !usd {
        tracing::debug!(currency = ?dto.currency, "dropping non-USD Finnhub market cap");
    }
    Ok(CompanyProfile {
        description: None,
        website: non_empty(dto.weburl.as_deref()),
        // Documented as "Country of company's headquarter."
        headquarters: non_empty(dto.country.as_deref()),
        employees: None,
        ceo: None,
        founded: None,
        fiscal_year_end: None,
        ipo_date: non_empty(dto.ipo.as_deref()),
        shares_outstanding: dto.share_outstanding.filter(|v| v.is_finite()).map(|v| v * PROFILE_UNIT),
        market_cap: dto.market_capitalization.filter(|v| usd && v.is_finite()).map(|v| v * PROFILE_UNIT),
        sic_code: None,
        sic_description: None,
    })
}

// ---------------------------------------------------------------------------
// Earnings
// ---------------------------------------------------------------------------

/// `Q{quarter} {yy}`, e.g. `Q1 23`.
pub(crate) fn fiscal_label(quarter: u32, year: i32) -> String {
    format!("Q{quarter} {:02}", year.rem_euclid(100))
}

/// `/stock/earnings` → `EarningsHistory`, newest first. The estimate is the
/// consensus Finnhub reports; revenue is not on this endpoint.
pub(crate) fn earnings(
    key: &SecurityKey,
    rows: Vec<EarningsDto>,
    fetched_at: UnixNanos,
    source_ref: &str,
) -> ProviderResult<EarningsHistory> {
    let mut records = rows
        .into_iter()
        .map(|r| {
            let period = r.period.as_deref().unwrap_or_default();
            let period_end = parse_date(period, "Finnhub earnings period")?;
            let label = match (r.quarter, r.year) {
                (Some(q), Some(y)) => fiscal_label(q, y),
                _ => period.trim().to_owned(),
            };
            Ok(EarningsRecord {
                fiscal_label: label,
                period_end,
                announce_date: None,
                eps_actual: r.actual.filter(|v| v.is_finite()),
                eps_estimate: r.estimate.filter(|v| v.is_finite()),
                revenue_actual: None,
                revenue_estimate: None,
            })
        })
        .collect::<ProviderResult<Vec<_>>>()?;
    if records.is_empty() {
        return Err(ProviderError::NotFound(format!("no Finnhub earnings history for {key}")));
    }
    records.sort_by(|a, b| b.period_end.cmp(&a.period_end));
    Ok(EarningsHistory {
        key: key.clone(),
        records,
        provenance: provenance(DataDelay::EndOfDay, fetched_at, source_ref),
    })
}

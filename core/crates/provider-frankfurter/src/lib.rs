//! Frankfurter FX reference rates, pinned to the European Central Bank.
//!
//! Frankfurter v2 (<https://frankfurter.dev/docs/>) blends 100+ central
//! banks unless a provider is named. We always pass `providers=ecb` so every
//! series comes from one consistent source: the ECB euro foreign exchange
//! reference rates. Pairs not quoted against EUR are cross rates that
//! Frankfurter derives from the ECB EUR rates; the attribution says so, as
//! the ECB's reuse terms require.
//!
//! A reference rate is one daily fixing, not a traded market: quotes carry
//! the latest fixing as `last` and the previous business day's fixing as
//! `prev_close`, and nothing else.

mod http;

use std::collections::BTreeMap;
use std::fmt::Write as _;

use async_trait::async_trait;
use chrono::{Datelike, Days, NaiveDate, Utc};
use meridian_provider::{
    AiPolicy, BarsRequest, CachePolicy, Capabilities, Capability, CapabilityEntry, Provider, ProviderError,
    ProviderResult, RateLimit,
};
use meridian_types::{
    Adjustment, AssetClass, Bar, BarInterval, BarSeries, DataDelay, FeedSource, Instrument, MarketSector,
    ProviderId, Provenance, Quote, SecurityKey, UnixNanos, date_to_nanos, datetime_to_nanos, nanos_to_date,
    quote_flags,
};
use serde::Deserialize;

/// Provider id used in provenance and routing.
pub const PROVIDER_ID: &str = "frankfurter";

/// Attribution shown wherever these rates are displayed. The ECB requires
/// the source to be cited and any modification (here: cross rates) to be
/// stated: <https://www.ecb.europa.eu/services/using-our-site/disclaimer/html/index.en.html>.
pub const ATTRIBUTION: &str = "Source: European Central Bank euro foreign exchange reference rates, via Frankfurter. \
Pairs not quoted against EUR are cross rates derived from the ECB EUR rates.";

const DEFAULT_BASE_URL: &str = "https://api.frankfurter.dev/v2";
const DOCS_URL: &str = "https://frankfurter.dev/docs/";
/// The provider filter that keeps every series on ECB data.
const ECB_PROVIDER: &str = "ecb";

/// Calendar days fetched to find the latest and previous fixings. The ECB's
/// longest publication gap (Christmas / Easter closures) is under a week.
const QUOTE_LOOKBACK_DAYS: u64 = 14;
/// A latest fixing older than this is flagged stale.
const STALE_AFTER_DAYS: i64 = 7;

/// Currencies the ECB provider lists, from `GET /v2/providers/ecb`
/// (2026-10-05). It includes currencies the ECB no longer publishes (e.g.
/// legacy euro-area currencies); requests for those return no recent rate,
/// so no quote is produced for them.
const ECB_CURRENCIES: &[&str] = &[
    "ARS", "AUD", "BGN", "BRL", "CAD", "CHF", "CNY", "CYP", "CZK", "DKK", "DZD", "EEK", "EUR", "GBP", "GRD", "HKD",
    "HRK", "HUF", "IDR", "ILS", "INR", "ISK", "JPY", "KRW", "LTL", "LVL", "MAD", "MTL", "MXN", "MYR", "NOK", "NZD",
    "PHP", "PLN", "RON", "RUB", "SEK", "SGD", "SIT", "SKK", "THB", "TRY", "TWD", "USD", "ZAR",
];

/// Currencies whose units are small enough that rates against them are
/// conventionally shown with two decimals.
const SMALL_UNIT_CURRENCIES: &[&str] = &["HUF", "IDR", "ISK", "JPY", "KRW"];

/// FX reference rates from the ECB through Frankfurter. No credentials.
pub struct FrankfurterProvider {
    client: reqwest::Client,
    base_url: String,
    caps: Capabilities,
}

impl FrankfurterProvider {
    pub fn new() -> ProviderResult<Self> {
        Ok(Self { client: http::client()?, base_url: DEFAULT_BASE_URL.to_owned(), caps: capabilities() })
    }

    /// Whether this provider can serve `key`: a `Curncy` key made of two
    /// distinct ECB currencies, e.g. `EURUSD Curncy` or `USDJPY Curncy`.
    #[must_use]
    pub fn covers(&self, key: &SecurityKey) -> bool {
        parse_pair(key).is_some()
    }

    fn rates_url(&self, base: &str, quotes: &[&str], from: Option<NaiveDate>, to: Option<NaiveDate>) -> String {
        let mut url = format!(
            "{}/rates?base={base}&quotes={}&providers={ECB_PROVIDER}",
            self.base_url,
            quotes.join(",")
        );
        // Writing to a String cannot fail.
        if let Some(f) = from {
            let _ = write!(url, "&from={f}");
        }
        if let Some(t) = to {
            let _ = write!(url, "&to={t}");
        }
        url
    }

    async fn fetch_rates(&self, url: &str) -> ProviderResult<Vec<RateDto>> {
        match http::send_text(self.client.get(url)).await {
            Ok(body) => parse_rates(&body),
            // Frankfurter answers 422 for an unknown currency code.
            Err(ProviderError::Http { status: 422, body_snippet }) => Err(ProviderError::NotFound(body_snippet)),
            Err(e) => Err(e),
        }
    }
}

#[async_trait]
impl Provider for FrankfurterProvider {
    fn covers(&self, key: &SecurityKey) -> bool {
        FrankfurterProvider::covers(self, key)
    }

    fn id(&self) -> ProviderId {
        ProviderId::new(PROVIDER_ID)
    }

    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    async fn instrument(&self, key: &SecurityKey) -> ProviderResult<Instrument> {
        let (base, quote) = parse_pair(key).ok_or_else(|| not_covered(key))?;
        let mut inst = Instrument::basic(key.clone(), format!("{base}/{quote} ECB reference rate"), AssetClass::Fx, quote);
        inst.exchange_name = Some("ECB euro reference rates (via Frankfurter)".into());
        inst.price_decimals = price_decimals(base, quote);
        inst.tick_size = 10f64.powi(-i32::from(inst.price_decimals));
        Ok(inst)
    }

    async fn quotes(&self, keys: &[SecurityKey]) -> ProviderResult<Vec<Quote>> {
        // One request per base currency, covering all its quote currencies.
        let mut by_base: BTreeMap<&str, Vec<(&str, &SecurityKey)>> = BTreeMap::new();
        for key in keys {
            if let Some((base, quote)) = parse_pair(key) {
                by_base.entry(base).or_default().push((quote, key));
            }
        }
        let now = Utc::now();
        let today = now.date_naive();
        let from = today.checked_sub_days(Days::new(QUOTE_LOOKBACK_DAYS));
        let mut out = Vec::new();
        for (base, wanted) in by_base {
            let mut quote_codes: Vec<&str> = wanted.iter().map(|(q, _)| *q).collect();
            quote_codes.sort_unstable();
            quote_codes.dedup();
            let url = self.rates_url(base, &quote_codes, from, None);
            let rates = self.fetch_rates(&url).await?;
            for (quote_ccy, key) in wanted {
                if let Some(q) = build_quote(key, quote_ccy, &rates, today, datetime_to_nanos(now)) {
                    let as_of = q.ts_event;
                    out.push(Quote { provenance: provenance(as_of, url.clone()), ..q });
                }
            }
        }
        Ok(out)
    }

    async fn bars(&self, req: &BarsRequest) -> ProviderResult<BarSeries> {
        if !matches!(req.interval, BarInterval::Day | BarInterval::Week | BarInterval::Month) {
            return Err(ProviderError::Unsupported { capability: Capability::IntradayBars });
        }
        let (base, quote) = parse_pair(&req.key).ok_or_else(|| not_covered(&req.key))?;
        let ecb_start = NaiveDate::from_ymd_opt(1999, 1, 4).unwrap_or_default();
        let from_date = req.from.map_or(ecb_start, nanos_to_date).max(ecb_start);
        let to_date = req.to.map_or_else(|| Utc::now().date_naive(), |t| nanos_to_date(t.saturating_sub(1)));
        let url = self.rates_url(base, &[quote], Some(from_date), Some(to_date));
        let mut series = BarSeries::new(
            req.key.clone(),
            req.interval,
            Adjustment::None,
            provenance(datetime_to_nanos(Utc::now()), url.clone()),
        );
        if from_date > to_date {
            return Ok(series);
        }
        let rates = self.fetch_rates(&url).await?;
        let daily = daily_bars(&rates, quote, req.from, req.to);
        let bars = match req.interval {
            BarInterval::Week => aggregate(&daily, week_start),
            BarInterval::Month => aggregate(&daily, month_start),
            _ => daily,
        };
        for b in bars {
            series.push(b);
        }
        series.normalize();
        if let Some(last) = series.ts.last() {
            series.provenance.as_of = *last;
        }
        Ok(series)
    }
}

fn capabilities() -> Capabilities {
    let entry = |capability, history: Option<&str>| CapabilityEntry {
        capability,
        asset_classes: vec![AssetClass::Fx],
        delay: DataDelay::EndOfDay,
        source: FeedSource::Official,
        history: history.map(str::to_owned),
    };
    Capabilities {
        entries: vec![
            entry(Capability::Quotes, Some("latest ECB fixing and the previous business day's")),
            entry(Capability::DailyBars, Some("daily ECB fixings since 1999-01-04 (weekly/monthly aggregated)")),
            entry(Capability::Reference, None),
        ],
        // Frankfurter: "no quotas", abuse rate-limited only. Stay polite.
        rate_limit: Some(RateLimit::per_minute(60)),
        max_stream_symbols: None,
        cache_policy: CachePolicy::Unrestricted,
        attribution: Some(ATTRIBUTION.to_owned()),
        display_allowed: true,
        // The ECB permits free reuse of its published information provided
        // the ECB is cited and modifications are stated (see ATTRIBUTION).
        ai_policy: AiPolicy::Allowed,
        requires_credentials: false,
        terms_note: "ECB euro foreign exchange reference rates served by Frankfurter (no key, no quotas, abuse \
rate-limited). ECB terms: free reuse provided the ECB is cited as the source and modifications (cross rates) \
are stated; https://www.ecb.europa.eu/services/using-our-site/disclaimer/html/index.en.html. Reference rates \
are published once per TARGET business day around 16:00 CET and are for information purposes; they are not \
tradable quotes."
            .into(),
        docs_url: DOCS_URL.into(),
    }
}

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

/// `BASEQUOTE Curncy` → (`BASE`, `QUOTE`), both ECB currencies.
fn parse_pair(key: &SecurityKey) -> Option<(&'static str, &'static str)> {
    if key.sector != MarketSector::Curncy {
        return None;
    }
    let sym = key.symbol.as_str();
    if sym.len() != 6 || !sym.is_ascii() {
        return None;
    }
    let (b, q) = sym.split_at(3);
    let base = ecb_currency(b)?;
    let quote = ecb_currency(q)?;
    (base != quote).then_some((base, quote))
}

fn ecb_currency(code: &str) -> Option<&'static str> {
    ECB_CURRENCIES.iter().copied().find(|c| c.eq_ignore_ascii_case(code))
}

fn not_covered(key: &SecurityKey) -> ProviderError {
    ProviderError::NotFound(format!("{key} is not an ECB reference-rate pair"))
}

fn price_decimals(base: &str, quote: &str) -> u8 {
    match (SMALL_UNIT_CURRENCIES.contains(&base), SMALL_UNIT_CURRENCIES.contains(&quote)) {
        (false, true) => 2,
        (true, false) => 6,
        _ => 4,
    }
}

/// One rate row as returned by `GET /v2/rates`.
#[derive(Debug, Clone, Deserialize)]
struct RateDto {
    date: NaiveDate,
    quote: String,
    rate: f64,
}

fn parse_rates(body: &str) -> ProviderResult<Vec<RateDto>> {
    let rows: Vec<RateDto> = http::parse_json(body, "Frankfurter rates")?;
    if let Some(bad) = rows.iter().find(|r| !r.rate.is_finite() || r.rate <= 0.0) {
        return Err(ProviderError::parse(format!("Frankfurter rate for {} on {} is {}", bad.quote, bad.date, bad.rate)));
    }
    Ok(rows)
}

/// Latest fixing as `last`, the one before it as `prev_close`. `None` when
/// the window holds no fixing for `quote_ccy`.
fn build_quote(
    key: &SecurityKey,
    quote_ccy: &str,
    rates: &[RateDto],
    today: NaiveDate,
    received: UnixNanos,
) -> Option<Quote> {
    let mut rows: Vec<&RateDto> = rates.iter().filter(|r| r.quote.eq_ignore_ascii_case(quote_ccy)).collect();
    rows.sort_by_key(|r| r.date);
    let latest = *rows.last()?;
    let prev = rows.len().checked_sub(2).map(|i| rows[i]);
    let mut q = Quote::empty(key.clone(), Provenance::synthetic(0));
    q.last = Some(latest.rate);
    q.prev_close = prev.map(|p| p.rate);
    // Fixings are dated, not timed: the event time is the fixing date at
    // 00:00 UTC.
    q.ts_event = date_to_nanos(latest.date);
    q.ts_recv = received;
    if (today - latest.date).num_days() > STALE_AFTER_DAYS {
        q.flags |= quote_flags::STALE;
    }
    Some(q)
}

/// Daily bars (open = high = low = close = the fixing) within
/// `[from, to)`. Reference rates have no volume; `volume` is 0.
fn daily_bars(rates: &[RateDto], quote_ccy: &str, from: Option<UnixNanos>, to: Option<UnixNanos>) -> Vec<Bar> {
    let mut out: Vec<Bar> = rates
        .iter()
        .filter(|r| r.quote.eq_ignore_ascii_case(quote_ccy))
        .map(|r| {
            let ts = date_to_nanos(r.date);
            Bar { ts, open: r.rate, high: r.rate, low: r.rate, close: r.rate, volume: 0.0 }
        })
        .filter(|b| from.is_none_or(|f| b.ts >= f) && to.is_none_or(|t| b.ts < t))
        .collect();
    out.sort_by_key(|b| b.ts);
    out
}

fn week_start(d: NaiveDate) -> NaiveDate {
    d - Days::new(u64::from(d.weekday().num_days_from_monday()))
}

fn month_start(d: NaiveDate) -> NaiveDate {
    d.with_day(1).unwrap_or(d)
}

/// Groups sorted daily fixings into periods keyed by `period_start`: open =
/// first fixing, high/low = extremes, close = last fixing. The bar is
/// stamped at the period start (Monday / the 1st, 00:00 UTC).
fn aggregate(daily: &[Bar], period_start: fn(NaiveDate) -> NaiveDate) -> Vec<Bar> {
    let mut out: Vec<Bar> = Vec::new();
    let mut current: Option<NaiveDate> = None;
    for b in daily {
        let p = period_start(nanos_to_date(b.ts));
        match out.last_mut() {
            Some(agg) if current == Some(p) => {
                agg.high = agg.high.max(b.high);
                agg.low = agg.low.min(b.low);
                agg.close = b.close;
            }
            _ => {
                current = Some(p);
                out.push(Bar { ts: date_to_nanos(p), ..*b });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: &str = include_str!("../tests/fixtures/rates_eur_usd_gbp_window.json");
    const USDJPY: &str = include_str!("../tests/fixtures/rates_usd_jpy_range.json");
    const EMPTY: &str = include_str!("../tests/fixtures/rates_empty.json");

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn parses_pairs() {
        assert_eq!(parse_pair(&SecurityKey::currency("EURUSD")), Some(("EUR", "USD")));
        assert_eq!(parse_pair(&SecurityKey::currency("usdjpy")), Some(("USD", "JPY")));
        assert_eq!(parse_pair(&SecurityKey::currency("BTCUSD")), None);
        assert_eq!(parse_pair(&SecurityKey::currency("EUREUR")), None);
        assert_eq!(parse_pair(&SecurityKey::currency("EURUSDT")), None);
        assert_eq!(parse_pair(&SecurityKey::equity("EURUSD")), None);
    }

    #[test]
    fn covers_only_ecb_pairs() {
        let p = FrankfurterProvider::new().unwrap();
        assert!(p.covers(&SecurityKey::currency("GBPUSD")));
        assert!(!p.covers(&SecurityKey::currency("BTCUSD")));
        assert!(!p.covers(&SecurityKey::equity("AAPL")));
    }

    #[test]
    fn builds_quote_with_previous_fixing() {
        let rates = parse_rates(WINDOW).unwrap();
        let key = SecurityKey::currency("EURUSD");
        let q = build_quote(&key, "USD", &rates, d(2026, 10, 5), 42).unwrap();
        assert_eq!(q.last, Some(1.1204));
        assert_eq!(q.prev_close, Some(1.1225));
        assert_eq!(q.ts_event, date_to_nanos(d(2026, 10, 5)));
        assert_eq!(q.ts_recv, 42);
        assert_eq!(q.flags & quote_flags::STALE, 0);
        // Nothing a fixing doesn't have.
        assert_eq!((q.bid, q.ask, q.open, q.high, q.low, q.volume), (None, None, None, None, None, None));

        let gbp = build_quote(&SecurityKey::currency("EURGBP"), "GBP", &rates, d(2026, 10, 5), 0).unwrap();
        assert_eq!(gbp.last, Some(0.8472));
        assert_eq!(gbp.prev_close, Some(0.85033));
    }

    #[test]
    fn flags_stale_fixings_and_skips_missing() {
        let rates = parse_rates(WINDOW).unwrap();
        let q = build_quote(&SecurityKey::currency("EURUSD"), "USD", &rates, d(2026, 10, 20), 0).unwrap();
        assert_ne!(q.flags & quote_flags::STALE, 0);
        assert!(build_quote(&SecurityKey::currency("EURJPY"), "JPY", &rates, d(2026, 10, 5), 0).is_none());
        assert!(parse_rates(EMPTY).unwrap().is_empty());
    }

    #[test]
    fn daily_bars_respect_half_open_range() {
        let rates = parse_rates(USDJPY).unwrap();
        let all = daily_bars(&rates, "JPY", None, None);
        assert_eq!(all.len(), 25);
        assert_eq!(all[0].ts, date_to_nanos(d(2026, 9, 1)));
        assert_eq!(all[0].close, 160.16);
        assert!(all.iter().all(|b| b.open == b.close && b.high == b.low && b.volume == 0.0));

        let from = date_to_nanos(d(2026, 9, 2));
        let to = date_to_nanos(d(2026, 9, 4));
        let some = daily_bars(&rates, "JPY", Some(from), Some(to));
        assert_eq!(some.iter().map(|b| b.close).collect::<Vec<_>>(), vec![159.6, 156.01]);
    }

    #[test]
    fn aggregates_weeks_and_months() {
        let rates = parse_rates(USDJPY).unwrap();
        let daily = daily_bars(&rates, "JPY", None, None);

        let weeks = aggregate(&daily, week_start);
        // 2026-09-01 is a Tuesday: its week starts Monday 2026-08-31.
        assert_eq!(weeks[0].ts, date_to_nanos(d(2026, 8, 31)));
        assert_eq!((weeks[0].open, weeks[0].high, weeks[0].low, weeks[0].close), (160.16, 160.16, 156.01, 156.25));
        assert_eq!(weeks.len(), 6);

        let months = aggregate(&daily, month_start);
        assert_eq!(months.len(), 2);
        assert_eq!(months[0].ts, date_to_nanos(d(2026, 9, 1)));
        assert_eq!((months[0].open, months[0].close), (160.16, 157.0));
        assert_eq!((months[0].high, months[0].low), (160.16, 153.27));
        assert_eq!((months[1].open, months[1].close), (157.98, 158.23));

        let mut s = BarSeries::new(
            SecurityKey::currency("USDJPY"),
            BarInterval::Week,
            Adjustment::None,
            Provenance::synthetic(0),
        );
        for b in weeks {
            s.push(b);
        }
        assert!(s.validate().is_ok());
    }

    #[test]
    fn rejects_malformed_payloads() {
        assert!(matches!(parse_rates("{\"oops\":1}"), Err(ProviderError::Parse { .. })));
        assert!(matches!(
            parse_rates("[{\"date\":\"2026-10-05\",\"base\":\"EUR\",\"quote\":\"USD\",\"rate\":-1}]"),
            Err(ProviderError::Parse { .. })
        ));
    }

    #[test]
    fn urls_pin_the_ecb_and_never_carry_secrets() {
        let p = FrankfurterProvider::new().unwrap();
        let url = p.rates_url("USD", &["EUR", "JPY"], Some(d(2026, 9, 1)), Some(d(2026, 10, 5)));
        assert_eq!(
            url,
            "https://api.frankfurter.dev/v2/rates?base=USD&quotes=EUR,JPY&providers=ecb&from=2026-09-01&to=2026-10-05"
        );
    }

    #[test]
    fn capabilities_are_honest() {
        let caps = capabilities();
        assert!(caps.supports(Capability::Quotes, Some(AssetClass::Fx)));
        assert!(caps.supports(Capability::DailyBars, Some(AssetClass::Fx)));
        assert!(!caps.supports(Capability::IntradayBars, Some(AssetClass::Fx)));
        assert!(!caps.supports(Capability::Quotes, Some(AssetClass::Crypto)));
        assert!(caps.attribution.as_deref().is_some_and(|a| a.contains("European Central Bank")));
        assert!(!caps.requires_credentials);
    }

    #[test]
    fn decimals_follow_currency_units() {
        assert_eq!(price_decimals("USD", "JPY"), 2);
        assert_eq!(price_decimals("JPY", "USD"), 6);
        assert_eq!(price_decimals("EUR", "USD"), 4);
    }

    #[test]
    fn invalid_currency_body_is_documented_shape() {
        let body = include_str!("../tests/fixtures/error_invalid_currency.json");
        let v: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(v["status"], 422);
    }
}

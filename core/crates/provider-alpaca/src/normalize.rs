//! Pure conversions between Meridian requests/types and Alpaca's wire
//! format. No I/O and no clocks: callers pass the fetch time in.

use std::collections::HashSet;

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use meridian_provider::{ProviderError, ProviderResult};
use meridian_types::{
    Adjustment, BarInterval, BarSeries, DataDelay, Dividend, DividendKind, ExerciseStyle, FeedSource, Greeks,
    GreeksSource, MarketSector, NewsItem, OptionContract, Provenance, ProviderId, Quote, SecurityKey, UnixNanos,
    datetime_to_nanos, nanos_to_datetime,
};

use crate::dto;
use crate::occ::parse_occ;

pub(crate) const PROVIDER_ID: &str = "alpaca";

/// Builds the provenance attached to every payload. `source_ref` is the
/// request URL, which never carries credentials (auth is in headers).
pub(crate) fn provenance(delay: DataDelay, source: FeedSource, as_of: UnixNanos, source_ref: &str) -> Provenance {
    Provenance {
        provider: ProviderId::new(PROVIDER_ID),
        synthetic: false,
        delay,
        source,
        as_of,
        source_ref: Some(source_ref.to_owned()),
        attribution: None,
    }
}

// ---------------------------------------------------------------------------
// Symbols
// ---------------------------------------------------------------------------

/// Alpaca stock symbol for a key, or `None` if Alpaca doesn't serve it.
///
/// Only US equities (`Equity`/`Pfd` sector, exchange `US` or none). Class
/// shares use a dot on Alpaca (`BRK.B`), so the terminal's `BRK/B` is
/// rewritten.
pub(crate) fn alpaca_symbol(key: &SecurityKey) -> Option<String> {
    if !matches!(key.sector, MarketSector::Equity | MarketSector::Pfd) {
        return None;
    }
    if !matches!(key.exchange.as_deref(), None | Some("US")) {
        return None;
    }
    let sym = key.symbol.trim().replace('/', ".").to_ascii_uppercase();
    let valid = !sym.is_empty()
        && sym.len() <= 16
        && !sym.starts_with('.')
        && !sym.ends_with('.')
        && sym.chars().all(|c| c.is_ascii_alphanumeric() || c == '.');
    valid.then_some(sym)
}

/// RFC 3339 timestamp with whole seconds, as Alpaca's `start`/`end` expect.
pub(crate) fn rfc3339(ns: UnixNanos) -> String {
    nanos_to_datetime(ns).to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn nanos(dt: DateTime<Utc>) -> UnixNanos {
    datetime_to_nanos(dt)
}

/// A price of `0` means "no active bid/ask" in Alpaca quotes.
fn positive(v: Option<f64>) -> Option<f64> {
    v.filter(|p| p.is_finite() && *p > 0.0)
}

// ---------------------------------------------------------------------------
// Quotes
// ---------------------------------------------------------------------------

/// Maps one stock snapshot to a [`Quote`].
///
/// `last`/`last_size` come from the latest trade, bid/ask from the latest
/// quote (sizes as reported: shares since 2025-11-03), open/high/low/
/// volume/vwap from the current daily bar, and `prev_close` from the previous
/// daily bar. `ts_event` is the newer of the trade and quote timestamps.
pub(crate) fn snapshot_to_quote(
    key: SecurityKey,
    snap: &dto::StockSnapshot,
    delay: DataDelay,
    source: FeedSource,
    fetched: UnixNanos,
    source_ref: &str,
) -> Quote {
    let trade = snap.latest_trade.as_ref();
    let quote = snap.latest_quote.as_ref();
    let daily = snap.daily_bar.as_ref();

    let bid = positive(quote.and_then(|q| q.bp));
    let ask = positive(quote.and_then(|q| q.ap));

    let ts_event = [trade.and_then(|t| t.t), quote.and_then(|q| q.t)]
        .into_iter()
        .flatten()
        .max()
        .or_else(|| snap.minute_bar.as_ref().and_then(|b| b.t))
        .or_else(|| daily.and_then(|b| b.t))
        .map(nanos);

    let mut q = Quote::empty(key, provenance(delay, source, ts_event.unwrap_or(fetched), source_ref));
    q.bid = bid;
    q.ask = ask;
    q.bid_size = bid.and(quote.and_then(|q| q.bs));
    q.ask_size = ask.and(quote.and_then(|q| q.ask_size));
    q.last = trade.and_then(|t| t.p);
    q.last_size = trade.and_then(|t| t.s);
    q.open = daily.and_then(|b| b.o);
    q.high = daily.and_then(|b| b.h);
    q.low = daily.and_then(|b| b.l);
    q.volume = daily.and_then(|b| b.v);
    q.vwap = daily.and_then(|b| b.vw);
    q.prev_close = snap.prev_daily_bar.as_ref().and_then(|b| b.c);
    q.ts_event = ts_event.unwrap_or(fetched);
    q.ts_recv = fetched;
    q
}

// ---------------------------------------------------------------------------
// Bars
// ---------------------------------------------------------------------------

/// Alpaca `timeframe` for an interval: `[1-59]Min`, `[1-23]Hour`, `1Day`,
/// `1Week`, `1Month`. Whole-hour minute counts are expressed in hours.
pub(crate) fn timeframe(interval: BarInterval) -> Option<String> {
    match interval {
        BarInterval::Minute(n @ 1..=59) => Some(format!("{n}Min")),
        BarInterval::Minute(n) if n % 60 == 0 && (1..=23).contains(&(n / 60)) => Some(format!("{}Hour", n / 60)),
        BarInterval::Hour(n @ 1..=23) => Some(format!("{n}Hour")),
        BarInterval::Day => Some("1Day".into()),
        BarInterval::Week => Some("1Week".into()),
        BarInterval::Month => Some("1Month".into()),
        BarInterval::Minute(_) | BarInterval::Hour(_) => None,
    }
}

/// Alpaca `adjustment` parameter. `all` also applies spin-off adjustments.
pub(crate) fn adjustment_param(adj: Adjustment) -> &'static str {
    match adj {
        Adjustment::None => "raw",
        Adjustment::Splits => "split",
        Adjustment::SplitsAndDividends => "all",
    }
}

/// Earliest default start for daily-or-longer bars (Alpaca: "since 2016").
pub(crate) fn default_daily_start() -> UnixNanos {
    NaiveDate::from_ymd_opt(2016, 1, 1).and_then(|d| d.and_hms_opt(0, 0, 0)).map_or(0, |dt| nanos(dt.and_utc()))
}

/// Default intraday look-back when the request has no `from`.
pub(crate) const DEFAULT_INTRADAY_LOOKBACK_DAYS: i64 = 30;

/// Maximum page size for bars (documented maximum).
pub(crate) const BARS_PAGE_LIMIT: u32 = 10_000;

/// Inputs for one bars request; pagination only changes `page_token`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BarsParams {
    pub timeframe: String,
    pub start: UnixNanos,
    pub end: UnixNanos,
    pub adjustment: Adjustment,
    pub feed: &'static str,
}

/// Query string for `GET /v2/stocks/{symbol}/bars`.
pub(crate) fn bars_query(p: &BarsParams, page_token: Option<&str>) -> Vec<(&'static str, String)> {
    let mut q = vec![
        ("timeframe", p.timeframe.clone()),
        ("start", rfc3339(p.start)),
        ("end", rfc3339(p.end)),
        ("limit", BARS_PAGE_LIMIT.to_string()),
        ("adjustment", adjustment_param(p.adjustment).to_owned()),
        ("feed", p.feed.to_owned()),
        ("sort", "asc".to_owned()),
    ];
    if let Some(t) = page_token {
        q.push(("page_token", t.to_owned()));
    }
    q
}

/// Validates a `next_page_token`: `None`/empty ends pagination; a token seen
/// before means the server is looping, which is a format problem, not a
/// reason to spin forever.
pub(crate) fn next_page(
    token: Option<String>,
    seen: &mut HashSet<String>,
    context: &str,
) -> ProviderResult<Option<String>> {
    match token.filter(|t| !t.is_empty()) {
        None => Ok(None),
        Some(t) if !seen.insert(t.clone()) => Err(ProviderError::parse(format!("{context}: repeated next_page_token"))),
        Some(t) => Ok(Some(t)),
    }
}

/// Appends bars inside `[from, to)` to `series`. Alpaca's `end` is
/// inclusive, so the exclusive bound is applied here.
pub(crate) fn push_bars(series: &mut BarSeries, bars: &[dto::Bar], from: UnixNanos, to: Option<UnixNanos>) {
    for b in bars {
        let ts = nanos(b.t);
        if ts < from || to.is_some_and(|to| ts >= to) {
            continue;
        }
        series.push(meridian_types::Bar { ts, open: b.o, high: b.h, low: b.l, close: b.c, volume: b.v });
    }
}

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/// Maps one option snapshot. Returns `None` if the symbol isn't a valid OCC
/// symbol. Open interest isn't in Alpaca snapshots, so it is always `None`.
pub(crate) fn option_contract(symbol: &str, snap: &dto::OptionSnapshot) -> Option<OptionContract> {
    let occ = parse_occ(symbol)?;
    let quote = snap.latest_quote.as_ref();
    let bid = positive(quote.and_then(|q| q.bp));
    let ask = positive(quote.and_then(|q| q.ap));
    let greeks = match (&snap.greeks, snap.implied_volatility) {
        (None, None) => None,
        (g, iv) => Some(Greeks {
            iv,
            delta: g.as_ref().and_then(|g| g.delta),
            gamma: g.as_ref().and_then(|g| g.gamma),
            theta: g.as_ref().and_then(|g| g.theta),
            vega: g.as_ref().and_then(|g| g.vega),
            rho: g.as_ref().and_then(|g| g.rho),
            source: GreeksSource::Vendor,
        }),
    };
    Some(OptionContract {
        contract_symbol: OptionContract::occ_symbol(&occ.root, occ.expiry, occ.right, occ.strike),
        expiry: occ.expiry,
        strike: occ.strike,
        right: occ.right,
        style: ExerciseStyle::American,
        multiplier: 100.0,
        bid,
        ask,
        bid_size: bid.and(quote.and_then(|q| q.bs)),
        ask_size: ask.and(quote.and_then(|q| q.ask_size)),
        last: snap.latest_trade.as_ref().and_then(|t| t.p),
        volume: snap.daily_bar.as_ref().and_then(|b| b.v),
        open_interest: None,
        greeks,
    })
}

/// Newest quote or trade timestamp in an option snapshot.
pub(crate) fn option_snapshot_time(snap: &dto::OptionSnapshot) -> Option<UnixNanos> {
    [snap.latest_quote.as_ref().and_then(|q| q.t), snap.latest_trade.as_ref().and_then(|t| t.t)]
        .into_iter()
        .flatten()
        .max()
        .map(nanos)
}

/// Sorts contracts by expiry, strike, then calls before puts.
pub(crate) fn sort_contracts(contracts: &mut [OptionContract]) {
    contracts.sort_by(|a, b| {
        a.expiry
            .cmp(&b.expiry)
            .then(a.strike.total_cmp(&b.strike))
            .then(a.right.cmp(&b.right))
            .then(a.contract_symbol.cmp(&b.contract_symbol))
    });
}

// ---------------------------------------------------------------------------
// News
// ---------------------------------------------------------------------------

/// Maps one article. `body` stays `None`: we request `include_content=false`.
pub(crate) fn news_item(a: &dto::NewsArticle, fetched: UnixNanos, source_ref: &str) -> NewsItem {
    let as_of = a.updated_at.map_or_else(|| nanos(a.created_at), nanos);
    NewsItem {
        id: a.id.to_string(),
        source: a.source.clone().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "alpaca".into()),
        headline: a.headline.as_deref().map(strip_html).unwrap_or_default(),
        summary: a.summary.as_deref().map(strip_html).filter(|s| !s.is_empty()),
        body: None,
        url: a.url.clone().filter(|u| !u.trim().is_empty()),
        published_at: nanos(a.created_at),
        received_at: fetched,
        tickers: a.symbols.clone(),
        topics: Vec::new(),
        provenance: provenance(DataDelay::RealTime, FeedSource::Aggregated, as_of, source_ref),
    }
}

/// Case-insensitive substring match on headline and summary.
pub(crate) fn matches_text(item: &NewsItem, text: Option<&str>) -> bool {
    let Some(needle) = text.map(str::trim).filter(|t| !t.is_empty()) else {
        return true;
    };
    let needle = needle.to_lowercase();
    item.headline.to_lowercase().contains(&needle)
        || item.summary.as_deref().is_some_and(|s| s.to_lowercase().contains(&needle))
}

/// Removes HTML tags, decodes common entities, and collapses whitespace.
pub(crate) fn strip_html(s: &str) -> String {
    let mut text = String::with_capacity(s.len());
    let mut tag: Option<String> = None;
    for c in s.chars() {
        match (&mut tag, c) {
            (None, '<') => tag = Some(String::new()),
            (None, _) => text.push(c),
            (Some(t), '>') => {
                // Block-level tags separate words; inline tags don't.
                let name: String = t
                    .trim_start_matches('/')
                    .chars()
                    .take_while(char::is_ascii_alphanumeric)
                    .collect::<String>()
                    .to_ascii_lowercase();
                if matches!(
                    name.as_str(),
                    "p" | "br"
                        | "div"
                        | "li"
                        | "ul"
                        | "ol"
                        | "tr"
                        | "td"
                        | "th"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                ) {
                    text.push(' ');
                }
                tag = None;
            }
            (Some(t), _) => t.push(c),
        }
    }
    let decoded = decode_entities(&text);
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let decoded = tail.find(';').filter(|&j| j <= 10).and_then(|j| entity(&tail[1..j]).map(|c| (c, j)));
        if let Some((c, j)) = decoded {
            out.push(c);
            rest = &tail[j + 1..];
        } else {
            out.push('&');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    if let Some(num) = name.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse().ok()?,
        };
        return char::from_u32(code);
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "ldquo" => '\u{201C}',
        "rdquo" => '\u{201D}',
        "lsquo" => '\u{2018}',
        "rsquo" => '\u{2019}',
        "mdash" => '\u{2014}',
        "ndash" => '\u{2013}',
        "hellip" => '\u{2026}',
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Corporate actions
// ---------------------------------------------------------------------------

/// Action types requested from `GET /v1/corporate-actions`.
pub(crate) const CORPORATE_ACTION_TYPES: &str = "cash_dividend,forward_split,reverse_split";

/// Documented maximum `limit` for corporate actions.
pub(crate) const CORPORATE_ACTIONS_PAGE_LIMIT: u32 = 1000;

/// Query string for `GET /v1/corporate-actions`. `start`/`end` filter on
/// Alpaca's `process_date` (inclusive). Newest first.
pub(crate) fn corporate_actions_query(
    symbol: &str,
    start: NaiveDate,
    end: NaiveDate,
    page_token: Option<&str>,
) -> Vec<(&'static str, String)> {
    let mut q = vec![
        ("symbols", symbol.to_owned()),
        ("types", CORPORATE_ACTION_TYPES.to_owned()),
        ("start", start.format("%Y-%m-%d").to_string()),
        ("end", end.format("%Y-%m-%d").to_string()),
        ("limit", CORPORATE_ACTIONS_PAGE_LIMIT.to_string()),
        ("sort", "desc".to_owned()),
    ];
    if let Some(t) = page_token {
        q.push(("page_token", t.to_owned()));
    }
    q
}

fn ymd(s: Option<&str>) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s?.trim(), "%Y-%m-%d").ok()
}

fn same_symbol(record: Option<&str>, symbol: &str) -> bool {
    record.is_some_and(|r| r.trim().eq_ignore_ascii_case(symbol))
}

/// Corporate actions for `symbol` → dividend and split events. Records for
/// other symbols are ignored. Returns the events and the number of records
/// skipped because the ex-date or amount was missing or invalid.
///
/// - Cash dividend → `Regular`, or `Special` when `special`; `amount` = `rate`
///   (per share); `currency` as sent, empty when Alpaca leaves it empty
///   (documented as possibly USD, not applicable, or unknown). There is no
///   declaration date or frequency in the response.
/// - Forward and reverse splits → `Split` with `amount` = `new_rate /
///   old_rate` (2.0 for 2-for-1, 0.1 for 1-for-10).
pub(crate) fn corporate_action_events(ca: &dto::CorporateActions, symbol: &str) -> (Vec<Dividend>, usize) {
    let mut out = Vec::new();
    let mut skipped = 0;
    for c in ca.cash_dividends.iter().flatten().filter(|c| same_symbol(c.symbol.as_deref(), symbol)) {
        match (ymd(c.ex_date.as_deref()), c.rate.filter(|r| r.is_finite() && *r >= 0.0)) {
            (Some(ex_date), Some(rate)) => out.push(Dividend {
                declared_date: None,
                ex_date,
                record_date: ymd(c.record_date.as_deref()),
                pay_date: ymd(c.payable_date.as_deref()),
                amount: rate,
                currency: c.currency.as_deref().map(str::trim).unwrap_or_default().to_owned(),
                frequency: None,
                kind: if c.special == Some(true) { DividendKind::Special } else { DividendKind::Regular },
            }),
            _ => skipped += 1,
        }
    }
    let splits = ca.forward_splits.iter().flatten().chain(ca.reverse_splits.iter().flatten());
    for s in splits.filter(|s| same_symbol(s.symbol.as_deref(), symbol)) {
        let ratio = match (s.new_rate, s.old_rate) {
            (Some(n), Some(o)) if n > 0.0 && o > 0.0 && (n / o).is_finite() => Some(n / o),
            _ => None,
        };
        match (ymd(s.ex_date.as_deref()), ratio) {
            (Some(ex_date), Some(ratio)) => out.push(Dividend {
                declared_date: None,
                ex_date,
                record_date: ymd(s.record_date.as_deref()),
                pay_date: ymd(s.payable_date.as_deref()),
                amount: ratio,
                currency: String::new(),
                frequency: None,
                kind: DividendKind::Split,
            }),
            _ => skipped += 1,
        }
    }
    (out, skipped)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use meridian_types::OptionRight;

    use super::*;
    use crate::http::parse_json;

    const URL: &str = "https://data.alpaca.markets/v2/stocks/snapshots?symbols=AAPL%2CTSLA&feed=iex";

    fn ns(s: &str) -> UnixNanos {
        nanos(DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc))
    }

    #[test]
    fn symbol_mapping() {
        assert_eq!(alpaca_symbol(&SecurityKey::equity("AAPL")).as_deref(), Some("AAPL"));
        assert_eq!(alpaca_symbol(&"BRK/B US Equity".parse().unwrap()).as_deref(), Some("BRK.B"));
        assert_eq!(alpaca_symbol(&"BRK.B Equity".parse().unwrap()).as_deref(), Some("BRK.B"));
        assert_eq!(alpaca_symbol(&"VOD LN Equity".parse().unwrap()), None);
        assert_eq!(alpaca_symbol(&SecurityKey::currency("EURUSD")), None);
        assert_eq!(alpaca_symbol(&SecurityKey::index("SPX")), None);
        assert_eq!(alpaca_symbol(&SecurityKey::equity("A-B")), None);
        assert_eq!(alpaca_symbol(&SecurityKey::equity(".X")), None);
    }

    #[test]
    fn snapshot_maps_to_quote() {
        let body = include_str!("../tests/fixtures/stock_snapshots.json");
        let resp: dto::StockSnapshotsResp = parse_json(body, "snapshots").unwrap();
        let snap = resp["AAPL"].as_ref().unwrap();
        let fetched = ns("2022-08-17T10:20:00Z");
        let q = snapshot_to_quote(
            SecurityKey::equity("AAPL"),
            snap,
            DataDelay::RealTime,
            FeedSource::SingleVenue("IEX".into()),
            fetched,
            URL,
        );
        assert_eq!(q.last, Some(172.61));
        assert_eq!(q.last_size, Some(160.0));
        assert_eq!(q.bid, Some(172.6));
        assert_eq!(q.bid_size, Some(2.0));
        assert_eq!(q.ask, Some(172.7));
        assert_eq!(q.ask_size, Some(5.0));
        assert_eq!(q.open, Some(172.62));
        assert_eq!(q.high, Some(173.71));
        assert_eq!(q.low, Some(171.6618));
        assert_eq!(q.volume, Some(56_457_696.0));
        assert_eq!(q.vwap, Some(172.743391));
        assert_eq!(q.prev_close, Some(173.19));
        // Quote (10:18:27) is newer than the trade (10:18:24).
        assert_eq!(q.ts_event, ns("2022-08-17T10:18:27.052763263Z"));
        assert_eq!(q.ts_recv, fetched);
        assert_eq!(q.provenance.provider.as_str(), "alpaca");
        assert!(!q.provenance.synthetic);
        assert_eq!(q.provenance.as_of, q.ts_event);
        assert_eq!(q.provenance.source, FeedSource::SingleVenue("IEX".into()));
        assert_eq!(q.provenance.source_ref.as_deref(), Some(URL));
        assert_eq!(q.flags, 0);

        let tsla = snapshot_to_quote(
            SecurityKey::equity("TSLA"),
            resp["TSLA"].as_ref().unwrap(),
            DataDelay::RealTime,
            FeedSource::Consolidated,
            fetched,
            URL,
        );
        assert_eq!(tsla.last, Some(911.99));
        assert_eq!(tsla.prev_close, Some(927.96));
        assert_eq!(tsla.open, Some(935.0));
    }

    #[test]
    fn empty_snapshot_and_zero_prices_become_none() {
        let snap: dto::StockSnapshot =
            parse_json(r#"{"latestQuote":{"ap":0,"as":0,"bp":0,"bs":0,"t":"2026-01-02T15:00:00Z"}}"#, "snap").unwrap();
        let fetched = ns("2026-01-02T15:00:05Z");
        let q = snapshot_to_quote(
            SecurityKey::equity("TEST"),
            &snap,
            DataDelay::RealTime,
            FeedSource::Consolidated,
            fetched,
            URL,
        );
        assert_eq!((q.bid, q.ask, q.bid_size, q.ask_size), (None, None, None, None));
        assert_eq!((q.last, q.open, q.prev_close, q.volume), (None, None, None, None));
        assert_eq!(q.ts_event, ns("2026-01-02T15:00:00Z"));

        let none: dto::StockSnapshot = parse_json("{}", "snap").unwrap();
        let q = snapshot_to_quote(
            SecurityKey::equity("TEST"),
            &none,
            DataDelay::RealTime,
            FeedSource::Consolidated,
            fetched,
            URL,
        );
        assert_eq!(q.ts_event, fetched);
        assert_eq!(q.provenance.as_of, fetched);
    }

    #[test]
    fn snapshots_response_tolerates_null_entries() {
        let resp: dto::StockSnapshotsResp = parse_json(r#"{"AAPL":null}"#, "snapshots").unwrap();
        assert!(resp["AAPL"].is_none());
    }

    #[test]
    fn timeframe_mapping() {
        assert_eq!(timeframe(BarInterval::Minute(1)).as_deref(), Some("1Min"));
        assert_eq!(timeframe(BarInterval::Minute(59)).as_deref(), Some("59Min"));
        assert_eq!(timeframe(BarInterval::Minute(60)).as_deref(), Some("1Hour"));
        assert_eq!(timeframe(BarInterval::Minute(120)).as_deref(), Some("2Hour"));
        assert_eq!(timeframe(BarInterval::Minute(90)), None);
        assert_eq!(timeframe(BarInterval::Hour(4)).as_deref(), Some("4Hour"));
        assert_eq!(timeframe(BarInterval::Hour(24)), None);
        assert_eq!(timeframe(BarInterval::Day).as_deref(), Some("1Day"));
        assert_eq!(timeframe(BarInterval::Week).as_deref(), Some("1Week"));
        assert_eq!(timeframe(BarInterval::Month).as_deref(), Some("1Month"));
    }

    #[test]
    fn adjustment_mapping() {
        assert_eq!(adjustment_param(Adjustment::None), "raw");
        assert_eq!(adjustment_param(Adjustment::Splits), "split");
        assert_eq!(adjustment_param(Adjustment::SplitsAndDividends), "all");
    }

    #[test]
    fn bars_query_and_pagination() {
        let p = BarsParams {
            timeframe: "1Day".into(),
            start: ns("2016-01-01T00:00:00Z"),
            end: ns("2026-10-05T13:44:00.5Z"),
            adjustment: Adjustment::SplitsAndDividends,
            feed: "sip",
        };
        let first = bars_query(&p, None);
        let get = |q: &[(&str, String)], k: &str| q.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
        assert_eq!(get(&first, "timeframe").as_deref(), Some("1Day"));
        assert_eq!(get(&first, "start").as_deref(), Some("2016-01-01T00:00:00Z"));
        assert_eq!(get(&first, "end").as_deref(), Some("2026-10-05T13:44:00Z"));
        assert_eq!(get(&first, "limit").as_deref(), Some("10000"));
        assert_eq!(get(&first, "adjustment").as_deref(), Some("all"));
        assert_eq!(get(&first, "feed").as_deref(), Some("sip"));
        assert_eq!(get(&first, "page_token"), None);

        let page: dto::StockBarsResp =
            parse_json(include_str!("../tests/fixtures/stock_bars_single.json"), "bars").unwrap();
        let mut seen = HashSet::new();
        let token = next_page(page.next_page_token, &mut seen, "bars").unwrap();
        assert_eq!(token.as_deref(), Some("QUFQTHxNfDIwMjItMDEtMDNUMDk6MDA6MDAuMDAwMDAwMDAwWg=="));
        let second = bars_query(&p, token.as_deref());
        assert_eq!(get(&second, "page_token"), token);
        // Everything else is unchanged between pages.
        assert_eq!(&second[..first.len()], &first[..]);

        let last: dto::StockBarsResp =
            parse_json(include_str!("../tests/fixtures/stock_bars_single_last_page.json"), "bars").unwrap();
        assert_eq!(next_page(last.next_page_token, &mut seen, "bars").unwrap(), None);
        assert_eq!(next_page(Some(String::new()), &mut seen, "bars").unwrap(), None);
        assert!(matches!(next_page(token, &mut seen, "bars"), Err(ProviderError::Parse { .. })));
    }

    #[test]
    fn bars_parse_and_half_open_filter() {
        let page: dto::StockBarsResp =
            parse_json(include_str!("../tests/fixtures/stock_bars_single.json"), "bars").unwrap();
        let bars = page.bars.unwrap();
        let prov = provenance(DataDelay::RealTime, FeedSource::Consolidated, 0, URL);
        let t = ns("2022-01-03T09:00:00Z");

        let mut s = BarSeries::new(SecurityKey::equity("AAPL"), BarInterval::Minute(1), Adjustment::None, prov.clone());
        push_bars(&mut s, &bars, t, Some(t + 60_000_000_000));
        assert_eq!(s.len(), 1);
        assert_eq!(s.ts[0], t);
        assert_eq!((s.open[0], s.high[0], s.low[0], s.close[0], s.volume[0]), (178.26, 178.26, 178.21, 178.21, 1118.0));
        assert!(s.validate().is_ok());

        // `to` is exclusive; `from` is inclusive.
        let mut s = BarSeries::new(SecurityKey::equity("AAPL"), BarInterval::Minute(1), Adjustment::None, prov.clone());
        push_bars(&mut s, &bars, t, Some(t));
        assert!(s.is_empty());
        let mut s = BarSeries::new(SecurityKey::equity("AAPL"), BarInterval::Minute(1), Adjustment::None, prov);
        push_bars(&mut s, &bars, t + 1, None);
        assert!(s.is_empty());

        let null_bars: dto::StockBarsResp =
            parse_json(r#"{"bars":null,"symbol":"X","next_page_token":null}"#, "bars").unwrap();
        assert!(null_bars.bars.is_none());
    }

    #[test]
    fn option_snapshot_maps_with_vendor_greeks() {
        let resp: dto::OptionChainResp =
            parse_json(include_str!("../tests/fixtures/option_chain.json"), "chain").unwrap();
        assert_eq!(resp.next_page_token.as_deref(), Some("QUFQTDI0MDYyMTAwMDIyMHwx"));
        let snaps = resp.snapshots.unwrap();
        let snap = snaps["AAPL240426C00162500"].as_ref().unwrap();
        let c = option_contract("AAPL240426C00162500", snap).unwrap();
        assert_eq!(c.contract_symbol, "AAPL  240426C00162500");
        assert_eq!(c.expiry, NaiveDate::from_ymd_opt(2024, 4, 26).unwrap());
        assert_eq!(c.strike, 162.5);
        assert_eq!(c.right, OptionRight::Call);
        assert_eq!(c.style, ExerciseStyle::American);
        assert_eq!(c.multiplier, 100.0);
        assert_eq!((c.bid, c.ask), (Some(4.15), Some(4.3)));
        assert_eq!((c.bid_size, c.ask_size), (Some(16.0), Some(91.0)));
        assert_eq!(c.last, Some(4.1));
        assert_eq!(c.volume, None);
        assert_eq!(c.open_interest, None);
        let g = c.greeks.unwrap();
        assert_eq!(g.source, GreeksSource::Vendor);
        assert_eq!(g.iv, Some(0.3372405712050441));
        assert_eq!(g.delta, Some(0.7521304109871954));
        assert_eq!(g.gamma, Some(0.06241426404871288));
        assert_eq!(g.theta, Some(-0.2847623059595503));
        assert_eq!(g.vega, Some(0.047540520834498785));
        assert_eq!(g.rho, Some(0.009910739032549095));
        assert_eq!(option_snapshot_time(snap), Some(ns("2024-04-22T19:59:59.992734208Z")));
    }

    #[test]
    fn option_snapshot_without_greeks_or_quote() {
        let resp: dto::OptionChainResp =
            parse_json(include_str!("../tests/fixtures/option_chain_constructed.json"), "chain").unwrap();
        assert_eq!(resp.next_page_token, None);
        let snaps: HashMap<String, dto::OptionSnapshot> =
            resp.snapshots.unwrap().into_iter().filter_map(|(k, v)| v.map(|v| (k, v))).collect();

        let put = option_contract("TEST261218P00012500", &snaps["TEST261218P00012500"]).unwrap();
        assert_eq!(put.contract_symbol, "TEST  261218P00012500");
        assert_eq!(put.right, OptionRight::Put);
        assert_eq!(put.strike, 12.5);
        assert_eq!((put.bid, put.ask, put.bid_size, put.ask_size), (None, None, None, None));
        assert_eq!(put.greeks, None);

        let call = option_contract("X261218C00002500", &snaps["X261218C00002500"]).unwrap();
        assert_eq!(call.contract_symbol, "X     261218C00002500");
        assert_eq!(call.last, Some(0.05));
        assert_eq!(call.volume, Some(3.0));
        let g = call.greeks.as_ref().unwrap();
        assert_eq!(g.iv, Some(0.5));
        assert_eq!(g.delta, None);

        assert!(option_contract("NOT-OCC", &snaps["X261218C00002500"]).is_none());

        let mut contracts = vec![call, put];
        sort_contracts(&mut contracts);
        assert_eq!(contracts[0].strike, 2.5);
    }

    #[test]
    fn news_maps_and_filters() {
        let resp: dto::NewsResp = parse_json(include_str!("../tests/fixtures/news.json"), "news").unwrap();
        assert_eq!(resp.next_page_token.as_deref(), Some("MTY0MDk0ODkyMzAwMDAwMDAwMHwyNDg0MzE3MQ=="));
        let a = &resp.news.unwrap()[0];
        let fetched = ns("2022-01-01T00:00:00Z");
        let item = news_item(a, fetched, "https://data.alpaca.markets/v1beta1/news?symbols=AAPL");
        assert_eq!(item.id, "24843171");
        assert_eq!(item.source, "benzinga");
        assert!(item.headline.starts_with("Apple Leader in Phone Sales in China"));
        assert_eq!(
            item.summary.as_deref(),
            Some(
                "This headline-only article is meant to show you why a stock is moving, the most difficult aspect of stock trading"
            )
        );
        assert_eq!(item.body, None);
        assert!(item.url.as_deref().unwrap().starts_with("https://www.benzinga.com/news/21/12/24843171/"));
        assert_eq!(item.published_at, ns("2021-12-31T11:08:42Z"));
        assert_eq!(item.received_at, fetched);
        assert_eq!(item.tickers, vec!["AAPL".to_owned()]);
        assert!(item.topics.is_empty());
        assert_eq!(item.provenance.as_of, ns("2021-12-31T11:08:43Z"));
        assert_eq!(item.provenance.source, FeedSource::Aggregated);
        assert_eq!(item.provenance.delay, DataDelay::RealTime);

        assert!(matches_text(&item, None));
        assert!(matches_text(&item, Some("  ")));
        assert!(matches_text(&item, Some("phone SALES")));
        assert!(matches_text(&item, Some("difficult aspect")));
        assert!(!matches_text(&item, Some("microsoft")));
    }

    #[test]
    fn html_is_stripped() {
        assert_eq!(strip_html("<p>Hello <b>world</b></p><p>Again</p>"), "Hello world Again");
        assert_eq!(strip_html("line<br/>break"), "line break");
        assert_eq!(
            strip_html(
                "Corsair Gaming, Inc. (NASDAQ:<a class=\"ticker\" href=\"x\">CRSR</a>) (&ldquo;Corsair&rdquo;) &amp; co"
            ),
            "Corsair Gaming, Inc. (NASDAQ:CRSR) (\u{201C}Corsair\u{201D}) & co"
        );
        assert_eq!(strip_html("AT&T &#36;5 &#x41; &bogus; a&b"), "AT&T $5 A &bogus; a&b");
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn documented_corporate_action_examples() {
        let body = include_str!("../tests/fixtures/corporate_actions_examples.json");
        let resp: dto::CorporateActionsResp = parse_json(body, "corporate actions").unwrap();
        assert_eq!(resp.next_page_token, None);
        let ca = resp.corporate_actions.unwrap();

        let (fcf, skipped) = corporate_action_events(&ca, "FCF");
        assert_eq!(skipped, 0);
        assert_eq!(
            fcf,
            vec![Dividend {
                declared_date: None,
                ex_date: d(2023, 5, 4),
                record_date: Some(d(2023, 5, 5)),
                pay_date: Some(d(2023, 5, 19)),
                amount: 0.125,
                // The example predates the `currency` field: unknown, not assumed USD.
                currency: String::new(),
                frequency: None,
                kind: DividendKind::Regular,
            }]
        );

        let (sre, _) = corporate_action_events(&ca, "sre");
        assert_eq!(sre.len(), 1);
        assert_eq!(sre[0].kind, DividendKind::Split);
        assert_eq!(sre[0].amount, 2.0);
        assert_eq!(sre[0].ex_date, d(2023, 8, 22));
        assert_eq!(sre[0].record_date, Some(d(2023, 8, 14)));
        assert_eq!(sre[0].pay_date, Some(d(2023, 8, 21)));

        // 1-for-50 reverse split.
        let (mnts, _) = corporate_action_events(&ca, "MNTS");
        assert_eq!(mnts.len(), 1);
        assert_eq!(mnts[0].kind, DividendKind::Split);
        assert!((mnts[0].amount - 0.02).abs() < 1e-12);
        assert_eq!(mnts[0].pay_date, None);

        assert!(corporate_action_events(&ca, "AAPL").0.is_empty());
    }

    #[test]
    fn corporate_actions_skip_invalid_records() {
        let body = include_str!("../tests/fixtures/corporate_actions_constructed_page1.json");
        let resp: dto::CorporateActionsResp = parse_json(body, "corporate actions").unwrap();
        let (events, skipped) = corporate_action_events(&resp.corporate_actions.unwrap(), "TESTA");
        // The record without an ex-date is skipped; TESTB's is someone else's.
        assert_eq!(skipped, 1);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, DividendKind::Regular);
        assert_eq!(events[0].currency, "USD");
        assert_eq!(events[1].kind, DividendKind::Special);
        assert_eq!(events[1].amount, 1.25);
        assert_eq!(events[1].currency, "");
        assert_eq!(events[1].record_date, None);

        let bad = dto::CorporateActions {
            cash_dividends: None,
            forward_splits: Some(vec![dto::Split {
                symbol: Some("TESTA".into()),
                new_rate: Some(2.0),
                old_rate: Some(0.0),
                ex_date: Some("2024-01-02".into()),
                record_date: None,
                payable_date: None,
            }]),
            reverse_splits: None,
        };
        assert_eq!(corporate_action_events(&bad, "TESTA"), (Vec::new(), 1));
    }

    #[test]
    fn corporate_actions_query_shape() {
        let q = corporate_actions_query("BRK.B", d(2016, 10, 7), d(2027, 1, 5), Some("tok"));
        let q: HashMap<&str, &str> = q.iter().map(|(k, v)| (*k, v.as_str())).collect();
        assert_eq!(q["symbols"], "BRK.B");
        assert_eq!(q["types"], "cash_dividend,forward_split,reverse_split");
        assert_eq!(q["start"], "2016-10-07");
        assert_eq!(q["end"], "2027-01-05");
        assert_eq!(q["limit"], "1000");
        assert_eq!(q["sort"], "desc");
        assert_eq!(q["page_token"], "tok");
        assert!(!corporate_actions_query("X", d(2020, 1, 1), d(2020, 1, 2), None).iter().any(|(k, _)| *k == "page_token"));
    }
}

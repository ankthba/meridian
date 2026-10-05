//! Vendor DTOs -> normalized `meridian_types`.
//!
//! Field semantics (see README): every "session" figure is Coinbase's
//! rolling 24 h window. `open` and `prev_close` are both the price 24 h ago
//! (`open_24h` / stats `open`), so the net change on screen is the rolling
//! 24 h change. `volume` is 24 h volume in base currency.

use chrono::{DateTime, Utc};
use meridian_provider::{ProviderError, ProviderResult};
use meridian_types::{
    Bar, DataDelay, FeedSource, Provenance, ProviderId, Quote, QuoteUpdate, SecurityKey, UnixNanos,
    datetime_to_nanos, nanos_from_secs,
};

use crate::PROVIDER_ID;
use crate::dto::{CandleRow, Num, StatsDto, TickerDto, WsTicker};

pub(crate) const FEED: &str = "COINBASE";

pub(crate) fn provenance(as_of: UnixNanos, source_ref: String) -> Provenance {
    Provenance {
        provider: ProviderId::new(PROVIDER_ID),
        synthetic: false,
        delay: DataDelay::RealTime,
        source: FeedSource::Exchange(FEED.to_owned()),
        as_of,
        source_ref: Some(source_ref),
        attribution: None,
    }
}

/// Optional number: absent or blank is `None`; anything else must parse.
pub(crate) fn num(v: Option<&Num>, field: &str) -> ProviderResult<Option<f64>> {
    match v {
        None => Ok(None),
        Some(n) if n.is_blank() => Ok(None),
        Some(n) => n
            .to_f64()
            .map(Some)
            .ok_or_else(|| ProviderError::parse(format!("coinbase {field}: {:?}", n.raw()))),
    }
}

/// RFC 3339 -> Unix nanos.
pub(crate) fn parse_time(s: &str) -> Option<UnixNanos> {
    DateTime::parse_from_rfc3339(s.trim())
        .ok()
        .map(|d| datetime_to_nanos(d.with_timezone(&Utc)))
}

fn opt_time(s: Option<&str>, field: &str) -> ProviderResult<Option<UnixNanos>> {
    match s.map(str::trim) {
        None | Some("") => Ok(None),
        Some(t) => parse_time(t)
            .map(Some)
            .ok_or_else(|| ProviderError::parse(format!("coinbase {field}: {t:?}"))),
    }
}

/// REST snapshot from `/ticker` + `/stats`. `ts_event` and `as_of` are the
/// last trade time when given, else the fetch time.
pub(crate) fn quote(
    key: SecurityKey,
    ticker: &TickerDto,
    stats: &StatsDto,
    source_ref: String,
    fetched_at: UnixNanos,
) -> ProviderResult<Quote> {
    let ts = opt_time(ticker.time.as_deref(), "ticker.time")?.unwrap_or(fetched_at);
    let open_24h = num(stats.open.as_ref(), "stats.open")?;
    let mut q = Quote::empty(key, provenance(ts, source_ref));
    q.bid = num(ticker.bid.as_ref(), "ticker.bid")?;
    q.ask = num(ticker.ask.as_ref(), "ticker.ask")?;
    q.last = num(ticker.price.as_ref(), "ticker.price")?;
    q.last_size = num(ticker.size.as_ref(), "ticker.size")?;
    q.volume = match num(ticker.volume.as_ref(), "ticker.volume")? {
        Some(v) => Some(v),
        None => num(stats.volume.as_ref(), "stats.volume")?,
    };
    q.open = open_24h;
    q.prev_close = open_24h;
    q.high = num(stats.high.as_ref(), "stats.high")?;
    q.low = num(stats.low.as_ref(), "stats.low")?;
    q.ts_event = ts;
    q.ts_recv = fetched_at;
    Ok(q)
}

/// Candle rows `[time, low, high, open, close, volume]` -> bars. Order is
/// preserved (Coinbase sends newest first).
pub(crate) fn candles(rows: &[CandleRow]) -> ProviderResult<Vec<Bar>> {
    rows.iter()
        .map(|CandleRow(t, low, high, open, close, volume)| {
            let f = |n: &Num, field: &str| {
                n.to_f64().ok_or_else(|| {
                    ProviderError::parse(format!("coinbase candle {field} at {t}: {:?}", n.raw()))
                })
            };
            Ok(Bar {
                ts: nanos_from_secs(*t),
                open: f(open, "open")?,
                high: f(high, "high")?,
                low: f(low, "low")?,
                close: f(close, "close")?,
                volume: f(volume, "volume")?,
            })
        })
        .collect()
}

/// Stream `ticker` -> partial quote update. A malformed field rejects the
/// whole message (the caller logs and drops it).
pub(crate) fn ws_ticker(t: &WsTicker, received_at: UnixNanos) -> ProviderResult<QuoteUpdate> {
    let open_24h = num(t.open_24h.as_ref(), "open_24h")?;
    Ok(QuoteUpdate {
        bid: num(t.best_bid.as_ref(), "best_bid")?,
        ask: num(t.best_ask.as_ref(), "best_ask")?,
        bid_size: num(t.best_bid_size.as_ref(), "best_bid_size")?,
        ask_size: num(t.best_ask_size.as_ref(), "best_ask_size")?,
        last: num(t.price.as_ref(), "price")?,
        last_size: num(t.last_size.as_ref(), "last_size")?,
        open: open_24h,
        high: num(t.high_24h.as_ref(), "high_24h")?,
        low: num(t.low_24h.as_ref(), "low_24h")?,
        prev_close: open_24h,
        volume: num(t.volume_24h.as_ref(), "volume_24h")?,
        volume_increment: None,
        vwap: None,
        ts_event: opt_time(t.time.as_deref(), "time")?.unwrap_or(received_at),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::WsMessage;

    const URL: &str = "https://api.exchange.coinbase.com/products/BTC-USD/ticker";

    #[test]
    fn rest_quote_from_fixtures() {
        let t: TickerDto =
            serde_json::from_str(include_str!("../tests/fixtures/ticker_btc_usd.json")).unwrap();
        let s: StatsDto =
            serde_json::from_str(include_str!("../tests/fixtures/stats_btc_usd.json")).unwrap();
        let q = quote(SecurityKey::currency("BTCUSD"), &t, &s, URL.into(), 42).unwrap();
        assert_eq!(q.bid, Some(85515.62));
        assert_eq!(q.ask, Some(85515.63));
        assert_eq!(q.last, Some(85515.62));
        assert_eq!(q.last_size, Some(0.00004765));
        assert_eq!(q.volume, Some(5958.18051798));
        assert_eq!(q.open, Some(85325.07));
        assert_eq!(q.prev_close, Some(85325.07));
        assert_eq!(q.high, Some(86996.0));
        assert_eq!(q.low, Some(84944.2));
        assert_eq!(q.bid_size, None);
        assert_eq!(q.vwap, None);
        // 2026-10-05T18:27:05.102634089Z
        assert_eq!(q.ts_event, 1_791_224_825_102_634_089);
        assert_eq!(q.ts_recv, 42);
        assert_eq!(q.provenance.as_of, q.ts_event);
        assert_eq!(q.provenance.provider.as_str(), "coinbase");
        assert!(!q.provenance.synthetic);
        assert_eq!(q.provenance.delay, DataDelay::RealTime);
        assert_eq!(q.provenance.source, FeedSource::Exchange("COINBASE".into()));
        assert_eq!(q.provenance.source_ref.as_deref(), Some(URL));
        assert_eq!(q.flags, 0);
    }

    #[test]
    fn rest_quote_rejects_malformed_price_and_keeps_missing_as_none() {
        let t: TickerDto = serde_json::from_str(r#"{"price":"abc"}"#).unwrap();
        let s: StatsDto = serde_json::from_str("{}").unwrap();
        let err = quote(SecurityKey::currency("BTCUSD"), &t, &s, URL.into(), 7).unwrap_err();
        assert!(matches!(err, ProviderError::Parse { .. }), "{err:?}");

        let t: TickerDto = serde_json::from_str(r#"{"price":"1.5"}"#).unwrap();
        let q = quote(SecurityKey::currency("BTCUSD"), &t, &s, URL.into(), 7).unwrap();
        assert_eq!(q.last, Some(1.5));
        assert_eq!(
            (q.bid, q.open, q.volume, q.prev_close),
            (None, None, None, None)
        );
        assert_eq!(q.ts_event, 7);
        assert_eq!(q.provenance.as_of, 7);
    }

    #[test]
    fn candles_keep_column_order() {
        let rows: Vec<CandleRow> =
            serde_json::from_str(include_str!("../tests/fixtures/candles_btc_usd_1d.json"))
                .unwrap();
        let bars = candles(&rows).unwrap();
        assert_eq!(bars.len(), 10);
        // [1791158400, 84944.2, 86996, 86502.65, 85497.44, 4518.77042928]
        assert_eq!(
            bars[0],
            Bar {
                ts: 1_791_158_400 * 1_000_000_000,
                open: 86502.65,
                high: 86996.0,
                low: 84944.2,
                close: 85497.44,
                volume: 4518.77042928
            }
        );
        for b in &bars {
            assert!(
                b.high >= b.low
                    && b.high >= b.open
                    && b.high >= b.close
                    && b.low <= b.open
                    && b.low <= b.close
            );
        }
        let hourly: Vec<CandleRow> =
            serde_json::from_str(include_str!("../tests/fixtures/candles_btc_usd_1h.json"))
                .unwrap();
        assert_eq!(candles(&hourly).unwrap().len(), 11);
        let bad: Vec<CandleRow> = serde_json::from_str(r#"[[1, "x", 1, 1, 1, 1]]"#).unwrap();
        assert!(candles(&bad).is_err());
    }

    #[test]
    fn ws_ticker_update() {
        let WsMessage::Ticker(t) =
            serde_json::from_str(include_str!("../tests/fixtures/ws_ticker.json")).unwrap()
        else {
            panic!("not a ticker");
        };
        assert_eq!(t.product_id, "BTC-USD");
        let u = ws_ticker(&t, 1).unwrap();
        assert_eq!(u.last, Some(85587.59));
        assert_eq!(u.last_size, Some(0.00000015));
        assert_eq!(u.bid, Some(85587.59));
        assert_eq!(u.bid_size, Some(0.14650470));
        assert_eq!(u.ask, Some(85587.60));
        assert_eq!(u.ask_size, Some(0.09357588));
        assert_eq!(u.open, Some(85325.07));
        assert_eq!(u.prev_close, Some(85325.07));
        assert_eq!(u.high, Some(86996.0));
        assert_eq!(u.low, Some(84944.2));
        assert_eq!(u.volume, Some(5960.81748046));
        assert_eq!(u.volume_increment, None);
        assert_eq!(u.vwap, None);
        // 2026-10-05T18:28:35.087894Z
        assert_eq!(u.ts_event, 1_791_224_915_087_894_000);
    }

    #[test]
    fn ws_ticker_malformed_is_error() {
        let WsMessage::Ticker(t) =
            serde_json::from_str(r#"{"type":"ticker","product_id":"BTC-USD","price":"NaN?"}"#)
                .unwrap()
        else {
            panic!("not a ticker");
        };
        assert!(ws_ticker(&t, 1).is_err());
    }
}

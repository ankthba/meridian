//! Vendor DTOs -> normalized `meridian_types`.
//!
//! Field semantics (see README): `high`/`low`/`volume`/`vwap` are Kraken's
//! rolling 24 h figures in both REST and the stream. `open` and
//! `prev_close` mean "price 24 h ago": the stream derives it as
//! `last - change` (Kraken's 24 h change); the REST `Ticker` has no 24 h
//! reference price, so REST quotes leave both `None`. Kraken's REST `o`
//! (today's open since 00:00 UTC) is deliberately not mapped because it
//! means something else.

use chrono::{DateTime, Utc};
use meridian_provider::{ProviderError, ProviderResult};
use meridian_types::{
    Bar, DataDelay, FeedSource, Provenance, ProviderId, Quote, QuoteUpdate, SecurityKey, UnixNanos,
    datetime_to_nanos, nanos_from_secs,
};

use crate::PROVIDER_ID;
use crate::dto::{Num, OhlcRow, TickerDto, WsTicker};

pub(crate) const FEED: &str = "KRAKEN";

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

/// Maps a Kraken `error` string (sent with HTTP 200).
pub(crate) fn api_error(e: &str) -> ProviderError {
    if e.starts_with("EGeneral:Too many requests")
        || e.starts_with("EAPI:Rate limit exceeded")
        || e.starts_with("EService:Throttled")
    {
        ProviderError::RateLimited { retry_after: None }
    } else if e.starts_with("EQuery:Unknown asset pair") {
        ProviderError::NotFound(format!("kraken: {e}"))
    } else {
        ProviderError::Upstream(format!("kraken: {e}"))
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
            .ok_or_else(|| ProviderError::parse(format!("kraken {field}: {:?}", n.raw()))),
    }
}

/// Element `i` of an optional array field.
fn at(v: Option<&Vec<Num>>, i: usize, field: &str) -> ProviderResult<Option<f64>> {
    num(v.and_then(|a| a.get(i)), field)
}

/// RFC 3339 -> Unix nanos.
pub(crate) fn parse_time(s: &str) -> Option<UnixNanos> {
    DateTime::parse_from_rfc3339(s.trim())
        .ok()
        .map(|d| datetime_to_nanos(d.with_timezone(&Utc)))
}

/// Rounds a derived price to the pair's price decimals (or 10 places when
/// unknown) so `last - change` doesn't carry float noise.
pub(crate) fn round_price(x: f64, decimals: Option<u8>) -> f64 {
    let f = 10f64.powi(i32::from(decimals.unwrap_or(10)));
    (x * f).round() / f
}

/// REST snapshot from `Ticker`. The endpoint carries no timestamp, so
/// `ts_event` and `as_of` are the fetch time.
pub(crate) fn quote(
    key: SecurityKey,
    t: &TickerDto,
    source_ref: String,
    fetched_at: UnixNanos,
) -> ProviderResult<Quote> {
    let mut q = Quote::empty(key, provenance(fetched_at, source_ref));
    q.ask = at(t.a.as_ref(), 0, "a[0]")?;
    q.ask_size = at(t.a.as_ref(), 2, "a[2]")?;
    q.bid = at(t.b.as_ref(), 0, "b[0]")?;
    q.bid_size = at(t.b.as_ref(), 2, "b[2]")?;
    q.last = at(t.c.as_ref(), 0, "c[0]")?;
    q.last_size = at(t.c.as_ref(), 1, "c[1]")?;
    q.volume = at(t.v.as_ref(), 1, "v[1]")?;
    q.vwap = at(t.p.as_ref(), 1, "p[1]")?;
    q.low = at(t.l.as_ref(), 1, "l[1]")?;
    q.high = at(t.h.as_ref(), 1, "h[1]")?;
    q.ts_event = fetched_at;
    q.ts_recv = fetched_at;
    Ok(q)
}

/// `OHLC` rows `[time, open, high, low, close, vwap, volume, count]` ->
/// bars, in the order given (ascending).
pub(crate) fn ohlc(rows: &[OhlcRow]) -> ProviderResult<Vec<Bar>> {
    rows.iter()
        .map(
            |OhlcRow(t, open, high, low, close, _vwap, volume, _count)| {
                let f = |n: &Num, field: &str| {
                    n.to_f64().ok_or_else(|| {
                        ProviderError::parse(format!("kraken OHLC {field} at {t}: {:?}", n.raw()))
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
            },
        )
        .collect()
}

/// Stream `ticker` -> partial update. A malformed field rejects the whole
/// element (the caller logs and drops it).
pub(crate) fn ws_ticker(
    t: &WsTicker,
    decimals: Option<u8>,
    received_at: UnixNanos,
) -> ProviderResult<QuoteUpdate> {
    let last = num(t.last.as_ref(), "last")?;
    let change = num(t.change.as_ref(), "change")?;
    let ago_24h = last.zip(change).map(|(l, c)| round_price(l - c, decimals));
    let ts_event = match t.timestamp.as_deref() {
        None => received_at,
        Some(s) => parse_time(s)
            .ok_or_else(|| ProviderError::parse(format!("kraken ticker timestamp: {s:?}")))?,
    };
    Ok(QuoteUpdate {
        bid: num(t.bid.as_ref(), "bid")?,
        ask: num(t.ask.as_ref(), "ask")?,
        bid_size: num(t.bid_qty.as_ref(), "bid_qty")?,
        ask_size: num(t.ask_qty.as_ref(), "ask_qty")?,
        last,
        last_size: None,
        open: ago_24h,
        high: num(t.high.as_ref(), "high")?,
        low: num(t.low.as_ref(), "low")?,
        prev_close: ago_24h,
        volume: num(t.volume.as_ref(), "volume")?,
        volume_increment: None,
        vwap: num(t.vwap.as_ref(), "vwap")?,
        ts_event,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::dto::{Envelope, WsFrame};

    const URL: &str = "https://api.kraken.com/0/public/Ticker?pair=XBTUSD&assetVersion=1";

    fn tickers() -> HashMap<String, TickerDto> {
        let env: Envelope =
            serde_json::from_str(include_str!("../tests/fixtures/ticker.json")).unwrap();
        serde_json::from_value(env.result.unwrap()).unwrap()
    }

    #[test]
    fn rest_quote_from_fixture() {
        let t = &tickers()["BTC/USD"];
        let q = quote(SecurityKey::currency("BTCUSD"), t, URL.into(), 99).unwrap();
        assert_eq!(q.ask, Some(85523.1));
        assert_eq!(q.ask_size, Some(1.0));
        assert_eq!(q.bid, Some(85523.0));
        assert_eq!(q.bid_size, Some(1.0));
        assert_eq!(q.last, Some(85523.0));
        assert_eq!(q.last_size, Some(0.01713163));
        assert_eq!(q.volume, Some(2515.84549985));
        assert_eq!(q.vwap, Some(85944.95739));
        assert_eq!(q.low, Some(84965.7));
        assert_eq!(q.high, Some(86973.6));
        assert_eq!((q.open, q.prev_close), (None, None));
        assert_eq!((q.ts_event, q.ts_recv, q.provenance.as_of), (99, 99, 99));
        assert_eq!(q.provenance.provider.as_str(), "kraken");
        assert_eq!(q.provenance.source, FeedSource::Exchange("KRAKEN".into()));
        assert_eq!(q.provenance.delay, DataDelay::RealTime);
        assert!(!q.provenance.synthetic);
        assert_eq!(q.provenance.source_ref.as_deref(), Some(URL));
        let usdc = quote(
            SecurityKey::currency("USDCUSD"),
            &tickers()["USDC/USD"],
            URL.into(),
            1,
        )
        .unwrap();
        assert_eq!(usdc.last, Some(0.9998));
    }

    #[test]
    fn rest_quote_rejects_malformed_and_keeps_missing_as_none() {
        let t: TickerDto = serde_json::from_str(r#"{"c": ["x", "1"]}"#).unwrap();
        assert!(matches!(
            quote(SecurityKey::currency("BTCUSD"), &t, URL.into(), 1),
            Err(ProviderError::Parse { .. })
        ));
        let t: TickerDto = serde_json::from_str(r#"{"c": ["2.5"]}"#).unwrap();
        let q = quote(SecurityKey::currency("BTCUSD"), &t, URL.into(), 1).unwrap();
        assert_eq!(
            (q.last, q.last_size, q.bid, q.volume),
            (Some(2.5), None, None, None)
        );
    }

    #[test]
    fn errors_map_by_prefix() {
        let env: Envelope =
            serde_json::from_str(include_str!("../tests/fixtures/error_unknown_pair.json"))
                .unwrap();
        assert!(matches!(
            api_error(&env.error[0]),
            ProviderError::NotFound(_)
        ));
        let env: Envelope = serde_json::from_str(include_str!(
            "../tests/fixtures/error_invalid_arguments.json"
        ))
        .unwrap();
        assert!(matches!(
            api_error(&env.error[0]),
            ProviderError::Upstream(_)
        ));
        assert!(matches!(
            api_error("EGeneral:Too many requests"),
            ProviderError::RateLimited { .. }
        ));
        assert!(matches!(
            api_error("EAPI:Rate limit exceeded"),
            ProviderError::RateLimited { .. }
        ));
        assert!(matches!(
            api_error("EService:Throttled: 1791224950"),
            ProviderError::RateLimited { .. }
        ));
        assert!(matches!(
            api_error("EService:Unavailable"),
            ProviderError::Upstream(_)
        ));
    }

    #[test]
    fn ohlc_rows() {
        let env: Envelope =
            serde_json::from_str(include_str!("../tests/fixtures/ohlc_btc_usd_1440.json")).unwrap();
        let result = env.result.unwrap();
        let rows: Vec<OhlcRow> = serde_json::from_value(result["BTC/USD"].clone()).unwrap();
        let bars = ohlc(&rows).unwrap();
        assert_eq!(bars.len(), 10);
        // [1790380800, "84090.5", "84451.3", "83777.0", "84426.7", "84067.1", "950.99825725", 60361]
        assert_eq!(
            bars[0],
            Bar {
                ts: 1_790_380_800 * 1_000_000_000,
                open: 84090.5,
                high: 84451.3,
                low: 83777.0,
                close: 84426.7,
                volume: 950.99825725
            }
        );
        assert!(bars.windows(2).all(|w| w[0].ts < w[1].ts));
        assert_eq!(result["last"], 1_791_072_000);
        let bad: Vec<OhlcRow> =
            serde_json::from_str(r#"[[1, "x", "1", "1", "1", "1", "1", 1]]"#).unwrap();
        assert!(ohlc(&bad).is_err());
    }

    fn ws_tickers(fixture: &str) -> Vec<WsTicker> {
        let f: WsFrame = serde_json::from_str(fixture).unwrap();
        serde_json::from_value(f.data.unwrap()).unwrap()
    }

    #[test]
    fn ws_snapshot_update() {
        let t = &ws_tickers(include_str!("../tests/fixtures/ws_ticker_snapshot.json"))[0];
        assert_eq!(t.symbol, "BTC/USD");
        let u = ws_ticker(t, Some(1), 0).unwrap();
        assert_eq!(u.bid, Some(85599.1));
        assert_eq!(u.bid_size, Some(1.46858616));
        assert_eq!(u.ask, Some(85599.2));
        assert_eq!(u.ask_size, Some(0.03857359));
        assert_eq!(u.last, Some(85599.2));
        assert_eq!(u.volume, Some(2517.8809049));
        assert_eq!(u.vwap, Some(85944.6));
        assert_eq!(u.low, Some(84965.7));
        assert_eq!(u.high, Some(86973.6));
        // last - change = 85599.2 - 290.0, at the pair's 1 decimal.
        assert_eq!(u.prev_close, Some(85309.2));
        assert_eq!(u.open, Some(85309.2));
        assert_eq!(u.last_size, None);
        // 2026-10-05T18:29:10.158542Z
        assert_eq!(u.ts_event, 1_791_224_950_158_542_000);
    }

    #[test]
    fn ws_update_eth() {
        let t = &ws_tickers(include_str!("../tests/fixtures/ws_ticker_update.json"))[0];
        let u = ws_ticker(t, Some(2), 0).unwrap();
        // 2710.30 - 9.46
        assert_eq!(u.prev_close, Some(2700.84));
    }

    #[test]
    fn ws_malformed_or_partial() {
        let t: WsTicker = serde_json::from_str(r#"{"symbol":"BTC/USD","last":"oops"}"#).unwrap();
        assert!(ws_ticker(&t, None, 0).is_err());
        let t: WsTicker = serde_json::from_str(r#"{"symbol":"BTC/USD","last":10.5}"#).unwrap();
        let u = ws_ticker(&t, None, 42).unwrap();
        assert_eq!((u.last, u.prev_close, u.ts_event), (Some(10.5), None, 42));
    }

    #[test]
    fn rounding() {
        assert_eq!(round_price(2710.30 - 9.46, Some(2)), 2700.84);
        assert_eq!(round_price(0.1 + 0.2, None), 0.3);
    }
}

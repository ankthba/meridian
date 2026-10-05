//! Live tests against the public Coinbase Exchange endpoints (no key).
//! Ignored by default; run with
//! `cargo test -p meridian-provider-coinbase --test live -- --ignored --nocapture`.

use std::sync::Arc;
use std::time::Duration;

use meridian_provider::{BarsRequest, EventSink, InstrumentQuery, Provider};
use meridian_provider_coinbase::CoinbaseProvider;
use meridian_types::{
    Adjustment, BarInterval, Clock, MarketSector, NANOS_PER_DAY, ProviderId, SecurityKey,
    StreamEvent, SystemClock,
};
use parking_lot::Mutex;

fn btc() -> SecurityKey {
    SecurityKey::currency("BTCUSD")
}

#[tokio::test]
#[ignore = "hits the live Coinbase API"]
async fn live_products_search_and_instrument() {
    let p = CoinbaseProvider::new().unwrap();
    let q = InstrumentQuery {
        text: "btc".into(),
        sector: Some(MarketSector::Curncy),
        limit: 5,
    };
    let hits = p.search(&q).await.unwrap();
    println!(
        "search btc: {:?}",
        hits.iter()
            .map(|i| (i.key.to_string(), i.name.clone()))
            .collect::<Vec<_>>()
    );
    assert_eq!(hits[0].key, btc());
    let inst = p.instrument(&btc()).await.unwrap();
    println!(
        "instrument: {} {:?} tick={} dec={}",
        inst.name, inst.exchange_name, inst.tick_size, inst.price_decimals
    );
    assert!(p.covers(&btc()));
    assert!(
        !p.covers(&SecurityKey::currency("USDCUSD")),
        "USDC-USD is not a Coinbase Exchange product"
    );
    assert!(
        p.instrument(&SecurityKey::currency("NOPEUSD"))
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "hits the live Coinbase API"]
async fn live_quotes() {
    let p = CoinbaseProvider::new().unwrap();
    let quotes = p
        .quotes(&[
            btc(),
            SecurityKey::currency("ETHUSD"),
            SecurityKey::currency("EURUSD"),
        ])
        .await
        .unwrap();
    for q in &quotes {
        println!(
            "{}: last={:?} bid={:?} ask={:?} open24h={:?} high={:?} low={:?} vol24h={:?} ts={} src={:?}",
            q.key,
            q.last,
            q.bid,
            q.ask,
            q.open,
            q.high,
            q.low,
            q.volume,
            q.ts_event,
            q.provenance.source_ref
        );
    }
    assert_eq!(quotes.len(), 2, "EURUSD is not covered");
    let b = &quotes[0];
    assert!(b.last.unwrap() > 0.0 && b.bid.unwrap() <= b.ask.unwrap());
    assert!(!b.provenance.synthetic);
}

#[tokio::test]
#[ignore = "hits the live Coinbase API"]
async fn live_bars() {
    let p = CoinbaseProvider::new().unwrap();
    let now = SystemClock.now();
    let daily = p
        .bars(&BarsRequest {
            key: btc(),
            interval: BarInterval::Day,
            from: Some(now - 400 * NANOS_PER_DAY),
            to: None,
            adjustment: Adjustment::None,
        })
        .await
        .unwrap();
    println!(
        "daily: {} bars, first={} last={}",
        daily.len(),
        daily.ts[0],
        daily.ts[daily.len() - 1]
    );
    assert!(daily.len() >= 399 && daily.validate().is_ok());

    let hourly = p
        .bars(&BarsRequest {
            key: btc(),
            interval: BarInterval::Hour(4),
            from: Some(now - 3 * NANOS_PER_DAY),
            to: Some(now),
            adjustment: Adjustment::None,
        })
        .await
        .unwrap();
    println!(
        "4h: {} bars, last close={}",
        hourly.len(),
        hourly.close[hourly.len() - 1]
    );
    assert!((17..=19).contains(&hourly.len()) && hourly.validate().is_ok());

    let weekly = p
        .bars(&BarsRequest {
            key: btc(),
            interval: BarInterval::Week,
            from: Some(now - 70 * NANOS_PER_DAY),
            to: None,
            adjustment: Adjustment::None,
        })
        .await
        .unwrap();
    println!("weekly: {} bars", weekly.len());
    assert!(weekly.len() >= 9 && weekly.validate().is_ok());
}

#[derive(Default)]
struct Collect(Mutex<Vec<StreamEvent>>);

impl EventSink for Collect {
    fn send(&self, _provider: &ProviderId, event: StreamEvent) {
        self.0.lock().push(event);
    }
}

#[tokio::test]
#[ignore = "hits the live Coinbase WebSocket feed"]
async fn live_stream_ticker() {
    let p = CoinbaseProvider::new().unwrap();
    let sink = Arc::new(Collect::default());
    let handle = p.streaming().unwrap().connect(sink.clone()).await.unwrap();
    handle.subscribe(&[btc()]);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let quote = loop {
        let found = sink
            .0
            .lock()
            .iter()
            .find(|e| matches!(e, StreamEvent::Quote { .. }))
            .cloned();
        if let Some(q) = found {
            break q;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no quote within 20 s; events: {:?}",
            sink.0.lock()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    println!("first status: {:?}", sink.0.lock().first());
    println!("first quote: {quote:?}");
    handle.close();
    tokio::time::sleep(Duration::from_millis(500)).await;
    println!("last event: {:?}", sink.0.lock().last());
}

//! Live tests against the public Kraken endpoints (no key). Ignored by
//! default; run with
//! `cargo test -p meridian-provider-kraken --test live -- --ignored --nocapture --test-threads=1`.
//! Kraken asks for at most ~1 public request per second, which the provider
//! paces itself.

use std::sync::Arc;
use std::time::Duration;

use meridian_provider::{BarsRequest, EventSink, InstrumentQuery, Provider};
use meridian_provider_kraken::KrakenProvider;
use meridian_types::{
    Adjustment, BarInterval, Clock, MarketSector, NANOS_PER_DAY, ProviderId, SecurityKey,
    StreamEvent, SystemClock,
};
use parking_lot::Mutex;

fn btc() -> SecurityKey {
    SecurityKey::currency("BTCUSD")
}

#[tokio::test]
#[ignore = "hits the live Kraken API"]
async fn live_pairs_search_and_instrument() {
    let p = KrakenProvider::new().unwrap();
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
    assert!(p.covers(&SecurityKey::currency("USDCUSD")));
    assert!(
        !p.covers(&SecurityKey::currency("EURUSD")),
        "FX stays with FX providers"
    );
    assert!(
        p.instrument(&SecurityKey::currency("NOPEUSD"))
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "hits the live Kraken API"]
async fn live_quotes() {
    let p = KrakenProvider::new().unwrap();
    let keys = [
        btc(),
        SecurityKey::currency("ETHUSD"),
        SecurityKey::currency("USDCUSD"),
        SecurityKey::currency("EURUSD"),
    ];
    let quotes = p.quotes(&keys).await.unwrap();
    for q in &quotes {
        println!(
            "{}: last={:?} bid={:?} ask={:?} high24h={:?} low24h={:?} vol24h={:?} vwap24h={:?} src={:?}",
            q.key, q.last, q.bid, q.ask, q.high, q.low, q.volume, q.vwap, q.provenance.source_ref
        );
    }
    assert_eq!(quotes.len(), 3, "EURUSD is not covered");
    let b = &quotes[0];
    assert!(b.last.unwrap() > 0.0 && b.bid.unwrap() <= b.ask.unwrap());
}

#[tokio::test]
#[ignore = "hits the live Kraken API"]
async fn live_bars() {
    let p = KrakenProvider::new().unwrap();
    let now = SystemClock.now();
    let daily = p
        .bars(&BarsRequest {
            key: btc(),
            interval: BarInterval::Day,
            from: None,
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
    assert!(daily.len() >= 700 && daily.validate().is_ok());

    let two_hour = p
        .bars(&BarsRequest {
            key: btc(),
            interval: BarInterval::Hour(2),
            from: Some(now - 2 * NANOS_PER_DAY),
            to: Some(now),
            adjustment: Adjustment::None,
        })
        .await
        .unwrap();
    println!(
        "2h: {} bars, last close={}",
        two_hour.len(),
        two_hour.close[two_hour.len() - 1]
    );
    assert!((23..=25).contains(&two_hour.len()) && two_hour.validate().is_ok());

    let weekly = p
        .bars(&BarsRequest {
            key: btc(),
            interval: BarInterval::Week,
            from: None,
            to: None,
            adjustment: Adjustment::None,
        })
        .await
        .unwrap();
    println!("weekly: {} bars, first={}", weekly.len(), weekly.ts[0]);

    let monthly = p
        .bars(&BarsRequest {
            key: btc(),
            interval: BarInterval::Month,
            from: None,
            to: None,
            adjustment: Adjustment::None,
        })
        .await
        .unwrap();
    println!("monthly: {} bars, first={}", monthly.len(), monthly.ts[0]);
    assert!(monthly.validate().is_ok());
}

#[derive(Default)]
struct Collect(Mutex<Vec<StreamEvent>>);

impl EventSink for Collect {
    fn send(&self, _provider: &ProviderId, event: StreamEvent) {
        self.0.lock().push(event);
    }
}

#[tokio::test]
#[ignore = "hits the live Kraken WebSocket feed"]
async fn live_stream_ticker() {
    let p = KrakenProvider::new().unwrap();
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

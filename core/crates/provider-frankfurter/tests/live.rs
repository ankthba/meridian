//! Live checks against the real Frankfurter API. Ignored by default:
//! `cargo test -p meridian-provider-frankfurter -- --ignored --nocapture`.

use meridian_provider::{BarsRequest, Provider};
use meridian_provider_frankfurter::FrankfurterProvider;
use meridian_types::{Adjustment, BarInterval, SecurityKey, nanos_to_date};

#[tokio::test]
#[ignore = "hits the live Frankfurter API"]
async fn live_quotes() {
    let p = FrankfurterProvider::new().unwrap();
    let keys = [SecurityKey::currency("EURUSD"), SecurityKey::currency("USDJPY"), SecurityKey::currency("BTCUSD")];
    let quotes = p.quotes(&keys).await.unwrap();
    for q in &quotes {
        println!(
            "{} last={:?} prev={:?} date={} ref={:?}",
            q.key,
            q.last,
            q.prev_close,
            nanos_to_date(q.ts_event),
            q.provenance.source_ref
        );
    }
    assert_eq!(quotes.len(), 2, "BTCUSD is not an ECB pair and must not be served");
    assert!(quotes.iter().all(|q| q.last.is_some() && q.prev_close.is_some()));
}

#[tokio::test]
#[ignore = "hits the live Frankfurter API"]
async fn live_bars() {
    let p = FrankfurterProvider::new().unwrap();
    for interval in [BarInterval::Day, BarInterval::Month] {
        let req = BarsRequest {
            key: SecurityKey::currency("EURUSD"),
            interval,
            from: Some(meridian_types::date_to_nanos(chrono::NaiveDate::from_ymd_opt(2025, 1, 1).unwrap())),
            to: None,
            adjustment: Adjustment::None,
        };
        let s = p.bars(&req).await.unwrap();
        s.validate().unwrap();
        println!(
            "{interval}: {} bars, first {} close {}, last {} close {}",
            s.len(),
            nanos_to_date(s.ts[0]),
            s.close[0],
            nanos_to_date(*s.ts.last().unwrap()),
            s.close.last().unwrap()
        );
        assert!(s.len() > 10);
    }
}

#[tokio::test]
#[ignore = "hits the live Frankfurter API"]
async fn live_unknown_pair_is_not_found() {
    let p = FrankfurterProvider::new().unwrap();
    let req = BarsRequest {
        key: SecurityKey::currency("BTCUSD"),
        interval: BarInterval::Day,
        from: None,
        to: None,
        adjustment: Adjustment::None,
    };
    assert!(matches!(p.bars(&req).await, Err(meridian_provider::ProviderError::NotFound(_))));
}

//! Live checks against the real Treasury feed. Ignored by default:
//! `cargo test -p meridian-provider-treasury --test live -- --ignored --nocapture`.

use chrono::NaiveDate;
use meridian_provider::{CurveRequest, Provider, SeriesRequest};
use meridian_provider_treasury::TreasuryProvider;

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

#[tokio::test]
#[ignore = "hits the live Treasury feed"]
async fn live_latest_curve() {
    let p = TreasuryProvider::new().unwrap();
    let c = p.yield_curve(&CurveRequest { name: "UST".into(), date: None }).await.unwrap();
    let pts: Vec<String> =
        c.points.iter().map(|p| format!("{}={}", p.tenor, p.yield_pct.map_or("-".into(), |v| v.to_string()))).collect();
    println!("latest UST curve {}: {}", c.date, pts.join(" "));
    println!("source_ref {:?}", c.provenance.source_ref);
    assert!(c.points.len() >= 10);
}

#[tokio::test]
#[ignore = "hits the live Treasury feed"]
async fn live_curve_on_a_holiday_and_an_old_date() {
    let p = TreasuryProvider::new().unwrap();
    // 2025-07-04 is a holiday: expect the curve of 2025-07-03.
    let c = p.yield_curve(&CurveRequest { name: "UST".into(), date: Some(d(2025, 7, 4)) }).await.unwrap();
    println!("asked 2025-07-04, got {} with {} points", c.date, c.points.len());
    assert_eq!(c.date, d(2025, 7, 3));
    // 2004: no 30-year.
    let old = p.yield_curve(&CurveRequest { name: "UST".into(), date: Some(d(2004, 1, 2)) }).await.unwrap();
    println!("2004-01-02: {:?}", old.points.iter().map(|p| p.tenor.as_str()).collect::<Vec<_>>());
    assert!(old.points.iter().all(|p| p.tenor != "30 Yr"));
}

#[tokio::test]
#[ignore = "hits the live Treasury feed"]
async fn live_series() {
    let p = TreasuryProvider::new().unwrap();
    let s = p
        .economic_series(&SeriesRequest { id: "UST:10 Yr".into(), from: Some(d(2025, 1, 1)), to: None })
        .await
        .unwrap();
    let last = s.latest().unwrap();
    println!("{}: {} observations, latest {} = {:?}", s.id, s.observations.len(), last.date, last.value);
    assert!(s.observations.len() > 300);

    let short = p
        .economic_series(&SeriesRequest {
            id: "UST:1.5 Mo".into(),
            from: Some(d(2025, 1, 1)),
            to: Some(d(2025, 12, 31)),
        })
        .await
        .unwrap();
    println!("{}: first {} last {}", short.id, short.observations[0].date, short.observations.last().unwrap().date);
    assert_eq!(short.observations[0].date, d(2025, 2, 18));
}

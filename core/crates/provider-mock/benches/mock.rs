//! Generation-speed benches for the mock provider.
//!
//! Budgets (release): 20 years of daily bars for one symbol < 5 ms; one
//! month of one-minute bars < 20 ms.

use std::hint::black_box;
use std::sync::Arc;

use chrono::{NaiveDate, TimeZone, Utc};
use criterion::{Criterion, criterion_group, criterion_main};
use meridian_provider::BarsRequest;
use meridian_provider_mock::{MockConfig, MockProvider};
use meridian_types::{Adjustment, BarInterval, FixedClock, SecurityKey, date_to_nanos, datetime_to_nanos};

fn provider() -> MockProvider {
    let now = datetime_to_nanos(Utc.with_ymd_and_hms(2026, 10, 5, 18, 0, 0).unwrap());
    MockProvider::new(MockConfig { seed: 7, clock: Arc::new(FixedClock(now)), ..MockConfig::default() })
}

fn benches(c: &mut Criterion) {
    let p = provider();
    let daily = BarsRequest {
        key: SecurityKey::equity("AAPL"),
        interval: BarInterval::Day,
        from: None,
        to: None,
        adjustment: Adjustment::Splits,
    };
    // Warm the shared factor cache once (shared across all symbols).
    let _ = p.bars_sync(&daily);
    c.bench_function("daily_20y_one_symbol", |b| b.iter(|| black_box(p.bars_sync(black_box(&daily)).unwrap())));

    let from = date_to_nanos(NaiveDate::from_ymd_opt(2026, 9, 4).unwrap());
    let minute = BarsRequest {
        key: SecurityKey::equity("MSFT"),
        interval: BarInterval::Minute(1),
        from: Some(from),
        to: None,
        adjustment: Adjustment::Splits,
    };
    c.bench_function("intraday_1m_one_month", |b| b.iter(|| black_box(p.bars_sync(black_box(&minute)).unwrap())));

    let quote = SecurityKey::equity("NVDA");
    c.bench_function("quote_snapshot_cached", |b| b.iter(|| black_box(p.quote_sync(black_box(&quote)).unwrap())));
}

criterion_group!(mock, benches);
criterion_main!(mock);

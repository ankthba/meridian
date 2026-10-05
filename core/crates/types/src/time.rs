//! Time representation. All timestamps are `i64` nanoseconds since the Unix
//! epoch, UTC. Calendar dates use `chrono::NaiveDate`.

use chrono::{DateTime, NaiveDate, TimeZone, Utc};

/// Nanoseconds since 1970-01-01T00:00:00Z.
pub type UnixNanos = i64;

pub const NANOS_PER_SEC: i64 = 1_000_000_000;
pub const NANOS_PER_MILLI: i64 = 1_000_000;
pub const NANOS_PER_DAY: i64 = 86_400 * NANOS_PER_SEC;

#[must_use]
pub fn nanos_from_secs(secs: i64) -> UnixNanos {
    secs.saturating_mul(NANOS_PER_SEC)
}

#[must_use]
pub fn nanos_from_millis(ms: i64) -> UnixNanos {
    ms.saturating_mul(NANOS_PER_MILLI)
}

#[must_use]
pub fn nanos_to_datetime(ns: UnixNanos) -> DateTime<Utc> {
    Utc.timestamp_nanos(ns)
}

#[must_use]
pub fn datetime_to_nanos(dt: DateTime<Utc>) -> UnixNanos {
    dt.timestamp_nanos_opt().unwrap_or(i64::MAX)
}

/// Midnight UTC of `date`, in nanos.
#[must_use]
pub fn date_to_nanos(date: NaiveDate) -> UnixNanos {
    let dt = date.and_hms_opt(0, 0, 0).map(|d| d.and_utc());
    dt.map_or(0, datetime_to_nanos)
}

#[must_use]
pub fn nanos_to_date(ns: UnixNanos) -> NaiveDate {
    nanos_to_datetime(ns).date_naive()
}

/// Days since 1970-01-01 (negative before). Used for compact FFI transfer.
#[must_use]
pub fn date_to_epoch_days(date: NaiveDate) -> i32 {
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid epoch");
    (date - epoch).num_days() as i32
}

#[must_use]
pub fn epoch_days_to_date(days: i32) -> NaiveDate {
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid epoch");
    epoch + chrono::Duration::days(i64::from(days))
}

/// A clock that can be injected so tests and the mock provider are
/// deterministic.
pub trait Clock: Send + Sync {
    fn now(&self) -> UnixNanos;
}

/// Wall clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> UnixNanos {
        datetime_to_nanos(Utc::now())
    }
}

/// Fixed clock for tests and snapshot rendering.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(pub UnixNanos);

impl Clock for FixedClock {
    fn now(&self) -> UnixNanos {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_days_round_trip() {
        let d = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        assert_eq!(epoch_days_to_date(date_to_epoch_days(d)), d);
        assert_eq!(date_to_epoch_days(NaiveDate::from_ymd_opt(1970, 1, 2).unwrap()), 1);
    }

    #[test]
    fn date_nanos_round_trip() {
        let d = NaiveDate::from_ymd_opt(2001, 9, 10).unwrap();
        assert_eq!(nanos_to_date(date_to_nanos(d)), d);
    }
}

//! Bar interval planning and client-side aggregation.
//!
//! Kraken's `OHLC` serves 1, 5, 15, 30, 60, 240, 1440, 10080 and 21600
//! minute candles and returns only the most recent 720 of them (plus the
//! current, not-yet-committed one), whatever `since` says. Other intervals
//! are built from the largest native interval that divides them.

use chrono::{Datelike, NaiveDate};
use meridian_types::{
    Bar, BarInterval, NANOS_PER_SEC, date_to_nanos, nanos_from_secs, nanos_to_date,
};

/// Native intervals, minutes.
pub(crate) const INTERVALS_MIN: [i64; 9] = [1, 5, 15, 30, 60, 240, 1_440, 10_080, 21_600];
/// Kraken returns at most this many committed candles.
pub(crate) const MAX_CANDLES: usize = 720;

/// How native candles are grouped into the requested interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bucket {
    /// Fixed-length buckets aligned to the Unix epoch (2h/6h/12h start at
    /// 00:00 UTC).
    Fixed(i64),
    /// Calendar months, UTC.
    Month,
}

impl Bucket {
    /// Start (seconds) of the bucket containing `ts`.
    pub(crate) fn start(self, ts: i64) -> i64 {
        match self {
            Bucket::Fixed(len) => ts.div_euclid(len) * len,
            Bucket::Month => {
                let d = nanos_to_date(nanos_from_secs(ts));
                d.with_day(1).map_or(ts, date_secs)
            }
        }
    }
}

fn date_secs(d: NaiveDate) -> i64 {
    date_to_nanos(d) / NANOS_PER_SEC
}

/// Native interval to fetch and how to aggregate it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Plan {
    pub interval_min: i64,
    /// `None` returns native candles as-is.
    pub aggregate: Option<Bucket>,
}

/// `None` if the interval can't be built from native candles.
///
/// Weeks use Kraken's native weekly candles, which start Thursday 00:00 UTC
/// (Unix-epoch aligned) and reach back ~13 years; months are aggregated
/// from daily candles, so they cover only the last ~23 months.
pub(crate) fn plan(interval: BarInterval) -> Option<Plan> {
    match interval {
        BarInterval::Minute(_) | BarInterval::Hour(_) => {
            let target = interval.seconds() / 60;
            if target <= 0 {
                return None;
            }
            let m = INTERVALS_MIN
                .iter()
                .rev()
                .copied()
                .find(|m| target % m == 0)?;
            Some(Plan {
                interval_min: m,
                aggregate: (m != target).then_some(Bucket::Fixed(target * 60)),
            })
        }
        BarInterval::Day => Some(Plan {
            interval_min: 1_440,
            aggregate: None,
        }),
        BarInterval::Week => Some(Plan {
            interval_min: 10_080,
            aggregate: None,
        }),
        BarInterval::Month => Some(Plan {
            interval_min: 1_440,
            aggregate: Some(Bucket::Month),
        }),
    }
}

/// Groups ascending native bars into buckets. With `drop_partial_first`
/// (the native history was cut off by the 720-candle limit), a leading
/// bucket that doesn't start with its first native candle is dropped rather
/// than shown with a wrong open.
pub(crate) fn aggregate(bars: &[Bar], bucket: Bucket, drop_partial_first: bool) -> Vec<Bar> {
    let mut out: Vec<Bar> = Vec::new();
    for b in bars {
        let start = nanos_from_secs(bucket.start(b.ts.div_euclid(NANOS_PER_SEC)));
        match out.last_mut() {
            Some(cur) if cur.ts == start => {
                cur.high = cur.high.max(b.high);
                cur.low = cur.low.min(b.low);
                cur.close = b.close;
                cur.volume += b.volume;
            }
            _ => out.push(Bar { ts: start, ..*b }),
        }
    }
    if drop_partial_first
        && bars
            .first()
            .zip(out.first())
            .is_some_and(|(b, o)| b.ts != o.ts)
    {
        out.remove(0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    fn secs(y: i32, m: u32, d: u32) -> i64 {
        date_secs(NaiveDate::from_ymd_opt(y, m, d).unwrap())
    }

    fn bar(ts: i64, o: f64, h: f64, l: f64, c: f64, v: f64) -> Bar {
        Bar {
            ts: nanos_from_secs(ts),
            open: o,
            high: h,
            low: l,
            close: c,
            volume: v,
        }
    }

    #[test]
    fn plans() {
        let p = |iv| plan(iv).unwrap();
        assert_eq!(
            p(BarInterval::Minute(1)),
            Plan {
                interval_min: 1,
                aggregate: None
            }
        );
        assert_eq!(
            p(BarInterval::Minute(30)),
            Plan {
                interval_min: 30,
                aggregate: None
            }
        );
        assert_eq!(
            p(BarInterval::Minute(3)),
            Plan {
                interval_min: 1,
                aggregate: Some(Bucket::Fixed(180))
            }
        );
        assert_eq!(
            p(BarInterval::Minute(10)),
            Plan {
                interval_min: 5,
                aggregate: Some(Bucket::Fixed(600))
            }
        );
        assert_eq!(
            p(BarInterval::Hour(1)),
            Plan {
                interval_min: 60,
                aggregate: None
            }
        );
        assert_eq!(
            p(BarInterval::Hour(2)),
            Plan {
                interval_min: 60,
                aggregate: Some(Bucket::Fixed(7_200))
            }
        );
        assert_eq!(
            p(BarInterval::Hour(4)),
            Plan {
                interval_min: 240,
                aggregate: None
            }
        );
        assert_eq!(
            p(BarInterval::Hour(6)),
            Plan {
                interval_min: 60,
                aggregate: Some(Bucket::Fixed(21_600))
            }
        );
        assert_eq!(
            p(BarInterval::Hour(12)),
            Plan {
                interval_min: 240,
                aggregate: Some(Bucket::Fixed(43_200))
            }
        );
        assert_eq!(
            p(BarInterval::Day),
            Plan {
                interval_min: 1_440,
                aggregate: None
            }
        );
        assert_eq!(
            p(BarInterval::Week),
            Plan {
                interval_min: 10_080,
                aggregate: None
            }
        );
        assert_eq!(
            p(BarInterval::Month),
            Plan {
                interval_min: 1_440,
                aggregate: Some(Bucket::Month)
            }
        );
        assert_eq!(plan(BarInterval::Hour(0)), None);
    }

    #[test]
    fn month_bucket_start() {
        assert_eq!(
            Bucket::Month.start(secs(2026, 10, 5) + 77),
            secs(2026, 10, 1)
        );
        assert_eq!(Bucket::Month.start(secs(2024, 2, 29)), secs(2024, 2, 1));
    }

    #[test]
    fn aggregates_ohlcv() {
        let h = 3_600;
        let bars = [
            bar(0, 10.0, 12.0, 9.0, 11.0, 1.0),
            bar(h, 11.0, 15.0, 10.0, 14.0, 2.0),
            bar(2 * h, 14.0, 14.5, 13.0, 13.5, 3.0),
            bar(3 * h, 13.5, 16.0, 13.0, 15.0, 4.0),
        ];
        let out = aggregate(&bars, Bucket::Fixed(2 * h), false);
        assert_eq!(
            out,
            vec![
                bar(0, 10.0, 15.0, 9.0, 14.0, 3.0),
                bar(2 * h, 14.0, 16.0, 13.0, 15.0, 7.0)
            ]
        );
    }

    #[test]
    fn monthly_from_daily_drops_partial_first_month_when_truncated() {
        let bars: Vec<Bar> = (0..40)
            .map(|i| {
                bar(
                    secs(2026, 8, 25) + i * DAY,
                    100.0 + i as f64,
                    200.0,
                    50.0,
                    101.0 + i as f64,
                    1.0,
                )
            })
            .collect();
        let all = aggregate(&bars, Bucket::Month, false);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].ts, nanos_from_secs(secs(2026, 8, 1)));
        assert_eq!(
            all[1],
            bar(secs(2026, 9, 1), 107.0, 200.0, 50.0, 137.0, 30.0)
        );
        let trimmed = aggregate(&bars, Bucket::Month, true);
        assert_eq!(trimmed.len(), 2);
        assert_eq!(trimmed[0].ts, nanos_from_secs(secs(2026, 9, 1)));
    }
}

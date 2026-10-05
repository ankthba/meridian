//! Bar interval planning, candle pagination, and client-side aggregation.
//!
//! Coinbase serves six granularities (1m, 5m, 15m, 1h, 6h, 1d) and at most
//! 300 candles per request. Other intervals are built from the largest
//! native granularity that divides them.

use chrono::{Datelike, NaiveDate};
use meridian_types::{
    Bar, BarInterval, NANOS_PER_SEC, UnixNanos, date_to_nanos, nanos_from_secs, nanos_to_date,
};

/// Native candle granularities, seconds.
pub(crate) const GRANULARITIES: [i64; 6] = [60, 300, 900, 3_600, 21_600, 86_400];
/// Coinbase rejects requests spanning more than this many candles.
pub(crate) const MAX_CANDLES: i64 = 300;

const DAY: i64 = 86_400;

/// How native candles are grouped into the requested interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bucket {
    /// Fixed-length buckets aligned to the Unix epoch (so 2h/4h/12h buckets
    /// start at 00:00 UTC).
    Fixed(i64),
    /// Calendar weeks starting Monday 00:00 UTC.
    WeekMonday,
    /// Calendar months, UTC.
    Month,
}

impl Bucket {
    /// Start (seconds) of the bucket containing `ts`.
    pub(crate) fn start(self, ts: i64) -> i64 {
        match self {
            Bucket::Fixed(len) => ts.div_euclid(len) * len,
            Bucket::WeekMonday => {
                // 1970-01-01 was a Thursday: day 0 is three days after Monday.
                let day = ts.div_euclid(DAY);
                (day - (day + 3).rem_euclid(7)) * DAY
            }
            Bucket::Month => {
                let d = nanos_to_date(nanos_from_secs(ts));
                d.with_day(1).map_or(ts, date_secs)
            }
        }
    }

    /// Exclusive end (seconds) of the bucket starting at `start`.
    pub(crate) fn end(self, start: i64) -> i64 {
        match self {
            Bucket::Fixed(len) => start + len,
            Bucket::WeekMonday => start + 7 * DAY,
            Bucket::Month => {
                let d = nanos_to_date(nanos_from_secs(start));
                let (y, m) = if d.month() == 12 {
                    (d.year() + 1, 1)
                } else {
                    (d.year(), d.month() + 1)
                };
                // Unreachable fallback: the first of a month always exists.
                NaiveDate::from_ymd_opt(y, m, 1).map_or(start + 31 * DAY, date_secs)
            }
        }
    }

    /// Smallest bucket start `>= ts`.
    pub(crate) fn ceil(self, ts: i64) -> i64 {
        let s = self.start(ts);
        if s == ts { s } else { self.end(s) }
    }
}

fn date_secs(d: NaiveDate) -> i64 {
    date_to_nanos(d) / NANOS_PER_SEC
}

/// Native granularity to fetch and how to aggregate it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Plan {
    pub granularity: i64,
    /// `None` returns native candles as-is.
    pub aggregate: Option<Bucket>,
}

/// `None` if the interval can't be built from native candles.
pub(crate) fn plan(interval: BarInterval) -> Option<Plan> {
    match interval {
        BarInterval::Minute(_) | BarInterval::Hour(_) => {
            let target = interval.seconds();
            if target <= 0 {
                return None;
            }
            let g = GRANULARITIES
                .iter()
                .rev()
                .copied()
                .find(|g| target % g == 0)?;
            Some(Plan {
                granularity: g,
                aggregate: (g != target).then_some(Bucket::Fixed(target)),
            })
        }
        BarInterval::Day => Some(Plan {
            granularity: DAY,
            aggregate: None,
        }),
        BarInterval::Week => Some(Plan {
            granularity: DAY,
            aggregate: Some(Bucket::WeekMonday),
        }),
        BarInterval::Month => Some(Plan {
            granularity: DAY,
            aggregate: Some(Bucket::Month),
        }),
    }
}

/// Smallest whole second `>= ns`.
pub(crate) fn ceil_secs(ns: UnixNanos) -> i64 {
    ns.div_euclid(NANOS_PER_SEC) + i64::from(ns.rem_euclid(NANOS_PER_SEC) != 0)
}

/// Converts a request range (`from` inclusive, `to` exclusive, nanos) into
/// the half-open range of native candle start times (seconds) to fetch. For
/// aggregated plans the range is widened to whole buckets, so every bucket
/// that starts in `[from, to)` is complete.
pub(crate) fn fetch_range(
    plan: Plan,
    from: Option<UnixNanos>,
    to: Option<UnixNanos>,
) -> (Option<i64>, Option<i64>) {
    let lo = from.map(ceil_secs);
    let hi = to.map(ceil_secs);
    match plan.aggregate {
        None => (lo, hi),
        Some(b) => (lo.map(|l| b.ceil(l)), hi.map(|h| b.end(b.start(h - 1)))),
    }
}

/// Walks backwards from the newest candle in windows of at most
/// [`MAX_CANDLES`]. Windows are `[start, end]`, both inclusive, in seconds,
/// matching Coinbase's `start`/`end` parameters.
#[derive(Debug, Clone)]
pub(crate) struct Pager {
    granularity: i64,
    next_end: i64,
    first: Option<i64>,
}

impl Pager {
    /// `lo`/`hi`: half-open range of candle starts; `now`: current time in
    /// seconds (the end is never later than the current candle).
    pub(crate) fn new(granularity: i64, lo: Option<i64>, hi: Option<i64>, now: i64) -> Self {
        let g = granularity;
        let current = now.div_euclid(g) * g;
        let last = hi.map_or(current, |h| ((h - 1).div_euclid(g) * g).min(current));
        let first = lo.map(|l| l.div_euclid(g) * g + if l.rem_euclid(g) == 0 { 0 } else { g });
        Self {
            granularity: g,
            next_end: last,
            first,
        }
    }

    pub(crate) fn next_window(&mut self) -> Option<(i64, i64)> {
        let end = self.next_end;
        let mut start = end - (MAX_CANDLES - 1) * self.granularity;
        if let Some(f) = self.first {
            if end < f {
                return None;
            }
            start = start.max(f);
        }
        self.next_end = start - self.granularity;
        Some((start, end))
    }
}

/// Groups ascending native bars into buckets. With `drop_partial_first`
/// (the native history was cut off by a depth limit), a leading bucket that
/// doesn't start with its first native candle is dropped rather than shown
/// with a wrong open.
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
                granularity: 60,
                aggregate: None
            }
        );
        assert_eq!(
            p(BarInterval::Minute(15)),
            Plan {
                granularity: 900,
                aggregate: None
            }
        );
        assert_eq!(
            p(BarInterval::Minute(30)),
            Plan {
                granularity: 900,
                aggregate: Some(Bucket::Fixed(1_800))
            }
        );
        assert_eq!(
            p(BarInterval::Minute(7)),
            Plan {
                granularity: 60,
                aggregate: Some(Bucket::Fixed(420))
            }
        );
        assert_eq!(
            p(BarInterval::Hour(1)),
            Plan {
                granularity: 3_600,
                aggregate: None
            }
        );
        assert_eq!(
            p(BarInterval::Hour(2)),
            Plan {
                granularity: 3_600,
                aggregate: Some(Bucket::Fixed(7_200))
            }
        );
        assert_eq!(
            p(BarInterval::Hour(4)),
            Plan {
                granularity: 3_600,
                aggregate: Some(Bucket::Fixed(14_400))
            }
        );
        assert_eq!(
            p(BarInterval::Hour(6)),
            Plan {
                granularity: 21_600,
                aggregate: None
            }
        );
        assert_eq!(
            p(BarInterval::Hour(12)),
            Plan {
                granularity: 21_600,
                aggregate: Some(Bucket::Fixed(43_200))
            }
        );
        assert_eq!(
            p(BarInterval::Day),
            Plan {
                granularity: DAY,
                aggregate: None
            }
        );
        assert_eq!(
            p(BarInterval::Week),
            Plan {
                granularity: DAY,
                aggregate: Some(Bucket::WeekMonday)
            }
        );
        assert_eq!(
            p(BarInterval::Month),
            Plan {
                granularity: DAY,
                aggregate: Some(Bucket::Month)
            }
        );
        assert_eq!(plan(BarInterval::Minute(0)), None);
    }

    #[test]
    fn week_buckets_start_monday() {
        // 2026-10-05 is a Monday, 2026-10-08 a Thursday.
        let mon = secs(2026, 10, 5);
        assert_eq!(Bucket::WeekMonday.start(mon), mon);
        assert_eq!(Bucket::WeekMonday.start(secs(2026, 10, 8) + 3_600), mon);
        assert_eq!(Bucket::WeekMonday.start(secs(2026, 10, 11) + 86_399), mon);
        assert_eq!(
            Bucket::WeekMonday.start(secs(2026, 10, 12)),
            secs(2026, 10, 12)
        );
        assert_eq!(Bucket::WeekMonday.start(0), secs(1969, 12, 29));
        assert_eq!(Bucket::WeekMonday.end(mon), secs(2026, 10, 12));
    }

    #[test]
    fn month_buckets() {
        assert_eq!(
            Bucket::Month.start(secs(2026, 10, 5) + 77),
            secs(2026, 10, 1)
        );
        assert_eq!(Bucket::Month.end(secs(2026, 12, 1)), secs(2027, 1, 1));
        assert_eq!(Bucket::Month.end(secs(2024, 2, 1)), secs(2024, 3, 1));
        assert_eq!(Bucket::Month.ceil(secs(2026, 10, 1)), secs(2026, 10, 1));
        assert_eq!(Bucket::Month.ceil(secs(2026, 10, 2)), secs(2026, 11, 1));
    }

    #[test]
    fn ceil_seconds() {
        assert_eq!(ceil_secs(0), 0);
        assert_eq!(ceil_secs(1), 1);
        assert_eq!(ceil_secs(NANOS_PER_SEC), 1);
        assert_eq!(ceil_secs(NANOS_PER_SEC + 1), 2);
    }

    #[test]
    fn fetch_range_widens_to_whole_buckets() {
        let native = plan(BarInterval::Day).unwrap();
        let from = nanos_from_secs(secs(2026, 9, 1));
        let to = nanos_from_secs(secs(2026, 10, 1));
        assert_eq!(
            fetch_range(native, Some(from), Some(to)),
            (Some(secs(2026, 9, 1)), Some(secs(2026, 10, 1)))
        );
        let monthly = plan(BarInterval::Month).unwrap();
        // from mid-September: first whole month is October; to mid-October:
        // the October bucket starts before `to`, so fetch all of it.
        let from = nanos_from_secs(secs(2026, 9, 15));
        let to = nanos_from_secs(secs(2026, 10, 15));
        assert_eq!(
            fetch_range(monthly, Some(from), Some(to)),
            (Some(secs(2026, 10, 1)), Some(secs(2026, 11, 1)))
        );
        assert_eq!(fetch_range(monthly, None, None), (None, None));
    }

    #[test]
    fn pager_windows_cover_range_backwards() {
        let g = 3_600;
        let lo = 1_000 * g;
        let hi = 1_700 * g; // exclusive: last candle starts at 1699 h
        let mut p = Pager::new(g, Some(lo), Some(hi), i64::MAX / 2);
        assert_eq!(p.next_window(), Some((1_400 * g, 1_699 * g)));
        assert_eq!(p.next_window(), Some((1_100 * g, 1_399 * g)));
        assert_eq!(p.next_window(), Some((1_000 * g, 1_099 * g)));
        assert_eq!(p.next_window(), None);
    }

    #[test]
    fn pager_aligns_and_clamps_to_now() {
        let g = 86_400;
        // Unaligned `lo` rounds up to the next candle start; `hi` in the
        // future is clamped to the current candle.
        let now = 20_000 * g + 5;
        let mut p = Pager::new(g, Some(19_900 * g + 1), Some(30_000 * g), now);
        assert_eq!(p.next_window(), Some((19_901 * g, 20_000 * g)));
        assert_eq!(p.next_window(), None);
        // Open-ended: keeps paging until the caller stops.
        let mut p = Pager::new(g, None, None, now);
        assert_eq!(p.next_window(), Some((19_701 * g, 20_000 * g)));
        assert_eq!(p.next_window(), Some((19_401 * g, 19_700 * g)));
    }

    #[test]
    fn pager_empty_when_range_is_empty() {
        let mut p = Pager::new(60, Some(600), Some(600), 10_000);
        assert_eq!(p.next_window(), None);
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
    fn drops_partial_leading_bucket_only_when_truncated() {
        let h = 3_600;
        let bars = [
            bar(h, 11.0, 15.0, 10.0, 14.0, 2.0),
            bar(2 * h, 14.0, 14.5, 13.0, 13.5, 3.0),
        ];
        assert_eq!(aggregate(&bars, Bucket::Fixed(2 * h), false).len(), 2);
        let out = aggregate(&bars, Bucket::Fixed(2 * h), true);
        assert_eq!(out, vec![bar(2 * h, 14.0, 14.5, 13.0, 13.5, 3.0)]);
        // A complete leading bucket is kept.
        let bars = [
            bar(0, 1.0, 1.0, 1.0, 1.0, 1.0),
            bar(h, 1.0, 1.0, 1.0, 1.0, 1.0),
        ];
        assert_eq!(aggregate(&bars, Bucket::Fixed(2 * h), true).len(), 1);
    }

    #[test]
    fn weekly_from_daily() {
        let mon = secs(2026, 9, 28);
        let bars: Vec<Bar> = (0..9)
            .map(|i| {
                bar(
                    mon + i * DAY,
                    100.0 + i as f64,
                    110.0 + i as f64,
                    90.0,
                    101.0 + i as f64,
                    1.0,
                )
            })
            .collect();
        let out = aggregate(&bars, Bucket::WeekMonday, false);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], bar(mon, 100.0, 116.0, 90.0, 107.0, 7.0));
        assert_eq!(out[1], bar(mon + 7 * DAY, 107.0, 118.0, 90.0, 109.0, 2.0));
    }
}

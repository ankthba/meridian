//! Intraday paths anchored to the daily bar.
//!
//! For each session day a one-minute path is generated as a Brownian bridge
//! from the daily open to the daily close with an intraday volatility
//! pattern, then mapped monotonically so the day's extremes land exactly on
//! the daily high and low. Minute bars therefore aggregate back to the daily
//! bar. Volume follows a U-shape (24h markets: a London/New York overlap
//! bump) with opening and closing auction spikes.

use chrono::NaiveDate;
use meridian_types::{Bar, UnixNanos};
use rand::Rng;
use rand_distr::StandardNormal;

use crate::cal::{NANOS_PER_MIN, Session};
use crate::daily::{DayBar, day_num};
use crate::hash::{Fast, tag};

fn vol_weight(session: &Session, j: usize, n: usize) -> f64 {
    let jf = j as f64;
    if session.is_24h() {
        let hour = jf / 60.0;
        1.0 + 0.4 * (-((hour - 15.0) / 3.0).powi(2)).exp()
    } else {
        1.0 + 1.0 * (-jf / 20.0).exp() + 0.4 * (-((n - 1 - j) as f64) / 15.0).exp()
    }
}

fn volume_weight(session: &Session, j: usize, n: usize) -> f64 {
    let jf = j as f64;
    if session.is_24h() {
        let hour = jf / 60.0;
        1.0 + 0.8 * (-((hour - 15.0) / 3.0).powi(2)).exp()
    } else {
        let mut w = 1.0 + 2.5 * (-jf / 12.0).exp() + 1.5 * (-((n - 1 - j) as f64) / 10.0).exp();
        if j == 0 {
            w *= 3.0;
        }
        if j + 1 == n {
            w *= 4.0;
        }
        w
    }
}

/// Rounds a volume to `decimals` places.
pub(crate) fn round_volume(v: f64, decimals: i32) -> f64 {
    let k = 10f64.powi(decimals);
    (v * k).round() / k
}

/// One-minute bars for a full session day (path terms, prices unrounded).
/// `rng_hash` selects the random stream (trackers reuse their underlying's).
pub(crate) fn day_minutes(
    seed: u64,
    rng_hash: u64,
    session: &Session,
    date: NaiveDate,
    bar: &DayBar,
    vol_decimals: i32,
) -> Vec<Bar> {
    let n = session.minutes().max(1) as usize;
    let mut r = Fast::new(&[seed, rng_hash, tag("intraday"), day_num(date)]);
    let (lo_o, lo_c) = (bar.o.ln(), bar.c.ln());
    let (ln_h, ln_l) = (bar.h.ln(), bar.l.ln());
    let sigma = ((ln_h - ln_l) / 1.6).max(1e-5);

    // Per-minute standard deviations (unit total variance).
    let mut s: Vec<f64> = (0..n).map(|j| vol_weight(session, j, n)).collect();
    let norm = s.iter().map(|x| x * x).sum::<f64>().sqrt();
    for x in &mut s {
        *x *= sigma / norm;
    }

    // Raw walk.
    let mut x = Vec::with_capacity(n + 1);
    let mut cum = Vec::with_capacity(n + 1);
    x.push(lo_o);
    cum.push(0.0);
    let mut acc_var = 0.0;
    for sj in &s {
        let z: f64 = r.sample(StandardNormal);
        let last = x[x.len() - 1];
        x.push(last + sj * z);
        acc_var += sj * sj;
        cum.push(acc_var);
    }
    // Bridge to the close using cumulative variance as time.
    let miss = lo_c - x[n];
    for j in 0..=n {
        x[j] += miss * cum[j] / acc_var;
    }
    x[n] = lo_c;

    // Bars with intra-minute extremes, in log space.
    let mut o = Vec::with_capacity(n);
    let mut h = Vec::with_capacity(n);
    let mut l = Vec::with_capacity(n);
    let mut c = Vec::with_capacity(n);
    let mut vw = Vec::with_capacity(n);
    for j in 0..n {
        let (oj, cj) = (x[j], x[j + 1]);
        let e1 = -(1.0 - r.random::<f64>()).ln();
        let e2 = -(1.0 - r.random::<f64>()).ln();
        o.push(oj);
        c.push(cj);
        h.push(oj.max(cj) + 0.5 * s[j] * e1);
        l.push(oj.min(cj) - 0.5 * s[j] * e2);
        let vz: f64 = r.sample(StandardNormal);
        vw.push(volume_weight(session, j, n) * (0.35 * vz).exp());
    }

    // Monotone map so the day's extremes equal the daily high and low.
    let a = lo_o.max(lo_c);
    let b = lo_o.min(lo_c);
    let hi_star = h.iter().copied().fold(f64::MIN, f64::max);
    let lo_star = l.iter().copied().fold(f64::MAX, f64::min);
    let k_hi = if hi_star - a > 1e-12 { (ln_h - a) / (hi_star - a) } else { 0.0 };
    let k_lo = if b - lo_star > 1e-12 { (b - ln_l) / (b - lo_star) } else { 0.0 };
    let map = |v: f64| -> f64 {
        if v > a {
            a + (v - a) * k_hi
        } else if v < b {
            b - (b - v) * k_lo
        } else {
            v
        }
    };
    let open_ns = session.open_utc(date);
    let mut out = Vec::with_capacity(n);
    let mut i_hi = 0;
    let mut i_lo = 0;
    for j in 0..n {
        let bar = Bar {
            ts: open_ns + j as i64 * NANOS_PER_MIN,
            open: map(o[j]).exp(),
            high: map(h[j]).exp(),
            low: map(l[j]).exp(),
            close: map(c[j]).exp(),
            volume: 0.0,
        };
        if h[j] > h[i_hi] {
            i_hi = j;
        }
        if l[j] < l[i_lo] {
            i_lo = j;
        }
        out.push(bar);
    }
    // Exactness at the extremes (guards degenerate maps and float drift).
    out[i_hi].high = bar.h;
    out[i_lo].low = bar.l;
    if let Some(first) = out.first_mut() {
        first.open = bar.o;
    }
    if let Some(last) = out.last_mut() {
        last.close = bar.c;
    }
    for b2 in &mut out {
        b2.high = b2.high.max(b2.open).max(b2.close);
        b2.low = b2.low.min(b2.open).min(b2.close);
    }

    // Volume: split the daily total by the weights, exact sum.
    let total = round_volume(bar.v, vol_decimals);
    if total > 0.0 {
        let wsum: f64 = vw.iter().sum();
        let mut assigned = 0.0;
        for (j, b2) in out.iter_mut().enumerate() {
            b2.volume = round_volume(total * vw[j] / wsum, vol_decimals);
            assigned += b2.volume;
        }
        let diff = round_volume(total - assigned, vol_decimals);
        if diff != 0.0 {
            // Put the rounding remainder on the largest bar.
            let jmax = (0..n).max_by(|p, q| out[*p].volume.total_cmp(&out[*q].volume)).unwrap_or(0);
            out[jmax].volume = round_volume((out[jmax].volume + diff).max(0.0), vol_decimals);
        }
    }
    out
}

/// Aggregates one-minute bars into `bucket`-minute bars aligned to the
/// session open.
pub(crate) fn aggregate(minutes: &[Bar], open_ns: UnixNanos, bucket: i64, out: &mut Vec<Bar>) {
    if bucket <= 1 {
        out.extend_from_slice(minutes);
        return;
    }
    let span = bucket * NANOS_PER_MIN;
    let mut cur: Option<Bar> = None;
    let mut cur_bucket = i64::MIN;
    for m in minutes {
        let k = (m.ts - open_ns).div_euclid(span);
        match cur.as_mut() {
            Some(b) if k == cur_bucket => {
                b.high = b.high.max(m.high);
                b.low = b.low.min(m.low);
                b.close = m.close;
                b.volume += m.volume;
            }
            _ => {
                if let Some(b) = cur.take() {
                    out.push(b);
                }
                cur_bucket = k;
                cur = Some(Bar { ts: open_ns + k * span, ..*m });
            }
        }
    }
    if let Some(b) = cur {
        out.push(b);
    }
}

/// Summary of a run of minute bars (partial-day bar, VWAP).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Summary {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub vwap: Option<f64>,
    pub prev_minute_close: Option<f64>,
    pub last_ts: UnixNanos,
}

pub(crate) fn summarize(minutes: &[Bar]) -> Option<Summary> {
    let first = minutes.first()?;
    let last = minutes.last()?;
    let mut high = f64::MIN;
    let mut low = f64::MAX;
    let mut vol = 0.0;
    let mut pv = 0.0;
    for m in minutes {
        high = high.max(m.high);
        low = low.min(m.low);
        vol += m.volume;
        pv += m.volume * (m.high + m.low + m.close) / 3.0;
    }
    Some(Summary {
        open: first.open,
        high,
        low,
        close: last.close,
        volume: vol,
        vwap: (vol > 0.0).then(|| pv / vol),
        prev_minute_close: minutes.len().checked_sub(2).map(|i| minutes[i].close),
        last_ts: last.ts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cal::ymd;

    #[test]
    fn minutes_match_daily_bar() {
        let bar = DayBar { date: ymd(2026, 3, 9), o: 100.0, h: 103.5, l: 98.2, c: 101.7, v: 1_234_567.0 };
        let m = day_minutes(3, 99, &Session::US_EQUITY, bar.date, &bar, 0);
        assert_eq!(m.len(), 390);
        let s = summarize(&m).unwrap();
        assert!((s.open - 100.0).abs() < 1e-9);
        assert!((s.close - 101.7).abs() < 1e-9);
        assert!((s.high - 103.5).abs() < 1e-9, "{}", s.high);
        assert!((s.low - 98.2).abs() < 1e-9, "{}", s.low);
        assert_eq!(s.volume, 1_234_567.0);
        for b in &m {
            assert!(b.high >= b.open.max(b.close) && b.low <= b.open.min(b.close));
        }
        // U-shape: first and last half hour busier than midday.
        let first: f64 = m[..30].iter().map(|b| b.volume).sum();
        let mid: f64 = m[180..210].iter().map(|b| b.volume).sum();
        assert!(first > mid);
        let mut agg = Vec::new();
        aggregate(&m, Session::US_EQUITY.open_utc(bar.date), 60, &mut agg);
        assert_eq!(agg.len(), 7);
        assert_eq!(agg.iter().map(|b| b.volume).sum::<f64>(), 1_234_567.0);
    }
}

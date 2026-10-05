//! Level-of-detail helpers for chart rendering.
//!
//! The renderer draws each pixel column from the coarsest pyramid level whose
//! buckets are still narrower than a pixel, so a frame costs O(width) no
//! matter how many bars the series has (ARCHITECTURE §7.4).

use serde::{Deserialize, Serialize};

/// One pyramid level: element `i` covers bars `[i·2^k, (i+1)·2^k)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MinMaxLevel {
    pub min: Vec<f32>,
    pub max: Vec<f32>,
}

/// Min/max decimation pyramid over bar highs and lows.
///
/// Level 0 holds the raw lows/highs (as `f32`); level `k` aggregates `2^k`
/// bars (a trailing odd bucket covers fewer). Levels are built until one has
/// at most one element, which is included. Empty input gives no levels; a
/// length mismatch uses the shorter input. Non-finite values are ignored
/// within a bucket (an all-`NaN` bucket stays `NaN`).
///
/// Values are converted to `f32`: subtract a price origin first (as the
/// series buffers do) if the prices need more than ~7 significant digits.
pub fn build_minmax_pyramid(high: &[f64], low: &[f64]) -> Vec<MinMaxLevel> {
    let n = high.len().min(low.len());
    if n == 0 {
        return Vec::new();
    }
    let mut levels = vec![MinMaxLevel {
        min: low[..n].iter().map(|&v| v as f32).collect(),
        max: high[..n].iter().map(|&v| v as f32).collect(),
    }];
    while let Some(prev) = levels.last().filter(|l| l.min.len() > 1) {
        // f32::min/max return the non-NaN operand, so NaNs are skipped.
        let min = prev.min.chunks(2).map(|c| c.iter().copied().fold(f32::NAN, f32::min)).collect();
        let max = prev.max.chunks(2).map(|c| c.iter().copied().fold(f32::NAN, f32::max)).collect();
        levels.push(MinMaxLevel { min, max });
    }
    levels
}

/// OHLCV columns, e.g. the output of [`aggregate_ohlc`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OhlcColumns {
    /// Timestamp of each bucket's first bar (Unix nanos).
    pub ts: Vec<i64>,
    pub open: Vec<f64>,
    pub high: Vec<f64>,
    pub low: Vec<f64>,
    pub close: Vec<f64>,
    pub volume: Vec<f64>,
}

/// Aggregates consecutive groups of `bucket` bars into one bar each: first
/// timestamp and open, max high, min low, last close, summed volume. The last
/// bucket may be partial. `bucket = 0` is treated as 1. Inputs of different
/// lengths are truncated to the shortest. Non-finite highs/lows/volumes are
/// ignored within a bucket.
pub fn aggregate_ohlc(
    ts: &[i64],
    open: &[f64],
    high: &[f64],
    low: &[f64],
    close: &[f64],
    volume: &[f64],
    bucket: usize,
) -> OhlcColumns {
    let n = [ts.len(), open.len(), high.len(), low.len(), close.len(), volume.len()].into_iter().min().unwrap_or(0);
    let b = bucket.max(1);
    let m = n.div_ceil(b);
    let mut out = OhlcColumns {
        ts: Vec::with_capacity(m),
        open: Vec::with_capacity(m),
        high: Vec::with_capacity(m),
        low: Vec::with_capacity(m),
        close: Vec::with_capacity(m),
        volume: Vec::with_capacity(m),
    };
    for start in (0..n).step_by(b) {
        let end = (start + b).min(n);
        out.ts.push(ts[start]);
        out.open.push(open[start]);
        out.high.push(high[start..end].iter().copied().fold(f64::NAN, f64::max));
        out.low.push(low[start..end].iter().copied().fold(f64::NAN, f64::min));
        out.close.push(close[end - 1]);
        out.volume.push(volume[start..end].iter().filter(|v| v.is_finite()).sum());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pyramid_levels() {
        let high = [5.0, 7.0, 6.0, 9.0, 4.0];
        let low = [1.0, 3.0, 2.0, 0.5, 2.5];
        let p = build_minmax_pyramid(&high, &low);
        // Lengths 5 → 3 → 2 → 1.
        assert_eq!(p.iter().map(|l| l.min.len()).collect::<Vec<_>>(), vec![5, 3, 2, 1]);
        assert_eq!(p[0].max, vec![5.0, 7.0, 6.0, 9.0, 4.0]);
        assert_eq!(p[1].max, vec![7.0, 9.0, 4.0]);
        assert_eq!(p[1].min, vec![1.0, 0.5, 2.5]);
        assert_eq!(p[2].max, vec![9.0, 4.0]);
        assert_eq!(p[3].max, vec![9.0]);
        assert_eq!(p[3].min, vec![0.5]);
        assert!(build_minmax_pyramid(&[], &[]).is_empty());
        assert_eq!(build_minmax_pyramid(&[1.0], &[0.0]).len(), 1);
    }

    #[test]
    fn pyramid_skips_nan_and_matches_bruteforce() {
        let n = 1000;
        let high: Vec<f64> = (0..n).map(|i| if i % 97 == 0 { f64::NAN } else { (i as f64 * 0.37).sin() * 10.0 + 50.0 }).collect();
        let low: Vec<f64> = high.iter().map(|h| h - 1.0).collect();
        let p = build_minmax_pyramid(&high, &low);
        assert_eq!(p.last().unwrap().max.len(), 1);
        let brute = |seg: &[f64], pick: fn(f64, f64) -> f64| -> f32 {
            seg.iter().copied().filter(|v| v.is_finite()).reduce(pick).map_or(f32::NAN, |v| v as f32)
        };
        for (k, level) in p.iter().enumerate() {
            let w = 1usize << k;
            for (i, (&mx, &mn)) in level.max.iter().zip(&level.min).enumerate() {
                let range = i * w..((i + 1) * w).min(n);
                let (want_max, want_min) = (brute(&high[range.clone()], f64::max), brute(&low[range], f64::min));
                assert!(mx == want_max || (mx.is_nan() && want_max.is_nan()), "level {k} bucket {i}");
                assert!(mn == want_min || (mn.is_nan() && want_min.is_nan()), "level {k} bucket {i}");
            }
        }
    }

    #[test]
    fn ohlc_buckets() {
        let ts = [10, 20, 30, 40, 50];
        let o = [1.0, 2.0, 3.0, 4.0, 5.0];
        let h = [1.5, 2.5, 3.5, 4.5, 5.5];
        let l = [0.5, 1.5, 2.5, 3.5, 4.5];
        let c = [1.2, 2.2, 3.2, 4.2, 5.2];
        let v = [10.0, 20.0, 30.0, 40.0, 50.0];
        let a = aggregate_ohlc(&ts, &o, &h, &l, &c, &v, 2);
        assert_eq!(a.ts, vec![10, 30, 50]);
        assert_eq!(a.open, vec![1.0, 3.0, 5.0]);
        assert_eq!(a.high, vec![2.5, 4.5, 5.5]);
        assert_eq!(a.low, vec![0.5, 2.5, 4.5]);
        assert_eq!(a.close, vec![2.2, 4.2, 5.2]);
        assert_eq!(a.volume, vec![30.0, 70.0, 50.0]);
        let one = aggregate_ohlc(&ts, &o, &h, &l, &c, &v, 0);
        assert_eq!(one.close, c.to_vec());
        let empty = aggregate_ohlc(&[], &[], &[], &[], &[], &[], 3);
        assert!(empty.ts.is_empty());
    }
}

//! Technical indicators over price/volume series.
//!
//! Conventions shared by every function here:
//!
//! - Output length equals input length; positions without enough history
//!   (warm-up) are `f64::NAN`.
//! - A period of `0` yields an all-`NaN` output.
//! - **Windowed** indicators (SMA, WMA, rolling std/min/max, stochastic) need
//!   a full window of finite values: a non-finite input restarts the warm-up.
//! - **Recursive** indicators (EMA, Wilder/RMA, RSI, ATR, MACD) skip leading
//!   non-finite values and seed with the simple average of the first `n`
//!   consecutive finite values. A non-finite value after seeding produces
//!   `NaN` at that index and the smoothing state carries over unchanged.
//! - Multi-input indicators (ATR, stochastic, VWAP, OBV) use the length of the
//!   shortest input.
//!
//! Units: outputs are in the units of the price input, except RSI and the
//! stochastic oscillator (0–100) and OBV (volume units).

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

/// Bollinger bands; each vector has the input's length.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bands {
    pub upper: Vec<f64>,
    pub middle: Vec<f64>,
    pub lower: Vec<f64>,
}

/// MACD line, signal line and histogram (`macd − signal`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Macd {
    pub macd: Vec<f64>,
    pub signal: Vec<f64>,
    pub hist: Vec<f64>,
}

fn nan_vec(len: usize) -> Vec<f64> {
    vec![f64::NAN; len]
}

/// Simple moving average over `n` periods.
pub fn sma(x: &[f64], n: usize) -> Vec<f64> {
    let mut out = nan_vec(x.len());
    if n == 0 {
        return out;
    }
    let mut sum = 0.0;
    let mut run = 0usize;
    for (i, &v) in x.iter().enumerate() {
        if !v.is_finite() {
            sum = 0.0;
            run = 0;
            continue;
        }
        sum += v;
        run += 1;
        if run > n {
            // x[i-n] is inside the current finite run.
            sum -= x[i - n];
            run = n;
        }
        if run == n {
            out[i] = sum / n as f64;
        }
    }
    out
}

/// Exponential smoothing with factor `alpha`, seeded by the SMA of the first
/// `n` consecutive finite values.
fn exp_smooth(x: &[f64], n: usize, alpha: f64) -> Vec<f64> {
    let mut out = nan_vec(x.len());
    if n == 0 {
        return out;
    }
    let mut state: Option<f64> = None;
    let mut seed_sum = 0.0;
    let mut seed_run = 0usize;
    for (i, &v) in x.iter().enumerate() {
        match state {
            None => {
                if v.is_finite() {
                    seed_sum += v;
                    seed_run += 1;
                    if seed_run == n {
                        let s = seed_sum / n as f64;
                        state = Some(s);
                        out[i] = s;
                    }
                } else {
                    seed_sum = 0.0;
                    seed_run = 0;
                }
            }
            Some(prev) => {
                if v.is_finite() {
                    let s = alpha * v + (1.0 - alpha) * prev;
                    state = Some(s);
                    out[i] = s;
                }
            }
        }
    }
    out
}

/// Exponential moving average with `alpha = 2/(n+1)`, seeded with the SMA of
/// the first `n` values (first output at index `n−1`).
pub fn ema(x: &[f64], n: usize) -> Vec<f64> {
    exp_smooth(x, n, 2.0 / (n as f64 + 1.0))
}

/// Wilder's moving average (a.k.a. RMA / SMMA): `alpha = 1/n`, seeded with the
/// SMA of the first `n` values. Used by [`rsi`] and [`atr`].
pub fn rma(x: &[f64], n: usize) -> Vec<f64> {
    exp_smooth(x, n, 1.0 / n as f64)
}

/// Linearly weighted moving average: weights `1..=n`, newest value weighted `n`.
pub fn wma(x: &[f64], n: usize) -> Vec<f64> {
    let mut out = nan_vec(x.len());
    if n == 0 {
        return out;
    }
    let nf = n as f64;
    let denom = nf * (nf + 1.0) / 2.0;
    let mut run = 0usize;
    // `plain` = Σ window values, `weighted` = Σ j·x_j with j = 1 (oldest) .. n.
    let mut plain = 0.0;
    let mut weighted = 0.0;
    for (i, &v) in x.iter().enumerate() {
        if !v.is_finite() {
            run = 0;
            continue;
        }
        run += 1;
        if run < n {
            continue;
        }
        if run == n {
            // (Re)build the window exactly.
            plain = 0.0;
            weighted = 0.0;
            for (j, &w) in x[i + 1 - n..=i].iter().enumerate() {
                plain += w;
                weighted += (j + 1) as f64 * w;
            }
        } else {
            // Slide by one: every weight drops by 1, the new value gets n.
            weighted += nf * v - plain;
            plain += v - x[i - n];
            run = n;
        }
        out[i] = weighted / denom;
    }
    out
}

/// Rolling **population** standard deviation (divisor `n`) over `n` periods.
///
/// Uses a sliding Welford update, which stays accurate when the mean is large
/// relative to the dispersion (e.g. index levels).
pub fn rolling_std(x: &[f64], n: usize) -> Vec<f64> {
    let mut out = nan_vec(x.len());
    if n == 0 {
        return out;
    }
    let nf = n as f64;
    let mut run = 0usize;
    let mut mean = 0.0;
    let mut m2 = 0.0;
    for (i, &v) in x.iter().enumerate() {
        if !v.is_finite() {
            run = 0;
            mean = 0.0;
            m2 = 0.0;
            continue;
        }
        if run < n {
            run += 1;
            let delta = v - mean;
            mean += delta / run as f64;
            m2 += delta * (v - mean);
        } else {
            let old = x[i - n];
            let old_mean = mean;
            mean += (v - old) / nf;
            m2 += (v - old) * (v - mean + old - old_mean);
        }
        if run == n {
            out[i] = (m2.max(0.0) / nf).sqrt();
        }
    }
    out
}

/// Bollinger bands: `middle = SMA(n)`, `upper/lower = middle ± k·σ` with σ the
/// rolling population standard deviation.
pub fn bollinger(x: &[f64], n: usize, k: f64) -> Bands {
    let middle = sma(x, n);
    let sd = rolling_std(x, n);
    let upper = middle.iter().zip(&sd).map(|(m, s)| m + k * s).collect();
    let lower = middle.iter().zip(&sd).map(|(m, s)| m - k * s).collect();
    Bands { upper, middle, lower }
}

/// Relative Strength Index (0–100) with Wilder smoothing.
///
/// The first value is at index `n`: average gain/loss of the first `n` price
/// changes, then `avg = (prev·(n−1) + current)/n`. When the average loss is 0
/// the RSI is 100 (or 50 if the average gain is also 0).
pub fn rsi(x: &[f64], n: usize) -> Vec<f64> {
    let len = x.len();
    let mut gains = nan_vec(len);
    let mut losses = nan_vec(len);
    for i in 1..len {
        let d = x[i] - x[i - 1];
        if d.is_finite() {
            gains[i] = d.max(0.0);
            losses[i] = (-d).max(0.0);
        }
    }
    let ag = rma(&gains, n);
    let al = rma(&losses, n);
    ag.iter()
        .zip(&al)
        .map(|(&g, &l)| {
            if !g.is_finite() || !l.is_finite() {
                f64::NAN
            } else if l == 0.0 {
                if g == 0.0 { 50.0 } else { 100.0 }
            } else {
                100.0 - 100.0 / (1.0 + g / l)
            }
        })
        .collect()
}

/// MACD: `EMA(fast) − EMA(slow)`, its `signal`-period EMA, and the histogram.
///
/// The signal EMA is seeded from the first `signal` finite MACD values, so the
/// first signal value is at index `slow + signal − 2` (for `fast < slow`).
pub fn macd(x: &[f64], fast: usize, slow: usize, signal: usize) -> Macd {
    let ef = ema(x, fast);
    let es = ema(x, slow);
    let line: Vec<f64> = ef.iter().zip(&es).map(|(f, s)| f - s).collect();
    let sig = ema(&line, signal);
    let hist = line.iter().zip(&sig).map(|(m, s)| m - s).collect();
    Macd { macd: line, signal: sig, hist }
}

/// True range series: `TR₀ = H₀ − L₀`, then
/// `TRᵢ = max(Hᵢ − Lᵢ, |Hᵢ − Cᵢ₋₁|, |Lᵢ − Cᵢ₋₁|)`.
pub fn true_range(high: &[f64], low: &[f64], close: &[f64]) -> Vec<f64> {
    let len = high.len().min(low.len()).min(close.len());
    (0..len)
        .map(|i| {
            let hl = high[i] - low[i];
            if i == 0 || !close[i - 1].is_finite() {
                hl
            } else {
                let pc = close[i - 1];
                hl.max((high[i] - pc).abs()).max((low[i] - pc).abs())
            }
        })
        .collect()
}

/// Average True Range with Wilder smoothing (price units).
///
/// Follows Wilder / StockCharts: the first TR is `H₀ − L₀` and the first ATR
/// (index `n−1`) is the mean of the first `n` true ranges.
pub fn atr(high: &[f64], low: &[f64], close: &[f64], n: usize) -> Vec<f64> {
    rma(&true_range(high, low, close), n)
}

/// Rolling maximum over `n` periods (monotonic deque, O(len)).
pub fn rolling_max(x: &[f64], n: usize) -> Vec<f64> {
    rolling_extreme(x, n, |a, b| a >= b)
}

/// Rolling minimum over `n` periods (monotonic deque, O(len)).
pub fn rolling_min(x: &[f64], n: usize) -> Vec<f64> {
    rolling_extreme(x, n, |a, b| a <= b)
}

/// `dominates(a, b)` is true when `a` makes `b` irrelevant as a future extreme.
fn rolling_extreme(x: &[f64], n: usize, dominates: impl Fn(f64, f64) -> bool) -> Vec<f64> {
    let mut out = nan_vec(x.len());
    if n == 0 {
        return out;
    }
    let mut dq: VecDeque<usize> = VecDeque::new();
    let mut run = 0usize;
    for (i, &v) in x.iter().enumerate() {
        if !v.is_finite() {
            dq.clear();
            run = 0;
            continue;
        }
        run += 1;
        while dq.back().is_some_and(|&j| dominates(v, x[j])) {
            dq.pop_back();
        }
        dq.push_back(i);
        while dq.front().is_some_and(|&j| j + n <= i) {
            dq.pop_front();
        }
        if run >= n
            && let Some(&j) = dq.front()
        {
            out[i] = x[j];
        }
    }
    out
}

/// Stochastic oscillator `(%K, %D)`, both 0–100.
///
/// `%K = 100·(C − LL)/(HH − LL)` over the last `k_period` highs/lows;
/// `%D = SMA(%K, d_period)`. When the range is zero `%K` is 50.
pub fn stochastic(
    high: &[f64],
    low: &[f64],
    close: &[f64],
    k_period: usize,
    d_period: usize,
) -> (Vec<f64>, Vec<f64>) {
    let len = high.len().min(low.len()).min(close.len());
    let hh = rolling_max(&high[..len], k_period);
    let ll = rolling_min(&low[..len], k_period);
    let k: Vec<f64> = (0..len)
        .map(|i| {
            let range = hh[i] - ll[i];
            if !range.is_finite() || !close[i].is_finite() {
                f64::NAN
            } else if range == 0.0 {
                50.0
            } else {
                100.0 * (close[i] - ll[i]) / range
            }
        })
        .collect();
    let d = sma(&k, d_period);
    (k, d)
}

/// Cumulative (session-anchored) VWAP using typical price `(H+L+C)/3`.
///
/// Bars with non-finite prices or non-finite/negative volume are skipped (the
/// running VWAP carries through them). `NaN` until cumulative volume is > 0.
/// The caller slices the input at session boundaries to anchor it.
pub fn vwap(high: &[f64], low: &[f64], close: &[f64], volume: &[f64]) -> Vec<f64> {
    let len = high.len().min(low.len()).min(close.len()).min(volume.len());
    let mut pv = 0.0;
    let mut vol = 0.0;
    (0..len)
        .map(|i| {
            let tp = (high[i] + low[i] + close[i]) / 3.0;
            let v = volume[i];
            if tp.is_finite() && v.is_finite() && v >= 0.0 {
                pv += tp * v;
                vol += v;
            }
            if vol > 0.0 { pv / vol } else { f64::NAN }
        })
        .collect()
}

/// On-Balance Volume, starting at 0 on the first bar. Volume is added on an
/// up close, subtracted on a down close, unchanged otherwise (including when
/// either close or the volume is non-finite).
pub fn obv(close: &[f64], volume: &[f64]) -> Vec<f64> {
    let len = close.len().min(volume.len());
    let mut out = Vec::with_capacity(len);
    let mut acc = 0.0;
    for i in 0..len {
        if i > 0 && volume[i].is_finite() {
            if close[i] > close[i - 1] {
                acc += volume[i];
            } else if close[i] < close[i - 1] {
                acc -= volume[i];
            }
        }
        out.push(acc);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_series(got: &[f64], want: &[f64], tol: f64) {
        assert_eq!(got.len(), want.len(), "length");
        for (i, (g, w)) in got.iter().zip(want).enumerate() {
            if w.is_nan() {
                assert!(g.is_nan(), "index {i}: got {g}, want NaN");
            } else {
                assert!((g - w).abs() <= tol, "index {i}: got {g}, want {w}");
            }
        }
    }

    const NAN: f64 = f64::NAN;

    #[test]
    fn sma_basic_and_nan_restart() {
        assert_series(&sma(&[1.0, 2.0, 3.0, 4.0, 5.0], 3), &[NAN, NAN, 2.0, 3.0, 4.0], 1e-12);
        assert_series(&sma(&[1.0, 2.0, NAN, 4.0, 5.0, 6.0], 2), &[NAN, 1.5, NAN, NAN, 4.5, 5.5], 1e-12);
        assert_series(&sma(&[1.0, 2.0], 0), &[NAN, NAN], 0.0);
        assert_series(&sma(&[1.0, 2.0], 3), &[NAN, NAN], 0.0);
        assert!(sma(&[], 3).is_empty());
    }

    #[test]
    fn ema_seeded_with_sma() {
        // n=3, alpha=0.5: seed = mean(1,2,3) = 2; then 0.5*4+0.5*2 = 3; 0.5*5+0.5*3 = 4.
        assert_series(&ema(&[1.0, 2.0, 3.0, 4.0, 5.0], 3), &[NAN, NAN, 2.0, 3.0, 4.0], 1e-12);
        // Leading NaNs are skipped.
        assert_series(&ema(&[NAN, 1.0, 2.0, 3.0, 4.0], 3), &[NAN, NAN, NAN, 2.0, 3.0], 1e-12);
        // NaN after seeding: NaN out, state carries.
        assert_series(&ema(&[1.0, 2.0, 3.0, NAN, 4.0], 3), &[NAN, NAN, 2.0, NAN, 3.0], 1e-12);
    }

    #[test]
    fn wma_weights() {
        // n=3, weights 1,2,3 / 6: (1+4+9)/6 = 14/6, (2+6+12)/6 = 20/6, (3+8+15)/6 = 26/6.
        let x = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_series(&wma(&x, 3), &[NAN, NAN, 14.0 / 6.0, 20.0 / 6.0, 26.0 / 6.0], 1e-12);
        // Incremental path matches the exact rebuild over a long series.
        let long: Vec<f64> = (0..500).map(|i| 100.0 + (f64::from(i) * 0.37).sin() * 5.0).collect();
        let w = wma(&long, 20);
        for i in 19..500 {
            let exact: f64 = (0..20).map(|j| (j + 1) as f64 * long[i - 19 + j]).sum::<f64>() / 210.0;
            assert!((w[i] - exact).abs() < 1e-9);
        }
    }

    #[test]
    fn rolling_std_population() {
        // Window [2,4,4,4,5,5,7,9] has population σ = 2.
        let x = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        let s = rolling_std(&x, 8);
        assert!((s[7] - 2.0).abs() < 1e-12);
        assert!(s[6].is_nan());
        // Sliding update matches a direct computation, also at large offsets.
        let long: Vec<f64> = (0..400).map(|i| 1.0e6 + (f64::from(i) * 0.7).cos()).collect();
        let s = rolling_std(&long, 10);
        for i in 9..400 {
            let w = &long[i - 9..=i];
            let m = w.iter().sum::<f64>() / 10.0;
            let v = w.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / 10.0;
            assert!((s[i] - v.sqrt()).abs() < 1e-8, "i={i}");
        }
    }

    #[test]
    fn bollinger_bands() {
        let x = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        let b = bollinger(&x, 8, 2.0);
        assert!((b.middle[7] - 5.0).abs() < 1e-12);
        assert!((b.upper[7] - 9.0).abs() < 1e-12);
        assert!((b.lower[7] - 1.0).abs() < 1e-12);
        assert!(b.upper[0].is_nan());
    }

    /// Worked example from StockCharts ChartSchool "Relative Strength Index"
    /// (14-period RSI spreadsheet); published values rounded to 2 decimals.
    #[test]
    fn rsi_matches_stockcharts_example() {
        let closes = [
            44.3389, 44.0902, 44.1497, 43.6124, 44.3278, 44.8264, 45.0955, 45.4245, 45.8433, 46.0826, 45.8931, 46.0328,
            45.6140, 46.2820, 46.2820, 46.0028, 46.0328, 46.4116, 46.2222, 45.6439, 46.2122, 46.2521, 45.7137, 46.4515,
            45.7835, 45.3548, 44.0288, 44.1783, 44.2181, 44.5672, 43.4205, 42.6628, 43.1314,
        ];
        let published = [
            70.53, 66.32, 66.55, 69.41, 66.36, 57.97, 62.93, 63.26, 56.06, 62.38, 54.71, 50.42, 39.99, 41.46, 41.87, 45.46,
            37.30, 33.08, 37.77,
        ];
        let r = rsi(&closes, 14);
        assert_eq!(r.len(), closes.len());
        assert!(r[..14].iter().all(|v| v.is_nan()));
        for (i, want) in published.iter().enumerate() {
            assert!((r[14 + i] - want).abs() < 0.005, "RSI[{}] = {}, published {want}", 14 + i, r[14 + i]);
        }
    }

    #[test]
    fn rsi_edge_cases() {
        let up = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(rsi(&up, 3)[3], 100.0);
        let flat = [5.0; 4];
        assert_eq!(rsi(&flat, 3)[3], 50.0);
    }

    #[test]
    fn macd_composition() {
        let x: Vec<f64> = (0..60).map(|i| 100.0 + f64::from(i) * 0.5 + (f64::from(i) * 0.9).sin()).collect();
        let m = macd(&x, 12, 26, 9);
        let ef = ema(&x, 12);
        let es = ema(&x, 26);
        assert!(m.macd[24].is_nan());
        assert!((m.macd[25] - (ef[25] - es[25])).abs() < 1e-12);
        // Signal seeded from MACD values 25..=33 → first at index 33.
        assert!(m.signal[32].is_nan());
        let seed = m.macd[25..=33].iter().sum::<f64>() / 9.0;
        assert!((m.signal[33] - seed).abs() < 1e-12);
        assert!((m.hist[40] - (m.macd[40] - m.signal[40])).abs() < 1e-12);
    }

    #[test]
    fn atr_wilder() {
        let high = [10.0, 11.0, 12.0, 11.5];
        let low = [9.0, 10.0, 10.5, 10.0];
        let close = [9.5, 10.5, 11.0, 10.2];
        // TR = [1, max(1, 1.5, 0.5)=1.5, max(1.5, 1.5, 0)=1.5, max(1.5, 0.5, 1)=1.5]
        let tr = true_range(&high, &low, &close);
        assert_series(&tr, &[1.0, 1.5, 1.5, 1.5], 1e-12);
        // n=2: seed (1+1.5)/2 = 1.25; then (1.25+1.5)/2 = 1.375; then 1.4375.
        assert_series(&atr(&high, &low, &close, 2), &[NAN, 1.25, 1.375, 1.4375], 1e-12);
    }

    #[test]
    fn rolling_extremes() {
        let x = [3.0, 1.0, 4.0, 1.0, 5.0, 9.0, 2.0, 6.0];
        assert_series(&rolling_max(&x, 3), &[NAN, NAN, 4.0, 4.0, 5.0, 9.0, 9.0, 9.0], 0.0);
        assert_series(&rolling_min(&x, 3), &[NAN, NAN, 1.0, 1.0, 1.0, 1.0, 2.0, 2.0], 0.0);
    }

    #[test]
    fn stochastic_values() {
        let high = [10.0, 12.0, 11.0, 13.0];
        let low = [8.0, 9.0, 9.5, 10.0];
        let close = [9.0, 11.0, 10.0, 12.5];
        let (k, d) = stochastic(&high, &low, &close, 2, 2);
        // i=1: HH=12, LL=8 → 100*(11-8)/4 = 75; i=2: HH=12, LL=9 → 33.33; i=3: HH=13, LL=9.5 → 85.71
        assert_series(&k, &[NAN, 75.0, 100.0 / 3.0, 100.0 * 3.0 / 3.5], 1e-9);
        assert_series(&d, &[NAN, NAN, f64::midpoint(75.0, 100.0 / 3.0), f64::midpoint(100.0 / 3.0, 100.0 * 3.0 / 3.5)], 1e-9);
    }

    #[test]
    fn vwap_cumulative() {
        let high = [11.0, 12.0];
        let low = [9.0, 10.0];
        let close = [10.0, 11.0];
        let vol = [100.0, 300.0];
        // TP = [10, 11]; VWAP = [10, (1000+3300)/400 = 10.75]
        assert_series(&vwap(&high, &low, &close, &vol), &[10.0, 10.75], 1e-12);
        assert!(vwap(&[1.0], &[1.0], &[1.0], &[0.0])[0].is_nan());
    }

    #[test]
    fn obv_accumulates() {
        let close = [10.0, 11.0, 10.5, 10.5, 12.0];
        let vol = [100.0, 200.0, 150.0, 50.0, 300.0];
        assert_series(&obv(&close, &vol), &[0.0, 200.0, 50.0, 50.0, 350.0], 0.0);
    }
}

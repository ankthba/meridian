//! Return series and performance statistics.
//!
//! All returns are decimal fractions per period (`0.01` = 1 %). Summary
//! statistics ignore non-finite values (a gap in a price series becomes `NaN`
//! returns, which are skipped) and return `NaN` when undefined (too few
//! observations, zero dispersion, non-positive prices).

use serde::{Deserialize, Serialize};

use crate::stats;

/// Simple returns `pᵢ/pᵢ₋₁ − 1`; length `len − 1` (empty for fewer than two
/// prices). Non-finite results (zero/NaN prices) are `NaN`.
pub fn simple_returns(prices: &[f64]) -> Vec<f64> {
    prices
        .windows(2)
        .map(|w| {
            let r = w[1] / w[0] - 1.0;
            if r.is_finite() { r } else { f64::NAN }
        })
        .collect()
}

/// Log returns `ln(pᵢ/pᵢ₋₁)`; length `len − 1`. `NaN` where the ratio is not
/// strictly positive and finite.
pub fn log_returns(prices: &[f64]) -> Vec<f64> {
    prices
        .windows(2)
        .map(|w| {
            let ratio = w[1] / w[0];
            if ratio.is_finite() && ratio > 0.0 { ratio.ln() } else { f64::NAN }
        })
        .collect()
}

/// Compounded cumulative return of a **simple-return** series:
/// `cᵢ = Π_{j≤i}(1 + rⱼ) − 1`, same length as `returns`. A non-finite return
/// is treated as a missing period (the cumulative value carries over).
pub fn cumulative_returns(returns: &[f64]) -> Vec<f64> {
    let mut growth = 1.0;
    returns
        .iter()
        .map(|&r| {
            if r.is_finite() {
                growth *= 1.0 + r;
            }
            growth - 1.0
        })
        .collect()
}

/// Annualized volatility: sample standard deviation of per-period returns ×
/// `√periods_per_year` (252 for US trading days, 365 for crypto days, 52, 12…).
pub fn annualized_volatility(returns: &[f64], periods_per_year: f64) -> f64 {
    stats::std_dev(returns) * periods_per_year.sqrt()
}

/// Largest peak-to-trough decline of a price (or equity) series.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Drawdown {
    /// Positive fraction: `0.25` means the series fell 25 % from its peak.
    pub max_drawdown: f64,
    /// Index of the running peak that preceded the trough.
    pub peak_index: usize,
    /// Index of the trough.
    pub trough_index: usize,
    /// First index after the trough where the series regained the peak level,
    /// if it ever did.
    pub recovery_index: Option<usize>,
}

/// Maximum drawdown of `prices`. Non-finite and non-positive values are
/// skipped. For an empty or monotonically rising series the drawdown is 0 with
/// both indices at the first valid point.
pub fn max_drawdown(prices: &[f64]) -> Drawdown {
    let mut best = Drawdown { max_drawdown: 0.0, peak_index: 0, trough_index: 0, recovery_index: None };
    let mut peak: Option<(usize, f64)> = None;
    let mut initialized = false;
    for (i, &p) in prices.iter().enumerate() {
        if !p.is_finite() || p <= 0.0 {
            continue;
        }
        if !initialized {
            best.peak_index = i;
            best.trough_index = i;
            initialized = true;
        }
        match peak {
            Some((pi, pv)) if p <= pv => {
                let dd = 1.0 - p / pv;
                if dd > best.max_drawdown {
                    best = Drawdown { max_drawdown: dd, peak_index: pi, trough_index: i, recovery_index: None };
                }
            }
            _ => peak = Some((i, p)),
        }
    }
    if best.max_drawdown > 0.0 {
        let level = prices[best.peak_index];
        best.recovery_index =
            prices.iter().enumerate().skip(best.trough_index + 1).find(|(_, p)| **p >= level).map(|(i, _)| i);
    }
    best
}

/// Drawdown at each point relative to the running peak: `pᵢ/peakᵢ − 1`
/// (`≤ 0`; `−0.25` = 25 % below the peak). `NaN` where the price is
/// non-finite/non-positive or before the first valid price.
pub fn drawdown_series(prices: &[f64]) -> Vec<f64> {
    let mut peak = f64::NAN;
    prices
        .iter()
        .map(|&p| {
            if !p.is_finite() || p <= 0.0 {
                return f64::NAN;
            }
            if peak.is_nan() || p > peak {
                peak = p;
            }
            p / peak - 1.0
        })
        .collect()
}

/// Annualized Sharpe ratio: `mean(r − rf) / sd(r − rf) · √periods_per_year`,
/// with `rf_per_period` the risk-free rate per period (e.g. `0.04/252`) and
/// the sample standard deviation. `NaN` with fewer than 2 returns or zero
/// dispersion.
pub fn sharpe(returns: &[f64], rf_per_period: f64, periods_per_year: f64) -> f64 {
    let excess: Vec<f64> = returns.iter().filter(|r| r.is_finite()).map(|r| r - rf_per_period).collect();
    let sd = stats::std_dev(&excess);
    if sd.is_nan() || sd <= 0.0 {
        return f64::NAN;
    }
    stats::mean(&excess) / sd * periods_per_year.sqrt()
}

/// Annualized Sortino ratio: `mean(r − rf) / DD · √periods_per_year`, where the
/// downside deviation `DD = √(Σ min(r − rf, 0)² / N)` uses all `N`
/// observations (Sortino & Price). `rf_per_period` doubles as the minimum
/// acceptable return. `NaN` when there is no downside or fewer than 2 returns.
pub fn sortino(returns: &[f64], rf_per_period: f64, periods_per_year: f64) -> f64 {
    let excess: Vec<f64> = returns.iter().filter(|r| r.is_finite()).map(|r| r - rf_per_period).collect();
    if excess.len() < 2 {
        return f64::NAN;
    }
    let dd = (excess.iter().map(|e| e.min(0.0).powi(2)).sum::<f64>() / excess.len() as f64).sqrt();
    if dd.is_nan() || dd <= 0.0 {
        return f64::NAN;
    }
    stats::mean(&excess) / dd * periods_per_year.sqrt()
}

/// Compound annual growth rate `(last/first)^(1/years) − 1`. `NaN` unless
/// `first > 0`, `last ≥ 0` and `years > 0`.
pub fn cagr(first: f64, last: f64, years: f64) -> f64 {
    let valid = first.is_finite() && last.is_finite() && years.is_finite() && first > 0.0 && last >= 0.0 && years > 0.0;
    if !valid {
        return f64::NAN;
    }
    (last / first).powf(1.0 / years) - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_and_log_returns() {
        let p = [100.0, 110.0, 99.0];
        let r = simple_returns(&p);
        assert_eq!(r.len(), 2);
        assert!((r[0] - 0.10).abs() < 1e-12);
        assert!((r[1] + 0.10).abs() < 1e-12);
        let l = log_returns(&p);
        assert!((l[0] - 1.1_f64.ln()).abs() < 1e-12);
        assert!((l[1] - 0.9_f64.ln()).abs() < 1e-12);
        assert!(simple_returns(&[1.0]).is_empty());
        assert!(log_returns(&[1.0, -1.0])[0].is_nan());
        assert!(simple_returns(&[0.0, 1.0])[0].is_nan());
    }

    #[test]
    fn cumulative_compounds() {
        let c = cumulative_returns(&[0.10, -0.10, f64::NAN, 0.05]);
        assert!((c[0] - 0.10).abs() < 1e-12);
        assert!((c[1] - (1.1 * 0.9 - 1.0)).abs() < 1e-12);
        assert!((c[2] - c[1]).abs() < 1e-15);
        assert!((c[3] - (1.1 * 0.9 * 1.05 - 1.0)).abs() < 1e-12);
        // Log returns sum equals ln of cumulative growth.
        let p = [10.0, 12.0, 9.0, 15.0];
        let cum = cumulative_returns(&simple_returns(&p));
        assert!((cum[2] - 0.5).abs() < 1e-12);
        let lsum: f64 = log_returns(&p).iter().sum();
        assert!((lsum - 1.5_f64.ln()).abs() < 1e-12);
    }

    #[test]
    fn volatility_annualizes() {
        // Returns [0.01, -0.01, 0.01, -0.01]: mean 0, sample var = 4e-4/3.
        let r = [0.01, -0.01, 0.01, -0.01];
        let want = (4e-4_f64 / 3.0).sqrt() * 252.0_f64.sqrt();
        assert!((annualized_volatility(&r, 252.0) - want).abs() < 1e-12);
    }

    #[test]
    fn drawdown_known_series() {
        let p = [100.0, 120.0, 90.0, 110.0, 80.0, 130.0, 125.0];
        let d = max_drawdown(&p);
        // Peak 120 (idx 1) → trough 80 (idx 4): 1/3; recovers at idx 5.
        assert!((d.max_drawdown - 1.0 / 3.0).abs() < 1e-12);
        assert_eq!((d.peak_index, d.trough_index, d.recovery_index), (1, 4, Some(5)));
        let s = drawdown_series(&p);
        assert!((s[4] + 1.0 / 3.0).abs() < 1e-12);
        assert_eq!(s[1], 0.0);
        assert_eq!(s[5], 0.0);
        assert!((s[6] - (125.0 / 130.0 - 1.0)).abs() < 1e-12);
        let up = max_drawdown(&[1.0, 2.0, 3.0]);
        assert_eq!(up.max_drawdown, 0.0);
        let none = max_drawdown(&[]);
        assert_eq!(none.max_drawdown, 0.0);
        let unrecovered = max_drawdown(&[10.0, 5.0, 6.0]);
        assert_eq!(unrecovered.recovery_index, None);
        assert!((unrecovered.max_drawdown - 0.5).abs() < 1e-12);
    }

    #[test]
    fn sharpe_and_sortino() {
        let r = [0.02, -0.01, 0.03, 0.00, -0.02];
        // mean 0.004; sample sd = sqrt(Σ(r-m)²/4)
        let m = 0.004;
        let ss: f64 = r.iter().map(|x| (x - m) * (x - m)).sum();
        let sd = (ss / 4.0).sqrt();
        assert!((sharpe(&r, 0.0, 252.0) - m / sd * 252.0_f64.sqrt()).abs() < 1e-12);
        // Downside: (0.01² + 0.02²)/5 = 1e-4 → DD = 0.01.
        assert!((sortino(&r, 0.0, 252.0) - m / 0.01 * 252.0_f64.sqrt()).abs() < 1e-12);
        assert!(sharpe(&[0.01, 0.01], 0.0, 252.0).is_nan());
        assert!(sortino(&[0.01, 0.02], 0.0, 252.0).is_nan());
    }

    #[test]
    fn cagr_values() {
        assert!((cagr(100.0, 121.0, 2.0) - 0.10).abs() < 1e-12);
        assert!((cagr(100.0, 100.0, 5.0)).abs() < 1e-15);
        assert!(cagr(0.0, 1.0, 1.0).is_nan());
        assert!(cagr(1.0, 2.0, 0.0).is_nan());
    }
}

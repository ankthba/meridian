//! Descriptive statistics, covariance/correlation, OLS regression and
//! percentiles.
//!
//! Non-finite values are ignored: single-series statistics drop them, and
//! two-series statistics use **pairwise deletion** — index `i` counts only
//! when both series are finite there. Series are assumed index-aligned (same
//! timestamps); if lengths differ, the common prefix is used. Undefined
//! results are `NaN`.

use serde::{Deserialize, Serialize};

use crate::error::{AnalyticsError, Result};
use crate::linalg;

fn finite(x: &[f64]) -> impl Iterator<Item = f64> + '_ {
    x.iter().copied().filter(|v| v.is_finite())
}

/// Pairs `(xᵢ, yᵢ)` where both are finite, over the common prefix.
fn finite_pairs<'a>(x: &'a [f64], y: &'a [f64]) -> impl Iterator<Item = (f64, f64)> + 'a {
    x.iter().zip(y).map(|(a, b)| (*a, *b)).filter(|(a, b)| a.is_finite() && b.is_finite())
}

/// Arithmetic mean of the finite values; `NaN` if there are none.
pub fn mean(x: &[f64]) -> f64 {
    let (sum, n) = finite(x).fold((0.0, 0usize), |(s, n), v| (s + v, n + 1));
    if n == 0 { f64::NAN } else { sum / n as f64 }
}

/// Sample variance (divisor `n − 1`, two-pass); `NaN` with fewer than 2 values.
pub fn variance(x: &[f64]) -> f64 {
    let m = mean(x);
    let (ss, n) = finite(x).fold((0.0, 0usize), |(s, n), v| (s + (v - m) * (v - m), n + 1));
    if n < 2 { f64::NAN } else { ss / (n - 1) as f64 }
}

/// Sample standard deviation (`√variance`).
pub fn std_dev(x: &[f64]) -> f64 {
    variance(x).sqrt()
}

/// Sample covariance (divisor `n − 1`) over pairwise-finite observations.
pub fn covariance(x: &[f64], y: &[f64]) -> f64 {
    let (sx, sy, n) = finite_pairs(x, y).fold((0.0, 0.0, 0usize), |(sx, sy, n), (a, b)| (sx + a, sy + b, n + 1));
    if n < 2 {
        return f64::NAN;
    }
    let (mx, my) = (sx / n as f64, sy / n as f64);
    let s: f64 = finite_pairs(x, y).map(|(a, b)| (a - mx) * (b - my)).sum();
    s / (n - 1) as f64
}

/// Pearson correlation over pairwise-finite observations, clamped to
/// `[-1, 1]`. `NaN` if either side has zero variance or fewer than 2 pairs.
pub fn correlation(x: &[f64], y: &[f64]) -> f64 {
    let (sx, sy, n) = finite_pairs(x, y).fold((0.0, 0.0, 0usize), |(sx, sy, n), (a, b)| (sx + a, sy + b, n + 1));
    if n < 2 {
        return f64::NAN;
    }
    let (mx, my) = (sx / n as f64, sy / n as f64);
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for (a, b) in finite_pairs(x, y) {
        let (da, db) = (a - mx, b - my);
        sxy += da * db;
        sxx += da * da;
        syy += db * db;
    }
    if !(sxx > 0.0 && syy > 0.0) {
        return f64::NAN;
    }
    (sxy / (sxx * syy).sqrt()).clamp(-1.0, 1.0)
}

/// Pairwise sample covariance matrix of `series` (each entry uses the
/// observations where both series are finite). `[i][j]` is `cov(series_i, series_j)`.
pub fn covariance_matrix(series: &[Vec<f64>]) -> Vec<Vec<f64>> {
    pairwise_matrix(series, covariance)
}

/// Pairwise Pearson correlation matrix of `series`; the diagonal is 1 (or
/// `NaN` for a series with zero variance).
pub fn correlation_matrix(series: &[Vec<f64>]) -> Vec<Vec<f64>> {
    pairwise_matrix(series, correlation)
}

fn pairwise_matrix(series: &[Vec<f64>], f: fn(&[f64], &[f64]) -> f64) -> Vec<Vec<f64>> {
    let k = series.len();
    let mut m = vec![vec![f64::NAN; k]; k];
    for i in 0..k {
        for j in i..k {
            let v = f(&series[i], &series[j]);
            m[i][j] = v;
            m[j][i] = v;
        }
    }
    m
}

/// Ordinary least squares fit with intercept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OlsResult {
    /// `[intercept, β₁, …, β_k]` in the order of the regressors.
    pub coefficients: Vec<f64>,
    /// Standard errors `σ̂·√((XᵀX)⁻¹)ᵢᵢ`, same order.
    pub std_errors: Vec<f64>,
    /// `coefficient / std_error`, same order.
    pub t_stats: Vec<f64>,
    /// `1 − SSR/SST` (centred). `NaN` if `y` is constant.
    pub r_squared: f64,
    /// `1 − (1 − R²)(n − 1)/(n − p)` with `p` = number of coefficients.
    pub adj_r_squared: f64,
    /// `σ̂ = √(SSR/(n − p))`, in units of `y`.
    pub residual_std: f64,
    /// Observations used (rows with any non-finite value are dropped).
    pub n: usize,
}

/// OLS regression of `y` on the regressors `xs` (each a column the length of
/// `y`) plus an intercept, solved by Householder QR (no normal equations).
///
/// Rows where `y` or any regressor is non-finite are dropped (listwise
/// deletion). Requires more observations than coefficients; perfectly
/// collinear regressors yield [`AnalyticsError::Singular`].
pub fn ols(y: &[f64], xs: &[Vec<f64>]) -> Result<OlsResult> {
    for x in xs {
        if x.len() != y.len() {
            return Err(AnalyticsError::LengthMismatch { expected: y.len(), got: x.len() });
        }
    }
    let p = xs.len() + 1;
    let rows: Vec<usize> = (0..y.len()).filter(|&i| y[i].is_finite() && xs.iter().all(|x| x[i].is_finite())).collect();
    let n = rows.len();
    if n <= p {
        return Err(AnalyticsError::InsufficientData { needed: p + 1, got: n });
    }
    let yv: Vec<f64> = rows.iter().map(|&i| y[i]).collect();
    let mut cols = Vec::with_capacity(p);
    cols.push(vec![1.0; n]);
    for x in xs {
        cols.push(rows.iter().map(|&i| x[i]).collect::<Vec<f64>>());
    }
    let ls = linalg::least_squares(&cols, &yv)?;
    let ssr: f64 = (0..n)
        .map(|r| {
            let fitted: f64 = cols.iter().zip(&ls.beta).map(|(c, b)| c[r] * b).sum();
            (yv[r] - fitted).powi(2)
        })
        .sum();
    let y_mean = yv.iter().sum::<f64>() / n as f64;
    let sst: f64 = yv.iter().map(|v| (v - y_mean).powi(2)).sum();
    let dof = (n - p) as f64;
    let sigma2 = ssr / dof;
    let std_errors: Vec<f64> = ls.xtx_inv_diag.iter().map(|d| (sigma2 * d).sqrt()).collect();
    let t_stats = ls.beta.iter().zip(&std_errors).map(|(b, s)| b / s).collect();
    let (r_squared, adj_r_squared) = if sst > 0.0 {
        let r2 = 1.0 - ssr / sst;
        (r2, 1.0 - (1.0 - r2) * (n - 1) as f64 / dof)
    } else {
        (f64::NAN, f64::NAN)
    };
    Ok(OlsResult {
        coefficients: ls.beta,
        std_errors,
        t_stats,
        r_squared,
        adj_r_squared,
        residual_std: sigma2.sqrt(),
        n,
    })
}

/// CAPM beta `cov(asset, market) / var(market)` over pairwise-finite
/// observations (returns, same periodicity). `NaN` if the market has zero
/// variance.
pub fn beta(asset_returns: &[f64], market_returns: &[f64]) -> f64 {
    let n = asset_returns.len().min(market_returns.len());
    let market: Vec<f64> = (0..n)
        .map(|i| if asset_returns[i].is_finite() { market_returns[i] } else { f64::NAN })
        .collect();
    let var = variance(&market);
    if var.is_nan() || var <= 0.0 {
        return f64::NAN;
    }
    covariance(asset_returns, &market) / var
}

/// Percentile with linear interpolation between order statistics (the
/// "linear"/type-7 definition used by NumPy and Excel `PERCENTILE.INC`).
/// `p ∈ [0, 1]`; `NaN` for `p` outside that range or no finite values.
pub fn percentile(x: &[f64], p: f64) -> f64 {
    let mut v: Vec<f64> = finite(x).collect();
    quantile_in_place(&mut v, p)
}

/// Type-7 quantile of finite values in `v` (reorders `v`; O(n) selection).
pub(crate) fn quantile_in_place(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() || !(0.0..=1.0).contains(&p) {
        return f64::NAN;
    }
    let h = (v.len() - 1) as f64 * p;
    let lo = h.floor() as usize;
    let frac = h - lo as f64;
    let (_, &mut lo_val, upper) = v.select_nth_unstable_by(lo, f64::total_cmp);
    if frac == 0.0 || upper.is_empty() {
        return lo_val;
    }
    let hi_val = upper.iter().copied().fold(f64::INFINITY, f64::min);
    lo_val + frac * (hi_val - lo_val)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moments() {
        let x = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        assert_eq!(mean(&x), 5.0);
        assert!((variance(&x) - 32.0 / 7.0).abs() < 1e-12);
        assert!((std_dev(&x) - (32.0_f64 / 7.0).sqrt()).abs() < 1e-12);
        assert!(mean(&[]).is_nan());
        assert!(variance(&[1.0]).is_nan());
        assert_eq!(mean(&[1.0, f64::NAN, 3.0]), 2.0);
    }

    #[test]
    fn covariance_and_correlation() {
        let x = [1.0, 2.0, 3.0, 4.0, 5.0];
        let y = [2.0, 4.0, 6.0, 8.0, 10.0];
        assert!((covariance(&x, &y) - 5.0).abs() < 1e-12);
        assert!((correlation(&x, &y) - 1.0).abs() < 1e-12);
        let z = [5.0, 4.0, 3.0, 2.0, 1.0];
        assert!((correlation(&x, &z) + 1.0).abs() < 1e-12);
        // Pairwise deletion: the NaN row is dropped from both.
        let xa = [1.0, 2.0, f64::NAN, 4.0];
        let ya = [1.0, 3.0, 100.0, 7.0];
        assert!((correlation(&xa, &ya) - 1.0).abs() < 1e-12);
        assert!(correlation(&[1.0, 1.0, 1.0], &x).is_nan());
    }

    #[test]
    fn matrices() {
        let s = vec![vec![1.0, 2.0, 3.0, 4.0], vec![2.0, 4.0, 6.0, 8.5], vec![4.0, 3.0, 2.0, 1.0]];
        let c = correlation_matrix(&s);
        let v = covariance_matrix(&s);
        for i in 0..3 {
            assert!((c[i][i] - 1.0).abs() < 1e-12);
            assert!((v[i][i] - variance(&s[i])).abs() < 1e-12);
            for j in 0..3 {
                assert_eq!(c[i][j], c[j][i]);
                assert!((c[i][j] - correlation(&s[i], &s[j])).abs() < 1e-15);
            }
        }
        assert!((c[0][2] + 1.0).abs() < 1e-12);
        assert!(correlation_matrix(&[]).is_empty());
    }

    /// Hand-solved: x = 1..5, y = [2,4,5,4,5] → ŷ = 2.2 + 0.6x, SSR = 2.4,
    /// SST = 6, σ̂² = 0.8, Sxx = 10, se(b) = √0.08, se(a) = √(0.8·(1/5 + 9/10)).
    #[test]
    fn ols_simple_regression_by_hand() {
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let y = [2.0, 4.0, 5.0, 4.0, 5.0];
        let r = ols(&y, &[x]).unwrap();
        assert_eq!(r.n, 5);
        assert!((r.coefficients[0] - 2.2).abs() < 1e-12);
        assert!((r.coefficients[1] - 0.6).abs() < 1e-12);
        assert!((r.r_squared - 0.6).abs() < 1e-12);
        assert!((r.adj_r_squared - (1.0 - 0.4 * 4.0 / 3.0)).abs() < 1e-12);
        assert!((r.residual_std - 0.8_f64.sqrt()).abs() < 1e-12);
        assert!((r.std_errors[1] - 0.08_f64.sqrt()).abs() < 1e-12);
        assert!((r.std_errors[0] - 0.88_f64.sqrt()).abs() < 1e-12);
        assert!((r.t_stats[1] - 0.6 / 0.08_f64.sqrt()).abs() < 1e-10);
    }

    /// Two regressors; reference solved exactly with rational arithmetic
    /// (Python `fractions`) on the normal equations.
    #[test]
    fn ols_two_regressors_exact_reference() {
        let x1 = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let x2 = vec![2.0, 1.0, 4.0, 3.0, 6.0, 5.0, 8.0, 9.0];
        let y = [3.0, 4.0, 8.0, 7.0, 12.0, 11.0, 15.0, 18.0];
        let r = ols(&y, &[x1, x2]).unwrap();
        // β = [26/51, 29/34, 58/51].
        let want_coef = [0.5098039215686274, 0.8529411764705882, 1.1372549019607843];
        let want_se = [0.475869062725945, 0.25994261928554613, 0.22612867832687836];
        for (g, w) in r.coefficients.iter().zip(want_coef) {
            assert!((g - w).abs() < 1e-11, "coef {g} vs {w}");
        }
        for (g, w) in r.std_errors.iter().zip(want_se) {
            assert!((g - w).abs() < 1e-11, "se {g} vs {w}");
        }
        assert!((r.r_squared - 0.9902728715507091).abs() < 1e-12);
    }

    #[test]
    fn ols_errors_and_missing_rows() {
        let x = vec![1.0, 2.0, 3.0];
        assert!(matches!(ols(&[1.0, 2.0], std::slice::from_ref(&x)), Err(AnalyticsError::LengthMismatch { .. })));
        assert!(matches!(ols(&[1.0, 2.0, 3.0], &[vec![1.0, 2.0, f64::NAN]]), Err(AnalyticsError::InsufficientData { .. })));
        let x4 = vec![1.0, 2.0, 3.0, 4.0];
        let dup = vec![2.0, 4.0, 6.0, 8.0];
        assert_eq!(ols(&[1.0, 2.0, 4.0, 3.0], &[x4, dup]), Err(AnalyticsError::Singular));
        // A NaN row is dropped.
        let r = ols(&[2.0, 4.0, f64::NAN, 8.0, 10.0], &[vec![1.0, 2.0, 3.0, 4.0, 5.0]]).unwrap();
        assert_eq!(r.n, 4);
        assert!((r.coefficients[1] - 2.0).abs() < 1e-12);
        // Intercept-only regression estimates the mean.
        let r0 = ols(&[1.0, 2.0, 6.0], &[]).unwrap();
        assert!((r0.coefficients[0] - 3.0).abs() < 1e-12);
    }

    #[test]
    fn beta_matches_ols_slope() {
        let m = [0.01, -0.02, 0.015, 0.003, -0.007, 0.02];
        let a = [0.02, -0.03, 0.02, 0.001, -0.01, 0.035];
        let r = ols(&a, &[m.to_vec()]).unwrap();
        assert!((beta(&a, &m) - r.coefficients[1]).abs() < 1e-12);
        assert!(beta(&a, &[0.0; 6]).is_nan());
    }

    #[test]
    fn percentile_linear() {
        let x = [15.0, 20.0, 35.0, 40.0, 50.0];
        assert_eq!(percentile(&x, 0.0), 15.0);
        assert_eq!(percentile(&x, 1.0), 50.0);
        assert_eq!(percentile(&x, 0.5), 35.0);
        // h = 4·0.4 = 1.6 → 20 + 0.6·15 = 29
        assert!((percentile(&x, 0.4) - 29.0).abs() < 1e-12);
        assert!((percentile(&[3.0, 1.0, 2.0, f64::NAN], 0.25) - 1.5).abs() < 1e-12);
        assert!(percentile(&x, 1.5).is_nan());
        assert!(percentile(&[], 0.5).is_nan());
    }
}

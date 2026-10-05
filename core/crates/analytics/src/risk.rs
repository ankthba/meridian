//! Value-at-Risk, expected shortfall and portfolio volatility.
//!
//! VaR and CVaR are reported as **positive numbers for losses**, as fractions
//! of portfolio value over the horizon of the input returns (a 1-day 95 % VaR
//! of `0.021` means a 2.1 % loss is exceeded on 5 % of days). A negative value
//! means even the tail quantile is a gain. `confidence` is e.g. `0.95` or
//! `0.99`; values outside `(0, 1)` give `NaN` (or an error for Monte Carlo).

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rand_distr::StandardNormal;

use crate::error::{AnalyticsError, Result};
use crate::linalg;
use crate::special::norm_inv_cdf;
use crate::stats;

fn valid_confidence(c: f64) -> bool {
    c > 0.0 && c < 1.0
}

/// Historical VaR: the negated `(1 − confidence)` percentile of `returns`
/// (linear interpolation, non-finite values ignored).
pub fn var_historical(returns: &[f64], confidence: f64) -> f64 {
    if !valid_confidence(confidence) {
        return f64::NAN;
    }
    -stats::percentile(returns, 1.0 - confidence)
}

/// Historical CVaR / expected shortfall: the negated mean of the returns at or
/// below the `(1 − confidence)` percentile.
pub fn cvar_historical(returns: &[f64], confidence: f64) -> f64 {
    if !valid_confidence(confidence) {
        return f64::NAN;
    }
    let q = stats::percentile(returns, 1.0 - confidence);
    if q.is_nan() {
        return f64::NAN;
    }
    let tail: Vec<f64> = returns.iter().copied().filter(|r| r.is_finite() && *r <= q).collect();
    -stats::mean(&tail)
}

/// Parametric (normal) VaR for returns with the given per-horizon `mean` and
/// standard deviation `std`: `−(mean + std·Φ⁻¹(1 − confidence))`.
pub fn var_parametric(mean: f64, std: f64, confidence: f64) -> f64 {
    if !valid_confidence(confidence) || std.is_nan() || std < 0.0 {
        return f64::NAN;
    }
    -(mean + std * norm_inv_cdf(1.0 - confidence))
}

/// Simulations per independently seeded chunk. Fixed so results do not depend
/// on the number of threads.
const MC_CHUNK: usize = 8192;

/// Monte Carlo VaR of a portfolio whose asset returns are multivariate normal
/// with per-horizon `means` and covariance `cov`, holding `weights`
/// (fractions of portfolio value, need not sum to 1).
///
/// Draws `sims` scenarios `r = μ + L·z` with `L` the Cholesky factor of `cov`
/// (semi-definite matrices are accepted) and `z` standard normal from a ChaCha8
/// stream seeded by `seed`, and returns the negated `(1 − confidence)`
/// quantile of the portfolio return `w·r`. Deterministic for a given seed,
/// with or without the `parallel` feature.
pub fn var_monte_carlo(
    means: &[f64],
    cov: &[Vec<f64>],
    weights: &[f64],
    confidence: f64,
    sims: usize,
    seed: u64,
) -> Result<f64> {
    let k = means.len();
    if weights.len() != k {
        return Err(AnalyticsError::LengthMismatch { expected: k, got: weights.len() });
    }
    if k == 0 {
        return Err(AnalyticsError::InsufficientData { needed: 1, got: 0 });
    }
    if !valid_confidence(confidence) {
        return Err(AnalyticsError::InvalidInput(format!("confidence must be in (0, 1), got {confidence}")));
    }
    if sims == 0 {
        return Err(AnalyticsError::InvalidInput("sims must be positive".into()));
    }
    if means.iter().chain(weights).any(|v| !v.is_finite()) {
        return Err(AnalyticsError::InvalidInput("means and weights must be finite".into()));
    }
    linalg::check_square(cov, k, "covariance matrix")?;
    let l = linalg::cholesky_psd(cov)?;
    // w·(μ + Lz) = w·μ + (Lᵀw)·z, so each scenario needs only a dot product.
    let mu_p: f64 = means.iter().zip(weights).map(|(m, w)| m * w).sum();
    let load: Vec<f64> = (0..k).map(|j| (j..k).map(|i| l[i][j] * weights[i]).sum()).collect();

    let n_chunks = sims.div_ceil(MC_CHUNK);
    let run_chunk = |c: usize| -> Vec<f64> {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        rng.set_stream(c as u64);
        let count = MC_CHUNK.min(sims - c * MC_CHUNK);
        (0..count)
            .map(|_| {
                let shock: f64 = load.iter().map(|a| a * rng.sample::<f64, _>(StandardNormal)).sum();
                mu_p + shock
            })
            .collect()
    };
    #[cfg(feature = "parallel")]
    let chunks: Vec<Vec<f64>> = {
        use rayon::prelude::*;
        (0..n_chunks).into_par_iter().map(run_chunk).collect()
    };
    #[cfg(not(feature = "parallel"))]
    let chunks: Vec<Vec<f64>> = (0..n_chunks).map(run_chunk).collect();

    let mut outcomes: Vec<f64> = chunks.into_iter().flatten().collect();
    Ok(-stats::quantile_in_place(&mut outcomes, 1.0 - confidence))
}

/// Portfolio volatility `√(wᵀΣw)` in the units/horizon of `cov`. `NaN` on a
/// dimension mismatch or if the quadratic form is negative (Σ not PSD).
pub fn portfolio_volatility(weights: &[f64], cov: &[Vec<f64>]) -> f64 {
    let k = weights.len();
    if cov.len() != k || cov.iter().any(|row| row.len() != k) {
        return f64::NAN;
    }
    let q: f64 = (0..k).map(|i| weights[i] * (0..k).map(|j| cov[i][j] * weights[j]).sum::<f64>()).sum();
    if q < 0.0 {
        // Allow for rounding on a PSD matrix with zero variance.
        return if q > -1e-15 { 0.0 } else { f64::NAN };
    }
    q.sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn historical_var_and_cvar() {
        // 1..=100 as percent losses: -0.01 .. -1.00 → returns r_i = -(i/100).
        let r: Vec<f64> = (1..=100).map(|i| -f64::from(i) / 100.0).collect();
        // 5th percentile: h = 99·0.05 = 4.95 on ascending [-1.00, -0.99, ...]
        // → -0.96 + 0.95·0.01 = -0.9505
        assert!((var_historical(&r, 0.95) - 0.9505).abs() < 1e-12);
        // Tail at/below -0.9505: -1.00..=-0.96 → mean -0.98.
        assert!((cvar_historical(&r, 0.95) - 0.98).abs() < 1e-12);
        assert!(cvar_historical(&r, 0.95) >= var_historical(&r, 0.95));
        assert!(var_historical(&r, 1.0).is_nan());
    }

    #[test]
    fn parametric_var() {
        // z_0.95 = 1.6448536269514715
        let v = var_parametric(0.0005, 0.02, 0.95);
        assert!((v - (0.02 * 1.6448536269514715 - 0.0005)).abs() < 1e-14);
        assert!(var_parametric(0.0, 0.02, 0.0).is_nan());
    }

    #[test]
    fn monte_carlo_converges_to_parametric_and_is_deterministic() {
        let means = [0.0005, 0.0002];
        let cov = vec![vec![0.0004, 0.00012], vec![0.00012, 0.0001]];
        let w = [0.6, 0.4];
        let sd = portfolio_volatility(&w, &cov);
        let mu = 0.6 * 0.0005 + 0.4 * 0.0002;
        let exact = var_parametric(mu, sd, 0.99);
        let mc = var_monte_carlo(&means, &cov, &w, 0.99, 200_000, 42).unwrap();
        assert!((mc - exact).abs() / exact < 0.02, "mc {mc} vs exact {exact}");
        let again = var_monte_carlo(&means, &cov, &w, 0.99, 200_000, 42).unwrap();
        assert_eq!(mc.to_bits(), again.to_bits());
        let other = var_monte_carlo(&means, &cov, &w, 0.99, 200_000, 43).unwrap();
        assert_ne!(mc.to_bits(), other.to_bits());
    }

    /// The chunked (possibly parallel) run equals a plain sequential replay of
    /// the same per-chunk streams, so thread count cannot change results.
    #[test]
    fn monte_carlo_matches_sequential_replay() {
        let means = [0.001, -0.0005, 0.0002];
        let cov = vec![vec![0.0004, 0.0001, 0.0], vec![0.0001, 0.0009, 0.0002], vec![0.0, 0.0002, 0.0001]];
        let w = [0.5, 0.3, 0.2];
        let sims = 3 * MC_CHUNK + 17;
        let got = var_monte_carlo(&means, &cov, &w, 0.975, sims, 7).unwrap();
        let l = linalg::cholesky_psd(&cov).unwrap();
        let mut all = Vec::with_capacity(sims);
        for c in 0..sims.div_ceil(MC_CHUNK) {
            let mut rng = ChaCha8Rng::seed_from_u64(7);
            rng.set_stream(c as u64);
            for _ in 0..MC_CHUNK.min(sims - c * MC_CHUNK) {
                let z: Vec<f64> = (0..3).map(|_| rng.sample::<f64, _>(StandardNormal)).collect();
                let r: f64 = (0..3).map(|i| w[i] * (means[i] + (0..=i).map(|j| l[i][j] * z[j]).sum::<f64>())).sum();
                all.push(r);
            }
        }
        let want = -stats::percentile(&all, 0.025);
        // Same draws; only the summation order of w·(μ + Lz) differs.
        assert!((got - want).abs() < 1e-15, "{got} vs {want}");
    }

    #[test]
    fn monte_carlo_validates() {
        let cov = vec![vec![1.0]];
        assert!(matches!(var_monte_carlo(&[0.0], &cov, &[1.0, 2.0], 0.95, 10, 1), Err(AnalyticsError::LengthMismatch { .. })));
        assert!(matches!(var_monte_carlo(&[0.0], &cov, &[1.0], 1.2, 10, 1), Err(AnalyticsError::InvalidInput(_))));
        assert!(matches!(var_monte_carlo(&[0.0], &cov, &[1.0], 0.95, 0, 1), Err(AnalyticsError::InvalidInput(_))));
        let bad = vec![vec![1.0, 2.0], vec![2.0, 1.0]];
        assert_eq!(var_monte_carlo(&[0.0, 0.0], &bad, &[0.5, 0.5], 0.95, 10, 1), Err(AnalyticsError::Singular));
        // Perfectly correlated assets (singular but PSD) are fine.
        let psd = vec![vec![1.0, 1.0], vec![1.0, 1.0]];
        assert!(var_monte_carlo(&[0.0, 0.0], &psd, &[0.5, 0.5], 0.95, 1000, 1).is_ok());
    }

    #[test]
    fn portfolio_vol() {
        let cov = vec![vec![0.04, 0.0], vec![0.0, 0.09]];
        // √(0.25·0.04 + 0.25·0.09) = √0.0325
        assert!((portfolio_volatility(&[0.5, 0.5], &cov) - 0.0325_f64.sqrt()).abs() < 1e-15);
        assert!(portfolio_volatility(&[1.0], &cov).is_nan());
    }
}

//! Special functions: error function and the standard normal distribution.
//!
//! Implemented from first principles (no external math crates):
//!
//! - Small arguments: the everywhere-positive series
//!   `erf(x) = 2/√π · x·e^{-x²} · Σ_{n≥0} (2x²)ⁿ / (1·3·…·(2n+1))`,
//!   which has no cancellation (used for `erf` when `|x| < 1` and for
//!   `erfc = 1 − erf` when `0 ≤ x < 0.5`).
//! - `x ≥ 0.5`: the even contraction of Laplace's continued fraction
//!   `erfc(x) = e^{-x²}/√π · 2x / (2x²+1 − 1·2/(2x²+5 − 3·4/(2x²+9 − …)))`,
//!   evaluated backward from a fixed depth.
//! - `e^{-x²}` is computed as `e^{-z²}·e^{(z−x)(z+x)}` with `z` = `x` truncated
//!   to 21 significant bits so `z²` is exact (the fdlibm trick).
//!
//! Accuracy (checked against CPython's `math.erfc` on a dense grid while
//! developing, and at fixed points in the tests): relative error ≲ 1e-15 for
//! `erfc(x)` over `x ∈ [0, 26]`, absolute error ≲ 2e-16 for `erf`. Hence
//! `norm_cdf` is accurate to ~1e-15 relative in the lower tail (plus a few ulps
//! × x² from scaling the argument by 1/√2) and ~1e-16 absolute everywhere.

use std::f64::consts::{FRAC_1_SQRT_2, PI};

/// `1/√π`.
const FRAC_1_SQRT_PI: f64 = 0.564_189_583_547_756_3;
/// `1/√(2π)`.
const FRAC_1_SQRT_2PI: f64 = 0.398_942_280_401_432_7;
/// Beyond this, `erfc(x)` underflows to zero in `f64`.
const ERFC_UNDERFLOW: f64 = 27.3;

/// `x` with the low 32 bits of its significand cleared (21 significant bits),
/// so that `z*z` is exact.
fn truncate_bits(x: f64) -> f64 {
    f64::from_bits(x.to_bits() & 0xffff_ffff_0000_0000)
}

/// `exp(-x²)` without the rounding error of forming `x²`.
fn exp_neg_sq(x: f64) -> f64 {
    let z = truncate_bits(x);
    (-z * z).exp() * ((z - x) * (z + x)).exp()
}

/// Series for `erf(x)`, valid and accurate for `0 ≤ x < 1`.
fn erf_series(x: f64) -> f64 {
    let two_x2 = 2.0 * x * x;
    let mut term = 1.0;
    let mut sum = 1.0;
    let mut n = 0.0_f64;
    // Terms shrink at least by 2/3 per step for x < 1; 40 steps is ample.
    for _ in 0..60 {
        n += 1.0;
        term *= two_x2 / (2.0 * n + 1.0);
        sum += term;
        if term <= sum * 1e-17 {
            break;
        }
    }
    2.0 * FRAC_1_SQRT_PI * x * exp_neg_sq(x) * sum
}

/// Continued fraction for `erfc(x)`, valid and accurate for `x ≥ 0.5`.
///
/// Evaluated bottom-up from a fixed depth (backward recurrence), which is
/// markedly more accurate than forward Lentz near `x ≈ 1`. The depth
/// `12 + 100/x²` exceeds the forward-convergence count with margin across the
/// range (412 terms at `x = 0.5`, 112 at `x = 1`, 37 at `x = 2`).
fn erfc_continued_fraction(x: f64) -> f64 {
    let x2 = x * x;
    // f = b0 + a1/(b1 + a2/(b2 + ...)), a_k = -(2k-1)(2k), b_k = 2x² + 4k + 1.
    let depth = (12.0 + 100.0 / x2) as u32;
    let mut tail = 0.0;
    for k in (1..=depth).rev() {
        let kf = f64::from(k);
        let a = -(2.0 * kf - 1.0) * (2.0 * kf);
        let b = 2.0 * x2 + 4.0 * kf + 1.0;
        tail = a / (b + tail);
    }
    let f = 2.0 * x2 + 1.0 + tail;
    exp_neg_sq(x) * FRAC_1_SQRT_PI * 2.0 * x / f
}

/// Complementary error function `erfc(x) = 1 − erf(x)`, relative accuracy
/// ~1e-15 for `x ≥ 0` (deep upper tail included).
pub fn erfc(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x < 0.0 {
        return 2.0 - erfc(-x);
    }
    if x < 0.5 {
        1.0 - erf_series(x)
    } else if x < ERFC_UNDERFLOW {
        erfc_continued_fraction(x)
    } else {
        0.0
    }
}

/// Error function `erf(x) = 2/√π ∫₀ˣ e^{-t²} dt`, absolute accuracy ~1e-16.
pub fn erf(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    let ax = x.abs();
    let v = if ax < 1.0 { erf_series(ax) } else { 1.0 - erfc(ax) };
    v.copysign(x)
}

/// Standard normal density `φ(x) = e^{-x²/2}/√(2π)`.
pub fn norm_pdf(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x.is_infinite() {
        return 0.0;
    }
    let z = truncate_bits(x);
    FRAC_1_SQRT_2PI * (-0.5 * z * z).exp() * (-0.5 * (x - z) * (x + z)).exp()
}

/// Standard normal cumulative distribution `Φ(x) = ½·erfc(−x/√2)`.
///
/// Accurate to ~1e-15 relative for `x ≤ 0` (including the far lower tail,
/// down to the `f64` underflow near `x ≈ −38`) and ~1e-16 absolute for `x > 0`.
pub fn norm_cdf(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    0.5 * erfc(-x * FRAC_1_SQRT_2)
}

// Acklam's rational approximation (relative error < 1.15e-9) used as the
// starting point for Halley refinement against `norm_cdf`.
const ACKLAM_A: [f64; 6] = [
    -3.969683028665376e+01,
    2.209460984245205e+02,
    -2.759285104469687e+02,
    1.38357751867269e+02,
    -3.066479806614716e+01,
    2.506628277459239e+00,
];
const ACKLAM_B: [f64; 5] = [
    -5.447609879822406e+01,
    1.615858368580409e+02,
    -1.556989798598866e+02,
    6.680131188771972e+01,
    -1.328068155288572e+01,
];
const ACKLAM_C: [f64; 6] = [
    -7.784894002430293e-03,
    -3.223964580411365e-01,
    -2.400758277161838e+00,
    -2.549732539343734e+00,
    4.374664141464968e+00,
    2.938163982698783e+00,
];
const ACKLAM_D: [f64; 4] = [7.784695709041462e-03, 3.224671290700398e-01, 2.445134137142996e+00, 3.754408661907416e+00];

/// Inverse normal CDF for `0 < p ≤ 0.5` (lower half, where `norm_cdf` is
/// relatively accurate so the Halley correction is well-conditioned).
fn norm_inv_lower(p: f64) -> f64 {
    const P_LOW: f64 = 0.02425;
    let (a, b, c, d) = (ACKLAM_A, ACKLAM_B, ACKLAM_C, ACKLAM_D);
    let mut x = if p < P_LOW {
        let q = (-2.0 * p.ln()).sqrt();
        (((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5])
            / ((((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1.0)
    } else {
        let q = p - 0.5;
        let r = q * q;
        (((((a[0] * r + a[1]) * r + a[2]) * r + a[3]) * r + a[4]) * r + a[5]) * q
            / (((((b[0] * r + b[1]) * r + b[2]) * r + b[3]) * r + b[4]) * r + 1.0)
    };
    // Two Halley steps take the 1e-9 seed to full double precision.
    for _ in 0..2 {
        let e = norm_cdf(x) - p;
        let u = e * (2.0 * PI).sqrt() * (0.5 * x * x).exp();
        if !u.is_finite() {
            break;
        }
        x -= u / (1.0 + 0.5 * x * u);
    }
    x
}

/// Inverse of the standard normal CDF (quantile function), accurate to
/// ~1e-15 relative. Returns `-∞`/`+∞` for `p = 0`/`p = 1` and `NaN` outside
/// `[0, 1]`.
pub fn norm_inv_cdf(p: f64) -> f64 {
    if p.is_nan() || !(0.0..=1.0).contains(&p) {
        return f64::NAN;
    }
    if p == 0.0 {
        return f64::NEG_INFINITY;
    }
    if p == 1.0 {
        return f64::INFINITY;
    }
    if p <= 0.5 {
        norm_inv_lower(p)
    } else {
        // 1 − p is exact for p in [0.5, 1] (Sterbenz).
        -norm_inv_lower(1.0 - p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel_err(a: f64, b: f64) -> f64 {
        if a == b { 0.0 } else { (a - b).abs() / b.abs() }
    }

    /// Reference values from CPython 3 `math.erfc`.
    const ERFC_REF: [(f64, f64); 19] = [
        (-3.0, 1.9999779095030015),
        (-1.5, 1.9661051464753108),
        (-0.5, 1.5204998778130465),
        (-0.1, 1.1124629160182848),
        (0.0, 1.0),
        (0.05, 0.9436280222029834),
        (0.3, 0.6713732405408727),
        (0.7, 0.3221988061625815),
        (0.99, 0.16149193044463026),
        (1.0, 0.15729920705028516),
        (1.01, 0.15318950377172333),
        (1.3, 0.06599205505934755),
        (2.0, 0.0046777349810472645),
        (3.5, 7.43098372341413e-07),
        (5.0, 1.537459794428035e-12),
        (8.0, 1.1224297172982926e-29),
        (12.0, 1.3562611692059042e-64),
        (20.0, 5.395865611607901e-176),
        (26.0, 5.663192408856143e-296),
    ];

    #[test]
    fn erfc_matches_reference() {
        for (x, want) in ERFC_REF {
            let got = erfc(x);
            assert!(rel_err(got, want) < 2e-15, "erfc({x}) = {got:e}, want {want:e}, rel {:e}", rel_err(got, want));
        }
    }

    #[test]
    fn erf_is_odd_and_complements_erfc() {
        for x in [0.0, 0.2, 0.9, 1.0, 1.7, 3.0, 6.0] {
            assert!((erf(x) + erf(-x)).abs() < 1e-16);
            assert!((erf(x) + erfc(x) - 1.0).abs() < 4e-16);
        }
        assert_eq!(erf(30.0), 1.0);
        assert_eq!(erfc(30.0), 0.0);
        assert_eq!(erfc(-30.0), 2.0);
        assert!(erf(f64::NAN).is_nan());
    }

    /// Reference values from CPython 3 `statistics.NormalDist().cdf`.
    #[test]
    fn norm_cdf_matches_reference() {
        let refs = [
            (-37.0, 5.725571222525138e-300),
            (-20.0, 2.7536241186063314e-89),
            (-10.0, 7.619853024160593e-24),
            (-8.0, 6.22096057427182e-16),
            (-5.0, 2.866515718791945e-07),
            (-3.0, 0.0013498980316300957),
            (-1.959963984540054, 0.02500000000000002),
            (-1.0, 0.15865525393145705),
            (-0.25, 0.4012936743170763),
            (0.0, 0.5),
            (0.25, 0.5987063256829237),
            (1.0, 0.8413447460685429),
            (1.959963984540054, 0.975),
            (3.0, 0.9986501019683699),
            (5.0, 0.9999997133484281),
            (8.0, 0.9999999999999993),
        ];
        for (x, want) in refs {
            let got = norm_cdf(x);
            // Relative 1e-14 in the tail (argument scaling by 1/√2 costs a few
            // ulps × x²), absolute 1e-16 near the centre.
            assert!(
                rel_err(got, want) < 1e-14 || (got - want).abs() < 2e-16,
                "Φ({x}) = {got:e}, want {want:e}"
            );
        }
        assert_eq!(norm_cdf(f64::INFINITY), 1.0);
        assert_eq!(norm_cdf(f64::NEG_INFINITY), 0.0);
    }

    #[test]
    fn norm_pdf_values() {
        assert!((norm_pdf(0.0) - FRAC_1_SQRT_2PI).abs() < 1e-17);
        // φ(1) = e^{-1/2}/√(2π)
        let want = (-0.5_f64).exp() / (2.0 * PI).sqrt();
        assert!(rel_err(norm_pdf(1.0), want) < 1e-15);
        assert!(rel_err(norm_pdf(-1.0), want) < 1e-15);
        assert_eq!(norm_pdf(f64::INFINITY), 0.0);
    }

    /// Reference values from CPython 3 `statistics.NormalDist().inv_cdf`
    /// (Wichura AS241).
    #[test]
    fn norm_inv_cdf_matches_reference() {
        let refs = [
            (0.95, 1.6448536269514715),
            (0.99, 2.3263478740408408),
            (0.975, 1.9599639845400534),
            (1e-10, -6.361340902404057),
            (0.5, 0.0),
        ];
        for (p, want) in refs {
            let got = norm_inv_cdf(p);
            assert!((got - want).abs() <= 4e-15 * want.abs().max(1.0), "Φ⁻¹({p}) = {got}, want {want}");
        }
        assert_eq!(norm_inv_cdf(0.0), f64::NEG_INFINITY);
        assert_eq!(norm_inv_cdf(1.0), f64::INFINITY);
        assert!(norm_inv_cdf(1.5).is_nan());
    }

    #[test]
    fn norm_inv_cdf_round_trips() {
        for i in 1..1000 {
            let p = f64::from(i) / 1000.0;
            let x = norm_inv_cdf(p);
            assert!((norm_cdf(x) - p).abs() < 1e-15, "p={p}");
        }
        for p in [1e-300, 1e-100, 1e-20, 1e-8] {
            let x = norm_inv_cdf(p);
            assert!(rel_err(norm_cdf(x), p) < 1e-13, "p={p}");
        }
    }
}

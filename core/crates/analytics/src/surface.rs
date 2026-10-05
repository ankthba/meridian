//! Implied-volatility surface on an expiry × log-moneyness grid.
//!
//! Quotes are stored as **total implied variance** `w = σ²·T` on a common
//! grid of log-moneyness `k = ln(K/S)` (spot-relative). Each expiry's smile is
//! linear in `w` between its quoted strikes and flat in volatility beyond
//! them; between expiries `w` is linear in `T` at fixed `k`. Together that is
//! bilinear interpolation of total variance over (expiry, log-moneyness),
//! which keeps interpolated variance non-negative and, for calendar-arbitrage-
//! free input, non-decreasing in expiry. Outside the quoted expiries the
//! volatility is held flat.
//!
//! Units: expiries in years, strikes in price units, vols annualized decimals.

use serde::{Deserialize, Serialize};

use crate::error::{AnalyticsError, Result};

/// One implied-volatility quote.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VolPoint {
    /// Time to expiry in years.
    pub expiry_years: f64,
    pub strike: f64,
    /// Annualized implied volatility (`0.25` = 25 %).
    pub iv: f64,
}

/// Interpolated implied-volatility surface.
///
/// Deserialization re-validates the grid, so a stored surface cannot break
/// the interpolation invariants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "SurfaceGrid")]
pub struct VolSurface {
    spot: f64,
    /// Ascending, strictly positive.
    expiries: Vec<f64>,
    /// Ascending union of every quoted `ln(K/S)`.
    log_moneyness: Vec<f64>,
    /// `total_variance[e][k]`, filled for every grid node.
    total_variance: Vec<Vec<f64>>,
}

/// Unvalidated wire form of [`VolSurface`].
#[derive(Deserialize)]
struct SurfaceGrid {
    spot: f64,
    expiries: Vec<f64>,
    log_moneyness: Vec<f64>,
    total_variance: Vec<Vec<f64>>,
}

impl TryFrom<SurfaceGrid> for VolSurface {
    type Error = AnalyticsError;

    fn try_from(g: SurfaceGrid) -> Result<Self> {
        let ascending = |v: &[f64]| v.windows(2).all(|w| w[0] < w[1]) && v.iter().all(|x| x.is_finite());
        let valid = g.spot.is_finite()
            && g.spot > 0.0
            && !g.expiries.is_empty()
            && !g.log_moneyness.is_empty()
            && ascending(&g.expiries)
            && g.expiries[0] > 0.0
            && ascending(&g.log_moneyness)
            && g.total_variance.len() == g.expiries.len()
            && g.total_variance.iter().all(|row| {
                row.len() == g.log_moneyness.len() && row.iter().all(|w| w.is_finite() && *w > 0.0)
            });
        if !valid {
            return Err(AnalyticsError::InvalidInput("malformed volatility surface grid".into()));
        }
        Ok(Self { spot: g.spot, expiries: g.expiries, log_moneyness: g.log_moneyness, total_variance: g.total_variance })
    }
}

/// Expiries closer than this (relative) are treated as the same expiry.
const EXPIRY_TOL: f64 = 1e-9;
/// Log-moneyness values closer than this are the same grid node.
const K_TOL: f64 = 1e-12;

/// Piecewise-linear interpolation of `ys` over ascending `xs`, flat beyond the ends.
fn interp_flat(xs: &[f64], ys: &[f64], x: f64) -> f64 {
    debug_assert!(!xs.is_empty() && xs.len() == ys.len());
    let last = xs.len() - 1;
    if x <= xs[0] {
        return ys[0];
    }
    if x >= xs[last] {
        return ys[last];
    }
    // xs[0] < x < xs[last], so 1 ≤ hi ≤ last.
    let hi = xs.partition_point(|&v| v <= x);
    let lo = hi - 1;
    let t = (x - xs[lo]) / (xs[hi] - xs[lo]);
    ys[lo] + t * (ys[hi] - ys[lo])
}

impl VolSurface {
    /// Builds a surface from quotes (any order). Quotes with a non-finite
    /// field, `expiry_years ≤ 0`, `strike ≤ 0` or `iv ≤ 0` are skipped;
    /// duplicate (expiry, strike) quotes are averaged.
    ///
    /// Errors: `spot` not positive/finite, or no usable quotes.
    pub fn from_points(points: impl AsRef<[VolPoint]>, spot: f64) -> Result<Self> {
        if !(spot.is_finite() && spot > 0.0) {
            return Err(AnalyticsError::InvalidInput(format!("spot must be positive, got {spot}")));
        }
        let mut pts: Vec<(f64, f64, f64)> = points
            .as_ref()
            .iter()
            .filter(|p| {
                p.expiry_years.is_finite()
                    && p.strike.is_finite()
                    && p.iv.is_finite()
                    && p.expiry_years > 0.0
                    && p.strike > 0.0
                    && p.iv > 0.0
            })
            .map(|p| (p.expiry_years, (p.strike / spot).ln(), p.iv))
            .collect();
        if pts.is_empty() {
            return Err(AnalyticsError::InsufficientData { needed: 1, got: 0 });
        }
        pts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));

        // Group into slices: (expiry, [(k, w)]), averaging duplicate strikes in vol.
        let mut slices: Vec<(f64, Vec<(f64, f64)>)> = Vec::new();
        let mut acc: Vec<(f64, f64, usize)> = Vec::new(); // (k, Σiv, count) for the open slice
        let mut slice_t = pts[0].0;
        let flush = |t: f64, acc: &mut Vec<(f64, f64, usize)>, slices: &mut Vec<(f64, Vec<(f64, f64)>)>| {
            let smile = acc
                .drain(..)
                .map(|(k, sum, n)| {
                    let iv = sum / n as f64;
                    (k, iv * iv * t)
                })
                .collect();
            slices.push((t, smile));
        };
        for &(t, k, iv) in &pts {
            if (t - slice_t).abs() > EXPIRY_TOL * slice_t.max(1.0) {
                flush(slice_t, &mut acc, &mut slices);
                slice_t = t;
            }
            match acc.last_mut() {
                Some(last) if (k - last.0).abs() <= K_TOL => {
                    last.1 += iv;
                    last.2 += 1;
                }
                _ => acc.push((k, iv, 1)),
            }
        }
        flush(slice_t, &mut acc, &mut slices);

        let mut grid: Vec<f64> = slices.iter().flat_map(|(_, s)| s.iter().map(|(k, _)| *k)).collect();
        grid.sort_by(f64::total_cmp);
        grid.dedup_by(|a, b| (*a - *b).abs() <= K_TOL);

        let expiries = slices.iter().map(|(t, _)| *t).collect();
        let total_variance = slices
            .iter()
            .map(|(_, smile)| {
                let ks: Vec<f64> = smile.iter().map(|(k, _)| *k).collect();
                let ws: Vec<f64> = smile.iter().map(|(_, w)| *w).collect();
                grid.iter().map(|&k| interp_flat(&ks, &ws, k)).collect()
            })
            .collect();
        Ok(Self { spot, expiries, log_moneyness: grid, total_variance })
    }

    /// Spot used for log-moneyness.
    pub fn spot(&self) -> f64 {
        self.spot
    }

    /// Quoted expiries (years, ascending).
    pub fn expiries(&self) -> &[f64] {
        &self.expiries
    }

    /// Grid of log-moneyness `ln(K/S)` (ascending).
    pub fn log_moneyness(&self) -> &[f64] {
        &self.log_moneyness
    }

    /// Total variance grid `[expiry][log-moneyness]`.
    pub fn total_variance(&self) -> &[Vec<f64>] {
        &self.total_variance
    }

    /// Implied-vol grid `[expiry][log-moneyness]` (e.g. for a heat map).
    pub fn iv_grid(&self) -> Vec<Vec<f64>> {
        self.expiries.iter().zip(&self.total_variance).map(|(t, row)| row.iter().map(|w| (w / t).sqrt()).collect()).collect()
    }

    /// Interpolated implied volatility at (`expiry_years`, `strike`).
    /// `None` for a non-positive/non-finite expiry or strike.
    pub fn iv_at(&self, expiry_years: f64, strike: f64) -> Option<f64> {
        if !(expiry_years.is_finite() && expiry_years > 0.0 && strike.is_finite() && strike > 0.0) {
            return None;
        }
        let k = (strike / self.spot).ln();
        let row_w = |e: usize| interp_flat(&self.log_moneyness, &self.total_variance[e], k);
        let last = self.expiries.len() - 1;
        let iv = if expiry_years <= self.expiries[0] {
            (row_w(0) / self.expiries[0]).sqrt()
        } else if expiry_years >= self.expiries[last] {
            (row_w(last) / self.expiries[last]).sqrt()
        } else {
            let hi = self.expiries.partition_point(|&t| t <= expiry_years);
            let lo = hi - 1;
            let (t0, t1) = (self.expiries[lo], self.expiries[hi]);
            let a = (expiry_years - t0) / (t1 - t0);
            let w = row_w(lo) + a * (row_w(hi) - row_w(lo));
            (w / expiry_years).sqrt()
        };
        (iv.is_finite() && iv > 0.0).then_some(iv)
    }

    /// Smile at `expiry_years`: `(strike, iv)` for every grid strike
    /// `K = S·e^k`, ascending in strike.
    pub fn smile(&self, expiry_years: f64) -> Vec<(f64, f64)> {
        self.log_moneyness
            .iter()
            .filter_map(|k| {
                let strike = self.spot * k.exp();
                self.iv_at(expiry_years, strike).map(|iv| (strike, iv))
            })
            .collect()
    }

    /// Term structure at a fixed strike: `(expiry_years, iv)` for each quoted
    /// expiry. Pass the spot (or the forward) as `atm` for the ATM term
    /// structure.
    pub fn term_structure(&self, atm: f64) -> Vec<(f64, f64)> {
        self.expiries.iter().filter_map(|&t| self.iv_at(t, atm).map(|iv| (t, iv))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(expiry_years: f64, strike: f64, iv: f64) -> VolPoint {
        VolPoint { expiry_years, strike, iv }
    }

    #[test]
    fn flat_surface_is_flat_everywhere() {
        let pts: Vec<VolPoint> =
            [0.1, 0.5, 1.0].iter().flat_map(|&t| [80.0, 100.0, 120.0].map(move |k| pt(t, k, 0.2))).collect();
        let s = VolSurface::from_points(&pts, 100.0).unwrap();
        for t in [0.01, 0.1, 0.3, 0.75, 1.0, 5.0] {
            for k in [10.0, 85.0, 100.0, 119.0, 500.0] {
                assert!((s.iv_at(t, k).unwrap() - 0.2).abs() < 1e-14);
            }
        }
        assert_eq!(s.expiries(), &[0.1, 0.5, 1.0]);
        assert_eq!(s.log_moneyness().len(), 3);
    }

    #[test]
    fn reproduces_quotes_and_interpolates_total_variance() {
        let pts = vec![pt(0.25, 100.0, 0.20), pt(1.0, 100.0, 0.30), pt(0.25, 110.0, 0.25), pt(1.0, 90.0, 0.35)];
        let s = VolSurface::from_points(pts.clone(), 100.0).unwrap();
        for p in &pts {
            assert!((s.iv_at(p.expiry_years, p.strike).unwrap() - p.iv).abs() < 1e-14);
        }
        // ATM at T = 0.5: w = 0.01 + (0.09 − 0.01)·(0.25/0.75); iv = √(w/0.5).
        let w: f64 = 0.01 + 0.08 / 3.0;
        assert!((s.iv_at(0.5, 100.0).unwrap() - (w / 0.5).sqrt()).abs() < 1e-14);
        // Within the 0.25 slice, linear in w between k=0 and k=ln(1.1).
        let k_mid = 0.5 * 1.1_f64.ln();
        let strike = 100.0 * k_mid.exp();
        let w_mid = 0.5 * (0.2_f64.powi(2) * 0.25 + 0.25_f64.powi(2) * 0.25);
        assert!((s.iv_at(0.25, strike).unwrap() - (w_mid / 0.25).sqrt()).abs() < 1e-14);
    }

    #[test]
    fn flat_extrapolation() {
        let pts = vec![pt(0.5, 90.0, 0.30), pt(0.5, 110.0, 0.20), pt(2.0, 100.0, 0.25)];
        let s = VolSurface::from_points(pts, 100.0).unwrap();
        // Beyond strikes: flat vol of the edge quote.
        assert!((s.iv_at(0.5, 50.0).unwrap() - 0.30).abs() < 1e-14);
        assert!((s.iv_at(0.5, 200.0).unwrap() - 0.20).abs() < 1e-14);
        // Before the first / after the last expiry: flat vol.
        assert!((s.iv_at(0.1, 50.0).unwrap() - 0.30).abs() < 1e-14);
        assert!((s.iv_at(10.0, 100.0).unwrap() - 0.25).abs() < 1e-14);
        assert_eq!(s.iv_at(0.0, 100.0), None);
        assert_eq!(s.iv_at(1.0, -5.0), None);
    }

    #[test]
    fn smile_and_term_structure() {
        let pts = vec![pt(0.25, 90.0, 0.3), pt(0.25, 100.0, 0.25), pt(0.25, 110.0, 0.22), pt(1.0, 100.0, 0.28)];
        let s = VolSurface::from_points(pts, 100.0).unwrap();
        let smile = s.smile(0.25);
        assert_eq!(smile.len(), 3);
        assert!((smile[0].0 - 90.0).abs() < 1e-9 && (smile[0].1 - 0.3).abs() < 1e-14);
        assert!(smile.windows(2).all(|w| w[0].0 < w[1].0));
        let ts = s.term_structure(100.0);
        assert_eq!(ts.len(), 2);
        assert!((ts[0].1 - 0.25).abs() < 1e-14 && (ts[1].1 - 0.28).abs() < 1e-14);
        let grid = s.iv_grid();
        assert_eq!(grid.len(), 2);
        assert!((grid[0][1] - 0.25).abs() < 1e-14);
    }

    #[test]
    fn serde_round_trip_and_validation() {
        let s = VolSurface::from_points(vec![pt(0.5, 90.0, 0.3), pt(1.0, 110.0, 0.2)], 100.0).unwrap();
        let json = serde_json::to_string(&s).unwrap();
        let back: VolSurface = serde_json::from_str(&json).unwrap();
        // serde_json's default float parser may differ from the original in the last ulp.
        assert_eq!(back.expiries(), s.expiries());
        for (t, k) in [(0.5, 90.0), (0.75, 100.0), (1.0, 110.0)] {
            assert!((back.iv_at(t, k).unwrap() - s.iv_at(t, k).unwrap()).abs() < 1e-14);
        }
        let broken = r#"{"spot":100.0,"expiries":[],"log_moneyness":[0.0],"total_variance":[]}"#;
        assert!(serde_json::from_str::<VolSurface>(broken).is_err());
        let ragged = r#"{"spot":100.0,"expiries":[1.0],"log_moneyness":[0.0,0.1],"total_variance":[[0.04]]}"#;
        assert!(serde_json::from_str::<VolSurface>(ragged).is_err());
    }

    #[test]
    fn duplicates_averaged_and_invalid_rejected() {
        let pts = vec![pt(1.0, 100.0, 0.2), pt(1.0, 100.0, 0.3), pt(f64::NAN, 100.0, 0.2), pt(1.0, 100.0, -0.1)];
        let s = VolSurface::from_points(pts, 100.0).unwrap();
        assert!((s.iv_at(1.0, 100.0).unwrap() - 0.25).abs() < 1e-14);
        assert!(matches!(VolSurface::from_points(Vec::<VolPoint>::new(), 100.0), Err(AnalyticsError::InsufficientData { .. })));
        assert!(matches!(VolSurface::from_points(vec![pt(1.0, 100.0, 0.2)], 0.0), Err(AnalyticsError::InvalidInput(_))));
    }
}

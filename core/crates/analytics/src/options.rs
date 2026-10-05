//! Option pricing: Black-Scholes-Merton, Cox-Ross-Rubinstein binomial trees,
//! Greeks and implied volatility.
//!
//! # Units and conventions
//!
//! - `spot`, `strike` and prices are in the underlying's quote currency, per
//!   unit of underlying (the contract multiplier is applied by the caller).
//! - `rate` and `dividend_yield` are continuously compounded annual rates
//!   (`0.05` = 5 %); `vol` is annualized (`0.20` = 20 %); `time_years` is the
//!   year fraction to expiry.
//! - **Terminal Greek conventions** ([`GreekValues`]):
//!   - `delta`: price change per 1.00 move in spot;
//!   - `gamma`: delta change per 1.00 move in spot;
//!   - `theta`: price change per **calendar day** (annual theta / 365),
//!     normally negative for long options;
//!   - `vega`: price change per **1 vol point** (σ + 0.01);
//!   - `rho`: price change per **1 % rate move** (r + 0.01).
//! - Edge cases: `time_years ≤ 0` prices at intrinsic value; `vol ≤ 0` (or a
//!   non-positive spot/strike) prices at the discounted intrinsic value of the
//!   forward, `max(±(S·e^{-qT} − K·e^{-rT}), 0)`. Non-finite inputs give `NaN`.

use std::ops::{Add, AddAssign};

pub use meridian_types::{ExerciseStyle, OptionRight};
use serde::{Deserialize, Serialize};

pub use crate::special::{norm_cdf, norm_inv_cdf, norm_pdf};

/// Calendar days per year used for theta.
pub const DAYS_PER_YEAR: f64 = 365.0;
/// One volatility point (vega scaling).
const VOL_POINT: f64 = 0.01;
/// One percentage point of rate (rho scaling).
const RATE_POINT: f64 = 0.01;

/// Inputs for a single-option valuation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BsInputs {
    /// Underlying price.
    pub spot: f64,
    /// Strike price.
    pub strike: f64,
    /// Risk-free rate, continuously compounded, annual.
    pub rate: f64,
    /// Dividend yield (or foreign rate / cost of carry offset), continuous, annual.
    pub dividend_yield: f64,
    /// Annualized volatility.
    pub vol: f64,
    /// Time to expiry in years.
    pub time_years: f64,
    pub right: OptionRight,
}

impl BsInputs {
    /// Copy with a different volatility.
    #[must_use]
    pub fn with_vol(&self, vol: f64) -> Self {
        Self { vol, ..*self }
    }

    fn all_finite(&self) -> bool {
        [self.spot, self.strike, self.rate, self.dividend_yield, self.vol, self.time_years].iter().all(|v| v.is_finite())
    }

    /// True when the full stochastic model applies (positive time, vol, spot, strike).
    fn is_regular(&self) -> bool {
        self.time_years > 0.0 && self.vol > 0.0 && self.spot > 0.0 && self.strike > 0.0
    }
}

/// Option sensitivities in terminal units (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct GreekValues {
    /// Per 1.00 move in spot.
    pub delta: f64,
    /// Delta change per 1.00 move in spot.
    pub gamma: f64,
    /// Per calendar day.
    pub theta: f64,
    /// Per 1 vol point.
    pub vega: f64,
    /// Per 1 % rate move.
    pub rho: f64,
}

impl GreekValues {
    pub const ZERO: Self = Self { delta: 0.0, gamma: 0.0, theta: 0.0, vega: 0.0, rho: 0.0 };
    pub const NAN: Self = Self { delta: f64::NAN, gamma: f64::NAN, theta: f64::NAN, vega: f64::NAN, rho: f64::NAN };

    /// Every Greek multiplied by `k` (e.g. a position quantity).
    #[must_use]
    pub fn scaled(self, k: f64) -> Self {
        Self { delta: self.delta * k, gamma: self.gamma * k, theta: self.theta * k, vega: self.vega * k, rho: self.rho * k }
    }
}

impl Add for GreekValues {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self {
            delta: self.delta + o.delta,
            gamma: self.gamma + o.gamma,
            theta: self.theta + o.theta,
            vega: self.vega + o.vega,
            rho: self.rho + o.rho,
        }
    }
}

impl AddAssign for GreekValues {
    fn add_assign(&mut self, o: Self) {
        *self = *self + o;
    }
}

/// Model selector for [`price`], [`greeks`] and [`implied_volatility`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PricingModel {
    /// Closed-form Black-Scholes-Merton. European by construction: the
    /// exercise style is ignored (exact for American calls with
    /// `dividend_yield ≤ 0` and `rate ≥ 0`, a lower bound otherwise).
    BlackScholes,
    /// Cox-Ross-Rubinstein binomial tree with `steps` time steps.
    Binomial { steps: usize },
}

/// Payoff of exercising now at spot `s`.
pub fn intrinsic(right: OptionRight, spot: f64, strike: f64) -> f64 {
    match right {
        OptionRight::Call => (spot - strike).max(0.0),
        OptionRight::Put => (strike - spot).max(0.0),
    }
}

/// Discounted intrinsic value of the forward (the zero-volatility price).
fn forward_intrinsic(i: &BsInputs) -> f64 {
    let fwd_s = i.spot * (-i.dividend_yield * i.time_years).exp();
    let pv_k = i.strike * (-i.rate * i.time_years).exp();
    match i.right {
        OptionRight::Call => (fwd_s - pv_k).max(0.0),
        OptionRight::Put => (pv_k - fwd_s).max(0.0),
    }
}

/// `(d1, d2)` for regular inputs.
fn d1_d2(i: &BsInputs) -> (f64, f64) {
    let sd = i.vol * i.time_years.sqrt();
    let d1 = ((i.spot / i.strike).ln() + (i.rate - i.dividend_yield) * i.time_years) / sd + 0.5 * sd;
    (d1, d1 - sd)
}

/// Black-Scholes-Merton price of a European option (per unit of underlying).
pub fn black_scholes_price(i: &BsInputs) -> f64 {
    if !i.all_finite() {
        return f64::NAN;
    }
    if i.time_years <= 0.0 {
        return intrinsic(i.right, i.spot, i.strike);
    }
    if !i.is_regular() {
        return forward_intrinsic(i);
    }
    let (d1, d2) = d1_d2(i);
    let fwd_s = i.spot * (-i.dividend_yield * i.time_years).exp();
    let pv_k = i.strike * (-i.rate * i.time_years).exp();
    let v = match i.right {
        OptionRight::Call => fwd_s * norm_cdf(d1) - pv_k * norm_cdf(d2),
        OptionRight::Put => pv_k * norm_cdf(-d2) - fwd_s * norm_cdf(-d1),
    };
    v.max(0.0)
}

/// Black-Scholes-Merton Greeks of a European option in terminal units
/// (theta per calendar day, vega per vol point, rho per 1 % rate).
pub fn black_scholes_greeks(i: &BsInputs) -> GreekValues {
    if !i.all_finite() {
        return GreekValues::NAN;
    }
    let sign = match i.right {
        OptionRight::Call => 1.0,
        OptionRight::Put => -1.0,
    };
    if i.time_years <= 0.0 {
        // At expiry: delta is the exercise indicator (½ exactly at the money).
        let m = (i.spot - i.strike) * sign;
        let delta = if m > 0.0 {
            sign
        } else if m < 0.0 {
            0.0
        } else {
            0.5 * sign
        };
        return GreekValues { delta, ..GreekValues::ZERO };
    }
    let t = i.time_years;
    let dq = (-i.dividend_yield * t).exp();
    let dr = (-i.rate * t).exp();
    if !i.is_regular() {
        // Deterministic forward: V = max(sign·(S·dq − K·dr), 0).
        let fwd_s = i.spot * dq;
        let pv_k = i.strike * dr;
        if sign * (fwd_s - pv_k) <= 0.0 {
            return GreekValues::ZERO;
        }
        // dV/dT = sign·(−q·S·dq + r·K·dr); theta = −dV/dT.
        let theta_annual = sign * (i.dividend_yield * fwd_s - i.rate * pv_k);
        return GreekValues {
            delta: sign * dq,
            gamma: 0.0,
            theta: theta_annual / DAYS_PER_YEAR,
            vega: 0.0,
            rho: sign * t * pv_k * RATE_POINT,
        };
    }
    let (d1, d2) = d1_d2(i);
    let sqrt_t = t.sqrt();
    let pdf = norm_pdf(d1);
    let gamma = dq * pdf / (i.spot * i.vol * sqrt_t);
    let vega = i.spot * dq * pdf * sqrt_t;
    let decay = -i.spot * dq * pdf * i.vol / (2.0 * sqrt_t);
    let (delta, theta, rho) = match i.right {
        OptionRight::Call => {
            let (n1, n2) = (norm_cdf(d1), norm_cdf(d2));
            (
                dq * n1,
                decay - i.rate * i.strike * dr * n2 + i.dividend_yield * i.spot * dq * n1,
                i.strike * t * dr * n2,
            )
        }
        OptionRight::Put => {
            let (n1, n2) = (norm_cdf(-d1), norm_cdf(-d2));
            (
                -dq * n1,
                decay + i.rate * i.strike * dr * n2 - i.dividend_yield * i.spot * dq * n1,
                -i.strike * t * dr * n2,
            )
        }
    };
    GreekValues { delta, gamma, theta: theta / DAYS_PER_YEAR, vega: vega * VOL_POINT, rho: rho * RATE_POINT }
}

/// Price, delta, gamma and annual theta read off one binomial tree.
struct TreeOut {
    price: f64,
    delta: f64,
    gamma: f64,
    theta_annual: f64,
}

/// CRR tree for regular inputs, built on Pelsser-Vorst's extended grid: the
/// root sits two steps *before* valuation time, so the three nodes at step 2
/// are `(S·d/u, S, S·u/d)` at t = 0 and delta, gamma and theta come from
/// nodes at the valuation date rather than one or two steps later.
///
/// Uses `u = e^{σ√Δt}, d = 1/u`; if that makes the risk-neutral probability
/// leave (0, 1) (very low vol with few steps), it switches to the drifted CRR
/// lattice `u,d = e^{(r−q)Δt ± σ√Δt}`, which is always arbitrage-free.
fn crr_tree(i: &BsInputs, steps: usize, style: ExerciseStyle) -> TreeOut {
    let n = steps.max(1);
    let dt = i.time_years / n as f64;
    let sdt = i.vol * dt.sqrt();
    let carry = (i.rate - i.dividend_yield) * dt;
    let prob = |lu: f64, ld: f64| (carry.exp_m1() - ld.exp_m1()) / (lu.exp_m1() - ld.exp_m1());
    let (mut lu, mut ld) = (sdt, -sdt);
    let mut p = prob(lu, ld);
    if !(p > 0.0 && p < 1.0) {
        lu = carry + sdt;
        ld = carry - sdt;
        p = prob(lu, ld);
    }
    let disc = (-i.rate * dt).exp();
    let (pu, pd) = (disc * p, disc * (1.0 - p));
    let m = n + 2;
    let pow_u: Vec<f64> = (0..=m).map(|j| (j as f64 * lu).exp()).collect();
    let pow_d: Vec<f64> = (0..=m).map(|j| (j as f64 * ld).exp()).collect();
    // Root spot S/(u·d) puts node (2, 1) exactly at S.
    let base = i.spot * (-(lu + ld)).exp();
    let spot_at = |step: usize, ups: usize| base * pow_u[ups] * pow_d[step - ups];
    let (right, strike) = (i.right, i.strike);
    let american = style == ExerciseStyle::American;

    let mut v: Vec<f64> = (0..=m).map(|j| intrinsic(right, spot_at(m, j), strike)).collect();
    let mut v4 = if m == 4 { Some([v[0], v[1], v[2], v[3], v[4]]) } else { None };
    for step in (2..m).rev() {
        for j in 0..=step {
            let cont = pu * v[j + 1] + pd * v[j];
            v[j] = if american { cont.max(intrinsic(right, spot_at(step, j), strike)) } else { cont };
        }
        if step == 4 {
            v4 = Some([v[0], v[1], v[2], v[3], v[4]]);
        }
    }
    let s = i.spot;
    let (s_dn, s_up) = (spot_at(2, 0), spot_at(2, 2));
    let (f_dn, f_mid, f_up) = (v[0], v[1], v[2]);
    let delta = (f_up - f_dn) / (s_up - s_dn);
    let gamma = ((f_up - f_mid) / (s_up - s) - (f_mid - f_dn) / (s - s_dn)) / (0.5 * (s_up - s_dn));
    // Theta from the middle node two steps later (time 2Δt). Its spot is
    // S·u·d — exactly S for CRR; on the drifted lattice shift it back to S
    // with that node's local delta.
    let theta_annual = v4.map_or(f64::NAN, |w| {
        let s_mid4 = spot_at(4, 2);
        let delta4 = (w[3] - w[1]) / (spot_at(4, 3) - spot_at(4, 1));
        let f_at_s = w[2] + delta4 * (s - s_mid4);
        (f_at_s - f_mid) / (2.0 * dt)
    });
    TreeOut { price: f_mid, delta, gamma, theta_annual }
}

/// Price with zero volatility (or degenerate spot/strike): the underlying
/// grows deterministically at `r − q`; an American holder exercises at the
/// best of the `steps + 1` grid dates.
fn deterministic_price(i: &BsInputs, steps: usize, style: ExerciseStyle) -> f64 {
    match style {
        ExerciseStyle::European => forward_intrinsic(i),
        ExerciseStyle::American => {
            let n = steps.max(1);
            (0..=n)
                .map(|k| {
                    let t = i.time_years * k as f64 / n as f64;
                    let s_t = i.spot * ((i.rate - i.dividend_yield) * t).exp();
                    (-i.rate * t).exp() * intrinsic(i.right, s_t, i.strike)
                })
                .fold(0.0, f64::max)
        }
    }
}

/// Cox-Ross-Rubinstein binomial price with continuous dividend yield.
///
/// `steps` is clamped to at least 1. The discretization error shrinks like
/// `1/steps` with an odd/even oscillation; at 500 steps European prices are
/// typically within a cent of Black-Scholes. Cost is O(steps²).
pub fn binomial_price(i: &BsInputs, steps: usize, style: ExerciseStyle) -> f64 {
    if !i.all_finite() {
        return f64::NAN;
    }
    if i.time_years <= 0.0 {
        return intrinsic(i.right, i.spot, i.strike);
    }
    if !i.is_regular() {
        return deterministic_price(i, steps, style);
    }
    crr_tree(i, steps, style).price
}

/// Central-difference Greeks of an arbitrary pricer (terminal units).
fn finite_difference_greeks(i: &BsInputs, pricer: impl Fn(&BsInputs) -> f64) -> GreekValues {
    let p0 = pricer(i);
    let h = 1e-4 * i.spot.abs().max(1e-4);
    let up = pricer(&BsInputs { spot: i.spot + h, ..*i });
    let dn = pricer(&BsInputs { spot: i.spot - h, ..*i });
    let day = 1.0 / DAYS_PER_YEAR;
    let theta = pricer(&BsInputs { time_years: (i.time_years - day).max(0.0), ..*i }) - p0;
    let hv = VOL_POINT;
    let vega = if i.vol > hv {
        (pricer(&i.with_vol(i.vol + hv)) - pricer(&i.with_vol(i.vol - hv))) / 2.0
    } else {
        pricer(&i.with_vol(i.vol.max(0.0) + hv)) - pricer(&i.with_vol(i.vol.max(0.0)))
    };
    let hr = 1e-3;
    let rho = (pricer(&BsInputs { rate: i.rate + hr, ..*i }) - pricer(&BsInputs { rate: i.rate - hr, ..*i })) / (2.0 * hr)
        * RATE_POINT;
    GreekValues { delta: (up - dn) / (2.0 * h), gamma: (up - 2.0 * p0 + dn) / (h * h), theta, vega, rho }
}

/// Binomial Greeks in terminal units.
///
/// Delta, gamma and theta come from the extended tree's valuation-date nodes
/// (see [`binomial_price`]); vega and rho are central bump-and-reprice
/// differences (±1 vol point, ±10 bp) on a tree with the same `steps`
/// (clamped to at least 2).
pub fn binomial_greeks(i: &BsInputs, steps: usize, style: ExerciseStyle) -> GreekValues {
    if !i.all_finite() {
        return GreekValues::NAN;
    }
    if i.time_years <= 0.0 || (!i.is_regular() && style == ExerciseStyle::European) {
        return black_scholes_greeks(i);
    }
    let n = steps.max(2);
    if !i.is_regular() {
        return finite_difference_greeks(i, |x| binomial_price(x, n, style));
    }
    let tree = crr_tree(i, n, style);
    let price = |x: &BsInputs| binomial_price(x, n, style);
    let hv = VOL_POINT;
    let vega = if i.vol > hv {
        (price(&i.with_vol(i.vol + hv)) - price(&i.with_vol(i.vol - hv))) / 2.0
    } else {
        price(&i.with_vol(i.vol + hv)) - tree.price
    };
    let hr = 1e-3;
    let rho = (price(&BsInputs { rate: i.rate + hr, ..*i }) - price(&BsInputs { rate: i.rate - hr, ..*i })) / (2.0 * hr)
        * RATE_POINT;
    GreekValues { delta: tree.delta, gamma: tree.gamma, theta: tree.theta_annual / DAYS_PER_YEAR, vega, rho }
}

/// Price under the chosen model. See [`PricingModel::BlackScholes`] for how
/// the exercise style is treated by the closed form.
pub fn price(i: &BsInputs, style: ExerciseStyle, model: PricingModel) -> f64 {
    match model {
        PricingModel::BlackScholes => black_scholes_price(i),
        PricingModel::Binomial { steps } => binomial_price(i, steps, style),
    }
}

/// Greeks under the chosen model (terminal units).
pub fn greeks(i: &BsInputs, style: ExerciseStyle, model: PricingModel) -> GreekValues {
    match model {
        PricingModel::BlackScholes => black_scholes_greeks(i),
        PricingModel::Binomial { steps } => binomial_greeks(i, steps, style),
    }
}

/// Lowest volatility searched by [`implied_volatility`].
pub const IV_MIN: f64 = 1e-4;
/// Highest volatility searched by [`implied_volatility`] (500 %).
pub const IV_MAX: f64 = 5.0;

/// Implied volatility that reproduces `target_price` under `model`
/// (`inputs.vol` is ignored).
///
/// Safeguarded Newton-Raphson on vega (analytic for Black-Scholes, a central
/// difference for the tree), keeping a bracket and falling back to Brent's
/// method on `[IV_MIN, IV_MAX]` whenever a Newton step leaves it.
///
/// Returns `None` when the price violates the no-arbitrage bounds (below the
/// discounted forward intrinsic value — or the immediate-exercise value for an
/// American tree — or at/above the underlying's (or strike's) bound), when
/// the solution lies outside `[IV_MIN, IV_MAX]`, when `time_years ≤ 0`, or
/// when spot/strike are not positive.
pub fn implied_volatility(target_price: f64, inputs: &BsInputs, style: ExerciseStyle, model: PricingModel) -> Option<f64> {
    let base = inputs.with_vol(0.2);
    if !target_price.is_finite() || !base.all_finite() || !base.is_regular() {
        return None;
    }
    let american = style == ExerciseStyle::American && matches!(model, PricingModel::Binomial { .. });
    let (s, k, t) = (base.spot, base.strike, base.time_years);
    let fwd_s = s * (-base.dividend_yield * t).exp();
    let pv_k = k * (-base.rate * t).exp();
    let (mut lower, upper) = match base.right {
        OptionRight::Call => ((fwd_s - pv_k).max(0.0), if american { s } else { fwd_s }),
        OptionRight::Put => ((pv_k - fwd_s).max(0.0), if american { k } else { pv_k }),
    };
    if american {
        lower = lower.max(intrinsic(base.right, s, k));
    }
    let scale = s.max(k);
    if target_price < lower - 1e-12 * scale || target_price >= upper {
        return None;
    }
    let price_tol = 1e-11 * scale;
    let f = |v: f64| price(&base.with_vol(v), style, model) - target_price;
    let f_lo = f(IV_MIN);
    if f_lo >= 0.0 {
        return (f_lo <= price_tol).then_some(IV_MIN);
    }
    let f_hi = f(IV_MAX);
    if f_hi <= 0.0 {
        return (-f_hi <= price_tol).then_some(IV_MAX);
    }
    let vega = |v: f64| -> f64 {
        match model {
            PricingModel::BlackScholes => {
                let x = base.with_vol(v);
                let (d1, _) = d1_d2(&x);
                fwd_s * norm_pdf(d1) * t.sqrt()
            }
            PricingModel::Binomial { .. } => {
                let h = 1e-4_f64.min(v * 0.5);
                (f(v + h) - f(v - h)) / (2.0 * h)
            }
        }
    };
    // Manaster-Koehler start (≥ the IV for European options, so Newton
    // converges monotonically); Brenner-Subrahmanyam near the money.
    let moneyness = (fwd_s / pv_k).ln().abs();
    let mut v = if moneyness > 1e-3 {
        (2.0 * moneyness / t).sqrt()
    } else {
        (2.0 * std::f64::consts::PI / t).sqrt() * target_price / fwd_s
    }
    .clamp(0.01, 3.0);
    let (mut lo, mut hi, mut flo, mut fhi) = (IV_MIN, IV_MAX, f_lo, f_hi);
    for _ in 0..50 {
        let fv = f(v);
        if !fv.is_finite() {
            break;
        }
        if fv.abs() <= price_tol {
            return Some(v);
        }
        if fv < 0.0 {
            (lo, flo) = (v, fv);
        } else {
            (hi, fhi) = (v, fv);
        }
        let dv = vega(v);
        if dv.is_nan() || dv <= 0.0 {
            break;
        }
        let next = v - fv / dv;
        if !(next > lo && next < hi) {
            break;
        }
        if (next - v).abs() <= 1e-14 * v {
            return Some(next);
        }
        v = next;
    }
    brent(f, lo, hi, flo, fhi, 1e-13, price_tol, 200)
}

/// Brent's root finder on a bracket with `fa < 0 < fb` (or vice versa).
/// Returns `None` if it fails to converge in `max_iter` evaluations.
fn brent(
    f: impl Fn(f64) -> f64,
    mut a: f64,
    mut b: f64,
    mut fa: f64,
    mut fb: f64,
    xtol: f64,
    ftol: f64,
    max_iter: usize,
) -> Option<f64> {
    if fa * fb > 0.0 {
        return None;
    }
    let (mut c, mut fc) = (b, fb);
    let mut d = b - a;
    let mut e = d;
    for _ in 0..max_iter {
        if (fb > 0.0) == (fc > 0.0) {
            (c, fc) = (a, fa);
            d = b - a;
            e = d;
        }
        if fc.abs() < fb.abs() {
            (a, fa) = (b, fb);
            (b, fb) = (c, fc);
            (c, fc) = (a, fa);
        }
        let tol = 2.0 * f64::EPSILON * b.abs() + 0.5 * xtol;
        let xm = 0.5 * (c - b);
        if xm.abs() <= tol || fb.abs() <= ftol {
            return Some(b);
        }
        if e.abs() >= tol && fa.abs() > fb.abs() {
            // Inverse quadratic interpolation (secant when a == c).
            let s = fb / fa;
            let (mut p, mut q) = if a == c {
                (2.0 * xm * s, 1.0 - s)
            } else {
                let qa = fa / fc;
                let r = fb / fc;
                (s * (2.0 * xm * qa * (qa - r) - (b - a) * (r - 1.0)), (qa - 1.0) * (r - 1.0) * (s - 1.0))
            };
            if p > 0.0 {
                q = -q;
            }
            p = p.abs();
            let min1 = 3.0 * xm * q - (tol * q).abs();
            let min2 = (e * q).abs();
            if 2.0 * p < min1.min(min2) {
                e = d;
                d = p / q;
            } else {
                d = xm;
                e = d;
            }
        } else {
            d = xm;
            e = d;
        }
        (a, fa) = (b, fb);
        b += if d.abs() > tol { d } else { tol.copysign(xm) };
        fb = f(b);
        if !fb.is_finite() {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const CALL: OptionRight = OptionRight::Call;
    const PUT: OptionRight = OptionRight::Put;
    const EURO: ExerciseStyle = ExerciseStyle::European;
    const AMER: ExerciseStyle = ExerciseStyle::American;

    fn inputs(spot: f64, strike: f64, rate: f64, q: f64, vol: f64, t: f64, right: OptionRight) -> BsInputs {
        BsInputs { spot, strike, rate, dividend_yield: q, vol, time_years: t, right }
    }

    /// Hull, *Options, Futures, and Other Derivatives*, Black-Scholes example:
    /// S=42, K=40, r=10 %, σ=20 %, T=0.5 → c = 4.76, p = 0.81.
    #[test]
    fn hull_black_scholes_example() {
        let c = black_scholes_price(&inputs(42.0, 40.0, 0.10, 0.0, 0.20, 0.5, CALL));
        let p = black_scholes_price(&inputs(42.0, 40.0, 0.10, 0.0, 0.20, 0.5, PUT));
        assert!((c - 4.76).abs() < 0.005, "call {c}");
        assert!((p - 0.81).abs() < 0.005, "put {p}");
        // Full-precision values (independent Python implementation).
        assert!((c - 4.759422392871535).abs() < 1e-12);
        assert!((p - 0.8085993729000958).abs() < 1e-12);
    }

    /// Haug, *The Complete Guide to Option Pricing Formulas*, generalized BSM
    /// example: index put S=100, K=95, T=0.5, r=10 %, q=5 %, σ=20 % → 2.4648.
    #[test]
    fn haug_bsm_dividend_example() {
        let p = black_scholes_price(&inputs(100.0, 95.0, 0.10, 0.05, 0.20, 0.5, PUT));
        assert!((p - 2.4648).abs() < 5e-5, "put {p}");
    }

    #[test]
    fn edge_cases() {
        // Expired: intrinsic.
        assert_eq!(black_scholes_price(&inputs(110.0, 100.0, 0.05, 0.0, 0.2, 0.0, CALL)), 10.0);
        assert_eq!(black_scholes_price(&inputs(110.0, 100.0, 0.05, 0.0, 0.2, -1.0, PUT)), 0.0);
        // Zero vol: discounted forward intrinsic.
        let c = black_scholes_price(&inputs(100.0, 100.0, 0.05, 0.0, 0.0, 1.0, CALL));
        assert!((c - (100.0 - 100.0 * (-0.05_f64).exp())).abs() < 1e-12);
        assert_eq!(black_scholes_price(&inputs(100.0, 100.0, 0.05, 0.0, 0.0, 1.0, PUT)), 0.0);
        assert!(black_scholes_price(&inputs(f64::NAN, 100.0, 0.05, 0.0, 0.2, 1.0, PUT)).is_nan());
        // Binomial agrees on the degenerate cases.
        assert_eq!(binomial_price(&inputs(110.0, 100.0, 0.05, 0.0, 0.2, 0.0, CALL), 100, AMER), 10.0);
        let am0 = binomial_price(&inputs(90.0, 100.0, 0.05, 0.0, 0.0, 1.0, PUT), 100, AMER);
        assert!((am0 - 10.0).abs() < 1e-12, "zero-vol American put exercises now: {am0}");
        // Greeks at expiry.
        let g = black_scholes_greeks(&inputs(110.0, 100.0, 0.05, 0.0, 0.2, 0.0, CALL));
        assert_eq!((g.delta, g.gamma, g.vega), (1.0, 0.0, 0.0));
        let g = black_scholes_greeks(&inputs(100.0, 100.0, 0.05, 0.0, 0.2, 0.0, PUT));
        assert_eq!(g.delta, -0.5);
    }

    fn rel(a: f64, b: f64) -> f64 {
        (a - b).abs() / b.abs().max(1e-12)
    }

    #[test]
    fn greeks_match_finite_differences_of_bs_price() {
        let cases = [
            inputs(100.0, 100.0, 0.05, 0.02, 0.25, 0.75, CALL),
            inputs(100.0, 110.0, 0.03, 0.0, 0.35, 0.3, PUT),
            inputs(42.0, 40.0, 0.10, 0.0, 0.20, 0.5, CALL),
            inputs(250.0, 200.0, 0.01, 0.04, 0.6, 2.0, PUT),
        ];
        for i in cases {
            let g = black_scholes_greeks(&i);
            let h = 1e-4 * i.spot;
            let p = |x: BsInputs| black_scholes_price(&x);
            let up = p(BsInputs { spot: i.spot + h, ..i });
            let dn = p(BsInputs { spot: i.spot - h, ..i });
            let mid = p(i);
            assert!(rel(g.delta, (up - dn) / (2.0 * h)) < 1e-6, "delta {i:?}");
            assert!(rel(g.gamma, (up - 2.0 * mid + dn) / (h * h)) < 1e-4, "gamma {i:?}");
            let hv = 1e-5;
            let vega_fd = (p(i.with_vol(i.vol + hv)) - p(i.with_vol(i.vol - hv))) / (2.0 * hv) * 0.01;
            assert!(rel(g.vega, vega_fd) < 1e-6, "vega {i:?}");
            let hr = 1e-5;
            let rho_fd = (p(BsInputs { rate: i.rate + hr, ..i }) - p(BsInputs { rate: i.rate - hr, ..i })) / (2.0 * hr) * 0.01;
            assert!(rel(g.rho, rho_fd) < 1e-6, "rho {i:?}");
            let ht = 1e-5;
            let theta_fd = -(p(BsInputs { time_years: i.time_years + ht, ..i })
                - p(BsInputs { time_years: i.time_years - ht, ..i }))
                / (2.0 * ht)
                / 365.0;
            assert!(rel(g.theta, theta_fd) < 1e-6, "theta {i:?}");
        }
    }

    #[test]
    fn zero_vol_greeks_are_derivatives_of_forward_value() {
        let i = inputs(100.0, 90.0, 0.05, 0.02, 0.0, 1.0, CALL);
        let g = black_scholes_greeks(&i);
        assert!((g.delta - (-0.02_f64).exp()).abs() < 1e-15);
        let h = 1e-6;
        let p = |x: BsInputs| black_scholes_price(&x);
        let theta_fd = -(p(BsInputs { time_years: 1.0 + h, ..i }) - p(BsInputs { time_years: 1.0 - h, ..i })) / (2.0 * h) / 365.0;
        assert!((g.theta - theta_fd).abs() < 1e-8);
    }

    /// Hull's 5-step American put tree (S=50, K=50, r=10 %, σ=40 %, T=5/12)
    /// is worth $4.49; an independent Python CRR gives 4.488458534725916 for
    /// 5 steps and 4.283021276450517 for 500 steps.
    #[test]
    fn hull_american_put_tree() {
        let i = inputs(50.0, 50.0, 0.10, 0.0, 0.40, 5.0 / 12.0, PUT);
        let p5 = binomial_price(&i, 5, AMER);
        assert!((p5 - 4.49).abs() < 0.005, "{p5}");
        assert!((p5 - 4.488458534725916).abs() < 1e-12);
        let p500 = binomial_price(&i, 500, AMER);
        assert!((p500 - 4.283021276450517).abs() < 1e-10, "{p500}");
    }

    #[test]
    fn binomial_european_converges_to_bs() {
        let cases = [
            inputs(42.0, 40.0, 0.10, 0.0, 0.20, 0.5, CALL),
            inputs(42.0, 40.0, 0.10, 0.0, 0.20, 0.5, PUT),
            inputs(100.0, 95.0, 0.10, 0.05, 0.20, 0.5, PUT),
            inputs(100.0, 120.0, 0.03, 0.01, 0.45, 2.0, CALL),
            inputs(100.0, 80.0, 0.0, 0.03, 0.15, 0.1, PUT),
        ];
        for i in cases {
            let bs = black_scholes_price(&i);
            let bin = binomial_price(&i, 500, EURO);
            assert!((bs - bin).abs() < 1e-2, "{i:?}: bs {bs} vs binomial {bin}");
        }
    }

    #[test]
    fn american_put_dominates_and_american_call_without_dividends_matches() {
        for (s, k, r, v, t) in [(100.0, 100.0, 0.05, 0.2, 1.0), (80.0, 100.0, 0.08, 0.3, 2.0), (120.0, 100.0, 0.02, 0.5, 0.25)] {
            let put = inputs(s, k, r, 0.0, v, t, PUT);
            let am = binomial_price(&put, 300, AMER);
            assert!(am >= binomial_price(&put, 300, EURO) - 1e-12);
            assert!(am >= black_scholes_price(&put) - 1e-2);
            assert!(am >= intrinsic(PUT, s, k));
            let call = inputs(s, k, r, 0.0, v, t, CALL);
            let diff = binomial_price(&call, 300, AMER) - binomial_price(&call, 300, EURO);
            assert!(diff.abs() < 1e-10, "American call with q=0 must equal European: {diff}");
        }
        // With a dividend yield, the American call can be worth more.
        let call = inputs(100.0, 80.0, 0.02, 0.08, 0.2, 2.0, CALL);
        assert!(binomial_price(&call, 300, AMER) > binomial_price(&call, 300, EURO) + 0.1);
    }

    #[test]
    fn low_vol_tree_uses_arbitrage_free_lattice() {
        // σ√Δt < (r−q)Δt makes plain CRR p > 1; the drifted lattice is used.
        let i = inputs(100.0, 100.0, 0.10, 0.0, 0.01, 1.0, CALL);
        let bin = binomial_price(&i, 10, EURO);
        let bs = black_scholes_price(&i);
        assert!((bin - bs).abs() < 0.05, "bin {bin} vs bs {bs}");
        let g = binomial_greeks(&i, 50, EURO);
        assert!(g.delta.is_finite() && g.theta.is_finite());
    }

    #[test]
    fn binomial_greeks_close_to_bs_for_european() {
        let i = inputs(100.0, 105.0, 0.04, 0.01, 0.3, 0.8, CALL);
        let b = binomial_greeks(&i, 1000, EURO);
        let a = black_scholes_greeks(&i);
        assert!((b.delta - a.delta).abs() < 2e-3, "delta {} vs {}", b.delta, a.delta);
        assert!((b.gamma - a.gamma).abs() < 2e-4, "gamma {} vs {}", b.gamma, a.gamma);
        assert!((b.theta - a.theta).abs() < 1e-3, "theta {} vs {}", b.theta, a.theta);
        assert!((b.vega - a.vega).abs() < 2e-3, "vega {} vs {}", b.vega, a.vega);
        assert!((b.rho - a.rho).abs() < 2e-3, "rho {} vs {}", b.rho, a.rho);
        // Put on the drift lattice side too.
        let ip = BsInputs { right: PUT, ..i };
        let bp = binomial_greeks(&ip, 1000, EURO);
        let ap = black_scholes_greeks(&ip);
        assert!((bp.delta - ap.delta).abs() < 2e-3 && (bp.theta - ap.theta).abs() < 1e-3);
        // American put: delta between -1 and 0, positive gamma, negative theta.
        let am = binomial_greeks(&BsInputs { right: PUT, spot: 95.0, ..i }, 500, AMER);
        assert!(am.delta < 0.0 && am.delta > -1.0 && am.gamma > 0.0 && am.theta < 0.0);
    }

    #[test]
    fn model_dispatch() {
        let i = inputs(42.0, 40.0, 0.10, 0.0, 0.20, 0.5, CALL);
        assert_eq!(price(&i, EURO, PricingModel::BlackScholes), black_scholes_price(&i));
        assert_eq!(price(&i, AMER, PricingModel::Binomial { steps: 50 }), binomial_price(&i, 50, AMER));
        assert_eq!(greeks(&i, EURO, PricingModel::BlackScholes), black_scholes_greeks(&i));
    }

    #[test]
    fn implied_vol_known_and_bounds() {
        let i = inputs(42.0, 40.0, 0.10, 0.0, 0.20, 0.5, CALL);
        let iv = implied_volatility(4.759422392871535, &i, EURO, PricingModel::BlackScholes).unwrap();
        assert!((iv - 0.20).abs() < 1e-10, "{iv}");
        // Below forward intrinsic (42 − 40e^{-0.05} = 3.95): arbitrage.
        assert_eq!(implied_volatility(3.0, &i, EURO, PricingModel::BlackScholes), None);
        // Above the spot: arbitrage.
        assert_eq!(implied_volatility(42.0, &i, EURO, PricingModel::BlackScholes), None);
        assert_eq!(implied_volatility(f64::NAN, &i, EURO, PricingModel::BlackScholes), None);
        assert_eq!(implied_volatility(1.0, &BsInputs { time_years: 0.0, ..i }, EURO, PricingModel::BlackScholes), None);
        // American put via the tree, round trip.
        let ap = inputs(50.0, 50.0, 0.10, 0.0, 0.40, 5.0 / 12.0, PUT);
        let model = PricingModel::Binomial { steps: 200 };
        let target = price(&ap, AMER, model);
        let iv = implied_volatility(target, &ap, AMER, model).unwrap();
        assert!((iv - 0.40).abs() < 1e-6, "{iv}");
        // An American put below immediate exercise value is an arbitrage.
        let deep = inputs(30.0, 50.0, 0.10, 0.0, 0.40, 1.0, PUT);
        assert_eq!(implied_volatility(19.0, &deep, AMER, model), None);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn put_call_parity(
            spot in 1.0_f64..1000.0,
            moneyness in 0.3_f64..3.0,
            rate in -0.02_f64..0.15,
            q in 0.0_f64..0.10,
            vol in 0.01_f64..2.0,
            t in 0.001_f64..5.0,
        ) {
            let strike = spot * moneyness;
            let c = black_scholes_price(&inputs(spot, strike, rate, q, vol, t, CALL));
            let p = black_scholes_price(&inputs(spot, strike, rate, q, vol, t, PUT));
            let parity = spot * (-q * t).exp() - strike * (-rate * t).exp();
            prop_assert!((c - p - parity).abs() <= 1e-10 * spot.max(strike), "c={c} p={p} parity={parity}");
            let gc = black_scholes_greeks(&inputs(spot, strike, rate, q, vol, t, CALL));
            let gp = black_scholes_greeks(&inputs(spot, strike, rate, q, vol, t, PUT));
            prop_assert!((gc.delta - gp.delta - (-q * t).exp()).abs() < 1e-12);
            prop_assert!((gc.gamma - gp.gamma).abs() <= 1e-12 * gc.gamma.abs().max(1.0));
            prop_assert!((gc.vega - gp.vega).abs() <= 1e-12 * gc.vega.abs().max(1.0));
        }

        #[test]
        fn implied_vol_round_trip(
            spot in 5.0_f64..500.0,
            moneyness in 0.6_f64..1.5,
            rate in 0.0_f64..0.08,
            q in 0.0_f64..0.05,
            vol in 0.05_f64..1.5,
            t in 0.02_f64..3.0,
            is_call in any::<bool>(),
        ) {
            let right = if is_call { CALL } else { PUT };
            let i = inputs(spot, spot * moneyness, rate, q, vol, t, right);
            let p = black_scholes_price(&i);
            let floor = forward_intrinsic(&i);
            // Skip prices with no measurable time value (IV is ill-posed there).
            prop_assume!(p - floor > 1e-6 * spot);
            let iv = implied_volatility(p, &i, EURO, PricingModel::BlackScholes);
            prop_assert!(iv.is_some(), "no IV for {i:?} price {p}");
            let iv = iv.unwrap_or(f64::NAN);
            let back = black_scholes_price(&i.with_vol(iv));
            prop_assert!((back - p).abs() <= 1e-9 * spot, "price {p} → iv {iv} → {back}");
            let vega = black_scholes_greeks(&i).vega / 0.01;
            if vega > 1e-3 * spot {
                prop_assert!((iv - vol).abs() < 1e-6, "iv {iv} vs vol {vol}");
            }
        }
    }
}

//! Multi-leg option strategies: expiry payoff, Black-Scholes mark-to-model
//! curves, breakevens, profit/loss extremes and aggregate Greeks.
//!
//! All P&L figures are **per unit of underlying** and net of the premium paid
//! (or received); the caller applies the contract multiplier. `quantity` is
//! signed: `+1` long, `−1` short. Option marks use European Black-Scholes
//! (early exercise is ignored) with each leg's own volatility.

use meridian_types::OptionRight;
use serde::{Deserialize, Serialize};

use crate::options::{BsInputs, GreekValues, black_scholes_greeks, black_scholes_price, intrinsic};

/// Instrument type of a strategy leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LegKind {
    Call,
    Put,
    Stock,
}

/// One position in a strategy.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Leg {
    pub kind: LegKind,
    /// Strike (ignored for stock).
    pub strike: f64,
    /// Signed units: `+` long, `−` short.
    pub quantity: f64,
    /// Premium paid per unit for options (received if short); entry price for stock.
    pub premium: f64,
    /// Years to expiry at entry (ignored for stock).
    pub expiry_years: f64,
    /// Annualized volatility used for mark-to-model (ignored for stock).
    pub vol: f64,
}

impl Leg {
    pub fn call(strike: f64, quantity: f64, premium: f64, expiry_years: f64, vol: f64) -> Self {
        Self { kind: LegKind::Call, strike, quantity, premium, expiry_years, vol }
    }

    pub fn put(strike: f64, quantity: f64, premium: f64, expiry_years: f64, vol: f64) -> Self {
        Self { kind: LegKind::Put, strike, quantity, premium, expiry_years, vol }
    }

    pub fn stock(quantity: f64, entry_price: f64) -> Self {
        Self { kind: LegKind::Stock, strike: 0.0, quantity, premium: entry_price, expiry_years: 0.0, vol: 0.0 }
    }

    fn right(&self) -> Option<OptionRight> {
        match self.kind {
            LegKind::Call => Some(OptionRight::Call),
            LegKind::Put => Some(OptionRight::Put),
            LegKind::Stock => None,
        }
    }

    /// Per-unit value at expiry (before premium).
    fn value_at_expiry(&self, spot: f64) -> f64 {
        self.right().map_or(spot, |r| intrinsic(r, spot, self.strike))
    }

    /// Per-unit model value with `years_elapsed` since entry.
    fn model_value(&self, spot: f64, rate: f64, dividend_yield: f64, years_elapsed: f64) -> f64 {
        self.right().map_or(spot, |right| {
            black_scholes_price(&BsInputs {
                spot,
                strike: self.strike,
                rate,
                dividend_yield,
                vol: self.vol,
                time_years: self.expiry_years - years_elapsed,
                right,
            })
        })
    }
}

/// Strategy P&L at expiry for one underlying price: `Σ qty·(value − premium)`.
pub fn payoff_at_expiry(legs: &[Leg], spot: f64) -> f64 {
    legs.iter().map(|l| l.quantity * (l.value_at_expiry(spot) - l.premium)).sum()
}

/// [`payoff_at_expiry`] over a grid of spots.
pub fn payoff_curve(legs: &[Leg], spots: &[f64]) -> Vec<f64> {
    spots.iter().map(|&s| payoff_at_expiry(legs, s)).collect()
}

/// Mark-to-model P&L over a grid of spots, `years_elapsed` after entry, with
/// each option valued by Black-Scholes at its remaining time
/// (`expiry_years − years_elapsed`; expired legs at intrinsic). `rate` and
/// `dividend_yield` are continuous annual rates.
pub fn theoretical_curve(legs: &[Leg], spots: &[f64], rate: f64, dividend_yield: f64, years_elapsed: f64) -> Vec<f64> {
    spots
        .iter()
        .map(|&s| legs.iter().map(|l| l.quantity * (l.model_value(s, rate, dividend_yield, years_elapsed) - l.premium)).sum())
        .collect()
}

/// Kink points of the expiry payoff within `(lo, hi)`, plus the ends, sorted.
fn breakpoints(legs: &[Leg], lo: f64, hi: f64) -> Vec<f64> {
    let mut pts = vec![lo, hi];
    pts.extend(legs.iter().filter(|l| l.kind != LegKind::Stock && l.strike > lo && l.strike < hi).map(|l| l.strike));
    pts.sort_by(f64::total_cmp);
    pts.dedup();
    pts
}

/// Tolerance for "P&L is zero" relative to the strategy's scale.
fn pnl_eps(legs: &[Leg], lo: f64, hi: f64) -> f64 {
    let scale = legs.iter().map(|l| l.quantity.abs() * (l.strike.abs() + l.premium.abs())).sum::<f64>() + lo.abs() + hi.abs();
    1e-12 * scale.max(1.0)
}

/// Underlying prices in `[lo, hi]` where the expiry P&L crosses zero, in
/// ascending order. Exact: the payoff is linear between strikes. If the P&L is
/// zero over a whole interval, the interval's end points are returned.
pub fn breakevens(legs: &[Leg], lo: f64, hi: f64) -> Vec<f64> {
    if !(lo.is_finite() && hi.is_finite() && lo < hi) || legs.is_empty() {
        return Vec::new();
    }
    let eps = pnl_eps(legs, lo, hi);
    let pts = breakpoints(legs, lo, hi);
    let vals: Vec<f64> = pts.iter().map(|&s| payoff_at_expiry(legs, s)).collect();
    let mut roots: Vec<f64> = Vec::new();
    for w in 0..pts.len() {
        if vals[w].abs() <= eps {
            roots.push(pts[w]);
        } else if w + 1 < pts.len() && vals[w + 1].abs() > eps && (vals[w] > 0.0) != (vals[w + 1] > 0.0) {
            let (a, b, fa, fb) = (pts[w], pts[w + 1], vals[w], vals[w + 1]);
            roots.push(a - fa * (b - a) / (fb - fa));
        }
    }
    let tol = 1e-9 * (lo.abs() + hi.abs()).max(1.0);
    roots.dedup_by(|a, b| (*a - *b).abs() <= tol);
    roots
}

/// `(max_profit, min_pnl)` of the expiry payoff over all spots `≥ 0`.
///
/// The payoff is piecewise linear, so the extremes are attained at spot 0, at
/// a strike, or as spot → ∞; the latter is detected from the slope beyond the
/// highest strike (`Σ qty` of calls and stock). `None` means unbounded:
/// `max_profit` is `None` when that slope is positive (e.g. long call) and
/// `min_pnl` is `None` when it is negative (e.g. naked short call). `min_pnl`
/// is signed — the worst-case P&L, negative for a loss.
///
/// `lo` and `hi` are added as evaluation points (and negative values are
/// ignored); because the result is exact over the whole domain they do not
/// restrict it.
pub fn max_profit_loss(legs: &[Leg], lo: f64, hi: f64) -> (Option<f64>, Option<f64>) {
    if legs.is_empty() {
        return (Some(0.0), Some(0.0));
    }
    let mut pts: Vec<f64> = vec![0.0];
    pts.extend([lo, hi].into_iter().filter(|v| v.is_finite() && *v > 0.0));
    pts.extend(legs.iter().filter(|l| l.kind != LegKind::Stock && l.strike > 0.0).map(|l| l.strike));
    let vals: Vec<f64> = pts.iter().map(|&s| payoff_at_expiry(legs, s)).collect();
    let max = vals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let min = vals.iter().copied().fold(f64::INFINITY, f64::min);
    let slope: f64 = legs.iter().filter(|l| l.kind != LegKind::Put).map(|l| l.quantity).sum();
    let eps = 1e-12 * legs.iter().map(|l| l.quantity.abs()).sum::<f64>().max(1.0);
    let max_profit = (slope <= eps).then_some(max);
    let min_pnl = (slope >= -eps).then_some(min);
    (max_profit, min_pnl)
}

/// Aggregate position Greeks (terminal units, per unit of underlying, signed
/// by quantity) at `spot` with continuous `rate` and `dividend_yield`. Stock
/// legs contribute delta = quantity.
pub fn strategy_greeks(legs: &[Leg], spot: f64, rate: f64, dividend_yield: f64) -> GreekValues {
    legs.iter().fold(GreekValues::ZERO, |acc, l| {
        let g = l.right().map_or(GreekValues { delta: 1.0, ..GreekValues::ZERO }, |right| {
            black_scholes_greeks(&BsInputs {
                spot,
                strike: l.strike,
                rate,
                dividend_yield,
                vol: l.vol,
                time_years: l.expiry_years,
                right,
            })
        });
        acc + g.scaled(l.quantity)
    })
}

/// Legs with every quantity multiplied by `factor` (`−1.0` turns a long
/// strategy into the corresponding short one).
pub fn scale_legs(legs: &[Leg], factor: f64) -> Vec<Leg> {
    legs.iter().map(|l| Leg { quantity: l.quantity * factor, ..*l }).collect()
}

/// Long one call.
pub fn long_call(strike: f64, premium: f64, expiry_years: f64, vol: f64) -> Vec<Leg> {
    vec![Leg::call(strike, 1.0, premium, expiry_years, vol)]
}

/// Long one unit of stock bought at `stock_price`, short one call.
pub fn covered_call(stock_price: f64, call_strike: f64, call_premium: f64, expiry_years: f64, vol: f64) -> Vec<Leg> {
    vec![Leg::stock(1.0, stock_price), Leg::call(call_strike, -1.0, call_premium, expiry_years, vol)]
}

/// Vertical spread: long `right` at `long_strike`, short at `short_strike`
/// (bull call: long the lower strike; bear put: long the higher strike).
pub fn vertical_spread(
    right: OptionRight,
    long_strike: f64,
    long_premium: f64,
    short_strike: f64,
    short_premium: f64,
    expiry_years: f64,
    vol: f64,
) -> Vec<Leg> {
    let leg = |strike, qty, premium| match right {
        OptionRight::Call => Leg::call(strike, qty, premium, expiry_years, vol),
        OptionRight::Put => Leg::put(strike, qty, premium, expiry_years, vol),
    };
    vec![leg(long_strike, 1.0, long_premium), leg(short_strike, -1.0, short_premium)]
}

/// Long straddle: long call and long put at the same strike.
pub fn straddle(strike: f64, call_premium: f64, put_premium: f64, expiry_years: f64, vol: f64) -> Vec<Leg> {
    vec![Leg::call(strike, 1.0, call_premium, expiry_years, vol), Leg::put(strike, 1.0, put_premium, expiry_years, vol)]
}

/// Long strangle: long OTM put at `put_strike`, long OTM call at `call_strike`.
pub fn strangle(
    put_strike: f64,
    put_premium: f64,
    call_strike: f64,
    call_premium: f64,
    expiry_years: f64,
    vol: f64,
) -> Vec<Leg> {
    vec![
        Leg::put(put_strike, 1.0, put_premium, expiry_years, vol),
        Leg::call(call_strike, 1.0, call_premium, expiry_years, vol),
    ]
}

/// Short (credit) iron condor with ascending `strikes = [K1, K2, K3, K4]`:
/// long put K1, short put K2, short call K3, long call K4; `premiums` in the
/// same order (all positive).
pub fn iron_condor(strikes: [f64; 4], premiums: [f64; 4], expiry_years: f64, vol: f64) -> Vec<Leg> {
    vec![
        Leg::put(strikes[0], 1.0, premiums[0], expiry_years, vol),
        Leg::put(strikes[1], -1.0, premiums[1], expiry_years, vol),
        Leg::call(strikes[2], -1.0, premiums[2], expiry_years, vol),
        Leg::call(strikes[3], 1.0, premiums[3], expiry_years, vol),
    ]
}

/// Long butterfly with ascending `strikes = [K1, K2, K3]`: +1 K1, −2 K2, +1 K3
/// of `right`; `premiums` in the same order.
pub fn butterfly(right: OptionRight, strikes: [f64; 3], premiums: [f64; 3], expiry_years: f64, vol: f64) -> Vec<Leg> {
    let leg = |strike, qty, premium| match right {
        OptionRight::Call => Leg::call(strike, qty, premium, expiry_years, vol),
        OptionRight::Put => Leg::put(strike, qty, premium, expiry_years, vol),
    };
    vec![leg(strikes[0], 1.0, premiums[0]), leg(strikes[1], -2.0, premiums[1]), leg(strikes[2], 1.0, premiums[2])]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn assert_roots(got: &[f64], want: &[f64]) {
        assert_eq!(got.len(), want.len(), "{got:?} vs {want:?}");
        for (g, w) in got.iter().zip(want) {
            assert!(close(*g, *w), "{got:?} vs {want:?}");
        }
    }

    #[test]
    fn long_call_profile() {
        let legs = long_call(100.0, 5.0, 0.5, 0.2);
        assert!(close(payoff_at_expiry(&legs, 90.0), -5.0));
        assert!(close(payoff_at_expiry(&legs, 110.0), 5.0));
        assert_roots(&breakevens(&legs, 50.0, 150.0), &[105.0]);
        assert_eq!(max_profit_loss(&legs, 50.0, 150.0), (None, Some(-5.0)));
    }

    #[test]
    fn covered_call_profile() {
        let legs = covered_call(100.0, 105.0, 2.0, 0.25, 0.2);
        assert!(close(payoff_at_expiry(&legs, 120.0), 7.0));
        assert_roots(&breakevens(&legs, 50.0, 150.0), &[98.0]);
        let (mp, ml) = max_profit_loss(&legs, 50.0, 150.0);
        assert!(close(mp.unwrap(), 7.0));
        // Stock to zero: −100 + 2.
        assert!(close(ml.unwrap(), -98.0));
    }

    #[test]
    fn vertical_and_volatility_spreads() {
        let bull = vertical_spread(OptionRight::Call, 100.0, 5.0, 110.0, 2.0, 0.5, 0.2);
        assert_roots(&breakevens(&bull, 50.0, 150.0), &[103.0]);
        let (mp, ml) = max_profit_loss(&bull, 50.0, 150.0);
        assert!(close(mp.unwrap(), 7.0) && close(ml.unwrap(), -3.0));

        let st = straddle(100.0, 5.0, 5.0, 0.5, 0.2);
        assert_roots(&breakevens(&st, 50.0, 150.0), &[90.0, 110.0]);
        assert_eq!(max_profit_loss(&st, 50.0, 150.0), (None, Some(-10.0)));

        let sg = strangle(95.0, 2.0, 105.0, 2.0, 0.5, 0.2);
        assert_roots(&breakevens(&sg, 50.0, 150.0), &[91.0, 109.0]);

        // Short straddle: unbounded loss above, bounded profit.
        let short = scale_legs(&st, -1.0);
        assert_eq!(max_profit_loss(&short, 50.0, 150.0), (Some(10.0), None));
    }

    #[test]
    fn iron_condor_and_butterfly() {
        let ic = iron_condor([90.0, 95.0, 105.0, 110.0], [1.0, 2.0, 2.0, 1.0], 0.25, 0.2);
        let (mp, ml) = max_profit_loss(&ic, 50.0, 150.0);
        assert!(close(mp.unwrap(), 2.0) && close(ml.unwrap(), -3.0));
        assert_roots(&breakevens(&ic, 50.0, 150.0), &[93.0, 107.0]);

        let bf = butterfly(OptionRight::Call, [90.0, 100.0, 110.0], [12.0, 5.0, 1.5], 0.25, 0.2);
        let (mp, ml) = max_profit_loss(&bf, 50.0, 150.0);
        assert!(close(mp.unwrap(), 6.5) && close(ml.unwrap(), -3.5));
        assert_roots(&breakevens(&bf, 50.0, 150.0), &[93.5, 106.5]);
    }

    #[test]
    fn short_put_loss_is_bounded_at_zero_spot() {
        let sp = vec![Leg::put(100.0, -1.0, 4.0, 0.5, 0.3)];
        // Worst case is the stock going to zero, even outside the [lo, hi] window.
        assert_eq!(max_profit_loss(&sp, 80.0, 120.0), (Some(4.0), Some(-96.0)));
        assert!(breakevens(&sp, 80.0, 120.0).iter().zip([96.0]).all(|(a, b)| close(*a, b)));
    }

    #[test]
    fn zero_segment_breakevens_and_degenerate_ranges() {
        // Long call bought for free: P&L is 0 on [0, K].
        let legs = vec![Leg::call(100.0, 1.0, 0.0, 1.0, 0.2)];
        assert_roots(&breakevens(&legs, 50.0, 150.0), &[50.0, 100.0]);
        assert!(breakevens(&legs, 150.0, 50.0).is_empty());
        assert!(breakevens(&[], 1.0, 2.0).is_empty());
    }

    #[test]
    fn theoretical_curve_converges_to_payoff() {
        let legs = iron_condor([90.0, 95.0, 105.0, 110.0], [1.0, 2.0, 2.0, 1.0], 0.25, 0.2);
        let spots: Vec<f64> = (60..=140).map(f64::from).collect();
        let at_expiry = theoretical_curve(&legs, &spots, 0.03, 0.0, 0.25);
        let payoff = payoff_curve(&legs, &spots);
        for (a, b) in at_expiry.iter().zip(&payoff) {
            assert!(close(*a, *b));
        }
        // At entry the mark equals Σ qty·(BS − premium).
        let now = theoretical_curve(&legs, &[100.0], 0.03, 0.0, 0.0)[0];
        let manual: f64 = legs
            .iter()
            .map(|l| {
                let right = if l.kind == LegKind::Call { OptionRight::Call } else { OptionRight::Put };
                let bs = black_scholes_price(&BsInputs {
                    spot: 100.0,
                    strike: l.strike,
                    rate: 0.03,
                    dividend_yield: 0.0,
                    vol: 0.2,
                    time_years: 0.25,
                    right,
                });
                l.quantity * (bs - l.premium)
            })
            .sum();
        assert!(close(now, manual));
    }

    #[test]
    fn greeks_aggregate() {
        let i = |right| BsInputs { spot: 100.0, strike: 100.0, rate: 0.02, dividend_yield: 0.0, vol: 0.25, time_years: 0.5, right };
        let c = black_scholes_greeks(&i(OptionRight::Call));
        let p = black_scholes_greeks(&i(OptionRight::Put));
        let g = strategy_greeks(&straddle(100.0, 7.0, 6.0, 0.5, 0.25), 100.0, 0.02, 0.0);
        assert!(close(g.delta, c.delta + p.delta));
        assert!(close(g.gamma, c.gamma + p.gamma));
        assert!(close(g.vega, c.vega + p.vega));
        let cc = strategy_greeks(&covered_call(100.0, 100.0, 7.0, 0.5, 0.25), 100.0, 0.02, 0.0);
        assert!(close(cc.delta, 1.0 - c.delta));
        assert!(close(cc.theta, -c.theta));
    }
}

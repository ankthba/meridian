//! Property tests for invariants across the public API.

use meridian_analytics::indicators::{ema, rolling_std, rsi, sma, wma};
use meridian_analytics::options::{
    BsInputs, ExerciseStyle, OptionRight, PricingModel, binomial_price, black_scholes_price, implied_volatility,
};
use meridian_analytics::returns::{cumulative_returns, drawdown_series, max_drawdown, simple_returns};
use meridian_analytics::stats::{correlation, percentile};
use meridian_analytics::strategies::{Leg, breakevens, max_profit_loss, payoff_at_expiry};
use meridian_analytics::surface::{VolPoint, VolSurface};
use proptest::prelude::*;

fn prices() -> impl Strategy<Value = Vec<f64>> {
    prop::collection::vec(1.0_f64..1000.0, 2..120)
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        // Integration tests have no lib.rs for the default persistence lookup.
        failure_persistence: Some(Box::new(prop::test_runner::FileFailurePersistence::WithSource("proptest-regressions"))),
        ..ProptestConfig::default()
    })]

    #[test]
    fn moving_averages_stay_within_window_range(x in prices(), n in 1usize..30) {
        let s = sma(&x, n);
        let w = wma(&x, n);
        let e = ema(&x, n);
        prop_assert_eq!(s.len(), x.len());
        for i in 0..x.len() {
            if i + 1 < n {
                prop_assert!(s[i].is_nan() && w[i].is_nan() && e[i].is_nan());
                continue;
            }
            let win = &x[i + 1 - n..=i];
            let lo = win.iter().copied().fold(f64::INFINITY, f64::min) - 1e-9;
            let hi = win.iter().copied().fold(f64::NEG_INFINITY, f64::max) + 1e-9;
            prop_assert!(s[i] >= lo && s[i] <= hi);
            prop_assert!(w[i] >= lo && w[i] <= hi);
            // EMA is bounded by the range of everything seen so far.
            let all_lo = x[..=i].iter().copied().fold(f64::INFINITY, f64::min) - 1e-9;
            let all_hi = x[..=i].iter().copied().fold(f64::NEG_INFINITY, f64::max) + 1e-9;
            prop_assert!(e[i] >= all_lo && e[i] <= all_hi);
        }
    }

    #[test]
    fn rsi_and_std_ranges(x in prices(), n in 1usize..20) {
        for v in rsi(&x, n).into_iter().filter(|v| !v.is_nan()) {
            prop_assert!((0.0..=100.0).contains(&v));
        }
        for v in rolling_std(&x, n).into_iter().filter(|v| !v.is_nan()) {
            prop_assert!(v >= 0.0);
        }
    }

    #[test]
    fn drawdown_invariants(x in prices()) {
        let d = max_drawdown(&x);
        prop_assert!((0.0..1.0).contains(&d.max_drawdown));
        prop_assert!(d.peak_index <= d.trough_index);
        let s = drawdown_series(&x);
        prop_assert!(s.iter().all(|v| *v <= 0.0 && *v > -1.0));
        let worst = s.iter().copied().fold(0.0_f64, f64::min);
        prop_assert!((worst + d.max_drawdown).abs() < 1e-12);
        let cum = cumulative_returns(&simple_returns(&x));
        let total = x[x.len() - 1] / x[0] - 1.0;
        prop_assert!((cum[cum.len() - 1] - total).abs() <= 1e-9 * total.abs().max(1.0));
    }

    #[test]
    fn correlation_bounded_and_percentile_monotone(
        x in prop::collection::vec(-100.0_f64..100.0, 3..60),
        y in prop::collection::vec(-100.0_f64..100.0, 3..60),
        p in 0.0_f64..1.0,
        q in 0.0_f64..1.0,
    ) {
        let c = correlation(&x, &y);
        prop_assert!(c.is_nan() || (-1.0..=1.0).contains(&c));
        let (a, b) = if p <= q { (p, q) } else { (q, p) };
        prop_assert!(percentile(&x, a) <= percentile(&x, b));
        let lo = x.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = x.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        prop_assert!(percentile(&x, a) >= lo && percentile(&x, b) <= hi);
    }

    #[test]
    fn american_dominates_european_and_intrinsic(
        spot in 20.0_f64..200.0,
        moneyness in 0.7_f64..1.3,
        rate in 0.0_f64..0.1,
        q in 0.0_f64..0.06,
        vol in 0.05_f64..0.8,
        t in 0.05_f64..2.0,
        is_call in any::<bool>(),
    ) {
        let right = if is_call { OptionRight::Call } else { OptionRight::Put };
        let i = BsInputs { spot, strike: spot * moneyness, rate, dividend_yield: q, vol, time_years: t, right };
        let am = binomial_price(&i, 150, ExerciseStyle::American);
        let eu = binomial_price(&i, 150, ExerciseStyle::European);
        let intrinsic = match right {
            OptionRight::Call => (spot - i.strike).max(0.0),
            OptionRight::Put => (i.strike - spot).max(0.0),
        };
        prop_assert!(am >= eu - 1e-10);
        prop_assert!(am >= intrinsic - 1e-10);
        prop_assert!((eu - black_scholes_price(&i)).abs() < 0.05 * spot.max(1.0) / 10.0);
    }

    #[test]
    fn binomial_implied_vol_round_trip(
        spot in 50.0_f64..150.0,
        moneyness in 0.85_f64..1.15,
        vol in 0.1_f64..0.6,
        t in 0.1_f64..1.5,
    ) {
        let i = BsInputs { spot, strike: spot * moneyness, rate: 0.03, dividend_yield: 0.0, vol, time_years: t, right: OptionRight::Put };
        let model = PricingModel::Binomial { steps: 100 };
        let p = binomial_price(&i, 100, ExerciseStyle::American);
        let iv = implied_volatility(p, &i, ExerciseStyle::American, model);
        prop_assert!(iv.is_some());
        let back = binomial_price(&i.with_vol(iv.unwrap_or(f64::NAN)), 100, ExerciseStyle::American);
        prop_assert!((back - p).abs() < 1e-8 * spot);
    }

    #[test]
    fn surface_interpolation_stays_within_quote_range(
        ivs in prop::collection::vec(0.05_f64..1.0, 9),
        t in 0.01_f64..4.0,
        strike in 10.0_f64..300.0,
    ) {
        let expiries = [0.25, 0.5, 1.0];
        let strikes = [80.0, 100.0, 120.0];
        let pts: Vec<VolPoint> = (0..9)
            .map(|n| VolPoint { expiry_years: expiries[n / 3], strike: strikes[n % 3], iv: ivs[n] })
            .collect();
        let s = VolSurface::from_points(&pts, 100.0).unwrap();
        let lo = ivs.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = ivs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let v = s.iv_at(t, strike).unwrap();
        prop_assert!(v >= lo - 1e-12 && v <= hi + 1e-12, "{} not in [{}, {}]", v, lo, hi);
    }

    #[test]
    fn breakevens_are_zeros_and_extremes_bound_payoff(
        k1 in 50.0_f64..100.0,
        width in 1.0_f64..30.0,
        q1 in -2.0_f64..2.0,
        q2 in -2.0_f64..2.0,
        p1 in 0.0_f64..10.0,
        p2 in 0.0_f64..10.0,
        spot in 0.0_f64..300.0,
    ) {
        let legs = vec![Leg::call(k1, q1, p1, 0.5, 0.2), Leg::put(k1 + width, q2, p2, 0.5, 0.2)];
        for b in breakevens(&legs, 1.0, 300.0) {
            prop_assert!(payoff_at_expiry(&legs, b).abs() < 1e-8);
        }
        let (max, min) = max_profit_loss(&legs, 1.0, 300.0);
        let v = payoff_at_expiry(&legs, spot);
        if let Some(m) = max { prop_assert!(v <= m + 1e-9); }
        if let Some(m) = min { prop_assert!(v >= m - 1e-9); }
    }
}

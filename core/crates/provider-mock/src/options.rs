//! Option chains priced with a private Black-Scholes model over a skewed,
//! term-structured volatility surface.
//!
//! This pricer exists only to make mock chains self-consistent; the real
//! analytics live in `meridian-analytics`. American-style equity options are
//! priced as European (a documented approximation).

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use meridian_provider::{ChainRequest, ProviderError, ProviderResult};
use meridian_types::{
    ExerciseStyle, Greeks, GreeksSource, NANOS_PER_DAY, OptionChain, OptionContract, OptionRight, Provenance,
    UnixNanos,
};

use crate::Inner;
use crate::cal::{Calendar, Session, SessionState, add_months, nth_weekday};
use crate::hash::{Cell, tag};
use crate::market::round_tick;
use crate::universe::{OptSpec, Sym};

/// Complementary error function (Numerical Recipes `erfcc`, Chebyshev
/// fit; fractional error < 1.2e-7 everywhere).
pub(crate) fn erfc(x: f64) -> f64 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.5 * z);
    let poly = -z * z - 1.265_512_23
        + t * (1.000_023_68
            + t * (0.374_091_96
                + t * (0.096_784_18
                    + t * (-0.186_288_06
                        + t * (0.278_868_07
                            + t * (-1.135_203_98 + t * (1.488_515_87 + t * (-0.822_152_23 + t * 0.170_872_77))))))));
    let ans = t * poly.exp();
    if x >= 0.0 { ans } else { 2.0 - ans }
}

pub(crate) fn norm_cdf(x: f64) -> f64 {
    0.5 * erfc(-x / std::f64::consts::SQRT_2)
}

pub(crate) fn norm_pdf(x: f64) -> f64 {
    (-0.5 * x * x).exp() / (2.0 * std::f64::consts::PI).sqrt()
}

/// Black-Scholes-Merton price and Greeks. Theta is per calendar day, vega
/// per volatility point (0.01), rho per percentage point (0.01).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Bs {
    pub price: f64,
    pub delta: f64,
    pub gamma: f64,
    pub theta: f64,
    pub vega: f64,
    pub rho: f64,
}

pub(crate) fn black_scholes(right: OptionRight, s: f64, k: f64, t: f64, r: f64, q: f64, sigma: f64) -> Bs {
    let t = t.max(1e-6);
    let sigma = sigma.max(1e-4);
    let st = sigma * t.sqrt();
    let d1 = ((s / k).ln() + (r - q + 0.5 * sigma * sigma) * t) / st;
    let d2 = d1 - st;
    let dq = (-q * t).exp();
    let dr = (-r * t).exp();
    let pdf = norm_pdf(d1);
    let gamma = dq * pdf / (s * st);
    let vega = s * dq * pdf * t.sqrt() / 100.0;
    let decay = -s * dq * pdf * sigma / (2.0 * t.sqrt());
    match right {
        OptionRight::Call => {
            let (n1, n2) = (norm_cdf(d1), norm_cdf(d2));
            Bs {
                price: s * dq * n1 - k * dr * n2,
                delta: dq * n1,
                gamma,
                theta: (decay - r * k * dr * n2 + q * s * dq * n1) / 365.0,
                vega,
                rho: k * t * dr * n2 / 100.0,
            }
        }
        OptionRight::Put => {
            let (n1, n2) = (norm_cdf(-d1), norm_cdf(-d2));
            Bs {
                price: k * dr * n2 - s * dq * n1,
                delta: -dq * n1,
                gamma,
                theta: (decay + r * k * dr * n2 - q * s * dq * n1) / 365.0,
                vega,
                rho: -k * t * dr * n2 / 100.0,
            }
        }
    }
}

const STEPS: [f64; 9] = [0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0];

/// (fine increment near the money, wide increment in the wings).
pub(crate) fn strike_increments(spot: f64) -> (f64, f64) {
    let target = spot * 0.0125;
    let i = STEPS.iter().rposition(|s| *s <= target).unwrap_or(0).min(STEPS.len() - 2);
    (STEPS[i], STEPS[i + 1])
}

pub(crate) fn strikes(spot: f64) -> Vec<f64> {
    let (fine, wide) = strike_increments(spot);
    let mut v = Vec::new();
    let lo = (spot * 0.6 / wide).ceil() as i64;
    let hi = (spot * 1.4 / wide).floor() as i64;
    for i in lo..=hi {
        v.push(i as f64 * wide);
    }
    let lo = (spot * 0.9 / fine).ceil() as i64;
    let hi = (spot * 1.1 / fine).floor() as i64;
    for i in lo..=hi {
        v.push(i as f64 * fine);
    }
    v.retain(|k| *k > 0.0);
    v.sort_by(f64::total_cmp);
    v.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    v
}

fn expiry_on(cal: Calendar, friday: NaiveDate) -> NaiveDate {
    if cal.is_trading_day(friday) { friday } else { cal.before(friday) }
}

/// (expiry, is_monthly) for the next ~12 months plus six weeklies.
pub(crate) fn expiries(today: NaiveDate, weeklies: bool) -> Vec<(NaiveDate, bool)> {
    let cal = Calendar::Nyse;
    let mut v: Vec<(NaiveDate, bool)> = Vec::new();
    for m in 0..13 {
        let d = add_months(today, m);
        let e = expiry_on(cal, nth_weekday(d.year(), d.month(), Weekday::Fri, 3));
        if e >= today {
            v.push((e, true));
        }
    }
    v.retain(|(e, _)| *e <= add_months(today, 12) + Duration::days(7));
    if weeklies {
        let mut f = today;
        while f.weekday() != Weekday::Fri {
            f += Duration::days(1);
        }
        for _ in 0..6 {
            let e = expiry_on(cal, f);
            if e >= today && !v.iter().any(|(x, _)| *x == e) {
                v.push((e, false));
            }
            f += Duration::days(7);
        }
    }
    v.sort();
    v
}

fn option_tick(price: f64) -> f64 {
    if price < 3.0 { 0.01 } else { 0.05 }
}

/// Implied volatility on the mock surface: SSVI (Gatheral–Jacquier) with a
/// power-law `phi`, ATM volatility mean-reverting in maturity. The
/// parameters satisfy `eta * (1 + |rho|) <= 2`, so each slice is free of
/// butterfly (and therefore vertical-spread) arbitrage.
pub(crate) fn surface_vol(atm_lr: f64, atm_now: f64, t: f64, k: f64, f: f64, index_like: bool) -> f64 {
    let t = t.max(1e-6);
    let atm = atm_lr + (atm_now - atm_lr) * (-t / 0.25).exp();
    let theta = atm * atm * t;
    let (rho, eta) = if index_like { (-0.7, 1.1) } else { (-0.4, 1.0) };
    let phi = eta / (theta.sqrt() * (1.0 + theta).sqrt());
    let x = (k / f).ln();
    let w = theta / 2.0 * (1.0 + rho * phi * x + ((phi * x + rho).powi(2) + 1.0 - rho * rho).sqrt());
    (w / t).sqrt()
}

impl Inner {
    pub(crate) fn option_chain_for(&self, req: &ChainRequest) -> ProviderResult<OptionChain> {
        let s = self.resolve(&req.underlying)?;
        let Some(spec) = s.p.options.clone() else {
            return Err(ProviderError::NotFound(format!("no listed options for {} in the mock universe", s.key())));
        };
        let now = self.clock.now();
        let spot = self
            .last_price(&s, now)
            .ok_or_else(|| ProviderError::NotFound(format!("{} has no price yet", s.key())))?;
        let st = SessionState::at(s.p.calendar, &s.p.session, now);
        let today = st.today;
        let r = (1.0 + self.ust_yield(today, 0.25) / 100.0).ln();
        let atm_now = self.current_vol(&s, today) * 1.1;
        let atm_lr = s.p.sigma * 1.1;
        let mut exps = expiries(today, spec.weeklies);
        if let Some(want) = req.expiry {
            exps.retain(|(e, _)| *e == want);
            if exps.is_empty() {
                return Err(ProviderError::NotFound(format!("{} has no listed expiry on {want}", s.key())));
            }
        }
        let ks = strikes(spot);
        let mut contracts = Vec::with_capacity(exps.len() * ks.len() * 2);
        for (expiry, monthly) in exps {
            let t = year_fraction(now, Session::US_EQUITY.close_utc(expiry));
            if t <= 0.0 {
                continue;
            }
            let fwd = spot * ((r - spec.q) * t).exp();
            let root = if monthly { &spec.root } else { &spec.weekly_root };
            for k in &ks {
                for right in [OptionRight::Call, OptionRight::Put] {
                    contracts.push(self.contract(&s, &spec, root, expiry, monthly, *k, right, spot, fwd, t, r, atm_lr, atm_now));
                }
            }
        }
        let mut provenance = Provenance::synthetic(now);
        provenance.source_ref = Some("mock chain: Black-Scholes on a synthetic surface".into());
        Ok(OptionChain { underlying: s.key().clone(), underlying_price: Some(spot), as_of: now, contracts, provenance })
    }

    #[allow(clippy::too_many_arguments)]
    fn contract(
        &self,
        s: &Sym,
        spec: &OptSpec,
        root: &str,
        expiry: NaiveDate,
        monthly: bool,
        k: f64,
        right: OptionRight,
        spot: f64,
        fwd: f64,
        t: f64,
        r: f64,
        atm_lr: f64,
        atm_now: f64,
    ) -> OptionContract {
        let iv = surface_vol(atm_lr, atm_now, t, k, fwd, spec.index_like);
        let bs = black_scholes(right, spot, k, t, r, spec.q, iv);
        let theo = bs.price.max(0.0);
        let tick = option_tick(theo);
        let z = ((k / fwd).ln() / t.sqrt()).abs();
        let mut c = Cell::new(&[
            self.seed,
            s.hash,
            tag("option"),
            crate::daily::day_num(expiry),
            (k * 1000.0).round() as u64,
            right as u64,
        ]);
        let pct = 0.015 + 0.02 * z.min(2.0) + 0.03 / (1.0 + spec.activity);
        let spread = (theo * pct).max(tick);
        // Bid and ask bracket the model value: floor/ceil to the tick.
        let cents = |x: f64| (x * 100.0).round() / 100.0;
        let bid = cents(((theo - spread / 2.0) / tick).floor().max(0.0) * tick);
        let ask = cents((((theo + spread / 2.0) / tick).ceil() * tick).max(bid + tick));
        let round_bonus = if (k / (strike_increments(spot).1 * 2.0)).fract().abs() < 1e-9 { 1.5 } else { 1.0 };
        let oi_base = spec.activity * 20_000.0 * (-z * z / 0.8).exp() / (1.0 + 4.0 * t) * round_bonus;
        let oi = (oi_base * c.range(0.6, 1.4) * if monthly { 1.4 } else { 0.8 }).round();
        let volume = (oi * (0.05 + 0.4 / (1.0 + 30.0 * t)) * c.range(0.5, 1.5)).round();
        let last = (volume > 0.0 && theo >= 0.01).then(|| round_tick((theo * (1.0 + 0.02 * c.normal())).max(0.01), tick));
        let r4 = |x: f64| (x * 10_000.0).round() / 10_000.0;
        OptionContract {
            contract_symbol: OptionContract::occ_symbol(root, expiry, right, k),
            expiry,
            strike: k,
            right,
            style: spec.style,
            multiplier: 100.0,
            bid: Some(bid),
            ask: Some(ask),
            bid_size: Some((c.int(1, 50) * if spec.style == ExerciseStyle::European { 5 } else { 10 }) as f64),
            ask_size: Some((c.int(1, 50) * if spec.style == ExerciseStyle::European { 5 } else { 10 }) as f64),
            last,
            volume: Some(volume),
            open_interest: Some(oi),
            greeks: Some(Greeks {
                iv: Some(r4(iv)),
                delta: Some(r4(bs.delta)),
                gamma: Some((bs.gamma * 1e6).round() / 1e6),
                theta: Some(r4(bs.theta)),
                vega: Some(r4(bs.vega)),
                rho: Some(r4(bs.rho)),
                source: GreeksSource::Computed { model: "mock-bs".into() },
            }),
        }
    }

    /// Current conditional volatility (annualized) from the daily path.
    pub(crate) fn current_vol(&self, s: &Sym, today: NaiveDate) -> f64 {
        let p = self.path(s, today);
        let i = p.index_on_or_before(today).unwrap_or(0);
        p.sd.get(i).map_or(s.p.sigma, |sd| f64::from(*sd) * s.p.periods_per_year.sqrt())
    }
}

/// Year fraction (ACT/365.25) between two instants.
pub(crate) fn year_fraction(now: UnixNanos, then: UnixNanos) -> f64 {
    (then - now) as f64 / (365.25 * NANOS_PER_DAY as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn norm_cdf_reference_values() {
        assert!((norm_cdf(0.0) - 0.5).abs() < 1e-7);
        assert!((norm_cdf(1.0) - 0.841_344_746).abs() < 1e-7);
        assert!((norm_cdf(-1.96) - 0.024_997_895).abs() < 1e-7);
        assert!((norm_cdf(3.0) - 0.998_650_102).abs() < 1e-7);
    }

    #[test]
    fn black_scholes_textbook() {
        // Hull: S=42, K=40, r=10%, sigma=20%, T=0.5 → C=4.76, P=0.81.
        let c = black_scholes(OptionRight::Call, 42.0, 40.0, 0.5, 0.1, 0.0, 0.2);
        let p = black_scholes(OptionRight::Put, 42.0, 40.0, 0.5, 0.1, 0.0, 0.2);
        assert!((c.price - 4.76).abs() < 0.005, "{}", c.price);
        assert!((p.price - 0.81).abs() < 0.005, "{}", p.price);
        let parity = c.price - p.price - (42.0 - 40.0 * (-0.05f64).exp());
        assert!(parity.abs() < 1e-9);
        assert!(c.delta > 0.0 && p.delta < 0.0 && c.gamma > 0.0 && c.theta < 0.0);
    }

    #[test]
    fn strike_grid() {
        assert_eq!(strike_increments(255.0), (2.5, 5.0));
        assert_eq!(strike_increments(6850.0), (50.0, 100.0));
        let k = strikes(255.0);
        assert!(k.first().unwrap() >= &(255.0 * 0.6) && k.last().unwrap() <= &(255.0 * 1.4));
        assert!(k.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn chain_put_call_parity_within_spreads() {
        use std::sync::Arc;

        use meridian_types::{FixedClock, SecurityKey, datetime_to_nanos};

        let now = datetime_to_nanos(chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 10, 5, 18, 0, 0).unwrap());
        let p = crate::MockProvider::new(crate::MockConfig {
            seed: 2,
            clock: Arc::new(FixedClock(now)),
            ..crate::MockConfig::default()
        });
        for key in [SecurityKey::equity("AAPL"), SecurityKey::index("SPX"), SecurityKey::equity("KO")] {
            let inner = &p.inner;
            let chain = inner.option_chain_for(&ChainRequest { underlying: key.clone(), expiry: None }).unwrap();
            let s = inner.resolve(&key).unwrap();
            let q = s.p.options.as_ref().unwrap().q;
            let today = crate::cal::ymd(2026, 10, 5);
            let r = (1.0 + inner.ust_yield(today, 0.25) / 100.0).ln();
            let spot = chain.underlying_price.unwrap();
            let mut checked = 0;
            for c in chain.contracts.iter().filter(|c| c.right == OptionRight::Call) {
                let put = chain
                    .contracts
                    .iter()
                    .find(|x| x.right == OptionRight::Put && x.expiry == c.expiry && x.strike == c.strike)
                    .unwrap();
                let t = year_fraction(now, Session::US_EQUITY.close_utc(c.expiry));
                let parity = spot * (-q * t).exp() - c.strike * (-r * t).exp();
                let mid = |x: &OptionContract| f64::midpoint(x.bid.unwrap(), x.ask.unwrap());
                let spread = |x: &OptionContract| x.ask.unwrap() - x.bid.unwrap();
                let err = (mid(c) - mid(put) - parity).abs();
                let tol = f64::midpoint(spread(c), spread(put)) + 1e-9;
                assert!(err <= tol, "{key} {} K={}: err {err} > tol {tol}", c.expiry, c.strike);
                checked += 1;
            }
            assert!(checked > 500, "{key}: {checked}");
        }
    }

    #[test]
    fn expiries_include_monthlies_and_weeklies() {
        let e = expiries(crate::cal::ymd(2026, 10, 5), true);
        assert!(e.contains(&(crate::cal::ymd(2026, 10, 16), true)));
        assert!(e.contains(&(crate::cal::ymd(2026, 10, 9), false)));
        assert!(e.len() >= 16);
    }
}

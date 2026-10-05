//! Daily price paths.
//!
//! Each symbol's log price follows GBM whose shocks mix a shared factor (with
//! GARCH(1,1) volatility and rare crash jumps) and an idiosyncratic
//! GARCH(1,1) term, plus Poisson jumps. Paths start at a fixed epoch and are
//! shifted so the close on the anchor date equals the symbol's reference
//! price; history therefore never changes as the clock advances.
//!
//! Pass 1 ([`simulate`]) produces log closes and the volatility state.
//! Pass 2 ([`day_bar`]) derives OHLCV for any single day from a per-day
//! counter-based generator, so a bar never depends on the requested range.

use std::sync::Arc;

use chrono::{Duration, NaiveDate};
use meridian_types::date_to_epoch_days;
use rand::Rng;
use rand_distr::StandardNormal;

use crate::cal::{Calendar, ymd};
use crate::hash::{Cell, Fast, rng, tag};
use crate::universe::{CCYS, Factor, Model, N_FACTORS, Sym, anchor_date};

/// First day of every generated price path.
pub(crate) fn path_epoch() -> NaiveDate {
    ymd(1999, 1, 4)
}

/// First day of the shared factor paths.
fn factor_epoch() -> NaiveDate {
    ymd(1998, 1, 1)
}

// ---------------------------------------------------------------------------
// Factors
// ---------------------------------------------------------------------------

/// Shared daily factor shocks (one value per calendar day).
#[derive(Debug)]
pub(crate) struct FactorPaths {
    start: NaiveDate,
    pub until: NaiveDate,
    /// Shock including conditional volatility: `sqrt(var) * z (+ jump)`.
    shock: Vec<[f64; N_FACTORS]>,
    /// Conditional variance multiplier (long-run mean 1).
    var: Vec<[f64; N_FACTORS]>,
}

impl FactorPaths {
    pub(crate) fn compute(seed: u64, until: NaiveDate) -> FactorPaths {
        let start = factor_epoch();
        let n = (until - start).num_days().max(0) as usize + 1;
        let mut shock = vec![[0.0; N_FACTORS]; n];
        let mut var = vec![[1.0; N_FACTORS]; n];
        const ALPHA: f64 = 0.08;
        const BETA: f64 = 0.90;
        const OMEGA: f64 = 1.0 - ALPHA - BETA;
        for f in 0..N_FACTORS {
            let mut r = rng(&[seed, tag("factor"), f as u64]);
            let (jump_rate, jump_mean, jump_sd) = match f {
                0 => (0.6 / 365.0, -3.5, 1.5),
                4 => (1.0 / 365.0, -2.5, 2.0),
                _ => (0.3 / 365.0, 0.0, 3.0),
            };
            let mut v = 1.0;
            let mut e_prev: f64 = 0.0;
            for i in 0..n {
                v = OMEGA + ALPHA * e_prev.powi(2).min(25.0) + BETA * v;
                let z: f64 = r.sample(StandardNormal);
                let mut e = v.sqrt() * z;
                let u: f64 = r.random();
                if u < jump_rate {
                    let j: f64 = r.sample(StandardNormal);
                    e += jump_mean + jump_sd * j;
                }
                shock[i][f] = e;
                var[i][f] = v;
                e_prev = e;
            }
        }
        // Europe and Asia load on the US factor plus their own innovation.
        let us = Factor::UsEq as usize;
        for (i, row) in shock.iter_mut().enumerate() {
            let vr = &mut var[i];
            for (f, load) in [(Factor::Europe as usize, 0.65f64), (Factor::Asia as usize, 0.5f64)] {
                let w = (1.0 - load * load).sqrt();
                row[f] = load * row[us] + w * row[f];
                vr[f] = load * load * vr[us] + w * w * vr[f];
            }
        }
        FactorPaths { start, until, shock, var }
    }

    fn idx(&self, d: NaiveDate) -> usize {
        ((d - self.start).num_days().max(0) as usize).min(self.shock.len().saturating_sub(1))
    }

    pub(crate) fn shock(&self, f: Factor, d: NaiveDate) -> f64 {
        self.shock[self.idx(d)][f as usize]
    }

    pub(crate) fn var(&self, f: Factor, d: NaiveDate) -> f64 {
        self.var[self.idx(d)][f as usize]
    }
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// Parameters of one simulated log-price process.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GenSpec {
    pub hash: u64,
    pub ref_price: f64,
    pub mu: f64,
    pub sigma: f64,
    pub rho: f64,
    pub factor: Factor,
    pub jumps_per_year: f64,
    pub jump_sd: f64,
    pub jump_mean: f64,
    /// Mean-reversion speed of the log price (per year).
    pub kappa: f64,
    pub calendar: Calendar,
    pub periods_per_year: f64,
}

impl GenSpec {
    pub(crate) fn of(s: &Sym) -> GenSpec {
        GenSpec {
            hash: s.hash,
            ref_price: s.p.ref_price,
            mu: s.p.mu,
            sigma: s.p.sigma,
            rho: s.p.rho,
            factor: s.p.factor,
            jumps_per_year: s.p.jumps_per_year,
            jump_sd: s.p.jump_sd,
            jump_mean: s.p.jump_mean,
            kappa: s.p.kappa,
            calendar: s.p.calendar,
            periods_per_year: s.p.periods_per_year,
        }
    }

    pub(crate) fn currency(i: usize) -> GenSpec {
        let c = &CCYS[i];
        GenSpec {
            hash: crate::hash::fnv1a(format!("CCY:{}", c.code).as_bytes()),
            ref_price: c.usd_value,
            mu: 0.0,
            sigma: c.vol,
            rho: c.rho,
            factor: Factor::Usd,
            jumps_per_year: 0.3,
            jump_sd: 2.0,
            jump_mean: 0.0,
            // Currencies mean-revert (half-life ~4.6 years).
            kappa: 0.15,
            calendar: Calendar::Weekdays,
            periods_per_year: 260.0,
        }
    }
}

/// Log-close path plus the state pass 2 needs.
#[derive(Debug, Clone, Default)]
pub(crate) struct Path {
    pub days: Vec<NaiveDate>,
    pub logc: Vec<f64>,
    /// Conditional daily standard deviation of the log return.
    pub sd: Vec<f32>,
    /// Standardized shock of the day (return / sd).
    pub z: Vec<f32>,
    /// Slow-moving activity state that drives volume.
    pub act: Vec<f32>,
}

impl Path {
    fn with_capacity(n: usize) -> Path {
        Path {
            days: Vec::with_capacity(n),
            logc: Vec::with_capacity(n),
            sd: Vec::with_capacity(n),
            z: Vec::with_capacity(n),
            act: Vec::with_capacity(n),
        }
    }

    /// Index of the last day `<= d`.
    pub(crate) fn index_on_or_before(&self, d: NaiveDate) -> Option<usize> {
        let n = self.days.partition_point(|x| *x <= d);
        n.checked_sub(1)
    }

    fn anchor(&mut self, ref_price: f64) {
        if let Some(i) = self.index_on_or_before(anchor_date()) {
            let shift = ref_price.ln() - self.logc[i];
            for v in &mut self.logc {
                *v += shift;
            }
        }
    }

    fn fill_activity(&mut self) {
        self.act.clear();
        let mut a = 0.0f32;
        for z in &self.z {
            a = 0.85 * a + 0.15 * (z.abs() - 0.8);
            self.act.push(a);
        }
    }
}

/// Pass 1: simulates the log-close path from the epoch through `until`
/// (and at least through the anchor date).
pub(crate) fn simulate(seed: u64, g: &GenSpec, factors: &FactorPaths, until: NaiveDate) -> Path {
    let end = until.max(anchor_date());
    let days = g.calendar.days(path_epoch(), end);
    let n = days.len();
    let mut p = Path::with_capacity(n);
    let mut r = Fast::new(&[seed, g.hash, tag("daily")]);
    const ALPHA: f64 = 0.06;
    const BETA: f64 = 0.92;
    const OMEGA: f64 = 1.0 - ALPHA - BETA;
    let dt = 1.0 / g.periods_per_year;
    let drift = (g.mu - 0.5 * g.sigma * g.sigma) * dt;
    let sdd = g.sigma * dt.sqrt();
    let rho = g.rho.clamp(0.0, 1.0);
    let w = (1.0 - rho * rho).sqrt();
    let jump_p = g.jumps_per_year * dt;
    let mut h = 1.0;
    let mut e_prev: f64 = 0.0;
    let mut logp = 0.0;
    for d in &days {
        h = OMEGA + ALPHA * e_prev.powi(2).min(25.0) + BETA * h;
        let zi: f64 = r.sample(StandardNormal);
        let e = h.sqrt() * zi;
        let fm = factors.shock(g.factor, *d);
        let fv = factors.var(g.factor, *d);
        let mut total = rho * fm + w * e;
        let condvar = (rho * rho * fv + w * w * h).max(1e-6);
        let u: f64 = r.random();
        if u < jump_p {
            let j: f64 = r.sample(StandardNormal);
            total += g.jump_mean + g.jump_sd * j;
        }
        logp += drift + sdd * total - g.kappa * dt * logp;
        let csd = condvar.sqrt();
        p.logc.push(logp);
        p.sd.push((sdd * csd) as f32);
        p.z.push((total / csd) as f32);
        e_prev = e;
    }
    p.days = days;
    p.anchor(g.ref_price);
    p.fill_activity();
    p
}

/// Path for any symbol model (tracking and FX handled here).
pub(crate) fn symbol_path(seed: u64, syms: &[Sym], s: &Sym, factors: &FactorPaths, until: NaiveDate) -> Path {
    match s.p.model {
        Model::Gbm => simulate(seed, &GenSpec::of(s), factors, until),
        Model::Tracks { under, ratio } => {
            let mut p = symbol_path(seed, syms, &syms[under], factors, until);
            let lr = ratio.ln();
            for (i, d) in p.days.iter().enumerate() {
                // Small, non-accumulating tracking difference.
                let te = 0.0004 * Cell::new(&[seed, s.hash, tag("track"), day_num(*d)]).normal();
                p.logc[i] += lr + te;
            }
            p
        }
        Model::FxPair { base, quote } => fx_path(seed, base, quote, factors, until),
        Model::VolIndex => vol_index_path(seed, s, factors, until),
    }
}

pub(crate) fn currency_path(seed: u64, i: usize, factors: &FactorPaths, until: NaiveDate) -> Path {
    if CCYS[i].vol == 0.0 {
        let days = Calendar::Weekdays.days(path_epoch(), until.max(anchor_date()));
        let n = days.len();
        return Path { days, logc: vec![0.0; n], sd: vec![0.0; n], z: vec![0.0; n], act: vec![0.0; n] };
    }
    simulate(seed, &GenSpec::currency(i), factors, until)
}

fn fx_path(seed: u64, base: usize, quote: usize, factors: &FactorPaths, until: NaiveDate) -> Path {
    let b = currency_path(seed, base, factors, until);
    let q = currency_path(seed, quote, factors, until);
    let n = b.days.len().min(q.days.len());
    let mut p = Path::with_capacity(n);
    for i in 0..n {
        p.logc.push(b.logc[i] - q.logc[i]);
        let sd = (b.sd[i].powi(2) + q.sd[i].powi(2)).sqrt().max(1e-5);
        let r = if i == 0 { 0.0 } else { p.logc[i] - p.logc[i - 1] };
        p.sd.push(sd);
        p.z.push((r / f64::from(sd)) as f32);
    }
    p.days = b.days[..n].to_vec();
    p.fill_activity();
    p
}

fn vol_index_path(seed: u64, s: &Sym, factors: &FactorPaths, until: NaiveDate) -> Path {
    let days = s.p.calendar.days(path_epoch(), until.max(anchor_date()));
    let mut p = Path::with_capacity(days.len());
    let base = 0.16 * 1.15 * 100.0;
    let mut noise = 0.0f64;
    let mut r = Fast::new(&[seed, s.hash, tag("vix")]);
    for d in &days {
        let v = factors.var(Factor::UsEq, *d);
        let z: f64 = r.sample(StandardNormal);
        noise = 0.9 * noise + 0.06 * z;
        let level = (base * v.sqrt() * noise.exp()).max(9.0);
        let prev = p.logc.last().copied().unwrap_or(level.ln());
        p.logc.push(level.ln());
        p.sd.push(0.06);
        p.z.push(((level.ln() - prev) / 0.06) as f32);
    }
    p.days = days;
    p.fill_activity();
    p
}

pub(crate) fn day_num(d: NaiveDate) -> u64 {
    date_to_epoch_days(d) as u32 as u64
}

/// One full-day bar in path terms (split-adjusted as of the anchor,
/// unrounded).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DayBar {
    pub date: NaiveDate,
    pub o: f64,
    pub h: f64,
    pub l: f64,
    pub c: f64,
    pub v: f64,
}

/// How pass 2 shapes a bar.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BarStyle {
    pub hash: u64,
    /// Continuous markets (FX, crypto) open at the previous close.
    pub continuous: bool,
    pub adv: f64,
}

impl BarStyle {
    pub(crate) fn of(s: &Sym) -> BarStyle {
        BarStyle {
            hash: s.hash,
            continuous: matches!(s.p.kind, crate::universe::Kind::Fx | crate::universe::Kind::Crypto),
            adv: s.p.adv,
        }
    }
}

/// Pass 2: OHLCV for day `i` of a path.
pub(crate) fn day_bar(seed: u64, s: &Sym, p: &Path, i: usize) -> DayBar {
    day_bar_styled(seed, BarStyle::of(s), p, i)
}

pub(crate) fn day_bar_styled(seed: u64, style: BarStyle, p: &Path, i: usize) -> DayBar {
    let d = p.days[i];
    let c = p.logc[i];
    let sd = f64::from(p.sd[i]).max(1e-5);
    let cp = if i > 0 { p.logc[i - 1] } else { c - sd * f64::from(p.z[i]) };
    let mut cell = Cell::new(&[seed, style.hash, tag("ohlc"), day_num(d)]);
    let (w, gap) = if style.continuous { (0.0, 0.02) } else { (cell.range(0.0, 0.6), 0.15) };
    let o = cp + w * (c - cp) + gap * sd * cell.normal();
    let e1 = -(1.0 - cell.u01()).ln();
    let e2 = -(1.0 - cell.u01()).ln();
    let h = o.max(c) + sd * 0.45 * e1;
    let l = o.min(c) - sd * 0.45 * e2;
    let vn = cell.normal();
    let v = if style.adv > 0.0 {
        let a = f64::from(p.act[i]);
        let z = f64::from(p.z[i]).abs();
        style.adv * (0.6 * a + 0.3 * (z - 0.8) + 0.22 * vn - 0.03).exp()
    } else {
        0.0
    };
    DayBar { date: d, o: o.exp(), h: h.exp(), l: l.exp(), c: c.exp(), v }
}

/// Full-day bars for trading days in `[from, to]` in path terms. Uses
/// `path` when provided (it must cover `to`).
pub(crate) fn day_bars_from_path(seed: u64, s: &Sym, p: &Path, from: NaiveDate, to: NaiveDate) -> Vec<DayBar> {
    let start = p.days.partition_point(|d| *d < from);
    let end = p.days.partition_point(|d| *d <= to);
    (start..end).map(|i| day_bar(seed, s, p, i)).collect()
}

/// Scales a tracker's bars from its underlying (identical OHLC shape).
pub(crate) fn tracker_bars(seed: u64, s: &Sym, under: &Sym, under_bars: &[DayBar], ratio: f64) -> Vec<DayBar> {
    under_bars
        .iter()
        .map(|b| {
            let te = 0.0004 * Cell::new(&[seed, s.hash, tag("track"), day_num(b.date)]).normal();
            let k = ratio * te.exp();
            let vol_ratio = if under.p.adv > 0.0 { b.v / under.p.adv } else { 1.0 };
            let vn = Cell::new(&[seed, s.hash, tag("trackvol"), day_num(b.date)]).normal();
            DayBar {
                date: b.date,
                o: b.o * k,
                h: b.h * k,
                l: b.l * k,
                c: b.c * k,
                v: s.p.adv * vol_ratio * (0.2 * vn - 0.02).exp(),
            }
        })
        .collect()
}

/// Shared factor paths, extended on demand.
pub(crate) fn ensure_factors(
    cache: &parking_lot::RwLock<Option<Arc<FactorPaths>>>,
    seed: u64,
    until: NaiveDate,
) -> Arc<FactorPaths> {
    if let Some(f) = cache.read().as_ref()
        && f.until >= until
    {
        return f.clone();
    }
    let mut w = cache.write();
    if let Some(f) = w.as_ref()
        && f.until >= until
    {
        return f.clone();
    }
    let target = until.max(anchor_date()) + Duration::days(400);
    let f = Arc::new(FactorPaths::compute(seed, target));
    *w = Some(f.clone());
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::universe::Universe;

    #[test]
    fn path_is_anchored_and_stable() {
        let u = Universe::build(0);
        let s = &u.syms[u.by_ticker("AAPL", meridian_types::MarketSector::Equity).unwrap()];
        let f = FactorPaths::compute(7, ymd(2030, 1, 1));
        let a = symbol_path(7, &u.syms, s, &f, ymd(2026, 6, 1));
        let b = symbol_path(7, &u.syms, s, &f, ymd(2027, 6, 1));
        let i = a.index_on_or_before(anchor_date()).unwrap();
        assert!((a.logc[i].exp() - 255.0).abs() < 1e-6);
        // Extending the horizon never changes earlier days.
        for k in 0..a.days.len() {
            assert_eq!(a.days[k], b.days[k]);
            assert_eq!(a.logc[k], b.logc[k]);
        }
    }

    #[test]
    fn day_bars_are_consistent() {
        let u = Universe::build(0);
        let f = FactorPaths::compute(1, ymd(2027, 1, 1));
        for s in u.syms.iter().take(40) {
            let p = symbol_path(1, &u.syms, s, &f, ymd(2026, 10, 5));
            for i in (0..p.days.len()).step_by(37) {
                let b = day_bar(1, s, &p, i);
                assert!(b.h >= b.o.max(b.c) && b.l <= b.o.min(b.c) && b.l > 0.0, "{} {:?}", s.inst.key, b);
            }
        }
    }
}

//! Synthetic financial statements with exact accounting identities.
//!
//! A company is simulated quarter by quarter from 2003 with sector-typical
//! growth, margins, working-capital days, capex, debt policy, dividends (the
//! same schedule `dividends()` reports) and buybacks. The balance sheet rolls
//! forward from the cash-flow statement, so assets equal liabilities plus
//! equity exactly: every amount is rounded to whole reporting units before
//! subtotals are formed, and sums of integer-valued `f64`s are exact.
//! Annual statements are sums of their four fiscal quarters (flows) and the
//! fourth quarter's balance sheet.

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use meridian_types::{
    EarningsEvent, EarningsHistory, EarningsRecord, EarningsSession, Estimate, EstimateMetric, Estimates,
    Fundamentals, PeriodType, Provenance, Statement, StatementKind, StatementLine, UnixNanos,
};

use crate::Inner;
use crate::cal::{add_months, month_end, ymd};
use crate::hash::{Cell, tag};
use crate::market::{split_factor_after, splits_of};
use crate::universe::{Sector, Sym, anchor_date};

#[derive(Debug, Clone, Copy)]
pub(crate) struct SectorProfile {
    pub ps: f64,
    pub gm: f64,
    pub sga: f64,
    pub rnd: f64,
    pub capex: f64,
    pub tax: f64,
    pub growth: f64,
    pub growth_sd: f64,
    pub dso: f64,
    pub dio: f64,
    pub dpo: f64,
    pub goodwill: f64,
    pub debt: f64,
    pub cash: f64,
    pub sti: f64,
    pub sbc: f64,
    pub season: [f64; 4],
    pub rev_per_employee: f64,
}

pub(crate) fn profile(s: Sector) -> SectorProfile {
    let flat = [1.0, 1.0, 1.0, 1.0];
    let retail = [0.94, 0.98, 0.97, 1.11];
    let tech = [0.96, 0.97, 1.0, 1.07];
    match s {
        Sector::Tech => SectorProfile { ps: 9.0, gm: 0.62, sga: 0.15, rnd: 0.14, capex: 0.06, tax: 0.16, growth: 0.10, growth_sd: 0.08, dso: 50.0, dio: 35.0, dpo: 60.0, goodwill: 0.15, debt: 0.25, cash: 0.20, sti: 0.15, sbc: 0.05, season: tech, rev_per_employee: 900_000.0 },
        Sector::Comm => SectorProfile { ps: 4.5, gm: 0.55, sga: 0.18, rnd: 0.10, capex: 0.14, tax: 0.18, growth: 0.07, growth_sd: 0.06, dso: 50.0, dio: 5.0, dpo: 45.0, goodwill: 0.40, debt: 0.60, cash: 0.10, sti: 0.05, sbc: 0.04, season: tech, rev_per_employee: 1_200_000.0 },
        Sector::Disc => SectorProfile { ps: 2.2, gm: 0.38, sga: 0.22, rnd: 0.03, capex: 0.05, tax: 0.21, growth: 0.07, growth_sd: 0.07, dso: 15.0, dio: 60.0, dpo: 50.0, goodwill: 0.08, debt: 0.35, cash: 0.08, sti: 0.03, sbc: 0.012, season: retail, rev_per_employee: 450_000.0 },
        Sector::Staples => SectorProfile { ps: 2.2, gm: 0.40, sga: 0.24, rnd: 0.01, capex: 0.03, tax: 0.21, growth: 0.04, growth_sd: 0.03, dso: 20.0, dio: 45.0, dpo: 50.0, goodwill: 0.30, debt: 0.35, cash: 0.06, sti: 0.02, sbc: 0.008, season: retail, rev_per_employee: 350_000.0 },
        Sector::Health => SectorProfile { ps: 4.5, gm: 0.65, sga: 0.25, rnd: 0.15, capex: 0.04, tax: 0.15, growth: 0.06, growth_sd: 0.05, dso: 55.0, dio: 90.0, dpo: 45.0, goodwill: 0.50, debt: 0.50, cash: 0.10, sti: 0.05, sbc: 0.015, season: flat, rev_per_employee: 600_000.0 },
        Sector::Fin => SectorProfile { ps: 3.0, gm: 0.80, sga: 0.40, rnd: 0.0, capex: 0.02, tax: 0.20, growth: 0.05, growth_sd: 0.06, dso: 30.0, dio: 0.0, dpo: 30.0, goodwill: 0.10, debt: 0.80, cash: 0.25, sti: 0.20, sbc: 0.015, season: flat, rev_per_employee: 700_000.0 },
        Sector::Ind => SectorProfile { ps: 2.5, gm: 0.30, sga: 0.12, rnd: 0.03, capex: 0.03, tax: 0.20, growth: 0.04, growth_sd: 0.06, dso: 60.0, dio: 80.0, dpo: 55.0, goodwill: 0.40, debt: 0.40, cash: 0.08, sti: 0.02, sbc: 0.01, season: [0.95, 1.01, 0.99, 1.05], rev_per_employee: 400_000.0 },
        Sector::Energy => SectorProfile { ps: 1.4, gm: 0.22, sga: 0.06, rnd: 0.005, capex: 0.11, tax: 0.23, growth: 0.03, growth_sd: 0.15, dso: 35.0, dio: 20.0, dpo: 40.0, goodwill: 0.02, debt: 0.25, cash: 0.08, sti: 0.01, sbc: 0.005, season: flat, rev_per_employee: 3_000_000.0 },
        Sector::Util => SectorProfile { ps: 2.8, gm: 0.45, sga: 0.20, rnd: 0.0, capex: 0.30, tax: 0.15, growth: 0.04, growth_sd: 0.03, dso: 40.0, dio: 25.0, dpo: 50.0, goodwill: 0.20, debt: 2.0, cash: 0.03, sti: 0.0, sbc: 0.005, season: [1.04, 0.94, 1.08, 0.94], rev_per_employee: 1_000_000.0 },
        Sector::RealEstate => SectorProfile { ps: 9.0, gm: 0.70, sga: 0.25, rnd: 0.0, capex: 0.20, tax: 0.05, growth: 0.06, growth_sd: 0.04, dso: 20.0, dio: 0.0, dpo: 40.0, goodwill: 0.10, debt: 3.0, cash: 0.05, sti: 0.0, sbc: 0.01, season: flat, rev_per_employee: 1_500_000.0 },
        Sector::Materials => SectorProfile { ps: 2.5, gm: 0.32, sga: 0.10, rnd: 0.02, capex: 0.08, tax: 0.21, growth: 0.03, growth_sd: 0.09, dso: 45.0, dio: 70.0, dpo: 50.0, goodwill: 0.30, debt: 0.50, cash: 0.08, sti: 0.02, sbc: 0.008, season: flat, rev_per_employee: 800_000.0 },
    }
}

/// One simulated fiscal quarter. Amounts are in path-terms shares (split
/// adjusted as of the anchor) and reporting units.
#[derive(Debug, Clone, Default)]
pub(crate) struct Quarter {
    pub fy: i32,
    pub fq: u32,
    pub end: NaiveDate,
    pub announce: NaiveDate,
    // Income
    pub revenue: f64,
    pub cogs: f64,
    pub gross: f64,
    pub sga: f64,
    pub rnd: f64,
    pub op: f64,
    pub da: f64,
    pub interest: f64,
    pub pretax: f64,
    pub tax: f64,
    pub ni: f64,
    pub ebitda: f64,
    pub shares_basic: f64,
    pub shares_diluted: f64,
    // Balance
    pub cash: f64,
    pub sti: f64,
    pub ar: f64,
    pub inv: f64,
    pub oca: f64,
    pub ppe: f64,
    pub gw: f64,
    pub onca: f64,
    pub ap: f64,
    pub std: f64,
    pub ocl: f64,
    pub ltd: f64,
    pub oncl: f64,
    pub eq: f64,
    // Cash flow
    pub sbc: f64,
    pub dwc: f64,
    pub capex: f64,
    pub d_sti: f64,
    pub acq: f64,
    pub other_inv: f64,
    pub debt_net: f64,
    pub div: f64,
    pub buyback: f64,
}

impl Quarter {
    pub(crate) fn tca(&self) -> f64 {
        self.cash + self.sti + self.ar + self.inv + self.oca
    }
    pub(crate) fn ta(&self) -> f64 {
        self.tca() + self.ppe + self.gw + self.onca
    }
    pub(crate) fn tcl(&self) -> f64 {
        self.ap + self.std + self.ocl
    }
    pub(crate) fn tl(&self) -> f64 {
        self.tcl() + self.ltd + self.oncl
    }
    pub(crate) fn cfo(&self) -> f64 {
        self.ni + self.da + self.sbc + self.dwc
    }
    pub(crate) fn cfi(&self) -> f64 {
        -self.capex - self.d_sti - self.acq + self.other_inv
    }
    pub(crate) fn cff(&self) -> f64 {
        self.debt_net - self.div - self.buyback
    }
    pub(crate) fn label(&self) -> String {
        format!("Q{} {:02}", self.fq, self.fy.rem_euclid(100))
    }
}

/// Fiscal (year, quarter) of a fiscal quarter ending in calendar month `m`
/// of year `y`, for a fiscal year ending in month `fye`.
pub(crate) fn fiscal_of(y: i32, m: u32, fye: u32) -> (i32, u32) {
    let q = ((m as i32 - fye as i32 - 1).rem_euclid(12) / 3 + 1) as u32;
    let fy = if m <= fye { y } else { y + 1 };
    (fy, q)
}

pub(crate) fn fy_label(fy: i32) -> String {
    format!("FY {:02}", fy.rem_euclid(100))
}

/// Unit all amounts are rounded to: millions for large companies,
/// thousands otherwise.
fn unit_for(revenue_ref: f64) -> f64 {
    if revenue_ref >= 2e9 { 1e6 } else { 1e3 }
}

fn announce_for(seed: u64, s: &Sym, end: NaiveDate, fq: u32) -> NaiveDate {
    let mut c = Cell::new(&[seed, s.hash, tag("announce"), crate::daily::day_num(end)]);
    let lag = if fq == 4 { c.int(28, 45) } else { c.int(21, 35) };
    let mut d = end + Duration::days(lag);
    // Earnings come out Tuesday–Thursday.
    while !matches!(d.weekday(), Weekday::Tue | Weekday::Wed | Weekday::Thu) {
        d += Duration::days(1);
    }
    d
}

pub(crate) struct CompanyModel {
    pub quarters: Vec<Quarter>,
    pub unit: f64,
    pub revenue_ref: f64,
    pub n_analysts: u32,
}

impl CompanyModel {
    pub(crate) fn reported(&self, today: NaiveDate) -> usize {
        self.quarters.iter().take_while(|q| q.announce <= today).count()
    }
}

pub(crate) fn n_analysts(s: &Sym) -> u32 {
    if s.p.filler {
        return 3 + (s.hash % 6) as u32;
    }
    let m = s.anchor_mcap();
    (12.0 + 30.0 * (m / 3e12).powf(0.3)).round().clamp(5.0, 55.0) as u32
}

impl Inner {
    /// Simulates a company's quarters from 2003 through ~3 years past `today`.
    pub(crate) fn company(&self, s: &Sym, today: NaiveDate) -> Option<CompanyModel> {
        let sector = s.p.sector?;
        let sp = profile(sector);
        let seed = self.seed;
        let mut jc = Cell::new(&[seed, s.hash, tag("company")]);
        let jit = |c: &mut Cell, x: f64, w: f64| x * c.range(1.0 - w, 1.0 + w);
        let growth_premium = 1.0 + 3.0 * (sp.growth + (s.p.mu - 0.10) * 0.8 - sp.growth).max(0.0);
        let ps = jit(&mut jc, sp.ps, 0.3) * growth_premium;
        let gm = jit(&mut jc, sp.gm, 0.12).clamp(0.08, 0.92);
        let sga = jit(&mut jc, sp.sga, 0.15);
        let rnd = jit(&mut jc, sp.rnd, 0.2);
        let capex_f = jit(&mut jc, sp.capex, 0.25);
        let tax_rate = jit(&mut jc, sp.tax, 0.15);
        let growth = (sp.growth + (s.p.mu - 0.10) * 0.8).clamp(-0.02, 0.40);
        let revenue_ref = s.anchor_mcap() / ps;
        let unit = unit_for(revenue_ref);
        let ru = |x: f64| (x / unit).round() * unit;
        let fye = s.p.fye_month.clamp(1, 12);

        // Fiscal quarter ends from 2003 to today + 3y.
        let mut ends = Vec::new();
        let mut m = fye;
        let mut y = 2002;
        loop {
            let e = month_end(y, m);
            if e >= ymd(2003, 1, 1) {
                ends.push(e);
            }
            if e > today + Duration::days(3 * 366) {
                break;
            }
            let n = add_months(ymd(y, m, 1), 3);
            y = n.year();
            m = n.month();
        }

        // Annual growth by fiscal year (AR(1)), and the revenue scale that
        // pins the fiscal year ending in 2025 to `revenue_ref`.
        let fy_of = |e: &NaiveDate| fiscal_of(e.year(), e.month(), fye);
        let first_fy = ends.first().map_or(2003, |e| fy_of(e).0);
        let last_fy = ends.last().map_or(2030, |e| fy_of(e).0);
        let mut g_by_fy = Vec::new();
        let mut g_prev = growth;
        for fy in first_fy..=last_fy {
            let z = Cell::new(&[seed, s.hash, tag("growth"), fy as u64]).normal();
            let g = (growth + 0.5 * (g_prev - growth) + sp.growth_sd * z).clamp(-0.35, 0.9);
            g_by_fy.push(g);
            g_prev = g;
        }
        let g_at = |fy: i32| g_by_fy[(fy - first_fy).clamp(0, g_by_fy.len() as i32 - 1) as usize];
        let ref_fy = fiscal_of(2025, fye, fye).0;
        let mut level = 1.0;
        let mut level_ref = 1.0;
        let mut levels = Vec::new();
        for fy in first_fy..=last_fy {
            if fy > first_fy {
                level *= 1.0 + g_at(fy);
            }
            if fy == ref_fy {
                level_ref = level;
            }
            levels.push(level);
        }
        let scale = revenue_ref / level_ref;
        let annual_rev = |fy: i32| levels[(fy - first_fy).clamp(0, levels.len() as i32 - 1) as usize] * scale;

        // Dividends per share (path terms) by quarter.
        let divs = self.dividend_events(s, today + Duration::days(3 * 366));

        let years_from_anchor = |d: NaiveDate| (d - anchor_date()).num_days() as f64 / 365.25;
        let share_drift = if s.p.div.is_some() && gm > 0.4 { -0.02 } else if growth > 0.15 { 0.012 } else { -0.005 };
        let buyback_f = if s.p.div.is_some() { 0.35 } else { 0.15 };

        let mut quarters: Vec<Quarter> = Vec::with_capacity(ends.len());
        let mut m_state = 0.0;
        let mut prev_end = ends.first().map_or(ymd(2003, 1, 1), |e| *e - Duration::days(91));
        for (qi, end) in ends.iter().enumerate() {
            let (fy, fq) = fy_of(end);
            let mut c = Cell::new(&[seed, s.hash, tag("quarter"), crate::daily::day_num(*end)]);
            let g = g_at(fy);
            let cal_q = ((end.month() - 1) / 3) as usize;
            let rev_q = ru(annual_rev(fy) / 4.0 * (1.0 + g).powf((f64::from(fq) - 2.5) / 4.0) * sp.season[cal_q] * (1.0 + 0.012 * c.normal()));
            let rev_q = rev_q.max(unit);
            m_state = 0.85 * m_state + 0.006 * c.normal();
            let gm_q = (gm + m_state + 0.08 * (g - growth)).clamp(0.05, 0.95);
            let cogs = ru(rev_q * (1.0 - gm_q));
            let gross = rev_q - cogs;
            let sga_q = ru(rev_q * sga * (1.0 + 0.03 * c.normal()));
            let rnd_q = ru(rev_q * rnd * (1.0 + 0.02 * c.normal()));
            let op = gross - sga_q - rnd_q;
            let run = 4.0 * rev_q;

            let prev = quarters.last().cloned().unwrap_or_else(|| {
                // Opening balance sheet; equity is the plug.
                let mut q0 = Quarter {
                    cash: ru(sp.cash * run),
                    sti: ru(sp.sti * run),
                    ar: ru(run * sp.dso / 365.0),
                    inv: ru(cogs * 4.0 * sp.dio / 365.0),
                    oca: ru(0.03 * run),
                    ppe: ru(capex_f * run * 8.0),
                    gw: ru(sp.goodwill * run),
                    onca: ru(0.05 * run),
                    ap: ru(cogs * 4.0 * sp.dpo / 365.0),
                    ocl: ru(0.08 * run),
                    oncl: ru(0.06 * run),
                    ..Quarter::default()
                };
                let debt = ru(sp.debt * run);
                q0.std = ru(0.1 * debt);
                q0.ltd = debt - q0.std;
                q0.eq = q0.ta() - q0.tl();
                q0
            });

            let da = ru(prev.ppe / 32.0);
            let interest = ru((prev.std + prev.ltd) * 0.045 / 4.0).max(0.0);
            let pretax = op - interest;
            let tax = ru(pretax * tax_rate);
            let ni = pretax - tax;
            let shares_diluted = ((s.p.shares * (share_drift * years_from_anchor(*end)).exp() * (1.0 + 0.002 * c.normal())) / 1e5).round() * 1e5;
            let shares_basic = ((shares_diluted * 0.99) / 1e5).round() * 1e5;

            let mut q = Quarter {
                fy,
                fq,
                end: *end,
                announce: announce_for(seed, s, *end, fq),
                revenue: rev_q,
                cogs,
                gross,
                sga: sga_q,
                rnd: rnd_q,
                op,
                da,
                interest,
                pretax,
                tax,
                ni,
                ebitda: op + da,
                shares_basic,
                shares_diluted,
                ..Quarter::default()
            };

            // Working capital and other balances follow revenue.
            q.ar = ru(run * sp.dso / 365.0);
            q.inv = ru(cogs * 4.0 * sp.dio / 365.0);
            q.ap = ru(cogs * 4.0 * sp.dpo / 365.0);
            q.oca = ru(0.03 * run);
            q.ocl = ru(0.08 * run);
            q.oncl = ru(0.06 * run);
            let onca = ru(0.05 * run);
            q.sti = ru(sp.sti * run);
            q.sbc = ru(sp.sbc * rev_q);
            q.capex = ru(capex_f * rev_q * (1.0 + 0.1 * c.normal())).max(0.0);
            q.ppe = prev.ppe + q.capex - da;
            q.acq = if qi > 0 && c.chance(0.03) { ru(0.15 * run) } else { 0.0 };
            q.gw = prev.gw + q.acq;
            q.other_inv = -(onca - prev.onca);
            q.onca = onca;
            q.d_sti = q.sti - prev.sti;
            q.dwc = -(q.ar - prev.ar) - (q.inv - prev.inv) - (q.oca - prev.oca)
                + (q.ap - prev.ap)
                + (q.ocl - prev.ocl)
                + (q.oncl - prev.oncl);

            let dps: f64 = divs.iter().filter(|d| d.ex > prev_end && d.ex <= *end).map(|d| d.amount_path).sum();
            q.div = ru(dps * shares_basic);
            let mut buyback = ru(buyback_f * ni.max(0.0));
            let debt_prev = prev.std + prev.ltd;
            let mut debt_net = ru(0.1 * (sp.debt * run - debt_prev));
            let cfo = q.cfo();
            let cfi = q.cfi();
            let provisional = prev.cash + cfo + cfi + debt_net - q.div - buyback;
            let target_cash = sp.cash * run;
            if provisional < 0.5 * target_cash {
                debt_net += ru(0.5 * target_cash - provisional + unit);
            } else if provisional > 2.5 * target_cash {
                buyback += ru(0.25 * (provisional - 2.5 * target_cash));
            }
            q.buyback = buyback;
            q.debt_net = debt_net;
            q.cash = prev.cash + cfo + cfi + q.cff();
            let debt = debt_prev + debt_net;
            q.std = ru(0.1 * debt);
            q.ltd = debt - q.std;
            q.eq = prev.eq + ni + q.sbc - q.div - q.buyback;
            prev_end = *end;
            quarters.push(q);
        }
        Some(CompanyModel { quarters, unit, revenue_ref, n_analysts: n_analysts(s) })
    }

    pub(crate) fn fundamentals_for(
        &self,
        s: &Sym,
        period_type: PeriodType,
        periods: usize,
        now: UnixNanos,
    ) -> Option<Fundamentals> {
        let today = meridian_types::nanos_to_date(now);
        let model = self.company(s, today)?;
        let k = split_factor_after(&splits_of(s), today);
        let reported = &model.quarters[..model.reported(today)];
        let mut statements = Vec::new();
        let mut periods_out: Vec<Period> = Vec::new();
        match period_type {
            PeriodType::Quarterly => {
                let n = if periods == 0 { 40 } else { periods };
                for q in reported.iter().rev().take(n).rev() {
                    periods_out.push(Period::single(q, PeriodType::Quarterly, format!("Q{}", q.fq)));
                }
            }
            PeriodType::Annual => {
                let n = if periods == 0 { 10 } else { periods };
                let mut years: Vec<Period> = Vec::new();
                let mut i = 0;
                while i + 3 < reported.len() {
                    let w = &reported[i..i + 4];
                    if w[0].fq == 1 && w[3].fq == 4 {
                        years.push(Period::sum(w, PeriodType::Annual, "FY".into()));
                        i += 4;
                    } else {
                        i += 1;
                    }
                }
                periods_out.extend(years.into_iter().rev().take(n).rev());
            }
            PeriodType::Ttm => {
                let n = if periods == 0 { 1 } else { periods };
                let mut ttm: Vec<Period> = Vec::new();
                for end in (4..=reported.len()).rev().take(n) {
                    ttm.push(Period::sum(&reported[end - 4..end], PeriodType::Ttm, "TTM".into()));
                }
                ttm.reverse();
                periods_out.extend(ttm);
            }
        }
        for kind in [StatementKind::Income, StatementKind::Balance, StatementKind::CashFlow] {
            for p in &periods_out {
                statements.push(p.statement(kind, k));
            }
        }
        let mut provenance = Provenance::synthetic(now);
        provenance.source_ref = Some("mock fundamentals (synthetic)".into());
        Some(Fundamentals { key: s.key().clone(), statements, reported_splits: Vec::new(), provenance })
    }

    pub(crate) fn estimates_for(&self, s: &Sym, now: UnixNanos) -> Option<Estimates> {
        let today = meridian_types::nanos_to_date(now);
        let model = self.company(s, today)?;
        let k = split_factor_after(&splits_of(s), today);
        let r = model.reported(today);
        let future = &model.quarters[r..];
        let mut out = Vec::new();
        for (h, q) in future.iter().take(4).enumerate() {
            let p = Period::single(q, PeriodType::Quarterly, format!("Q{}", q.fq));
            for metric in [EstimateMetric::Eps, EstimateMetric::Revenue, EstimateMetric::Ebitda] {
                out.push(self.estimate(s, &model, &p, metric, h as f64, k, q.label()));
            }
        }
        // Current fiscal year (first unreported FY) and the next one.
        let Some(first) = future.first() else {
            return Some(Estimates { key: s.key().clone(), estimates: out, provenance: Provenance::synthetic(now) });
        };
        for (h, fy) in [first.fy, first.fy + 1].into_iter().enumerate() {
            let qs: Vec<Quarter> = model.quarters.iter().filter(|q| q.fy == fy).cloned().collect();
            if qs.len() != 4 {
                continue;
            }
            let p = Period::sum(&qs, PeriodType::Annual, "FY".into());
            for metric in [EstimateMetric::Eps, EstimateMetric::Revenue, EstimateMetric::Ebitda] {
                out.push(self.estimate(s, &model, &p, metric, 2.0 + 2.0 * h as f64, k, fy_label(fy)));
            }
        }
        let mut provenance = Provenance::synthetic(now);
        provenance.source_ref = Some("mock consensus (synthetic)".into());
        Some(Estimates { key: s.key().clone(), estimates: out, provenance })
    }

    #[allow(clippy::too_many_arguments)]
    fn estimate(
        &self,
        s: &Sym,
        model: &CompanyModel,
        p: &Period,
        metric: EstimateMetric,
        horizon: f64,
        k: f64,
        label: String,
    ) -> Estimate {
        let truth = match metric {
            EstimateMetric::Eps => p.eps_diluted() * k,
            EstimateMetric::Revenue => p.revenue,
            EstimateMetric::Ebitda => p.ebitda,
            EstimateMetric::NetIncome => p.ni,
        };
        let mut c = Cell::new(&[self.seed, s.hash, tag("estimate"), crate::daily::day_num(p.end), metric as u64]);
        let mean = truth * (1.0 - 0.02 + (0.02 + 0.01 * horizon) * c.normal());
        let disp = 0.04 + 0.02 * horizon;
        let round = match metric {
            EstimateMetric::Eps => 0.01,
            _ => model.unit,
        };
        let r = |x: f64| (x / round).round() * round;
        let spread = mean.abs() * disp;
        let count = (f64::from(model.n_analysts) * c.range(0.7, 1.0)).round().max(1.0) as u32;
        Estimate {
            metric,
            period_type: p.kind,
            fiscal_label: label,
            period_end: p.end,
            mean: Some(r(mean)),
            median: Some(r(mean + spread * 0.1 * c.normal())),
            high: Some(r(mean + spread * c.range(0.8, 1.4))),
            low: Some(r(mean - spread * c.range(0.8, 1.4))),
            count: Some(count),
            actual: None,
        }
    }

    pub(crate) fn earnings_for(&self, s: &Sym, now: UnixNanos) -> Option<EarningsHistory> {
        let today = meridian_types::nanos_to_date(now);
        let model = self.company(s, today)?;
        let k = split_factor_after(&splits_of(s), today);
        let r = model.reported(today);
        let mut records = Vec::new();
        for q in model.quarters[..r].iter().rev().take(12).rev() {
            let p = Period::single(q, PeriodType::Quarterly, format!("Q{}", q.fq));
            let mut c = Cell::new(&[self.seed, s.hash, tag("surprise"), crate::daily::day_num(q.end)]);
            let eps = round2(p.eps_diluted() * k);
            let s_eps = (0.03 + 0.06 * c.normal()).clamp(-0.4, 0.4);
            let s_rev = (0.008 + 0.015 * c.normal()).clamp(-0.1, 0.1);
            let rev_est = ((q.revenue / (1.0 + s_rev)) / model.unit).round() * model.unit;
            records.push(EarningsRecord {
                fiscal_label: q.label(),
                period_end: q.end,
                announce_date: Some(q.announce),
                eps_actual: Some(eps),
                eps_estimate: Some(round2(eps / (1.0 + s_eps))),
                revenue_actual: Some(q.revenue),
                revenue_estimate: Some(rev_est),
            });
        }
        if let Some(q) = model.quarters.get(r) {
            let p = Period::single(q, PeriodType::Quarterly, format!("Q{}", q.fq));
            let e = self.estimate(s, &model, &p, EstimateMetric::Eps, 0.0, k, q.label());
            let rv = self.estimate(s, &model, &p, EstimateMetric::Revenue, 0.0, k, q.label());
            records.push(EarningsRecord {
                fiscal_label: q.label(),
                period_end: q.end,
                announce_date: Some(q.announce),
                eps_actual: None,
                eps_estimate: e.mean,
                revenue_actual: None,
                revenue_estimate: rv.mean,
            });
        }
        Some(EarningsHistory { key: s.key().clone(), records, provenance: Provenance::synthetic(now) })
    }
}

impl Inner {
    /// Earnings releases dated in `[from, to]`: reported ones with actual
    /// and estimate (as in `earnings_for`), upcoming ones with the
    /// consensus EPS estimate. The session (before open / after close) is a
    /// fixed per-company habit.
    pub(crate) fn earnings_events_for(&self, s: &Sym, from: NaiveDate, to: NaiveDate, now: UnixNanos) -> Vec<EarningsEvent> {
        let today = meridian_types::nanos_to_date(now);
        let Some(model) = self.company(s, today) else { return Vec::new() };
        let k = split_factor_after(&splits_of(s), today);
        let session = if Cell::new(&[self.seed, s.hash, tag("session")]).u01() < 0.55 {
            EarningsSession::AfterClose
        } else {
            EarningsSession::BeforeOpen
        };
        let mut out = Vec::new();
        for q in model.quarters.iter().filter(|q| q.announce >= from && q.announce <= to) {
            let p = Period::single(q, PeriodType::Quarterly, format!("Q{}", q.fq));
            let (eps_actual, eps_estimate, revenue_actual, revenue_estimate) = if q.announce <= today {
                let mut c = Cell::new(&[self.seed, s.hash, tag("surprise"), crate::daily::day_num(q.end)]);
                let eps = round2(p.eps_diluted() * k);
                let s_eps = (0.03 + 0.06 * c.normal()).clamp(-0.4, 0.4);
                let s_rev = (0.008 + 0.015 * c.normal()).clamp(-0.1, 0.1);
                let rev_est = ((q.revenue / (1.0 + s_rev)) / model.unit).round() * model.unit;
                (Some(eps), Some(round2(eps / (1.0 + s_eps))), Some(q.revenue), Some(rev_est))
            } else {
                let e = self.estimate(s, &model, &p, EstimateMetric::Eps, 0.0, k, q.label());
                let rv = self.estimate(s, &model, &p, EstimateMetric::Revenue, 0.0, k, q.label());
                (None, e.mean, None, rv.mean)
            };
            out.push(EarningsEvent {
                key: s.key().clone(),
                date: q.announce,
                session: Some(session),
                fiscal_year: Some(q.fy),
                fiscal_quarter: Some(q.fq),
                eps_estimate,
                eps_actual,
                revenue_estimate,
                revenue_actual,
            });
        }
        out
    }
}

pub(crate) fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// A reporting period (one quarter or a sum of quarters).
#[derive(Debug, Clone)]
pub(crate) struct Period {
    pub kind: PeriodType,
    pub fiscal_year: i32,
    pub label: String,
    pub end: NaiveDate,
    pub revenue: f64,
    pub cogs: f64,
    pub gross: f64,
    pub sga: f64,
    pub rnd: f64,
    pub op: f64,
    pub da: f64,
    pub interest: f64,
    pub pretax: f64,
    pub tax: f64,
    pub ni: f64,
    pub ebitda: f64,
    pub shares_basic: f64,
    pub shares_diluted: f64,
    pub bal: Quarter,
    pub sbc: f64,
    pub dwc: f64,
    pub capex: f64,
    pub d_sti: f64,
    pub acq: f64,
    pub other_inv: f64,
    pub debt_net: f64,
    pub div: f64,
    pub buyback: f64,
}

impl Period {
    pub(crate) fn single(q: &Quarter, period_type: PeriodType, fp: String) -> Period {
        Period::sum(std::slice::from_ref(q), period_type, fp)
    }

    pub(crate) fn sum(qs: &[Quarter], period_type: PeriodType, fp: String) -> Period {
        let last = qs.last().cloned().unwrap_or_default();
        let sum = |f: fn(&Quarter) -> f64| qs.iter().map(f).sum::<f64>();
        let n = qs.len().max(1) as f64;
        let avg_sh = |f: fn(&Quarter) -> f64| ((sum(f) / n) / 1e5).round() * 1e5;
        Period {
            kind: period_type,
            fiscal_year: last.fy,
            label: fp,
            end: last.end,
            revenue: sum(|q| q.revenue),
            cogs: sum(|q| q.cogs),
            gross: sum(|q| q.gross),
            sga: sum(|q| q.sga),
            rnd: sum(|q| q.rnd),
            op: sum(|q| q.op),
            da: sum(|q| q.da),
            interest: sum(|q| q.interest),
            pretax: sum(|q| q.pretax),
            tax: sum(|q| q.tax),
            ni: sum(|q| q.ni),
            ebitda: sum(|q| q.ebitda),
            shares_basic: avg_sh(|q| q.shares_basic),
            shares_diluted: avg_sh(|q| q.shares_diluted),
            sbc: sum(|q| q.sbc),
            dwc: sum(|q| q.dwc),
            capex: sum(|q| q.capex),
            d_sti: sum(|q| q.d_sti),
            acq: sum(|q| q.acq),
            other_inv: sum(|q| q.other_inv),
            debt_net: sum(|q| q.debt_net),
            div: sum(|q| q.div),
            buyback: sum(|q| q.buyback),
            bal: last,
        }
    }

    pub(crate) fn eps_diluted(&self) -> f64 {
        if self.shares_diluted > 0.0 { self.ni / self.shares_diluted } else { 0.0 }
    }

    pub(crate) fn eps_basic(&self) -> f64 {
        if self.shares_basic > 0.0 { self.ni / self.shares_basic } else { 0.0 }
    }

    pub(crate) fn cfo(&self) -> f64 {
        self.ni + self.da + self.sbc + self.dwc
    }

    pub(crate) fn cfi(&self) -> f64 {
        -self.capex - self.d_sti - self.acq + self.other_inv
    }

    pub(crate) fn cff(&self) -> f64 {
        self.debt_net - self.div - self.buyback
    }

    /// `k` converts per-share figures to today's share basis.
    pub(crate) fn statement(&self, kind: StatementKind, k: f64) -> Statement {
        let mut lines = Vec::with_capacity(24);
        let mut l = |code: &str, label: &str, value: f64, depth: u8| {
            lines.push(StatementLine { code: code.into(), label: label.into(), value: Some(value), depth, source_tag: None });
        };
        match kind {
            StatementKind::Income => {
                l("revenue", "Revenue", self.revenue, 0);
                l("cost_of_revenue", "Cost of Revenue", self.cogs, 1);
                l("gross_profit", "Gross Profit", self.gross, 0);
                l("sga", "Selling, General & Administrative", self.sga, 1);
                l("rnd", "Research & Development", self.rnd, 1);
                l("operating_income", "Operating Income", self.op, 0);
                l("interest_expense", "Interest Expense", self.interest, 1);
                l("pretax_income", "Pretax Income", self.pretax, 0);
                l("income_tax", "Income Tax Expense", self.tax, 1);
                l("net_income", "Net Income", self.ni, 0);
                l("eps_basic", "EPS — Basic", round2(self.eps_basic() * k), 0);
                l("eps_diluted", "EPS — Diluted", round2(self.eps_diluted() * k), 0);
                l("shares_basic", "Weighted Avg Shares — Basic", (self.shares_basic / k).round(), 1);
                l("shares_diluted", "Weighted Avg Shares — Diluted", (self.shares_diluted / k).round(), 1);
                l("ebitda", "EBITDA", self.ebitda, 0);
            }
            StatementKind::Balance => {
                let b = &self.bal;
                l("cash", "Cash & Equivalents", b.cash, 1);
                l("short_term_investments", "Short-Term Investments", b.sti, 1);
                l("receivables", "Accounts Receivable", b.ar, 1);
                l("inventory", "Inventories", b.inv, 1);
                l("other_current_assets", "Other Current Assets", b.oca, 1);
                l("total_current_assets", "Total Current Assets", b.tca(), 0);
                l("ppe_net", "Property, Plant & Equipment, Net", b.ppe, 1);
                l("goodwill", "Goodwill & Intangibles", b.gw, 1);
                l("other_non_current_assets", "Other Non-Current Assets", b.onca, 1);
                l("total_assets", "Total Assets", b.ta(), 0);
                l("accounts_payable", "Accounts Payable", b.ap, 1);
                l("short_term_debt", "Short-Term Debt", b.std, 1);
                l("other_current_liabilities", "Other Current Liabilities", b.ocl, 1);
                l("total_current_liabilities", "Total Current Liabilities", b.tcl(), 0);
                l("long_term_debt", "Long-Term Debt", b.ltd, 1);
                l("other_non_current_liabilities", "Other Non-Current Liabilities", b.oncl, 1);
                l("total_liabilities", "Total Liabilities", b.tl(), 0);
                l("total_equity", "Total Shareholders' Equity", b.eq, 0);
                l("total_liabilities_and_equity", "Total Liabilities & Equity", b.tl() + b.eq, 0);
            }
            StatementKind::CashFlow => {
                l("net_income", "Net Income", self.ni, 1);
                l("depreciation_amortization", "Depreciation & Amortization", self.da, 1);
                l("stock_based_compensation", "Stock-Based Compensation", self.sbc, 1);
                l("change_in_working_capital", "Change in Working Capital", self.dwc, 1);
                l("cfo", "Cash from Operating Activities", self.cfo(), 0);
                l("capex", "Capital Expenditures", -self.capex, 1);
                l("net_investment_purchases", "Net Purchases of Investments", -self.d_sti, 1);
                l("acquisitions", "Acquisitions", -self.acq, 1);
                l("other_investing", "Other Investing Activities", self.other_inv, 1);
                l("cfi", "Cash from Investing Activities", self.cfi(), 0);
                l("debt_issued_repaid", "Net Debt Issued (Repaid)", self.debt_net, 1);
                l("dividends_paid", "Dividends Paid", -self.div, 1);
                l("buybacks", "Repurchase of Common Stock", -self.buyback, 1);
                l("cff", "Cash from Financing Activities", self.cff(), 0);
                l("net_change_cash", "Net Change in Cash", self.cfo() + self.cfi() + self.cff(), 0);
                l("fcf", "Free Cash Flow", self.cfo() - self.capex, 0);
            }
        }
        Statement {
            kind,
            period_type: self.kind,
            fiscal_year: self.fiscal_year,
            fiscal_period: self.label.clone(),
            period_end: self.end,
            currency: "USD".into(),
            lines,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fiscal_mapping() {
        // Apple: FY ends September. Dec quarter is Q1 of the next FY.
        assert_eq!(fiscal_of(2025, 12, 9), (2026, 1));
        assert_eq!(fiscal_of(2025, 9, 9), (2025, 4));
        // NVIDIA: FY ends January. April quarter is Q1 of the next FY.
        assert_eq!(fiscal_of(2025, 4, 1), (2026, 1));
        assert_eq!(fiscal_of(2026, 1, 1), (2026, 4));
        assert_eq!(fiscal_of(2025, 12, 12), (2025, 4));
        assert_eq!(fiscal_of(2025, 3, 12), (2025, 1));
    }
}

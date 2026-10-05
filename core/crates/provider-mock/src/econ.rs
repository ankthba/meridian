//! Synthetic macro data: a rates model (policy rate stepped at FOMC
//! meetings, Nelson-Siegel curve), monthly activity and price indices, the
//! economic calendar, and the UST yield curve.
//!
//! Series use FRED-style IDs, titles, units and frequencies so screens can be
//! built against them, but every value is generated locally. The calendar
//! derives its actuals from the same series (CPI MoM from CPIAUCSL, …) and
//! its release dates from the same rules that decide when a series'
//! observation becomes available.

use std::sync::Arc;

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use meridian_provider::{CalendarRequest, CurveRequest, ProviderError, ProviderResult, SeriesRequest};
use meridian_types::{
    CurvePoint, EconomicEvent, EconomicSeries, Importance, Observation, Provenance, UnixNanos, YieldCurve,
};
use rand::Rng;
use rand_distr::StandardNormal;

use crate::Inner;
use crate::cal::{Calendar, Tz, is_weekend, last_weekday, month_end, nth_weekday, ymd};
use crate::hash::{rng, tag};

const START_YEAR: i32 = 1990;

fn ym_index(y: i32, m: u32) -> i32 {
    (y - START_YEAR) * 12 + m as i32 - 1
}

fn ym_of(i: i32) -> (i32, u32) {
    (START_YEAR + i.div_euclid(12), i.rem_euclid(12) as u32 + 1)
}

fn month_abbr(m: u32) -> &'static str {
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][(m.clamp(1, 12) - 1) as usize]
}

fn round_to(x: f64, decimals: i32) -> f64 {
    let k = 10f64.powi(decimals);
    (x * k).round() / k
}

/// FOMC decision days: eight Wednesdays a year by a fixed rule.
pub(crate) fn fomc_dates(y: i32) -> [NaiveDate; 8] {
    use Weekday::Wed;
    [
        last_weekday(y, 1, Wed),
        nth_weekday(y, 3, Wed, 3),
        nth_weekday(y, 5, Wed, 1),
        nth_weekday(y, 6, Wed, 3),
        last_weekday(y, 7, Wed),
        nth_weekday(y, 9, Wed, 3),
        last_weekday(y, 10, Wed),
        nth_weekday(y, 12, Wed, 2),
    ]
}

fn ecb_dates(y: i32) -> [NaiveDate; 8] {
    use Weekday::Thu;
    [
        nth_weekday(y, 1, Thu, 4),
        nth_weekday(y, 3, Thu, 2),
        nth_weekday(y, 4, Thu, 3),
        nth_weekday(y, 6, Thu, 1),
        nth_weekday(y, 7, Thu, 4),
        nth_weekday(y, 9, Thu, 2),
        nth_weekday(y, 10, Thu, 4),
        nth_weekday(y, 12, Thu, 3),
    ]
}

fn boe_dates(y: i32) -> [NaiveDate; 8] {
    use Weekday::Thu;
    [
        nth_weekday(y, 2, Thu, 1),
        nth_weekday(y, 3, Thu, 3),
        nth_weekday(y, 5, Thu, 1),
        nth_weekday(y, 6, Thu, 3),
        nth_weekday(y, 8, Thu, 1),
        nth_weekday(y, 9, Thu, 3),
        nth_weekday(y, 11, Thu, 1),
        nth_weekday(y, 12, Thu, 3),
    ]
}

fn boj_dates(y: i32) -> [NaiveDate; 8] {
    use Weekday::Fri;
    [
        nth_weekday(y, 1, Fri, 4),
        nth_weekday(y, 3, Fri, 3),
        last_weekday(y, 4, Fri),
        nth_weekday(y, 6, Fri, 3),
        last_weekday(y, 7, Fri),
        nth_weekday(y, 9, Fri, 3),
        last_weekday(y, 10, Fri),
        nth_weekday(y, 12, Fri, 3),
    ]
}

/// Nelson-Siegel loadings, decay 1.8 years.
fn ns(l: f64, s: f64, c: f64, tau: f64) -> f64 {
    let x = (tau / 1.8).max(1e-6);
    let f1 = (1.0 - (-x).exp()) / x;
    let f2 = f1 - (-x).exp();
    (l + s * f1 + c * f2).max(0.02)
}

pub(crate) const UST_TENORS: [(&str, f64); 13] = [
    ("1 Mo", 1.0 / 12.0),
    ("2 Mo", 2.0 / 12.0),
    ("3 Mo", 0.25),
    ("4 Mo", 4.0 / 12.0),
    ("6 Mo", 0.5),
    ("1 Yr", 1.0),
    ("2 Yr", 2.0),
    ("3 Yr", 3.0),
    ("5 Yr", 5.0),
    ("7 Yr", 7.0),
    ("10 Yr", 10.0),
    ("20 Yr", 20.0),
    ("30 Yr", 30.0),
];

/// Daily rates state on weekdays.
#[derive(Debug)]
pub(crate) struct Rates {
    start: NaiveDate,
    /// Fed funds target upper bound.
    upper: Vec<f64>,
    l: Vec<f64>,
    s: Vec<f64>,
    c: Vec<f64>,
    mspread: Vec<f64>,
}

impl Rates {
    fn compute(seed: u64, until: NaiveDate) -> Rates {
        let start = ymd(START_YEAR, 1, 1);
        let days = Calendar::Weekdays.days(start, until);
        let n = days.len();
        let mut r = rng(&[seed, tag("rates")]);
        let dt: f64 = 1.0 / 260.0;
        let sq = dt.sqrt();
        let (mut d, mut p, mut l, mut c, mut ms) = (7.5f64, 8.25f64, 8.0f64, 0.0f64, 1.8f64);
        let mut out = Rates {
            start,
            upper: Vec::with_capacity(n),
            l: Vec::with_capacity(n),
            s: Vec::with_capacity(n),
            c: Vec::with_capacity(n),
            mspread: Vec::with_capacity(n),
        };
        let mut year = 0;
        let mut meetings = fomc_dates(START_YEAR);
        for day in &days {
            if day.year() != year {
                year = day.year();
                meetings = fomc_dates(year);
            }
            let z: [f64; 4] = [r.sample(StandardNormal), r.sample(StandardNormal), r.sample(StandardNormal), r.sample(StandardNormal)];
            d += 0.3 * (3.0 - d) * dt + 1.1 * sq * z[0];
            d = d.clamp(-1.0, 9.0);
            if meetings.contains(day) {
                let target = ((d + 0.15) * 4.0).round() / 4.0;
                let mv = ((target - p).clamp(-0.75, 0.75) * 4.0).round() / 4.0;
                p = (p + mv).clamp(0.25, 9.0);
            }
            l += 0.15 * (4.3 - l) * dt + 0.75 * sq * z[1];
            l = l.clamp(1.0, 8.5);
            c += 0.8 * (0.0 - c) * dt + 0.9 * sq * z[2];
            c = c.clamp(-3.0, 3.0);
            ms += 0.5 * (1.8 - ms) * dt + 0.3 * sq * z[3];
            ms = ms.clamp(1.2, 3.2);
            let short = p - 0.12 + 0.35 * (d - p);
            out.upper.push(p);
            out.l.push(l);
            out.s.push(short - l);
            out.c.push(c);
            out.mspread.push(ms);
        }
        out
    }

    fn idx(&self, d: NaiveDate) -> usize {
        // Weekday index: whole weeks plus weekdays into the current week.
        let d = if is_weekend(d) { Calendar::Weekdays.on_or_before(d) } else { d };
        let days = (d - self.start).num_days().max(0);
        let start_wd = self.start.weekday().num_days_from_monday() as i64;
        let total = days + start_wd;
        let weeks = total / 7;
        let rem = (total % 7).min(4);
        let idx = weeks * 5 + rem - start_wd.min(4);
        (idx.max(0) as usize).min(self.upper.len().saturating_sub(1))
    }

    pub(crate) fn yield_at(&self, d: NaiveDate, tau: f64) -> f64 {
        let i = self.idx(d);
        ns(self.l[i], self.s[i], self.c[i], tau)
    }

    fn upper_at(&self, d: NaiveDate) -> f64 {
        self.upper[self.idx(d)]
    }

    fn effr(&self, d: NaiveDate) -> f64 {
        (self.upper_at(d) - 0.17).max(0.05)
    }
}

/// Monthly macro state from January 1990.
#[derive(Debug)]
pub(crate) struct Monthly {
    cpi: Vec<f64>,
    core: Vec<f64>,
    pce: Vec<f64>,
    unrate: Vec<f64>,
    payems: Vec<f64>,
    indpro: Vec<f64>,
    rsafs: Vec<f64>,
    houst: Vec<f64>,
    umcsent: Vec<f64>,
    fedfunds: Vec<f64>,
    /// Quarterly real GDP from 1990Q1.
    gdp: Vec<f64>,
    /// Unrounded unemployment, for weekly claims.
    u_raw: Vec<f64>,
}

impl Monthly {
    fn compute(seed: u64, rates: &Rates, until: NaiveDate) -> Monthly {
        let n = (ym_index(until.year(), until.month()) + 1).max(1) as usize;
        let r = std::cell::RefCell::new(rng(&[seed, tag("macro-monthly")]));
        let nz = || -> f64 { r.borrow_mut().sample(StandardNormal) };
        let uniform = || -> f64 { r.borrow_mut().random() };
        let mut cpi_l = vec![0.0; n];
        let mut core_l = vec![0.0; n];
        let mut pce_l = vec![0.0; n];
        let mut u = vec![0.0; n];
        let mut pay = vec![0.0; n];
        let mut ip_l = vec![0.0; n];
        let mut rs_l = vec![0.0; n];
        let mut houst = vec![0.0; n];
        let mut sent = vec![0.0; n];
        let mut ff = vec![0.0; n];
        let (mut pi, mut trend, mut high) = (0.0021f64, 0.0021f64, false);
        let (mut ur, mut rec) = (5.4f64, false);
        let (mut hx, mut sx) = (0.0f64, 0.0f64);
        let (mut lc, mut lcore, mut lpce, mut lp, mut lip, mut lrs) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for i in 0..n {
            let (y, m) = ym_of(i as i32);
            // Inflation with a rare high-inflation regime (enter ~every 25
            // years, lasting ~1.5 years).
            let switch = uniform();
            if !high && switch < 0.0035 {
                high = true;
            } else if high && switch < 0.06 {
                high = false;
            }
            let regime = if high { 0.0035 } else { 0.0 };
            pi = 0.0021 + regime + 0.55 * (pi - 0.0021 - regime) + 0.0018 * nz();
            trend = 0.8 * trend + 0.2 * pi;
            let pc = 0.6 * trend + 0.4 * 0.0021 + 0.0008 * nz();
            let pp = 0.85 * pc + 0.0004 * nz();
            lc += pi;
            lcore += pc;
            lpce += pp;
            cpi_l[i] = lc;
            core_l[i] = lcore;
            pce_l[i] = lpce;
            // Unemployment with recessions (enter ~every 13 years, lasting
            // ~10 months).
            let rec_draw = uniform();
            if !rec && rec_draw < 0.0065 {
                rec = true;
            } else if rec && rec_draw < 0.10 {
                rec = false;
            }
            let u_prev = ur;
            ur = if rec { ur + 0.25 + 0.15 * nz() } else { ur + 0.03 * (4.6 - ur) + 0.12 * nz() };
            ur = ur.clamp(3.2, 12.0);
            u[i] = ur;
            let du = ur - u_prev;
            lp += 140.0 - 700.0 * du + 60.0 * nz();
            pay[i] = lp;
            lip += 0.0012 - 0.006 * du + 0.005 * nz();
            ip_l[i] = lip;
            lrs += 0.0015 + pi - 0.004 * du + 0.006 * nz();
            rs_l[i] = lrs;
            // Mortgage rate this month drives housing.
            let mid = ymd(y, m, 15);
            let mort = rates.yield_at(mid, 10.0) + rates.mspread[rates.idx(mid)];
            hx = 0.92 * hx + 0.06 * nz();
            houst[i] = (1400.0 * (hx - 0.15 * (mort - 5.0) - 0.03 * (ur - 5.0)).exp()).clamp(450.0, 2300.0).round();
            sx = 0.8 * sx + 2.5 * nz();
            let yoy = if i >= 12 { (cpi_l[i] - cpi_l[i - 12]).exp() - 1.0 } else { 0.025 };
            sent[i] = round_to((88.0 - 3.0 * (ur - 5.0) - 400.0 * (yoy - 0.025).max(0.0) + sx).clamp(50.0, 105.0), 1);
            // Fed funds: average effective rate over the month's weekdays.
            let days = Calendar::Weekdays.days(ymd(y, m, 1), month_end(y, m));
            let avg = days.iter().map(|d| rates.effr(*d)).sum::<f64>() / days.len().max(1) as f64;
            ff[i] = round_to(avg, 2);
        }
        let anchor = (ym_index(2025, 12) as usize).min(n - 1);
        let level = |logs: &[f64], at: f64, dec: i32| -> Vec<f64> {
            let shift = at.ln() - logs[anchor];
            logs.iter().map(|x| round_to((x + shift).exp(), dec)).collect()
        };
        let pay_shift = 159_800.0 - pay[anchor];
        // Quarterly GDP from the change in unemployment over the quarter.
        let nq = n.div_ceil(3);
        let mut gl = vec![0.0; nq];
        let mut lg = 0.0;
        for (q, g) in gl.iter_mut().enumerate() {
            let a = (q * 3).min(n - 1);
            let b = (q * 3 + 2).min(n - 1);
            let du = u[b] - if a > 0 { u[a - 1] } else { u[a] };
            let growth = 2.3 - 6.0 * du + 1.2 * nz();
            lg += (1.0 + growth / 100.0).ln() / 4.0;
            *g = lg;
        }
        let gq_anchor = ((ym_index(2025, 7) / 3) as usize).min(nq - 1);
        let gshift = 24_050.0f64.ln() - gl[gq_anchor];
        Monthly {
            cpi: level(&cpi_l, 325.0, 3),
            core: level(&core_l, 331.5, 3),
            pce: level(&pce_l, 126.0, 3),
            unrate: u.iter().map(|x| round_to(*x, 1)).collect(),
            payems: pay.iter().map(|x| (x + pay_shift).round()).collect(),
            indpro: level(&ip_l, 103.4, 4),
            rsafs: level(&rs_l, 735_000.0, 0),
            houst,
            umcsent: sent,
            fedfunds: ff,
            gdp: gl.iter().map(|x| round_to((x + gshift).exp(), 3)).collect(),
            u_raw: u,
        }
    }

    fn get(v: &[f64], i: i32) -> Option<f64> {
        usize::try_from(i).ok().and_then(|i| v.get(i).copied())
    }
}

/// Weekly claims for the week ending Saturday `sat`.
fn claims(seed: u64, m: &Monthly, sat: NaiveDate) -> f64 {
    let i = ym_index(sat.year(), sat.month());
    let u = Monthly::get(&m.u_raw, i).unwrap_or(4.5);
    let mut c = crate::hash::Cell::new(&[seed, tag("claims"), crate::daily::day_num(sat)]);
    let base = 215_000.0 * (0.6 * (u - 4.5) / 4.5 + 0.05 * c.normal()).exp();
    (base / 1000.0).round() * 1000.0
}

pub(crate) struct Macro {
    pub rates: Rates,
    pub monthly: Monthly,
    pub until: NaiveDate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Freq {
    Daily,
    WeeklyThu,
    WeeklySat,
    Monthly,
    Quarterly,
}

struct SeriesDef {
    id: &'static str,
    title: &'static str,
    units: &'static str,
    freq: Freq,
    sa: &'static str,
}

const NSA: &str = "Not Seasonally Adjusted";
const SA: &str = "Seasonally Adjusted";

#[rustfmt::skip]
const SERIES: &[SeriesDef] = &[
    SeriesDef { id: "CPIAUCSL", title: "Consumer Price Index for All Urban Consumers: All Items in U.S. City Average", units: "Index 1982-1984=100", freq: Freq::Monthly, sa: SA },
    SeriesDef { id: "CPILFESL", title: "Consumer Price Index for All Urban Consumers: All Items Less Food and Energy in U.S. City Average", units: "Index 1982-1984=100", freq: Freq::Monthly, sa: SA },
    SeriesDef { id: "UNRATE", title: "Unemployment Rate", units: "Percent", freq: Freq::Monthly, sa: SA },
    SeriesDef { id: "PAYEMS", title: "All Employees, Total Nonfarm", units: "Thousands of Persons", freq: Freq::Monthly, sa: SA },
    SeriesDef { id: "GDPC1", title: "Real Gross Domestic Product", units: "Billions of Chained 2017 Dollars", freq: Freq::Quarterly, sa: "Seasonally Adjusted Annual Rate" },
    SeriesDef { id: "FEDFUNDS", title: "Federal Funds Effective Rate", units: "Percent", freq: Freq::Monthly, sa: NSA },
    SeriesDef { id: "DGS2", title: "Market Yield on U.S. Treasury Securities at 2-Year Constant Maturity, Quoted on an Investment Basis", units: "Percent", freq: Freq::Daily, sa: NSA },
    SeriesDef { id: "DGS10", title: "Market Yield on U.S. Treasury Securities at 10-Year Constant Maturity, Quoted on an Investment Basis", units: "Percent", freq: Freq::Daily, sa: NSA },
    SeriesDef { id: "DGS30", title: "Market Yield on U.S. Treasury Securities at 30-Year Constant Maturity, Quoted on an Investment Basis", units: "Percent", freq: Freq::Daily, sa: NSA },
    SeriesDef { id: "T10Y2Y", title: "10-Year Treasury Constant Maturity Minus 2-Year Treasury Constant Maturity", units: "Percent", freq: Freq::Daily, sa: NSA },
    SeriesDef { id: "MORTGAGE30US", title: "30-Year Fixed Rate Mortgage Average in the United States", units: "Percent", freq: Freq::WeeklyThu, sa: NSA },
    SeriesDef { id: "INDPRO", title: "Industrial Production: Total Index", units: "Index 2017=100", freq: Freq::Monthly, sa: SA },
    SeriesDef { id: "RSAFS", title: "Advance Retail Sales: Retail Trade and Food Services", units: "Millions of Dollars", freq: Freq::Monthly, sa: SA },
    SeriesDef { id: "HOUST", title: "New Privately-Owned Housing Units Started: Total Units", units: "Thousands of Units", freq: Freq::Monthly, sa: "Seasonally Adjusted Annual Rate" },
    SeriesDef { id: "UMCSENT", title: "University of Michigan: Consumer Sentiment", units: "Index 1966:Q1=100", freq: Freq::Monthly, sa: NSA },
    SeriesDef { id: "PCEPILFE", title: "Personal Consumption Expenditures Excluding Food and Energy (Chain-Type Price Index)", units: "Index 2017=100", freq: Freq::Monthly, sa: SA },
    SeriesDef { id: "ICSA", title: "Initial Claims", units: "Number", freq: Freq::WeeklySat, sa: SA },
];

pub(crate) fn series_ids() -> impl Iterator<Item = &'static str> {
    SERIES.iter().map(|s| s.id)
}

fn freq_label(f: Freq) -> &'static str {
    match f {
        Freq::Daily => "Daily",
        Freq::WeeklyThu => "Weekly, Ending Thursday",
        Freq::WeeklySat => "Weekly, Ending Saturday",
        Freq::Monthly => "Monthly",
        Freq::Quarterly => "Quarterly",
    }
}

fn weekday_on_or_after(d: NaiveDate) -> NaiveDate {
    Calendar::UsBond.on_or_after(d)
}

/// Release time (local date, local minutes, tz) of a monthly/quarterly
/// series' observation for period `(y, m)`.
fn release_of(id: &str, y: i32, m: u32) -> Option<(NaiveDate, i32)> {
    let next = crate::cal::add_months(ymd(y, m, 1), 1);
    let (ny, nm) = (next.year(), next.month());
    let at = |d: NaiveDate, h: i32, mi: i32| Some((d, h * 60 + mi));
    match id {
        "CPIAUCSL" | "CPILFESL" => {
            let mut d = ymd(ny, nm, 10);
            while d.weekday() != Weekday::Wed {
                d += Duration::days(1);
            }
            at(weekday_on_or_after(d), 8, 30)
        }
        "PCEPILFE" => at(last_weekday(ny, nm, Weekday::Fri), 8, 30),
        "UNRATE" | "PAYEMS" => {
            let f = nth_weekday(ny, nm, Weekday::Fri, 1);
            at(if f.day() < 3 { f + Duration::days(7) } else { f }, 8, 30)
        }
        "RSAFS" => at(weekday_on_or_after(ymd(ny, nm, 15)), 8, 30),
        "INDPRO" => at(weekday_on_or_after(ymd(ny, nm, 16)), 9, 15),
        "HOUST" => at(weekday_on_or_after(ymd(ny, nm, 18)), 8, 30),
        "UMCSENT" => at(last_weekday(y, m, Weekday::Fri), 10, 0),
        "FEDFUNDS" => at(weekday_on_or_after(ymd(ny, nm, 1)), 16, 0),
        "GDPC1" => {
            // `m` is the quarter's first month; advance estimate ~4 weeks after quarter end.
            let after = crate::cal::add_months(ymd(y, m, 1), 4);
            at(last_weekday(after.year(), after.month(), Weekday::Thu), 8, 30)
        }
        _ => None,
    }
}

fn released(id: &str, y: i32, m: u32, now: UnixNanos) -> bool {
    release_of(id, y, m).is_some_and(|(d, mins)| Tz::NewYork.to_utc(d, mins) <= now)
}

impl Inner {
    pub(crate) fn macro_state(&self, until: NaiveDate) -> Arc<Macro> {
        if let Some(m) = self.macro_cache.read().as_ref()
            && m.until >= until
        {
            return m.clone();
        }
        let mut w = self.macro_cache.write();
        if let Some(m) = w.as_ref()
            && m.until >= until
        {
            return m.clone();
        }
        let target = until + Duration::days(800);
        let rates = Rates::compute(self.seed, target);
        let monthly = Monthly::compute(self.seed, &rates, target);
        let m = Arc::new(Macro { rates, monthly, until: target });
        *w = Some(m.clone());
        m
    }

    /// Constant-maturity UST yield (percent) on or before `d`.
    pub(crate) fn ust_yield(&self, d: NaiveDate, tau: f64) -> f64 {
        let m = self.macro_state(d);
        round_to(m.rates.yield_at(Calendar::UsBond.on_or_before(d), tau), 2)
    }

    /// Latest business day whose UST data has been published at `now`.
    fn latest_curve_date(now: UnixNanos) -> NaiveDate {
        let (today, mins) = Tz::NewYork.to_local(now);
        if Calendar::UsBond.is_trading_day(today) && mins >= 18 * 60 {
            today
        } else {
            Calendar::UsBond.before(today)
        }
    }

    pub(crate) fn economic_series_for(&self, req: &SeriesRequest) -> ProviderResult<EconomicSeries> {
        let id = req.id.trim().to_ascii_uppercase();
        let def = SERIES
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| ProviderError::NotFound(format!("series {id} is not available in the mock")))?;
        let now = self.clock.now();
        let today = Tz::NewYork.to_local(now).0;
        let st = self.macro_state(today);
        let mo = &st.monthly;
        let rates = &st.rates;
        let mut obs: Vec<Observation> = Vec::new();
        match def.freq {
            Freq::Daily => {
                let last = Self::latest_curve_date(now);
                for d in Calendar::Weekdays.days(ymd(START_YEAR, 1, 2), last) {
                    let value = Calendar::UsBond.is_trading_day(d).then(|| {
                        let y = |t: f64| round_to(rates.yield_at(d, t), 2);
                        match def.id {
                            "DGS2" => y(2.0),
                            "DGS10" => y(10.0),
                            "DGS30" => y(30.0),
                            _ => round_to(y(10.0) - y(2.0), 2),
                        }
                    });
                    obs.push(Observation { date: d, value });
                }
            }
            Freq::WeeklyThu => {
                let mut d = nth_weekday(START_YEAR, 1, Weekday::Thu, 1);
                while d <= today {
                    let i = rates.idx(d);
                    let v = round_to(round_to(rates.yield_at(d, 10.0), 2) + rates.mspread[i], 2);
                    if Tz::NewYork.to_utc(d, 12 * 60) <= now {
                        obs.push(Observation { date: d, value: Some(v) });
                    }
                    d += Duration::days(7);
                }
            }
            Freq::WeeklySat => {
                let mut d = nth_weekday(START_YEAR, 1, Weekday::Sat, 1);
                loop {
                    let release = d + Duration::days(5);
                    if Tz::NewYork.to_utc(release, 8 * 60 + 30) > now {
                        break;
                    }
                    obs.push(Observation { date: d, value: Some(claims(self.seed, mo, d)) });
                    d += Duration::days(7);
                }
            }
            Freq::Monthly => {
                let v = match def.id {
                    "CPIAUCSL" => &mo.cpi,
                    "CPILFESL" => &mo.core,
                    "UNRATE" => &mo.unrate,
                    "PAYEMS" => &mo.payems,
                    "FEDFUNDS" => &mo.fedfunds,
                    "INDPRO" => &mo.indpro,
                    "RSAFS" => &mo.rsafs,
                    "HOUST" => &mo.houst,
                    "UMCSENT" => &mo.umcsent,
                    _ => &mo.pce,
                };
                for (i, x) in v.iter().enumerate() {
                    let (y, m) = ym_of(i as i32);
                    if !released(def.id, y, m, now) {
                        break;
                    }
                    obs.push(Observation { date: ymd(y, m, 1), value: Some(*x) });
                }
            }
            Freq::Quarterly => {
                for (q, x) in mo.gdp.iter().enumerate() {
                    let (y, m) = ym_of(q as i32 * 3);
                    if !released(def.id, y, m, now) {
                        break;
                    }
                    obs.push(Observation { date: ymd(y, m, 1), value: Some(*x) });
                }
            }
        }
        obs.retain(|o| req.from.is_none_or(|f| o.date >= f) && req.to.is_none_or(|t| o.date <= t));
        let mut provenance = Provenance::synthetic(now);
        provenance.source_ref = Some(format!("mock:{}", def.id));
        Ok(EconomicSeries {
            id: def.id.into(),
            title: def.title.into(),
            units: def.units.into(),
            frequency: freq_label(def.freq).into(),
            seasonal_adjustment: Some(def.sa.into()),
            last_updated: Some(now),
            notes: Some("SYNTHETIC — MOCK DATA. Generated locally; not the published series.".into()),
            observations: obs,
            provenance,
        })
    }

    pub(crate) fn yield_curve_for(&self, req: &CurveRequest) -> ProviderResult<YieldCurve> {
        if !req.name.trim().eq_ignore_ascii_case("UST") {
            return Err(ProviderError::NotFound(format!("curve {} is not available in the mock (only UST)", req.name)));
        }
        let now = self.clock.now();
        let latest = Self::latest_curve_date(now);
        let date = match req.date {
            Some(d) => {
                let d = Calendar::UsBond.on_or_before(d);
                if d > latest {
                    return Err(ProviderError::NotFound(format!("no UST curve published for {d} yet")));
                }
                d
            }
            None => latest,
        };
        let st = self.macro_state(date);
        let points = UST_TENORS
            .iter()
            .map(|(label, years)| CurvePoint {
                tenor: (*label).into(),
                years: *years,
                yield_pct: Some(round_to(st.rates.yield_at(date, *years), 2)),
            })
            .collect();
        let mut provenance = Provenance::synthetic(now);
        provenance.source_ref = Some("mock:UST".into());
        Ok(YieldCurve { name: "UST".into(), date, points, provenance })
    }

    pub(crate) fn economic_calendar_for(&self, req: &CalendarRequest) -> Vec<EconomicEvent> {
        let now = self.clock.now();
        let from = req.from;
        let to = req.to.min(from + Duration::days(366));
        if to < from {
            return Vec::new();
        }
        let st = self.macro_state(to + Duration::days(60));
        let want = |c: &str| req.countries.is_empty() || req.countries.iter().any(|x| x.eq_ignore_ascii_case(c));
        let mut out = Vec::new();
        let mut ev = EventBuilder { seed: self.seed, now, out: &mut out };

        // Monthly US releases: walk reference months whose release can fall in range.
        let start_i = ym_index(from.year(), from.month()) - 2;
        let end_i = ym_index(to.year(), to.month()) + 1;
        let mo = &st.monthly;
        if want("US") {
            for i in start_i..=end_i {
                let (y, m) = ym_of(i);
                let per = || Some(month_abbr(m).to_string());
                let pct = |v: &[f64], i: i32, lag: i32| -> Option<f64> {
                    let a = Monthly::get(v, i)?;
                    let b = Monthly::get(v, i - lag)?;
                    Some(round_to((a / b - 1.0) * 100.0, 1))
                };
                let diff = |v: &[f64], i: i32| Some(Monthly::get(v, i)? - Monthly::get(v, i - 1)?);
                let mut add = |id: &str, name: &str, imp: Importance, unit: &str, value: Option<f64>, prior: Option<f64>, dec: i32| {
                    if let Some((d, mins)) = release_of(id, y, m)
                        && d >= from
                        && d <= to
                    {
                        let series = if id.is_empty() { None } else { Some(id.to_string()) };
                        ev.push(Tz::NewYork.to_utc(d, mins), "US", name, per(), value, prior, unit, imp, series, dec);
                    }
                };
                add("CPIAUCSL", "CPI MoM", Importance::High, "%", pct(&mo.cpi, i, 1), pct(&mo.cpi, i - 1, 1), 1);
                add("CPIAUCSL", "CPI YoY", Importance::High, "%", pct(&mo.cpi, i, 12), pct(&mo.cpi, i - 1, 12), 1);
                add("CPILFESL", "CPI Ex Food and Energy MoM", Importance::High, "%", pct(&mo.core, i, 1), pct(&mo.core, i - 1, 1), 1);
                add("PAYEMS", "Change in Nonfarm Payrolls", Importance::High, "K", diff(&mo.payems, i), diff(&mo.payems, i - 1), 0);
                add("UNRATE", "Unemployment Rate", Importance::High, "%", Monthly::get(&mo.unrate, i), Monthly::get(&mo.unrate, i - 1), 1);
                add("RSAFS", "Retail Sales Advance MoM", Importance::Medium, "%", pct(&mo.rsafs, i, 1), pct(&mo.rsafs, i - 1, 1), 1);
                add("PCEPILFE", "PCE Core Deflator MoM", Importance::Medium, "%", pct(&mo.pce, i, 1), pct(&mo.pce, i - 1, 1), 1);
                add("HOUST", "Housing Starts", Importance::Low, "K", Monthly::get(&mo.houst, i), Monthly::get(&mo.houst, i - 1), 0);
                add("UMCSENT", "U. of Mich. Sentiment", Importance::Medium, "", Monthly::get(&mo.umcsent, i), Monthly::get(&mo.umcsent, i - 1), 1);
                add("INDPRO", "Industrial Production MoM", Importance::Low, "%", pct(&mo.indpro, i, 1), pct(&mo.indpro, i - 1, 1), 1);
                if m % 3 == 1 {
                    let q = i / 3;
                    let g = |q: i32| -> Option<f64> {
                        let a = Monthly::get(&mo.gdp, q)?;
                        let b = Monthly::get(&mo.gdp, q - 1)?;
                        Some(round_to(((a / b).powi(4) - 1.0) * 100.0, 1))
                    };
                    if let Some((d, mins)) = release_of("GDPC1", y, m)
                        && d >= from
                        && d <= to
                    {
                        let per = Some(format!("{}Q", (m - 1) / 3 + 1));
                        ev.push(Tz::NewYork.to_utc(d, mins), "US", "GDP Annualized QoQ", per, g(q), g(q - 1), "%", Importance::High, Some("GDPC1".into()), 1);
                    }
                }
                // ISM surveys (no series): first and third business days of the next month.
                let next = crate::cal::add_months(ymd(y, m, 1), 1);
                let b1 = Calendar::UsBond.on_or_after(next);
                let b3 = Calendar::UsBond.add_days(b1, 2);
                let ism = |kind: u64, i: i32| ar_value(self.seed, kind, i, 51.0, 0.85, 2.0, 1);
                if b1 >= from && b1 <= to {
                    ev.push(Tz::NewYork.to_utc(b1, 600), "US", "ISM Manufacturing", per(), Some(ism(1, i)), Some(ism(1, i - 1)), "", Importance::Medium, None, 1);
                }
                if b3 >= from && b3 <= to {
                    ev.push(Tz::NewYork.to_utc(b3, 600), "US", "ISM Services Index", per(), Some(ism(2, i) + 2.0), Some(ism(2, i - 1) + 2.0), "", Importance::Medium, None, 1);
                }
            }
            // FOMC decisions.
            for y in from.year()..=to.year() {
                for d in fomc_dates(y) {
                    if d >= from && d <= to {
                        let after = st.rates.upper_at(d);
                        let before = st.rates.upper_at(d - Duration::days(1));
                        ev.push(Tz::NewYork.to_utc(d, 14 * 60), "US", "FOMC Rate Decision (Upper Bound)", None, Some(after), Some(before), "%", Importance::High, Some("FEDFUNDS".into()), 2);
                    }
                }
            }
            // Weekly jobless claims (Thursdays, week ending the prior Saturday).
            let mut d = from;
            while d <= to {
                if d.weekday() == Weekday::Thu {
                    let sat = d - Duration::days(5);
                    let per = Some(sat.format("%b %-d").to_string());
                    let v = claims(self.seed, mo, sat) / 1000.0;
                    let p = claims(self.seed, mo, sat - Duration::days(7)) / 1000.0;
                    ev.push(Tz::NewYork.to_utc(d, 8 * 60 + 30), "US", "Initial Jobless Claims", per, Some(v), Some(p), "K", Importance::Medium, Some("ICSA".into()), 0);
                }
                d += Duration::days(1);
            }
        }

        // Europe, UK, Japan, China: rule-based dates and AR(1) values.
        for i in start_i..=end_i {
            let (y, m) = ym_of(i);
            let per = Some(month_abbr(m).to_string());
            let next = crate::cal::add_months(ymd(y, m, 1), 1);
            let (ny, nm) = (next.year(), next.month());
            let mut push_ar = |country: &str, name: &str, d: NaiveDate, tz: Tz, mins: i32, kind: u64, mean: f64, phi: f64, sd: f64, dec: i32, imp: Importance, unit: &str| {
                if d >= from && d <= to && want(country) {
                    let v = ar_value(self.seed, kind, i, mean, phi, sd, dec);
                    let p = ar_value(self.seed, kind, i - 1, mean, phi, sd, dec);
                    ev.push(tz.to_utc(d, mins), country, name, per.clone(), Some(v), Some(p), unit, imp, None, dec);
                }
            };
            push_ar("EU", "Eurozone CPI Estimate YoY", last_weekday_any(y, m), Tz::Cet, 11 * 60, 10, 2.0, 0.95, 0.3, 1, Importance::High, "%");
            push_ar("EU", "Eurozone Composite PMI", Calendar::Weekdays.on_or_after(ymd(y, m, 23)), Tz::Cet, 10 * 60, 11, 50.5, 0.85, 1.5, 1, Importance::Medium, "");
            push_ar("DE", "ZEW Survey Expectations", nth_weekday(y, m, Weekday::Tue, 3), Tz::Cet, 11 * 60, 12, 15.0, 0.8, 10.0, 1, Importance::Medium, "");
            push_ar("DE", "IFO Business Climate", Calendar::Weekdays.on_or_after(ymd(y, m, 24)), Tz::Cet, 10 * 60, 13, 90.0, 0.9, 1.5, 1, Importance::Medium, "");
            push_ar("GB", "UK CPI YoY", nth_weekday(ny, nm, Weekday::Wed, 3), Tz::London, 7 * 60, 14, 2.5, 0.95, 0.35, 1, Importance::High, "%");
            push_ar("JP", "Japan Natl CPI YoY", nth_weekday(ny, nm, Weekday::Fri, 3), Tz::Tokyo, 8 * 60 + 30, 15, 1.0, 0.95, 0.3, 1, Importance::Medium, "%");
            push_ar("CN", "China CPI YoY", Calendar::Weekdays.on_or_after(ymd(ny, nm, 9)), Tz::Shanghai, 9 * 60 + 30, 16, 1.2, 0.9, 0.4, 1, Importance::Medium, "%");
            push_ar("CN", "China Official Manufacturing PMI", Calendar::Weekdays.on_or_before(month_end(y, m)), Tz::Shanghai, 9 * 60 + 30, 17, 50.0, 0.8, 0.8, 1, Importance::Medium, "");
            if m % 3 == 0 {
                let qper = Some(format!("{}Q", m / 3));
                let q = i / 3;
                let mut push_q = |country: &str, name: &str, d: NaiveDate, tz: Tz, mins: i32, kind: u64, mean: f64, sd: f64, imp: Importance, unit: &str| {
                    if d >= from && d <= to && want(country) {
                        let v = ar_value(self.seed, kind, q, mean, 0.7, sd, 1);
                        let p = ar_value(self.seed, kind, q - 1, mean, 0.7, sd, 1);
                        ev.push(tz.to_utc(d, mins), country, name, qper.clone(), Some(v), Some(p), unit, imp, None, 1);
                    }
                };
                push_q("EU", "Eurozone GDP SA QoQ", Calendar::Weekdays.on_or_before(ymd(ny, nm, 30)), Tz::Cet, 11 * 60, 20, 0.3, 0.3, Importance::Medium, "%");
                push_q("CN", "China GDP YoY", Calendar::Weekdays.on_or_after(ymd(ny, nm, 16)), Tz::Shanghai, 10 * 60, 21, 5.0, 0.4, Importance::High, "%");
                let tankan_day = if m == 9 { Calendar::Weekdays.on_or_after(ymd(y, 12, 13)) } else { Calendar::Weekdays.on_or_after(ymd(ny, nm, 1)) };
                push_q("JP", "Tankan Large Manufacturers Index", tankan_day, Tz::Tokyo, 8 * 60 + 50, 22, 10.0, 4.0, Importance::Medium, "");
            }
        }
        for y in from.year()..=to.year() {
            for (country, name, dates, tz, mins, kind, mean, floor) in [
                ("EU", "ECB Deposit Facility Rate", ecb_dates(y), Tz::Cet, 14 * 60 + 15, 30u64, 1.5, -0.5),
                ("GB", "Bank of England Bank Rate", boe_dates(y), Tz::London, 12 * 60, 31, 2.75, 0.1),
                ("JP", "BOJ Target Rate", boj_dates(y), Tz::Tokyo, 12 * 60, 32, 0.3, -0.1),
            ] {
                if !want(country) {
                    continue;
                }
                for (k, d) in dates.iter().enumerate() {
                    if *d >= from && *d <= to {
                        let meeting = (y - START_YEAR) * 8 + k as i32;
                        let after = policy_rate(self.seed, kind, meeting, mean, floor);
                        let before = policy_rate(self.seed, kind, meeting - 1, mean, floor);
                        ev.push(tz.to_utc(*d, mins), country, name, None, Some(after), Some(before), "%", Importance::High, None, 2);
                    }
                }
            }
        }
        out.sort_by(|a, b| a.release_time.cmp(&b.release_time).then(a.event.cmp(&b.event)));
        out
    }
}

fn last_weekday_any(y: i32, m: u32) -> NaiveDate {
    Calendar::Weekdays.on_or_before(month_end(y, m))
}

/// AR(1) indicator value for period `i` (months or quarters since 1990).
fn ar_value(seed: u64, kind: u64, i: i32, mean: f64, phi: f64, sd: f64, dec: i32) -> f64 {
    let mut x = mean;
    let mut r = rng(&[seed, tag("ar-indicator"), kind]);
    for _ in 0..=i.max(0) {
        let z: f64 = r.sample(StandardNormal);
        x = mean + phi * (x - mean) + sd * (1.0 - phi * phi).sqrt() * z;
    }
    round_to(x, dec)
}

/// Policy rate after meeting `n` (8 per year since 1990) for a non-US bank.
fn policy_rate(seed: u64, kind: u64, n: i32, mean: f64, floor: f64) -> f64 {
    let mut r = rng(&[seed, tag("policy"), kind]);
    let (mut d, mut p) = (mean + 2.0, ((mean + 2.0) * 4.0).round() / 4.0);
    for _ in 0..=n.max(0) {
        let z: f64 = r.sample(StandardNormal);
        d += 0.06 * (mean - d) + 0.35 * z;
        let target = (d * 4.0).round() / 4.0;
        let mv = ((target - p).clamp(-0.5, 0.5) * 4.0).round() / 4.0;
        p = (p + mv).max(floor);
    }
    round_to(p, 2)
}

struct EventBuilder<'a> {
    seed: u64,
    now: UnixNanos,
    out: &'a mut Vec<EconomicEvent>,
}

impl EventBuilder<'_> {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        at: UnixNanos,
        country: &str,
        name: &str,
        period: Option<String>,
        value: Option<f64>,
        prior: Option<f64>,
        unit: &str,
        importance: Importance,
        series_id: Option<String>,
        dec: i32,
    ) {
        let mut c = crate::hash::Cell::new(&[self.seed, tag("consensus"), crate::hash::fnv1a(name.as_bytes()), at as u64]);
        let consensus = value.map(|v| {
            let scale = if dec == 0 { (v.abs() * 0.08).max(1.0) } else { 10f64.powi(-dec) * 1.5 };
            round_to(v + scale * c.normal(), dec)
        });
        let released = at <= self.now;
        self.out.push(EconomicEvent {
            release_time: at,
            time_known: true,
            country: country.into(),
            event: name.into(),
            period,
            actual: if released { value } else { None },
            consensus,
            prior,
            unit: (!unit.is_empty()).then(|| unit.into()),
            importance,
            series_id,
            provenance: Provenance::synthetic(self.now),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekday_index_matches_calendar() {
        let r = Rates::compute(1, ymd(1995, 1, 1));
        let days = Calendar::Weekdays.days(ymd(START_YEAR, 1, 1), ymd(1995, 1, 1));
        for (i, d) in days.iter().enumerate().step_by(17) {
            assert_eq!(r.idx(*d), i, "{d}");
        }
    }

    #[test]
    fn fomc_rule_matches_2025() {
        let d = fomc_dates(2025);
        assert_eq!(d[0], ymd(2025, 1, 29));
        assert_eq!(d[1], ymd(2025, 3, 19));
        assert_eq!(d[7], ymd(2025, 12, 10));
    }
}

//! Dividends and splits, analyst recommendations, holders, and profiles.
//!
//! Firm, fund, analyst, and insider names are invented. They are generic
//! compositions chosen not to match real organizations or people.

use chrono::{Datelike, Duration, NaiveDate};
use meridian_types::{
    AnalystRating, CompanyProfile, Dividend, DividendKind, Dividends, Holder, HolderKind, Holders, Provenance,
    Recommendations, UnixNanos,
};

use crate::Inner;
use crate::cal::{Calendar, days_in_month, ymd};
use crate::hash::{Cell, tag};
use crate::market::{round_tick, split_factor_after, splits_of};
use crate::universe::{Kind, Sym};

/// One dividend in path terms (per share on the anchor-date share basis).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DivEvent {
    pub ex: NaiveDate,
    pub declared: NaiveDate,
    pub record: NaiveDate,
    pub pay: NaiveDate,
    pub amount_path: f64,
}

const FIRM_PREFIX: &[&str] = &[
    "Northbridge", "Ashford Lane", "Kestrel Point", "Marlowe Finch", "Bayview Row", "Corbin Hale", "Larkspur",
    "Eastmoor", "Whitcombe", "Holloway Grant", "Brannock", "Quillfield", "Tamsin Reed", "Calder Rowe", "Ravensworth",
    "Penhallow", "Drummond Vale", "Orchard Hill", "Fairhaven Cross", "Greystone Point", "Ellsworth Park",
    "Merriwether", "Hawthorne Bay", "Linden Row", "Alderbrook",
];
const FIRM_SUFFIX: &[&str] =
    &["Securities", "Capital Markets", "Research", "& Co.", "Partners", "Equity Research", "Advisors"];
const INSTITUTION_PREFIX: &[&str] = &[
    "Harbor Ridge", "Granite Peak", "Bluewater Crest", "Summit Hollow", "Ironwood Row", "Cedar Point Lane",
    "Meadowbrook", "Redstone Harbor", "Eastgate Hollow", "Aldergrove", "Copperleaf", "Lakeshore Fiduciary", "Oakmere",
    "Highmoor", "Brightwater Lane", "Kingsmere", "Fernhill", "Thornbury", "Halcyon Ridge", "Pinecrest Vale",
    "Riverbend Crest", "Westmarch", "Silverbirch", "Stonecrop",
];
const INSTITUTION_SUFFIX: &[&str] =
    &["Asset Management", "Capital Partners", "Investors", "Advisors", "Investment Management", "Global Advisors"];
const FUND_SUFFIX: &[&str] = &[
    "Total Market Index Fund", "500 Index Portfolio", "Large Cap Growth Fund", "Dividend Income Fund",
    "Equity Opportunities Fund", "Balanced Fund", "Core Equity Trust", "Blue Chip Fund",
];
pub(crate) const FIRST_NAMES: &[&str] = &[
    "Jordan", "Avery", "Morgan", "Riley", "Casey", "Quinn", "Taylor", "Reese", "Harper", "Rowan", "Emerson",
    "Sawyer", "Dakota", "Hayden", "Parker", "Logan", "Blair", "Cameron", "Devon", "Kendall", "Marlow", "Sutton",
    "Tatum", "Lennox",
];
pub(crate) const LAST_NAMES: &[&str] = &[
    "Ellery", "Thorne", "Vance", "Whitlock", "Ashby", "Calloway", "Draycott", "Fenwick", "Galloway", "Hartwell",
    "Kingsley", "Langford", "Merrick", "Northcott", "Oakley", "Pendleton", "Radcliffe", "Sheridan", "Talbot",
    "Underhill", "Wexley", "Yardley", "Brightman", "Castellan", "Delacourt", "Everhart",
];
const INSIDER_ROLES: &[&str] = &[
    "Chief Executive Officer", "Chief Financial Officer", "Chief Operating Officer", "General Counsel", "Director",
    "Director", "Director", "Chief Technology Officer", "Chief Accounting Officer", "Director",
];

/// Deterministically shuffled indices `0..n`.
pub(crate) fn shuffled(c: &mut Cell, n: usize) -> Vec<usize> {
    let mut v: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        let j = (c.next_u64() % (i as u64 + 1)) as usize;
        v.swap(i, j);
    }
    v
}

pub(crate) fn person(c: &mut Cell) -> String {
    format!("{} {}", c.pick(FIRST_NAMES), c.pick(LAST_NAMES))
}

/// The `n` broker firm names used for a company (distinct).
pub(crate) fn brokers(seed: u64, s: &Sym, n: usize) -> Vec<String> {
    let mut c = Cell::new(&[seed, s.hash, tag("brokers")]);
    let order = shuffled(&mut c, FIRM_PREFIX.len());
    (0..n)
        .map(|i| {
            let p = FIRM_PREFIX[order[i % order.len()]];
            let sfx = FIRM_SUFFIX[(i + (s.hash % 7) as usize) % FIRM_SUFFIX.len()];
            if i < order.len() { format!("{p} {sfx}") } else { format!("{p} {sfx} II") }
        })
        .collect()
}

fn pick_str(c: &mut Cell, items: &[&'static str]) -> &'static str {
    items[(c.next_u64() % items.len() as u64) as usize]
}

fn rating_text(c: &mut Cell, score: u8) -> &'static str {
    match score {
        5 => "Strong Buy",
        4 => pick_str(c, &["Buy", "Outperform", "Overweight", "Accumulate", "Positive"]),
        3 => pick_str(c, &["Hold", "Neutral", "Equal Weight", "Market Perform", "Sector Perform"]),
        2 => pick_str(c, &["Underperform", "Underweight", "Reduce"]),
        _ => pick_str(c, &["Sell", "Strong Sell"]),
    }
}

fn month_name(m: u32) -> &'static str {
    [
        "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November",
        "December",
    ][(m.clamp(1, 12) - 1) as usize]
}

impl Inner {
    /// Dividend schedule in path terms, ascending by ex-date, including
    /// dividends declared on or before `asof`.
    pub(crate) fn dividend_events(&self, s: &Sym, asof: NaiveDate) -> Vec<DivEvent> {
        let Some(spec) = s.p.div else { return Vec::new() };
        let cal = s.p.calendar;
        let per_year = spec.per_year.clamp(1, 12);
        let step = 12 / per_year;
        let first_year = spec.start_year.max(1999);
        let closes = self.closes(s, ymd(first_year - 1, 11, 1), asof);
        let close_before = |d: NaiveDate| -> Option<f64> {
            let i = closes.partition_point(|(cd, _)| *cd < d);
            i.checked_sub(1).map(|i| closes[i].1).or_else(|| closes.first().map(|c| c.1))
        };
        let mut out = Vec::new();
        let mut annual = 0.0f64;
        let mut cell = Cell::new(&[self.seed, s.hash, tag("dividends")]);
        for y in first_year..=asof.year() + 1 {
            let cycle_start = ymd(y, spec.first_month, 1);
            let Some(p) = close_before(cycle_start) else { continue };
            let target = spec.yield_pct / 100.0 * p;
            annual = if annual == 0.0 {
                target
            } else {
                let grown = annual * (1.0 + spec.growth * cell.range(0.5, 1.5));
                if grown < 0.7 * target {
                    0.7 * target
                } else if grown > 1.8 * target {
                    target
                } else {
                    grown
                }
            };
            let amount = (annual / f64::from(per_year) * 10_000.0).round() / 10_000.0;
            for k in 0..per_year {
                let total = (y * 12 + (spec.first_month - 1) as i32) + (k * step) as i32;
                let (yy, mm) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
                let ex = cal.on_or_after(ymd(yy, mm, spec.ex_day.min(days_in_month(yy, mm))));
                let mut c = Cell::new(&[self.seed, s.hash, tag("divdates"), crate::daily::day_num(ex)]);
                let declared = cal.on_or_before(ex - Duration::days(c.int(21, 35)));
                if declared > asof {
                    continue;
                }
                let record = if ex >= ymd(2024, 5, 28) { ex } else { cal.add_days(ex, 1) };
                let pay = Calendar::Weekdays.on_or_after(record + Duration::days(c.int(10, 25)));
                if amount > 0.0 && ex >= s.p.listed {
                    out.push(DivEvent { ex, declared, record, pay, amount_path: amount });
                }
            }
        }
        out.sort_by_key(|d| d.ex);
        out
    }

    pub(crate) fn dividends_for(&self, s: &Sym, now: UnixNanos) -> Option<Dividends> {
        if !matches!(s.p.kind, Kind::Equity | Kind::Etf) {
            return None;
        }
        let today = meridian_types::nanos_to_date(now);
        let splits = splits_of(s);
        let freq = s.p.div.map(|d| match d.per_year {
            12 => "Monthly",
            4 => "Quarter",
            2 => "Semi-Anl",
            _ => "Annual",
        });
        let since = today - Duration::days(3653);
        let mut v: Vec<Dividend> = self
            .dividend_events(s, today)
            .into_iter()
            .filter(|d| d.ex >= since)
            .map(|d| Dividend {
                declared_date: Some(d.declared),
                ex_date: d.ex,
                record_date: Some(d.record),
                pay_date: Some(d.pay),
                amount: (d.amount_path * split_factor_after(&splits, d.ex) * 10_000.0).round() / 10_000.0,
                currency: "USD".into(),
                frequency: freq.map(Into::into),
                kind: DividendKind::Regular,
            })
            .collect();
        for (date, ratio) in splits.iter().filter(|(d, _)| *d <= today) {
            v.push(Dividend {
                declared_date: Some(*date - Duration::days(45)),
                ex_date: *date,
                record_date: None,
                pay_date: None,
                amount: *ratio,
                currency: "USD".into(),
                frequency: None,
                kind: DividendKind::Split,
            });
        }
        v.sort_by(|a, b| b.ex_date.cmp(&a.ex_date));
        Some(Dividends { key: s.key().clone(), dividends: v, per_period: Vec::new(), reported_splits: Vec::new(), provenance: Provenance::synthetic(now) })
    }

    pub(crate) fn recommendations_for(&self, s: &Sym, now: UnixNanos) -> Option<Recommendations> {
        if s.p.kind != Kind::Equity {
            return None;
        }
        let today = meridian_types::nanos_to_date(now);
        let price = self.last_price(s, now)?;
        let n = crate::fundamentals::n_analysts(s) as usize;
        let mut c = Cell::new(&[self.seed, s.hash, tag("recs"), (today.year() * 12 + today.month() as i32) as u64]);
        let bullish = c.range(0.35, 0.75);
        let firms = brokers(self.seed, s, n);
        let mut ratings = Vec::with_capacity(n);
        let mut counts = [0u32; 5];
        let mut targets = Vec::with_capacity(n);
        for (i, firm) in firms.into_iter().enumerate() {
            let mut a = Cell::new(&[self.seed, s.hash, tag("analyst"), i as u64, (today.year() * 12 + today.month() as i32) as u64]);
            let u = a.u01();
            let score: u8 = if u < bullish * 0.25 {
                5
            } else if u < bullish {
                4
            } else if u < bullish + (1.0 - bullish) * 0.8 {
                3
            } else if u < 0.985 {
                2
            } else {
                1
            };
            counts[(5 - score) as usize] += 1;
            let upside = 0.10 + 0.07 * (f64::from(score) - 3.0) + 0.10 * a.normal();
            let raw = price * (1.0 + upside).max(0.3);
            let target = if raw >= 20.0 { raw.round() } else { round_tick(raw, 0.5) };
            targets.push(target);
            let date = today - Duration::days(a.int(0, 150));
            ratings.push(AnalystRating {
                firm,
                analyst: Some(format!("{}. {}", &a.pick(FIRST_NAMES)[..1], a.pick(LAST_NAMES))),
                rating: rating_text(&mut a, score).into(),
                score: Some(score),
                target_price: Some(target),
                date,
            });
        }
        ratings.sort_by(|x, y| y.date.cmp(&x.date).then(x.firm.cmp(&y.firm)));
        let keep = (10 + (s.hash % 16) as usize).min(ratings.len());
        ratings.truncate(keep);
        let mean = targets.iter().sum::<f64>() / targets.len().max(1) as f64;
        Some(Recommendations {
            key: s.key().clone(),
            as_of: today,
            strong_buy: counts[0],
            buy: counts[1],
            hold: counts[2],
            sell: counts[3],
            strong_sell: counts[4],
            target_mean: Some((mean * 100.0).round() / 100.0),
            target_high: targets.iter().copied().reduce(f64::max),
            target_low: targets.iter().copied().reduce(f64::min),
            ratings,
            provenance: Provenance::synthetic(now),
        })
    }

    pub(crate) fn holders_for(&self, s: &Sym, now: UnixNanos) -> Option<Holders> {
        if !matches!(s.p.kind, Kind::Equity | Kind::Etf) {
            return None;
        }
        let today = meridian_types::nanos_to_date(now);
        let price = self.last_price(s, now)?;
        let k = split_factor_after(&splits_of(s), today);
        let outstanding = s.p.shares / k;
        // 13F filings are due 45 days after quarter end.
        let mut qe = crate::cal::month_end(today.year(), ((today.month() - 1) / 3) * 3 + 1);
        qe = crate::cal::add_months(qe, -1);
        let qe = crate::cal::month_end(qe.year(), qe.month());
        let filing = if qe + Duration::days(45) <= today { qe + Duration::days(45) } else { crate::cal::month_end(crate::cal::add_months(qe, -3).year(), crate::cal::add_months(qe, -3).month()) + Duration::days(45) };
        let mut c = Cell::new(&[self.seed, s.hash, tag("holders")]);
        let order = shuffled(&mut c, INSTITUTION_PREFIX.len());
        let mut holders = Vec::with_capacity(30);
        let mut pct = c.range(7.0, 9.5);
        for i in 0..20 {
            let pre = INSTITUTION_PREFIX[order[i % order.len()]];
            let fund = i % 5 == 3;
            let name = if fund {
                format!("{pre} {}", FUND_SUFFIX[(i + s.hash as usize) % FUND_SUFFIX.len()])
            } else {
                format!("{pre} {}", INSTITUTION_SUFFIX[(i * 3 + s.hash as usize) % INSTITUTION_SUFFIX.len()])
            };
            let shares = (outstanding * pct / 100.0).round();
            let change = (shares * 0.04 * c.normal()).round();
            holders.push(Holder {
                name,
                kind: if fund { HolderKind::Fund } else { HolderKind::Institution },
                shares,
                pct_outstanding: Some((pct * 100.0).round() / 100.0),
                market_value: Some((shares * price).round()),
                change_shares: Some(change),
                filing_date: Some(filing - Duration::days(c.int(0, 10))),
                source: Some(if fund { "MOCK N-PORT".into() } else { "MOCK 13F".into() }),
            });
            pct *= c.range(0.72, 0.9);
        }
        holders.sort_by(|a, b| b.shares.total_cmp(&a.shares));
        if s.p.kind == Kind::Equity {
            let mut ic = Cell::new(&[self.seed, s.hash, tag("insiders")]);
            for role in INSIDER_ROLES {
                let shares = (outstanding * ic.range(0.00002, 0.0012)).round();
                holders.push(Holder {
                    name: person(&mut ic),
                    kind: HolderKind::Insider,
                    shares,
                    pct_outstanding: Some((shares / outstanding * 100.0 * 10_000.0).round() / 10_000.0),
                    market_value: Some((shares * price).round()),
                    change_shares: Some(-(shares * ic.range(0.0, 0.08)).round()),
                    filing_date: Some(today - Duration::days(ic.int(3, 200))),
                    source: Some(format!("MOCK Form 4 ({role})")),
                });
            }
        }
        Some(Holders { key: s.key().clone(), holders, provenance: Provenance::synthetic(now) })
    }

    pub(crate) fn profile_for(&self, s: &Sym, now: UnixNanos) -> CompanyProfile {
        let today = meridian_types::nanos_to_date(now);
        let price = self.last_price(s, now);
        let k = split_factor_after(&splits_of(s), today);
        let mut p = CompanyProfile::default();
        match s.p.kind {
            Kind::Equity => {
                let sector = s.inst.sector.clone().unwrap_or_default();
                let industry = s.inst.industry.clone().unwrap_or_default();
                p.description = Some(format!(
                    "[MOCK] {} is shown with a synthetic profile: a {} company in the {} sector. \
                     All figures on this screen are generated locally and are not real.",
                    s.inst.name,
                    industry.to_lowercase(),
                    sector
                ));
                let shares = s.p.shares / k;
                p.shares_outstanding = Some(shares.round());
                p.market_cap = price.map(|px| (px * shares).round());
                p.fiscal_year_end = Some(month_name(s.p.fye_month).into());
                if let Some(model) = self.company(s, today) {
                    let rpe = s.p.sector.map_or(500_000.0, |sec| crate::fundamentals::profile(sec).rev_per_employee);
                    let mut c = Cell::new(&[self.seed, s.hash, tag("employees")]);
                    p.employees = Some(((model.revenue_ref / rpe * c.range(0.7, 1.3)) / 100.0).round() as u64 * 100);
                }
                if s.p.listed > ymd(1999, 1, 4) {
                    p.ipo_date = Some(s.p.listed.format("%Y-%m-%d").to_string());
                }
            }
            Kind::Etf => {
                p.description = Some(format!(
                    "[MOCK] Synthetic profile for {} ({}). Holdings, flows, and prices are generated locally.",
                    s.inst.name,
                    s.inst.industry.clone().unwrap_or_default()
                ));
                p.shares_outstanding = Some(s.p.shares);
                p.market_cap = price.map(|px| (px * s.p.shares).round());
            }
            Kind::Crypto => {
                p.description = Some(format!("[MOCK] Synthetic profile for {}. Supply and prices are generated locally.", s.inst.name));
                p.shares_outstanding = Some(s.p.shares);
                p.market_cap = price.map(|px| (px * s.p.shares).round());
            }
            Kind::Index | Kind::Fx | Kind::Future => {
                p.description = Some(format!("[MOCK] Synthetic data for {}. Levels are generated locally.", s.inst.name));
            }
        }
        p
    }
}

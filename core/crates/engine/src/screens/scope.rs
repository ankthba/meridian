//! What "my securities" means for CALENDAR, FILINGS and TODAY: open
//! positions across every portfolio, the securities on every watchlist,
//! date windows in US market dates, and the in-memory names used for rows.

use std::collections::HashMap;

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, Weekday};
use meridian_types::{MarketSector, SecurityKey, UnixNanos, nanos_to_datetime};

use super::analysis::positions;
use crate::core::Engine;

/// One open position, summed over every portfolio.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Holding {
    pub key: SecurityKey,
    pub qty: f64,
    /// Average-cost basis of the open quantity.
    pub cost: f64,
    /// Net quantity traded on `today` (included in `qty`).
    pub today_qty: f64,
    /// Σ quantity × price of today's trades.
    pub today_cash: f64,
}

impl Holding {
    /// Change in value since the previous close: shares held overnight move
    /// from `prev_close`, shares traded today from their trade price.
    /// `None` without a last price, or without a previous close when shares
    /// were held overnight.
    #[must_use]
    pub fn day_change(&self, last: Option<f64>, prev_close: Option<f64>) -> Option<f64> {
        let last = last?;
        let overnight = self.qty - self.today_qty;
        let held = if overnight.abs() < 1e-9 { 0.0 } else { overnight * (last - prev_close?) };
        Some(held + self.today_qty * last - self.today_cash)
    }
}

/// Which securities a screen covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// Holdings, then watchlists (the default).
    Mine,
    Holdings,
    Watchlists,
    /// Every security the sources cover.
    All,
}

impl Scope {
    pub const OPTIONS: [&'static str; 4] = ["Holdings and watchlists", "Holdings", "Watchlists", "All securities"];

    /// Parses `holdings`, `watchlists`, `all` (and the option labels);
    /// anything else is the default.
    #[must_use]
    pub fn parse(arg: Option<&str>) -> Self {
        match arg.map(|a| a.trim().to_ascii_lowercase()).as_deref() {
            Some("holdings") => Scope::Holdings,
            Some("watchlists" | "watchlist") => Scope::Watchlists,
            Some("all" | "all securities" | "market") => Scope::All,
            _ => Scope::Mine,
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Scope::Mine => Self::OPTIONS[0],
            Scope::Holdings => Self::OPTIONS[1],
            Scope::Watchlists => Self::OPTIONS[2],
            Scope::All => Self::OPTIONS[3],
        }
    }
}

/// An inclusive range of US market dates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Window {
    pub from: NaiveDate,
    pub to: NaiveDate,
    /// The option label it came from.
    pub label: &'static str,
}

/// Range choices: the default (next 10 days), today, this week (Monday to
/// Sunday), next week, this month.
pub(crate) const RANGE_OPTIONS: [&str; 5] = ["Next 10 days", "Today", "This week", "Next week", "This month"];

impl Window {
    /// `today`, `week`, `next week`, `month` (and the option labels);
    /// anything else is the next 10 days from `today`.
    #[must_use]
    pub fn parse(arg: Option<&str>, today: NaiveDate) -> Self {
        let a = arg.map(|a| a.trim().to_ascii_lowercase());
        let a = a.as_deref().map(|a| a.strip_prefix("this ").unwrap_or(a));
        let monday = today - Duration::days(i64::from(today.weekday().num_days_from_monday()));
        match a {
            Some("today") => Window { from: today, to: today, label: RANGE_OPTIONS[1] },
            Some("week") => Window { from: monday, to: monday + Duration::days(6), label: RANGE_OPTIONS[2] },
            Some("next week") => {
                Window { from: monday + Duration::days(7), to: monday + Duration::days(13), label: RANGE_OPTIONS[3] }
            }
            Some("month") => {
                let first = today.with_day(1).unwrap_or(today);
                let next = if first.month() == 12 {
                    NaiveDate::from_ymd_opt(first.year() + 1, 1, 1)
                } else {
                    NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1)
                };
                let last = next.map_or(today, |n| n - Duration::days(1));
                Window { from: first, to: last, label: RANGE_OPTIONS[4] }
            }
            _ => Window { from: today, to: today + Duration::days(9), label: RANGE_OPTIONS[0] },
        }
    }

    #[must_use]
    pub fn describe(&self) -> String {
        if self.from == self.to {
            self.from.format("%a %m/%d/%Y").to_string()
        } else {
            format!("{} – {}", self.from.format("%a %m/%d"), self.to.format("%a %m/%d/%Y"))
        }
    }
}

/// Hours New York is behind UTC at `utc` (US rules since 2007: daylight
/// time from the second Sunday of March 02:00 local to the first Sunday of
/// November 02:00 local).
fn new_york_offset_hours(utc: NaiveDateTime) -> i64 {
    let nth_sunday = |month: u32, n: u32| {
        NaiveDate::from_weekday_of_month_opt(utc.year(), month, Weekday::Sun, u8::try_from(n).unwrap_or(1))
    };
    let (Some(start), Some(end)) = (nth_sunday(3, 2), nth_sunday(11, 1)) else { return 5 };
    // 02:00 EST = 07:00 UTC; 02:00 EDT = 06:00 UTC.
    let (start, end) = (start.and_hms_opt(7, 0, 0), end.and_hms_opt(6, 0, 0));
    match (start, end) {
        (Some(s), Some(e)) if utc >= s && utc < e => 4,
        _ => 5,
    }
}

/// New York wall-clock time for `ns`.
#[must_use]
pub fn new_york_time(ns: UnixNanos) -> NaiveDateTime {
    let utc = nanos_to_datetime(ns).naive_utc();
    utc - Duration::hours(new_york_offset_hours(utc))
}

/// The US market date for `ns` (New York calendar date).
#[must_use]
pub fn new_york_date(ns: UnixNanos) -> NaiveDate {
    new_york_time(ns).date()
}

/// Securities that can have SEC filings, earnings or dividends: US-style
/// equity and preferred keys.
#[must_use]
pub(crate) fn is_company_key(k: &SecurityKey) -> bool {
    matches!(k.sector, MarketSector::Equity | MarketSector::Pfd)
}

impl Engine {
    /// Open positions summed over every portfolio, in order of first trade.
    /// `today` is the market date whose trades count as today's.
    pub(crate) fn holdings(&self, today: NaiveDate) -> Vec<Holding> {
        let store = &self.stores().app;
        let today_s = today.format("%Y-%m-%d").to_string();
        let mut out: Vec<Holding> = Vec::new();
        for p in store.portfolios().unwrap_or_default() {
            let txs = store.transactions(p.id).unwrap_or_default();
            let mut todays: HashMap<&str, (f64, f64)> = HashMap::new();
            for t in txs.iter().filter(|t| t.trade_date == today_s) {
                let e = todays.entry(t.security.as_str()).or_default();
                e.0 += t.quantity;
                e.1 += t.quantity * t.price;
            }
            for (sec, pos) in positions(&txs) {
                if pos.qty.abs() < 1e-9 {
                    continue;
                }
                let Ok(key) = sec.parse::<SecurityKey>() else { continue };
                let (tq, tc) = todays.get(sec.as_str()).copied().unwrap_or_default();
                match out.iter_mut().find(|h| h.key == key) {
                    Some(h) => {
                        h.qty += pos.qty;
                        h.cost += pos.cost;
                        h.today_qty += tq;
                        h.today_cash += tc;
                    }
                    None => out.push(Holding { key, qty: pos.qty, cost: pos.cost, today_qty: tq, today_cash: tc }),
                }
            }
        }
        out.retain(|h| h.qty.abs() > 1e-9);
        out
    }

    /// Every watchlist's securities, in list then position order, without
    /// duplicates.
    pub(crate) fn watchlist_keys(&self) -> Vec<SecurityKey> {
        let mut out: Vec<SecurityKey> = Vec::new();
        for l in self.stores().app.watchlists().unwrap_or_default() {
            for k in l.securities.iter().filter_map(|s| s.parse::<SecurityKey>().ok()) {
                if !out.contains(&k) {
                    out.push(k);
                }
            }
        }
        out
    }

    /// The securities `scope` names (empty for [`Scope::All`], meaning "no
    /// filter").
    pub(crate) fn scope_keys(&self, scope: Scope, today: NaiveDate) -> Vec<SecurityKey> {
        let mut keys: Vec<SecurityKey> = match scope {
            Scope::All => return Vec::new(),
            Scope::Watchlists => Vec::new(),
            Scope::Mine | Scope::Holdings => self.holdings(today).into_iter().map(|h| h.key).collect(),
        };
        if matches!(scope, Scope::Mine | Scope::Watchlists) {
            for k in self.watchlist_keys() {
                if !keys.contains(&k) {
                    keys.push(k);
                }
            }
        }
        keys
    }

    /// Display name from reference data already in memory (no network), or
    /// the symbol.
    pub(crate) fn known_name(&self, key: &SecurityKey) -> String {
        self.instrument_in_memory(key).map_or_else(|| key.symbol.clone(), |i| i.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn windows() {
        let wed = d(2026, 10, 7);
        assert_eq!(Window::parse(None, wed), Window { from: wed, to: d(2026, 10, 16), label: "Next 10 days" });
        assert_eq!(Window::parse(Some("today"), wed).to, wed);
        let w = Window::parse(Some("week"), wed);
        assert_eq!((w.from, w.to), (d(2026, 10, 5), d(2026, 10, 11)));
        assert_eq!(Window::parse(Some("This week"), wed), w);
        let n = Window::parse(Some("next week"), wed);
        assert_eq!((n.from, n.to), (d(2026, 10, 12), d(2026, 10, 18)));
        let m = Window::parse(Some("month"), wed);
        assert_eq!((m.from, m.to), (d(2026, 10, 1), d(2026, 10, 31)));
        let dec = Window::parse(Some("This month"), d(2026, 12, 15));
        assert_eq!((dec.from, dec.to), (d(2026, 12, 1), d(2026, 12, 31)));
        // A Sunday belongs to the week that started the Monday before.
        let sun = Window::parse(Some("week"), d(2026, 10, 11));
        assert_eq!(sun.from, d(2026, 10, 5));
        assert_eq!(Window::parse(Some("whenever"), wed).label, "Next 10 days");
    }

    #[test]
    fn scopes() {
        assert_eq!(Scope::parse(None), Scope::Mine);
        assert_eq!(Scope::parse(Some("Holdings")), Scope::Holdings);
        assert_eq!(Scope::parse(Some("watchlists")), Scope::Watchlists);
        assert_eq!(Scope::parse(Some("all")), Scope::All);
        assert_eq!(Scope::parse(Some("All securities")), Scope::All);
        for o in Scope::OPTIONS {
            assert_eq!(Scope::parse(Some(o)).label(), o);
        }
    }

    #[test]
    fn new_york_dates_follow_daylight_time() {
        let at = |y, m, day, h| meridian_types::datetime_to_nanos(d(y, m, day).and_hms_opt(h, 0, 0).unwrap().and_utc());
        // 2026-10-08 01:00 UTC is 21:00 on the 7th in New York (EDT).
        assert_eq!(new_york_date(at(2026, 10, 8, 1)), d(2026, 10, 7));
        assert_eq!(new_york_date(at(2026, 10, 8, 4)), d(2026, 10, 8));
        // January: EST, five hours.
        assert_eq!(new_york_time(at(2026, 1, 15, 14)).time(), chrono::NaiveTime::from_hms_opt(9, 0, 0).unwrap());
        // DST starts 2026-03-08 07:00 UTC and ends 2026-11-01 06:00 UTC.
        assert_eq!(new_york_offset_hours(d(2026, 3, 8).and_hms_opt(6, 59, 0).unwrap()), 5);
        assert_eq!(new_york_offset_hours(d(2026, 3, 8).and_hms_opt(7, 0, 0).unwrap()), 4);
        assert_eq!(new_york_offset_hours(d(2026, 11, 1).and_hms_opt(5, 59, 0).unwrap()), 4);
        assert_eq!(new_york_offset_hours(d(2026, 11, 1).and_hms_opt(6, 0, 0).unwrap()), 5);
    }

    #[test]
    fn day_change_counts_overnight_and_todays_shares() {
        // Held 10 overnight at a 100 close, sold 5 at 105, last 110.
        let h = Holding { key: SecurityKey::equity("X"), qty: 5.0, cost: 0.0, today_qty: -5.0, today_cash: -525.0 };
        assert!((h.day_change(Some(110.0), Some(100.0)).unwrap() - 75.0).abs() < 1e-9);
        // Bought everything today: no previous close needed.
        let b = Holding { key: SecurityKey::equity("X"), qty: 5.0, cost: 0.0, today_qty: 5.0, today_cash: 540.0 };
        assert!((b.day_change(Some(110.0), None).unwrap() - 10.0).abs() < 1e-9);
        // Held overnight without a previous close: unknown.
        let o = Holding { key: SecurityKey::equity("X"), qty: 5.0, cost: 0.0, today_qty: 0.0, today_cash: 0.0 };
        assert_eq!(o.day_change(Some(110.0), None), None);
        assert_eq!(o.day_change(None, Some(100.0)), None);
    }
}

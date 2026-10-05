//! Exchange calendars, holidays, time zones (with DST), and sessions.
//!
//! Implemented locally so the mock has no time-zone database dependency.
//! Holiday coverage: the NYSE list (with weekend observance and the
//! unscheduled closures since 2001) and the US bond-market additions.
//! Non-US markets close on weekends only; that's a documented simplification.

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use meridian_types::{NANOS_PER_SEC, UnixNanos, date_to_nanos, nanos_to_date};

pub(crate) const NANOS_PER_MIN: i64 = 60 * NANOS_PER_SEC;

/// Builds a date from parts known to be valid.
pub(crate) fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
    // Callers pass constant or range-checked parts; the fallback is never hit
    // for valid input and keeps this function panic-free.
    NaiveDate::from_ymd_opt(y, m, d).unwrap_or_default()
}

pub(crate) fn days_in_month(y: i32, m: u32) -> u32 {
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    (ymd(ny, nm, 1) - ymd(y, m, 1)).num_days() as u32
}

pub(crate) fn month_end(y: i32, m: u32) -> NaiveDate {
    ymd(y, m, days_in_month(y, m))
}

/// Adds `n` calendar months, clamping the day.
pub(crate) fn add_months(d: NaiveDate, n: i32) -> NaiveDate {
    let total = d.year() * 12 + d.month0() as i32 + n;
    let (y, m) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
    ymd(y, m, d.day().min(days_in_month(y, m)))
}

/// The `n`-th (1-based) `wd` of a month.
pub(crate) fn nth_weekday(y: i32, m: u32, wd: Weekday, n: u32) -> NaiveDate {
    let first = ymd(y, m, 1);
    let off = (7 + wd.num_days_from_monday() - first.weekday().num_days_from_monday()) % 7;
    first + Duration::days(i64::from(off + 7 * (n.max(1) - 1)))
}

/// The last `wd` of a month.
pub(crate) fn last_weekday(y: i32, m: u32, wd: Weekday) -> NaiveDate {
    let last = month_end(y, m);
    let back = (7 + last.weekday().num_days_from_monday() - wd.num_days_from_monday()) % 7;
    last - Duration::days(i64::from(back))
}

/// Gregorian Easter Sunday (anonymous Gregorian / Meeus-Jones-Butcher).
pub(crate) fn easter(y: i32) -> NaiveDate {
    let a = y % 19;
    let b = y / 100;
    let c = y % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = (h + l - 7 * m + 114) % 31 + 1;
    ymd(y, month as u32, day as u32)
}

pub(crate) fn is_weekend(d: NaiveDate) -> bool {
    matches!(d.weekday(), Weekday::Sat | Weekday::Sun)
}

/// Saturday holidays move to Friday, Sunday holidays to Monday.
fn observed(d: NaiveDate) -> NaiveDate {
    match d.weekday() {
        Weekday::Sat => d - Duration::days(1),
        Weekday::Sun => d + Duration::days(1),
        _ => d,
    }
}

/// Unscheduled NYSE closures since 2001.
const SPECIAL_CLOSURES: [(i32, u32, u32); 10] = [
    (2001, 9, 11),
    (2001, 9, 12),
    (2001, 9, 13),
    (2001, 9, 14),
    (2004, 6, 11),
    (2007, 1, 2),
    (2012, 10, 29),
    (2012, 10, 30),
    (2018, 12, 5),
    (2025, 1, 9),
];

/// NYSE full-day holidays for a year, observed dates.
pub(crate) fn nyse_holidays(y: i32) -> Vec<NaiveDate> {
    let mut v = Vec::with_capacity(14);
    // New Year's Day: Sunday moves to Monday; Saturday is not observed on
    // the prior Friday (NYSE rule 7.2).
    let ny = ymd(y, 1, 1);
    match ny.weekday() {
        Weekday::Sat => {}
        Weekday::Sun => v.push(ny + Duration::days(1)),
        _ => v.push(ny),
    }
    if y >= 1998 {
        v.push(nth_weekday(y, 1, Weekday::Mon, 3)); // Martin Luther King Jr. Day
    }
    v.push(nth_weekday(y, 2, Weekday::Mon, 3)); // Washington's Birthday
    v.push(easter(y) - Duration::days(2)); // Good Friday
    v.push(last_weekday(y, 5, Weekday::Mon)); // Memorial Day
    if y >= 2022 {
        v.push(observed(ymd(y, 6, 19))); // Juneteenth
    }
    v.push(observed(ymd(y, 7, 4))); // Independence Day
    v.push(nth_weekday(y, 9, Weekday::Mon, 1)); // Labor Day
    v.push(nth_weekday(y, 11, Weekday::Thu, 4)); // Thanksgiving
    v.push(observed(ymd(y, 12, 25))); // Christmas
    for (sy, sm, sd) in SPECIAL_CLOSURES {
        if sy == y {
            v.push(ymd(sy, sm, sd));
        }
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// US bond market (SIFMA-style): NYSE holidays plus Columbus and Veterans Day.
pub(crate) fn us_bond_holidays(y: i32) -> Vec<NaiveDate> {
    let mut v = nyse_holidays(y);
    v.push(nth_weekday(y, 10, Weekday::Mon, 2));
    let vet = ymd(y, 11, 11);
    match vet.weekday() {
        Weekday::Sat => {}
        Weekday::Sun => v.push(vet + Duration::days(1)),
        _ => v.push(vet),
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// Which days an instrument trades.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Calendar {
    /// NYSE: weekdays minus NYSE holidays.
    Nyse,
    /// US bond market: weekdays minus NYSE holidays, Columbus and Veterans Day.
    UsBond,
    /// Monday–Friday (FX, non-US indices).
    Weekdays,
    /// Every day (crypto).
    AllDays,
}

impl Calendar {
    fn holidays(self, y: i32) -> Vec<NaiveDate> {
        match self {
            Calendar::Nyse => nyse_holidays(y),
            Calendar::UsBond => us_bond_holidays(y),
            Calendar::Weekdays | Calendar::AllDays => Vec::new(),
        }
    }

    pub(crate) fn is_trading_day(self, d: NaiveDate) -> bool {
        match self {
            Calendar::AllDays => true,
            Calendar::Weekdays => !is_weekend(d),
            _ => !is_weekend(d) && !self.holidays(d.year()).contains(&d),
        }
    }

    /// Trading days in `[from, to]`, ascending.
    pub(crate) fn days(self, from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
        if to < from {
            return Vec::new();
        }
        let span = (to - from).num_days().max(0) as usize + 1;
        let mut out = Vec::with_capacity(if self == Calendar::AllDays { span } else { span * 5 / 7 + 2 });
        let mut year = from.year();
        let mut hol = self.holidays(year);
        let mut d = from;
        while d <= to {
            if d.year() != year {
                year = d.year();
                hol = self.holidays(year);
            }
            let ok = match self {
                Calendar::AllDays => true,
                Calendar::Weekdays => !is_weekend(d),
                _ => !is_weekend(d) && !hol.contains(&d),
            };
            if ok {
                out.push(d);
            }
            d += Duration::days(1);
        }
        out
    }

    /// Latest trading day `<= d`.
    pub(crate) fn on_or_before(self, mut d: NaiveDate) -> NaiveDate {
        for _ in 0..15 {
            if self.is_trading_day(d) {
                return d;
            }
            d -= Duration::days(1);
        }
        d
    }

    /// Latest trading day `< d`.
    pub(crate) fn before(self, d: NaiveDate) -> NaiveDate {
        self.on_or_before(d - Duration::days(1))
    }

    /// Earliest trading day `>= d`.
    pub(crate) fn on_or_after(self, mut d: NaiveDate) -> NaiveDate {
        for _ in 0..15 {
            if self.is_trading_day(d) {
                return d;
            }
            d += Duration::days(1);
        }
        d
    }

    /// Adds `n` trading days (n >= 0).
    pub(crate) fn add_days(self, mut d: NaiveDate, n: u32) -> NaiveDate {
        for _ in 0..n {
            d = self.on_or_after(d + Duration::days(1));
        }
        d
    }
}

/// Time zones the mock needs, with their DST rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Tz {
    Utc,
    NewYork,
    London,
    /// Central European Time (Frankfurt, Paris, Zurich, Amsterdam, Madrid, Milan).
    Cet,
    Tokyo,
    HongKong,
    Shanghai,
    Seoul,
    Sydney,
    India,
    Taipei,
    Singapore,
    MexicoCity,
    SaoPaulo,
}

/// US daylight saving time for a local date (2007+ rule, and the 1987–2006
/// rule before that). Transitions happen at 02:00 local, outside any session.
pub(crate) fn us_dst(d: NaiveDate) -> bool {
    let y = d.year();
    let (start, end) = if y >= 2007 {
        (nth_weekday(y, 3, Weekday::Sun, 2), nth_weekday(y, 11, Weekday::Sun, 1))
    } else {
        (nth_weekday(y, 4, Weekday::Sun, 1), last_weekday(y, 10, Weekday::Sun))
    };
    d >= start && d < end
}

/// EU summer time: last Sunday of March to last Sunday of October.
pub(crate) fn eu_dst(d: NaiveDate) -> bool {
    let y = d.year();
    d >= last_weekday(y, 3, Weekday::Sun) && d < last_weekday(y, 10, Weekday::Sun)
}

/// New South Wales: first Sunday of October to first Sunday of April.
fn au_dst(d: NaiveDate) -> bool {
    let y = d.year();
    d < nth_weekday(y, 4, Weekday::Sun, 1) || d >= nth_weekday(y, 10, Weekday::Sun, 1)
}

impl Tz {
    /// UTC offset in minutes for a local calendar date.
    pub(crate) fn offset_minutes(self, d: NaiveDate) -> i32 {
        match self {
            Tz::Utc => 0,
            Tz::NewYork => {
                if us_dst(d) {
                    -240
                } else {
                    -300
                }
            }
            Tz::London => {
                if eu_dst(d) {
                    60
                } else {
                    0
                }
            }
            Tz::Cet => {
                if eu_dst(d) {
                    120
                } else {
                    60
                }
            }
            Tz::Tokyo | Tz::Seoul => 540,
            Tz::HongKong | Tz::Shanghai | Tz::Taipei | Tz::Singapore => 480,
            Tz::Sydney => {
                if au_dst(d) {
                    660
                } else {
                    600
                }
            }
            Tz::India => 330,
            Tz::MexicoCity => -360,
            Tz::SaoPaulo => -180,
        }
    }

    /// UTC instant of a local date + minutes after local midnight.
    pub(crate) fn to_utc(self, d: NaiveDate, local_minutes: i32) -> UnixNanos {
        date_to_nanos(d) + i64::from(local_minutes - self.offset_minutes(d)) * NANOS_PER_MIN
    }

    /// Local date and minutes after local midnight for a UTC instant.
    pub(crate) fn to_local(self, ns: UnixNanos) -> (NaiveDate, i32) {
        let utc_date = nanos_to_date(ns);
        let mut off = self.offset_minutes(utc_date);
        let mut local = ns + i64::from(off) * NANOS_PER_MIN;
        let local_date = nanos_to_date(local);
        let off2 = self.offset_minutes(local_date);
        if off2 != off {
            off = off2;
            local = ns + i64::from(off) * NANOS_PER_MIN;
        }
        let date = nanos_to_date(local);
        let mins = ((local - date_to_nanos(date)) / NANOS_PER_MIN) as i32;
        (date, mins)
    }
}

/// A daily trading session in local time. Lunch breaks are ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Session {
    pub tz: Tz,
    /// Local minutes after midnight.
    pub open: i32,
    pub close: i32,
}

impl Session {
    pub(crate) const fn new(tz: Tz, open_hm: (i32, i32), close_hm: (i32, i32)) -> Self {
        Self { tz, open: open_hm.0 * 60 + open_hm.1, close: close_hm.0 * 60 + close_hm.1 }
    }

    pub(crate) const US_EQUITY: Session = Session::new(Tz::NewYork, (9, 30), (16, 0));
    pub(crate) const UTC_24H: Session = Session::new(Tz::Utc, (0, 0), (24, 0));

    pub(crate) fn minutes(&self) -> i32 {
        self.close - self.open
    }

    pub(crate) fn is_24h(&self) -> bool {
        self.minutes() >= 1440
    }

    pub(crate) fn open_utc(&self, d: NaiveDate) -> UnixNanos {
        self.tz.to_utc(d, self.open)
    }

    pub(crate) fn close_utc(&self, d: NaiveDate) -> UnixNanos {
        self.tz.to_utc(d, self.close)
    }
}

/// Where an instrument's market is at an instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SessionState {
    /// Local calendar date at `now`.
    pub today: NaiveDate,
    pub today_trading: bool,
    /// Whole minutes of today's session that have completed (0..=minutes).
    pub elapsed: i32,
    pub minutes: i32,
}

impl SessionState {
    pub(crate) fn at(cal: Calendar, s: &Session, now: UnixNanos) -> Self {
        let (today, local_min) = s.tz.to_local(now);
        let minutes = s.minutes();
        let elapsed = (local_min - s.open).clamp(0, minutes);
        Self { today, today_trading: cal.is_trading_day(today), elapsed, minutes }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.today_trading && self.elapsed > 0 && self.elapsed < self.minutes
    }

    /// Today's session has fully completed.
    pub(crate) fn today_complete(&self) -> bool {
        self.today_trading && self.elapsed >= self.minutes
    }

    /// Most recent trading day whose session has fully completed.
    pub(crate) fn last_complete_day(&self, cal: Calendar) -> NaiveDate {
        if self.today_complete() { self.today } else { cal.before(self.today) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easter_known_dates() {
        assert_eq!(easter(2024), ymd(2024, 3, 31));
        assert_eq!(easter(2025), ymd(2025, 4, 20));
        assert_eq!(easter(2026), ymd(2026, 4, 5));
        assert_eq!(easter(2000), ymd(2000, 4, 23));
        assert_eq!(easter(2038), ymd(2038, 4, 25));
    }

    #[test]
    fn nyse_holidays_2026() {
        let h = nyse_holidays(2026);
        let expect = [
            ymd(2026, 1, 1),
            ymd(2026, 1, 19),
            ymd(2026, 2, 16),
            ymd(2026, 4, 3),
            ymd(2026, 5, 25),
            ymd(2026, 6, 19),
            ymd(2026, 7, 3),
            ymd(2026, 9, 7),
            ymd(2026, 11, 26),
            ymd(2026, 12, 25),
        ];
        assert_eq!(h, expect);
    }

    #[test]
    fn saturday_new_year_not_observed() {
        // 2022-01-01 was a Saturday; NYSE stayed open on 2021-12-31.
        assert!(Calendar::Nyse.is_trading_day(ymd(2021, 12, 31)));
        assert!(!Calendar::Nyse.is_trading_day(ymd(2025, 1, 9)));
        assert!(!Calendar::Nyse.is_trading_day(ymd(2021, 7, 5)));
    }

    #[test]
    fn dst_rules() {
        assert!(!us_dst(ymd(2026, 3, 7)));
        assert!(us_dst(ymd(2026, 3, 8)));
        assert!(us_dst(ymd(2026, 10, 31)));
        assert!(!us_dst(ymd(2026, 11, 1)));
        assert!(eu_dst(ymd(2026, 3, 29)));
        assert!(!eu_dst(ymd(2026, 3, 28)));
        // 09:30 New York in winter is 14:30 UTC, in summer 13:30 UTC.
        let w = Session::US_EQUITY.open_utc(ymd(2026, 1, 5));
        let s = Session::US_EQUITY.open_utc(ymd(2026, 7, 6));
        assert_eq!((w - date_to_nanos(ymd(2026, 1, 5))) / NANOS_PER_MIN, 14 * 60 + 30);
        assert_eq!((s - date_to_nanos(ymd(2026, 7, 6))) / NANOS_PER_MIN, 13 * 60 + 30);
    }

    #[test]
    fn to_local_round_trip() {
        for tz in [Tz::NewYork, Tz::Tokyo, Tz::Sydney, Tz::Cet, Tz::India] {
            let d = ymd(2026, 10, 5);
            let ns = tz.to_utc(d, 600);
            assert_eq!(tz.to_local(ns), (d, 600));
        }
    }

    #[test]
    fn trading_days_skip_weekends_and_holidays() {
        let days = Calendar::Nyse.days(ymd(2026, 11, 23), ymd(2026, 11, 30));
        assert_eq!(days, vec![ymd(2026, 11, 23), ymd(2026, 11, 24), ymd(2026, 11, 25), ymd(2026, 11, 27), ymd(2026, 11, 30)]);
        assert_eq!(Calendar::AllDays.days(ymd(2026, 1, 1), ymd(2026, 1, 7)).len(), 7);
    }
}

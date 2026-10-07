//! Cell value parsing shared by every format: money and quantities with
//! `$`, thousands separators, parenthesized or trailing negatives, and the
//! date layouts US brokers use.

use chrono::NaiveDate;

/// Why a cell could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadValue {
    pub what: &'static str,
    pub text: String,
}

impl BadValue {
    fn new(what: &'static str, text: &str) -> Self {
        Self { what, text: text.trim().to_owned() }
    }

    /// `"price '12..3'"`.
    #[must_use]
    pub fn describe(&self) -> String {
        format!("{} '{}'", self.what, self.text)
    }
}

/// Placeholders brokers write for "no value".
fn is_missing(s: &str) -> bool {
    matches!(s, "" | "-" | "--" | "---" | "n/a" | "N/A" | "NA" | "na" | "None" | "none")
}

/// Parses a number written like `1,234.56`, `$1,234.56`, `-$1,234.56`,
/// `$-1,234.56`, `($1,234.56)`, `1,234.56-`, `+0.5` or `1.5e3`. Returns
/// `Ok(None)` for an empty cell or a missing-value placeholder (`--`,
/// `n/a`).
pub fn number(raw: &str, what: &'static str) -> Result<Option<f64>, BadValue> {
    let s = raw.trim();
    if is_missing(s) {
        return Ok(None);
    }
    let mut negative = false;
    let mut t = s;
    if t.starts_with('(') && t.ends_with(')') && t.len() >= 2 {
        negative = true;
        t = t[1..t.len() - 1].trim();
    }
    if let Some(rest) = t.strip_suffix('-') {
        negative = !negative;
        t = rest.trim();
    }
    let mut cleaned = String::with_capacity(t.len());
    let mut sign_seen = false;
    for c in t.chars() {
        match c {
            '$' | ',' | ' ' | '\u{a0}' => {}
            '-' | '\u{2212}' if cleaned.is_empty() && !sign_seen => {
                negative = !negative;
                sign_seen = true;
            }
            '+' if cleaned.is_empty() && !sign_seen => sign_seen = true,
            _ => cleaned.push(c),
        }
    }
    if cleaned.is_empty() || !cleaned.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '-' | '+')) {
        return Err(BadValue::new(what, raw));
    }
    let v: f64 = cleaned.parse().map_err(|_| BadValue::new(what, raw))?;
    if !v.is_finite() {
        return Err(BadValue::new(what, raw));
    }
    Ok(Some(if negative { -v } else { v }))
}

/// Date layouts accepted in a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateOrder {
    /// `MM/DD/YYYY` (US brokers), also `YYYY-MM-DD`.
    MonthFirst,
    /// `DD/MM/YYYY` or `DD.MM.YYYY`, also `YYYY-MM-DD`.
    DayFirst,
}

/// Parses a date cell: `MM/DD/YYYY` (or `DD/MM/YYYY`), one-digit month and
/// day, two-digit years (`MM/DD/YY`, read as 20YY), and ISO `YYYY-MM-DD` or
/// `YYYY/MM/DD`. A trailing time (`10/07/2026 09:30:00`) is ignored.
pub fn date(raw: &str, order: DateOrder) -> Result<NaiveDate, BadValue> {
    let s = raw.trim();
    let head = s.split(|c: char| c.is_whitespace() || c == 'T').next().unwrap_or("");
    let bad = || BadValue::new("date", raw);
    let parts: Vec<&str> = head.split(['/', '-', '.']).collect();
    let [a, b, c] = parts.as_slice() else { return Err(bad()) };
    let num = |x: &str| -> Option<u32> {
        if x.is_empty() || x.len() > 4 || !x.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        x.parse().ok()
    };
    let (Some(a_n), Some(b_n), Some(c_n)) = (num(a), num(b), num(c)) else { return Err(bad()) };
    let (y, m, d) = if a.len() == 4 {
        (a_n, b_n, c_n)
    } else {
        let year = match c.len() {
            4 => c_n,
            2 => 2000 + c_n,
            _ => return Err(bad()),
        };
        match order {
            DateOrder::MonthFirst => (year, a_n, b_n),
            DateOrder::DayFirst => (year, b_n, a_n),
        }
    };
    NaiveDate::from_ymd_opt(y as i32, m, d).ok_or_else(bad)
}

/// Normalizes a ticker cell: trims, upper-cases, and drops Fidelity's `**`
/// marker on core (money market) positions. Returns `None` for an empty
/// cell or a placeholder.
#[must_use]
pub fn symbol(raw: &str) -> Option<String> {
    let s = raw.trim().trim_end_matches('*').trim();
    if is_missing(s) {
        return None;
    }
    Some(s.to_ascii_uppercase())
}

/// Collapses whitespace (including the line breaks inside quoted cells).
#[must_use]
pub fn text(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> Option<f64> {
        number(s, "amount").unwrap()
    }

    #[test]
    fn money_formats() {
        assert_eq!(n("1,234.56"), Some(1234.56));
        assert_eq!(n("$1,234.56"), Some(1234.56));
        assert_eq!(n("-$1,234.56"), Some(-1234.56));
        assert_eq!(n("$-1,234.56"), Some(-1234.56));
        assert_eq!(n("($1,234.56)"), Some(-1234.56));
        assert_eq!(n("(12.5)"), Some(-12.5));
        assert_eq!(n("1,234.56-"), Some(-1234.56));
        assert_eq!(n(" +0.5 "), Some(0.5));
        assert_eq!(n("1.5e3"), Some(1500.0));
        assert_eq!(n("\u{2212}3"), Some(-3.0));
        assert_eq!(n("0.000123"), Some(0.000123));
    }

    #[test]
    fn missing_values() {
        for s in ["", "  ", "--", "-", "n/a", "N/A"] {
            assert_eq!(n(s), None, "{s:?}");
        }
    }

    #[test]
    fn garbage_is_an_error() {
        for s in ["abc", "12..3", "$", "1 2 3x", "1-2", "()"] {
            let e = number(s, "price").unwrap_err();
            assert_eq!(e.what, "price", "{s:?}");
        }
        assert_eq!(number("abc", "price").unwrap_err().describe(), "price 'abc'");
    }

    #[test]
    fn dates() {
        let d = |y, m, dd| NaiveDate::from_ymd_opt(y, m, dd).unwrap();
        let us = |s| date(s, DateOrder::MonthFirst).unwrap();
        assert_eq!(us("10/07/2026"), d(2026, 10, 7));
        assert_eq!(us("1/2/2026"), d(2026, 1, 2));
        assert_eq!(us("01/02/26"), d(2026, 1, 2));
        assert_eq!(us("2026-10-07"), d(2026, 10, 7));
        assert_eq!(us("2026/10/07"), d(2026, 10, 7));
        assert_eq!(us("10/07/2026 09:30:00"), d(2026, 10, 7));
        assert_eq!(us("2026-10-07T09:30:00Z"), d(2026, 10, 7));
        assert_eq!(date("07/10/2026", DateOrder::DayFirst).unwrap(), d(2026, 10, 7));
        assert_eq!(date("07.10.2026", DateOrder::DayFirst).unwrap(), d(2026, 10, 7));
        for bad in ["", "13/01/2026", "02/30/2026", "2026-10", "Oct 7 2026", "10/07/202", "1/1/1"] {
            assert!(date(bad, DateOrder::MonthFirst).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn symbols_and_text() {
        assert_eq!(symbol(" spaxx** ").as_deref(), Some("SPAXX"));
        assert_eq!(symbol("BRK.B").as_deref(), Some("BRK.B"));
        assert_eq!(symbol("--"), None);
        assert_eq!(symbol(""), None);
        assert_eq!(text("Apple\nCUSIP:  037833100 "), "Apple CUSIP: 037833100");
    }
}

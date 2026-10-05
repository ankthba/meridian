//! Parser for the Daily Treasury Par Yield Curve XML feed (Atom + OData
//! properties). Tenors are read from the element names (`BC_10YEAR`,
//! `BC_1_5MONTH`, …) rather than a fixed list, so tenors Treasury adds or
//! drops over time are picked up without code changes.

use chrono::{DateTime, NaiveDate, Utc};
use quick_xml::Reader;
use quick_xml::events::Event;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum TenorUnit {
    Week,
    Month,
    Year,
}

/// A curve tenor, stored exactly (hundredths of a unit) so `1.5 Mo` from a
/// series ID and `BC_1_5MONTH` from the feed compare equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Tenor {
    unit: TenorUnit,
    hundredths: u32,
}

impl Tenor {
    /// `BC_10YEAR` → 10 Yr, `BC_1_5MONTH` → 1.5 Mo. `BC_30YEARDISPLAY` (a
    /// display duplicate that reads 0.00 when the 30-year wasn't published)
    /// and other non-tenor elements → `None`.
    pub(crate) fn from_element(name: &str) -> Option<Tenor> {
        let rest = name.strip_prefix("BC_")?;
        let (num, unit) = if let Some(n) = rest.strip_suffix("MONTH") {
            (n, TenorUnit::Month)
        } else if let Some(n) = rest.strip_suffix("YEAR") {
            (n, TenorUnit::Year)
        } else if let Some(n) = rest.strip_suffix("WEEK") {
            (n, TenorUnit::Week)
        } else {
            return None;
        };
        Some(Tenor { unit, hundredths: parse_hundredths(&num.replace('_', "."))? })
    }

    /// Parses user-facing tenor labels: `10 Yr`, `10Y`, `10 Years`, `1.5 Mo`,
    /// `1.5 Month`, `3M`, `6 Wk`. Case-insensitive.
    pub(crate) fn parse_label(s: &str) -> Option<Tenor> {
        let s = s.trim();
        let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
        let (num, unit) = s.split_at(split);
        let unit = match unit.trim().to_ascii_lowercase().as_str() {
            "w" | "wk" | "wks" | "week" | "weeks" => TenorUnit::Week,
            "m" | "mo" | "mos" | "month" | "months" => TenorUnit::Month,
            "y" | "yr" | "yrs" | "year" | "years" => TenorUnit::Year,
            _ => return None,
        };
        Some(Tenor { unit, hundredths: parse_hundredths(num)? })
    }

    /// Label as Treasury publishes it on its rate tables (`1 Mo`,
    /// `1.5 Month`, `10 Yr`).
    pub(crate) fn label(self) -> String {
        let n = format_hundredths(self.hundredths);
        match self.unit {
            TenorUnit::Week => format!("{n} Wk"),
            TenorUnit::Month if !self.hundredths.is_multiple_of(100) => format!("{n} Month"),
            TenorUnit::Month => format!("{n} Mo"),
            TenorUnit::Year => format!("{n} Yr"),
        }
    }

    pub(crate) fn years(self) -> f64 {
        let n = f64::from(self.hundredths) / 100.0;
        match self.unit {
            TenorUnit::Week => n / 52.0,
            TenorUnit::Month => n / 12.0,
            TenorUnit::Year => n,
        }
    }
}

fn parse_hundredths(num: &str) -> Option<u32> {
    let (whole, frac) = num.split_once('.').unwrap_or((num, ""));
    if whole.is_empty() || frac.len() > 2 || !whole.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    let whole: u32 = whole.parse().ok()?;
    let frac: u32 = if frac.is_empty() { 0 } else { format!("{frac:0<2}").parse().ok()? };
    let h = whole.checked_mul(100)?.checked_add(frac)?;
    (h > 0).then_some(h)
}

fn format_hundredths(h: u32) -> String {
    if h.is_multiple_of(100) {
        (h / 100).to_string()
    } else {
        format!("{}.{:02}", h / 100, h % 100).trim_end_matches('0').to_owned()
    }
}

/// One business day's curve as published.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CurveRow {
    pub date: NaiveDate,
    /// Only the tenors present in the entry.
    pub points: Vec<(Tenor, f64)>,
}

impl CurveRow {
    pub(crate) fn get(&self, tenor: Tenor) -> Option<f64> {
        self.points.iter().find(|(t, _)| *t == tenor).map(|(_, v)| *v)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Feed {
    /// The feed-level `<updated>` stamp.
    pub updated: Option<DateTime<Utc>>,
    /// Sorted by date.
    pub rows: Vec<CurveRow>,
}

pub(crate) fn parse_feed(xml: &str) -> Result<Feed, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut saw_feed = false;
    let mut in_entry = false;
    let mut in_props = false;
    // Element whose text we're collecting (a property, or the feed's
    // `updated`).
    let mut field: Option<String> = None;
    let mut text = String::new();
    let mut updated = None;
    let mut date: Option<NaiveDate> = None;
    let mut points: Vec<(Tenor, f64)> = Vec::new();
    let mut rows = Vec::new();

    loop {
        let event = reader.read_event().map_err(|e| format!("XML error at byte {}: {e}", reader.error_position()))?;
        match event {
            Event::Start(e) => {
                let local = e.local_name();
                let name: &str = local.as_ref();
                match name {
                    "feed" => saw_feed = true,
                    "entry" => in_entry = true,
                    "properties" if in_entry => {
                        in_props = true;
                        date = None;
                        points.clear();
                    }
                    "updated" if !in_entry => {
                        field = Some(name.to_owned());
                        text.clear();
                    }
                    _ if in_props => {
                        field = Some(name.to_owned());
                        text.clear();
                    }
                    _ => {}
                }
            }
            Event::Text(t) if field.is_some() => text.push_str(&t.xml10_content()),
            Event::End(e) => {
                let local = e.local_name();
                let name: &str = local.as_ref();
                if field.as_deref() == Some(name) {
                    field = None;
                    let value = text.trim();
                    if in_props {
                        if name == "NEW_DATE" {
                            date = Some(parse_feed_date(value)?);
                        } else if let Some(tenor) = Tenor::from_element(name)
                            && !value.is_empty()
                        {
                            let v: f64 = value.parse().map_err(|_| format!("{name} value {value:?} is not a number"))?;
                            if !v.is_finite() {
                                return Err(format!("{name} value {value:?} is not finite"));
                            }
                            points.push((tenor, v));
                        }
                    } else if name == "updated" {
                        updated = DateTime::parse_from_rfc3339(value).ok().map(|d| d.with_timezone(&Utc));
                    }
                } else if name == "properties" && in_props {
                    in_props = false;
                    let d = date.ok_or("entry without NEW_DATE")?;
                    rows.push(CurveRow { date: d, points: std::mem::take(&mut points) });
                } else if name == "entry" {
                    in_entry = false;
                }
            }
            // `<d:BC_X m:null="true" />`: no value published.
            Event::Eof => break,
            _ => {}
        }
    }
    if !saw_feed {
        return Err("response is not an Atom feed".into());
    }
    rows.sort_by_key(|r| r.date);
    Ok(Feed { updated, rows })
}

/// `2026-10-01T00:00:00` → 2026-10-01.
fn parse_feed_date(s: &str) -> Result<NaiveDate, String> {
    s.get(..10)
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .ok_or_else(|| format!("NEW_DATE {s:?} is not a date"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(label: &str) -> Tenor {
        Tenor::parse_label(label).unwrap()
    }

    #[test]
    fn tenor_elements() {
        assert_eq!(Tenor::from_element("BC_10YEAR"), Some(t("10 Yr")));
        assert_eq!(Tenor::from_element("BC_1_5MONTH"), Some(t("1.5 Mo")));
        assert_eq!(Tenor::from_element("BC_6WEEK"), Some(t("6 Wk")));
        assert_eq!(Tenor::from_element("BC_30YEARDISPLAY"), None);
        assert_eq!(Tenor::from_element("NEW_DATE"), None);
        assert_eq!(Tenor::from_element("BC_YEAR"), None);
    }

    #[test]
    fn tenor_labels() {
        for (input, label) in [
            ("10 Yr", "10 Yr"),
            ("10y", "10 Yr"),
            ("10 YEARS", "10 Yr"),
            ("1.5 Mo", "1.5 Month"),
            ("1.5 Month", "1.5 Month"),
            ("1.50m", "1.5 Month"),
            ("3M", "3 Mo"),
            ("6 wk", "6 Wk"),
        ] {
            assert_eq!(t(input).label(), label, "{input}");
        }
        for bad in ["", "Yr", "10", "10 Decades", "1.555 Mo", "0 Yr", "-1 Yr", "1..5 Mo"] {
            assert_eq!(Tenor::parse_label(bad), None, "{bad}");
        }
        assert!((t("1.5 Mo").years() - 0.125).abs() < 1e-12);
        assert!((t("30 Yr").years() - 30.0).abs() < 1e-12);
    }

    #[test]
    fn parses_current_feed() {
        let feed = parse_feed(include_str!("../tests/fixtures/par_yield_202610.xml")).unwrap();
        assert_eq!(feed.updated.unwrap().to_rfc3339(), "2026-10-05T02:01:09+00:00");
        assert_eq!(feed.rows.len(), 2);
        let last = &feed.rows[1];
        assert_eq!(last.date, NaiveDate::from_ymd_opt(2026, 10, 2).unwrap());
        assert_eq!(last.points.len(), 14, "14 tenors incl. 1.5 Mo; the 30-year display duplicate is skipped");
        assert_eq!(last.get(t("1 Mo")), Some(4.04));
        assert_eq!(last.get(t("1.5 Mo")), Some(4.09));
        assert_eq!(last.get(t("10 Yr")), Some(5.28));
        assert_eq!(last.get(t("30 Yr")), Some(5.63));
    }

    #[test]
    fn handles_tenor_set_changes() {
        // 2004: no 30-year (not issued 2002-2006), and no 1.5/2/4 month.
        let old = parse_feed(include_str!("../tests/fixtures/par_yield_200401_trimmed.xml")).unwrap();
        let first = &old.rows[0];
        assert_eq!(first.date, NaiveDate::from_ymd_opt(2004, 1, 2).unwrap());
        assert_eq!(first.get(t("30 Yr")), None, "BC_30YEARDISPLAY 0.00 must not become a 30-year point");
        assert_eq!(first.get(t("10 Yr")), Some(4.38));
        assert_eq!(first.points.len(), 10);

        // 2025: the 1.5-month tenor starts on 2025-02-18.
        let y2025 = parse_feed(include_str!("../tests/fixtures/par_yield_2025_trimmed.xml")).unwrap();
        let with = y2025.rows.iter().filter(|r| r.get(t("1.5 Mo")).is_some()).collect::<Vec<_>>();
        assert_eq!(with.len(), 1);
        assert_eq!(with[0].date, NaiveDate::from_ymd_opt(2025, 2, 18).unwrap());
        assert_eq!(with[0].get(t("1.5 Mo")), Some(4.41));
    }

    #[test]
    fn empty_feed_and_errors() {
        let empty = parse_feed(include_str!("../tests/fixtures/par_yield_empty_month.xml")).unwrap();
        assert!(empty.rows.is_empty());
        assert!(empty.updated.is_some());

        assert!(parse_feed("<html><body>Maintenance</body></html>").is_err());
        assert!(parse_feed("<feed><entry><content><m:properties><d:BC_1YEAR>x</d:BC_1YEAR></m:properties></content></entry></feed>").is_err());
    }

    #[test]
    fn null_elements_are_skipped() {
        // Hand-made: an OData null property, as the feed format allows.
        let xml = r#"<feed xmlns:d="d" xmlns:m="m"><entry><content><m:properties>
            <d:NEW_DATE m:type="Edm.DateTime">2026-01-02T00:00:00</d:NEW_DATE>
            <d:BC_1MONTH m:type="Edm.Double" m:null="true" />
            <d:BC_1YEAR m:type="Edm.Double">3.50</d:BC_1YEAR>
            </m:properties></content></entry></feed>"#;
        let feed = parse_feed(xml).unwrap();
        assert_eq!(feed.rows[0].points, vec![(t("1 Yr"), 3.5)]);
    }
}

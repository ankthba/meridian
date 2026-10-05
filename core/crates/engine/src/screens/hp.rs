//! HP — historical price table.

use std::sync::Arc;

use chrono::{Datelike, NaiveDate};
use meridian_types::{Bar, BarInterval, BarSeries, NANOS_PER_DAY, date_to_nanos, nanos_to_date};

use super::{ScreenRequest, error_screen, parse_date, require_security, stale_notice};
use crate::core::Engine;
use crate::screen::{Block, Cell, Column, Field, Format, Input, InputKind, Row, Screen, Style, Table};

const TITLE: &str = "Historical Prices";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Period {
    Daily,
    Weekly,
    Monthly,
}

/// Aggregates daily bars into calendar weeks (ISO) or months.
pub(crate) fn aggregate(series: &BarSeries, period: Period) -> Vec<Bar> {
    let mut out: Vec<Bar> = Vec::new();
    let mut last_bucket: Option<(i32, u32)> = None;
    for i in 0..series.len() {
        let b = series.bar(i);
        let d = nanos_to_date(b.ts);
        let bucket = match period {
            Period::Daily => (d.year(), d.ordinal()),
            Period::Weekly => (d.iso_week().year(), d.iso_week().week()),
            Period::Monthly => (d.year(), d.month()),
        };
        match out.last_mut() {
            Some(agg) if last_bucket == Some(bucket) => {
                agg.high = agg.high.max(b.high);
                agg.low = agg.low.min(b.low);
                agg.close = b.close;
                agg.volume += b.volume;
                agg.ts = b.ts; // label by the period's last trading day
            }
            _ => out.push(b),
        }
        last_bucket = Some(bucket);
    }
    out
}

pub(crate) async fn hp(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    let key = match require_security("HP", TITLE, &req) {
        Ok(k) => k,
        Err(s) => return *s,
    };
    let now = engine.now();
    let today = nanos_to_date(now);
    let period = match req.arg("period").unwrap_or("Daily") {
        "Weekly" | "W" => Period::Weekly,
        "Monthly" | "M" => Period::Monthly,
        _ => Period::Daily,
    };
    let end = req.arg("end").and_then(parse_date).unwrap_or(today);
    let default_start = match period {
        Period::Daily => end - chrono::Duration::days(92),
        Period::Weekly => end - chrono::Duration::days(366),
        Period::Monthly => NaiveDate::from_ymd_opt(end.year() - 5, end.month(), 1).unwrap_or(end),
    };
    let start = req.arg("start").and_then(parse_date).unwrap_or(default_start);
    if start > end {
        let mut s = Screen::new("HP", TITLE, Some(key.to_string()));
        s.push(Block::Notice { level: crate::screen::NoticeLevel::Error, text: "Start date is after end date".into() });
        return s;
    }

    let fetched = match engine.bars(&key, BarInterval::Day, Some(date_to_nanos(start)), date_to_nanos(end) + NANOS_PER_DAY).await {
        Ok(f) => f,
        Err(e) => return error_screen("HP", TITLE, Some(&key), &e),
    };
    let dec = engine.price_decimals(&key);
    let mut s = Screen::new("HP", format!("{key} — {TITLE}"), Some(key.to_string()));
    s.source(&fetched.value.provenance);
    stale_notice(&mut s, &fetched);

    let period_label = match period {
        Period::Daily => "Daily",
        Period::Weekly => "Weekly",
        Period::Monthly => "Monthly",
    };
    s.push(Block::Inputs {
        title: None,
        inputs: vec![
            Input { id: "start".into(), label: "Range".into(), value: start.format("%m/%d/%Y").to_string(), kind: InputKind::Date, options: vec![] },
            Input { id: "end".into(), label: "–".into(), value: end.format("%m/%d/%Y").to_string(), kind: InputKind::Date, options: vec![] },
            Input {
                id: "period".into(),
                label: "Period".into(),
                value: period_label.into(),
                kind: InputKind::Choice,
                options: vec!["Daily".into(), "Weekly".into(), "Monthly".into()],
            },
        ],
    });

    let bars = aggregate(&fetched.value, period);
    if bars.is_empty() {
        s.push(Block::Notice { level: crate::screen::NoticeLevel::Info, text: "No prices in this range".into() });
        return s;
    }

    // Summary over the range.
    let (hi_i, hi) = bars.iter().enumerate().map(|(i, b)| (i, b.high)).fold((0, f64::MIN), |a, b| if b.1 > a.1 { b } else { a });
    let (lo_i, lo) = bars.iter().enumerate().map(|(i, b)| (i, b.low)).fold((0, f64::MAX), |a, b| if b.1 < a.1 { b } else { a });
    let first = bars[0].close;
    let last = bars[bars.len() - 1].close;
    let avg = bars.iter().map(|b| b.close).sum::<f64>() / bars.len() as f64;
    let avg_vol = bars.iter().map(|b| b.volume).sum::<f64>() / bars.len() as f64;
    s.push(Block::Fields {
        title: None,
        columns: 3,
        fields: vec![
            Field::num("High", Some(hi), Format::Price { decimals: dec }),
            Field::text("on", nanos_to_date(bars[hi_i].ts).format("%m/%d/%y").to_string()),
            Field::num("Average", Some(avg), Format::Price { decimals: dec }),
            Field::num("Low", Some(lo), Format::Price { decimals: dec }),
            Field::text("on", nanos_to_date(bars[lo_i].ts).format("%m/%d/%y").to_string()),
            Field::num("Avg Volume", Some(avg_vol), Format::Large { decimals: 2 }),
            Field::num("Net Chg", Some(last - first), Format::Change { decimals: dec }),
            Field::num("% Chg", Some((last / first - 1.0) * 100.0), Format::ChangePercent { decimals: 2 }),
            Field::num("Observations", Some(bars.len() as f64), Format::Integer),
        ],
    });

    let mut rows = Vec::with_capacity(bars.len());
    for i in (0..bars.len()).rev() {
        let b = &bars[i];
        let prev = (i > 0).then(|| bars[i - 1].close);
        let chg = prev.map(|p| b.close - p);
        let pct = prev.filter(|p| *p != 0.0).map(|p| (b.close / p - 1.0) * 100.0);
        let date = nanos_to_date(b.ts);
        rows.push(Row::new(vec![
            Cell::text(date.format("%a %m/%d/%y").to_string()),
            Cell::num(Some(b.open)),
            Cell::num(Some(b.high)),
            Cell::num(Some(b.low)),
            Cell::num(Some(b.close)).styled(Style::Emphasis),
            Cell::signed(chg),
            Cell::signed(pct),
            Cell::num(Some(b.volume)),
        ]));
    }
    s.push(Block::Table(Table {
        title: None,
        columns: vec![
            Column::text("Date", 14),
            Column::num("Open", Format::Price { decimals: dec }, 10),
            Column::num("High", Format::Price { decimals: dec }, 10),
            Column::num("Low", Format::Price { decimals: dec }, 10),
            Column::num("Last Px", Format::Price { decimals: dec }, 10),
            Column::num("Net Chg", Format::Change { decimals: dec }, 9),
            Column::num("% Chg", Format::ChangePercent { decimals: 2 }, 8),
            Column::num("Volume", Format::Large { decimals: 2 }, 10),
        ],
        rows,
        page_size: Some(25),
        numbered: false,
    }));
    s
}

#[cfg(test)]
mod tests {
    use meridian_types::{Adjustment, Provenance, SecurityKey};

    use super::*;

    #[test]
    fn weekly_aggregation() {
        let mut s = BarSeries::new(SecurityKey::equity("X"), BarInterval::Day, Adjustment::None, Provenance::synthetic(0));
        // Mon 2026-01-05 .. Fri 2026-01-16 (two weeks).
        let mut d = NaiveDate::from_ymd_opt(2026, 1, 5).unwrap();
        let mut px = 10.0;
        while d <= NaiveDate::from_ymd_opt(2026, 1, 16).unwrap() {
            if d.weekday().number_from_monday() <= 5 {
                s.push(Bar { ts: date_to_nanos(d), open: px, high: px + 1.0, low: px - 1.0, close: px + 0.5, volume: 100.0 });
                px += 1.0;
            }
            d = d.succ_opt().unwrap();
        }
        let w = aggregate(&s, Period::Weekly);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].open, 10.0);
        assert_eq!(w[0].close, 14.5);
        assert_eq!(w[0].high, 15.0);
        assert_eq!(w[0].volume, 500.0);
        assert_eq!(nanos_to_date(w[1].ts), NaiveDate::from_ymd_opt(2026, 1, 16).unwrap());
    }
}

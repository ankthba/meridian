//! COMPARE — securities side by side: performance over a range rebased to
//! 100, and a table of price changes and key figures.
//!
//! `securities` lists the keys, comma-separated (`aapl vs msft` sets it);
//! without it the request's security is shown alone. `range` is a chart
//! preset (default 1Y). Every line is rebased at the first date all of them
//! have data. A figure no source supplies is NOT AVAILABLE in its cell, with
//! the reason in a notice under the table.

use std::sync::Arc;

use chrono::{Datelike, Months, NaiveDate};
use meridian_types::{BarInterval, BarSeries, MarketSector, NANOS_PER_DAY, PeriodType, Provenance, SecurityKey, date_to_nanos, nanos_to_date};

use super::analysis::Metrics;
use super::{ScreenRequest, parse_range};
use crate::core::Engine;
use crate::screen::{
    Action, Block, Cell, Column, Format, Input, InputKind, LiveField, NoticeLevel, Row, Screen, ScreenStatus, Style, Table, XyChart, XySeries,
};

const FUNCTION: &str = "COMPARE";
const TITLE: &str = "Compare";
/// Range presets offered in the menu.
const RANGES: &[&str] = &["1M", "3M", "6M", "YTD", "1Y", "2Y", "5Y", "10Y", "MAX"];
/// Every preset the screen accepts (the menu's plus the short ones).
const ACCEPTED: &[&str] = &["1D", "5D", "1M", "3M", "6M", "YTD", "1Y", "2Y", "3Y", "5Y", "10Y", "20Y", "MAX"];
const DEFAULT_RANGE: &str = "1Y";
/// Points per line in the chart; longer series are thinned evenly.
const MAX_POINTS: usize = 400;
/// Daily history fetched for the 1M, YTD and 1Y changes.
const CHANGE_HISTORY_DAYS: i64 = 400;
const NA: &str = "NOT AVAILABLE";

/// What was fetched for one security.
struct Line {
    key: SecurityKey,
    name: String,
    /// Bars over the chosen range.
    chart: Result<BarSeries, String>,
    /// About thirteen months of daily bars for the change columns.
    daily: Result<BarSeries, String>,
    last: Option<f64>,
    change_1d: Option<f64>,
    /// Valuation figures; `Err` holds why they're missing.
    figures: Result<Metrics, String>,
    sources: Vec<Provenance>,
}

/// Bar size for the chart: intraday for one or five days, weekly from five
/// years up, daily otherwise.
fn chart_interval(range: &str) -> BarInterval {
    match range {
        "1D" => BarInterval::Minute(5),
        "5D" => BarInterval::Minute(30),
        "5Y" | "10Y" | "20Y" | "MAX" => BarInterval::Week,
        _ => BarInterval::Day,
    }
}

/// The date whose close a range is measured from: one month back for 1M,
/// the prior year-end for YTD, and so on. `None` for intraday ranges and
/// MAX, which start at their first bar.
fn range_start(range: &str, today: NaiveDate) -> Option<NaiveDate> {
    let months = match range {
        "1M" => 1,
        "3M" => 3,
        "6M" => 6,
        "1Y" => 12,
        "2Y" => 24,
        "3Y" => 36,
        "5Y" => 60,
        "10Y" => 120,
        "20Y" => 240,
        "YTD" => return NaiveDate::from_ymd_opt(today.year() - 1, 12, 31),
        _ => return None,
    };
    today.checked_sub_months(Months::new(months))
}

/// Exclusive upper bound for the base bar of a range: bars before the end of
/// its start date.
fn range_cutoff(range: &str, today: NaiveDate) -> Option<i64> {
    range_start(range, today).map(|d| date_to_nanos(d) + NANOS_PER_DAY)
}

async fn load(engine: Arc<Engine>, key: SecurityKey, range: String) -> Line {
    let now = engine.now();
    let (preset_from, to) = parse_range(&range, now);
    // A week before the start date, so the base close is included.
    let from = match range_start(&range, nanos_to_date(now)) {
        Some(d) => Some(date_to_nanos(d) - 7 * NANOS_PER_DAY),
        None => preset_from,
    };
    let (inst, quote, chart, daily) = tokio::join!(
        engine.instrument(&key),
        engine.quote_row(&key),
        engine.bars(&key, chart_interval(&range), from, to),
        engine.bars(&key, BarInterval::Day, Some(now - CHANGE_HISTORY_DAYS * NANOS_PER_DAY), to),
    );
    let mut sources = Vec::new();
    let chart = chart.map(|f| f.value).map_err(|e| e.user_message());
    let daily = daily.map(|f| f.value).map_err(|e| e.user_message());
    if let Ok(b) = &daily {
        sources.push(b.provenance.clone());
    }
    let quote = quote.filter(|q| q.last.is_finite() && q.last > 0.0);
    if quote.is_some()
        && let Some(p) = engine.quote_provenance(&key)
    {
        sources.push(p);
    }
    let closes = daily.as_ref().map(|b| b.close.as_slice()).unwrap_or_default();
    let last = quote.map(|q| q.last).or_else(|| closes.last().copied());
    let change_1d = quote
        .map(|q| q.pct_change)
        .filter(|c| c.is_finite())
        .or_else(|| match closes {
            [.., prev, last] if *prev != 0.0 => Some((last / prev - 1.0) * 100.0),
            _ => None,
        });
    let figures = if key.sector == MarketSector::Equity {
        match engine.fundamentals(&key, PeriodType::Annual, 3).await {
            Ok(f) => {
                sources.push(f.value.provenance.clone());
                let mut m = Metrics { key: Some(key.clone()), price: last, ..Metrics::default() };
                m.apply_fundamentals(&f.value);
                Ok(m)
            }
            Err(e) => Err(e.user_message()),
        }
    } else {
        Err("company financials apply to stocks only".to_owned())
    };
    Line { name: inst.map(|i| i.name).unwrap_or_default(), key, chart, daily, last, change_1d, figures, sources }
}

/// Percent change from the last close on or before `cutoff` to the last close.
fn change_since(bars: &BarSeries, cutoff: NaiveDate) -> Option<f64> {
    let cut = date_to_nanos(cutoff) + NANOS_PER_DAY;
    let base = bars.ts.iter().zip(&bars.close).rev().find(|(t, _)| **t < cut).map(|(_, c)| *c)?;
    let last = *bars.close.last()?;
    (base != 0.0 && base.is_finite() && last.is_finite()).then(|| (last / base - 1.0) * 100.0)
}

/// Rebased lines: 100 × close / base close. Each line's base is its last
/// bar before `cutoff` (the range's start date; the first bar without one),
/// moved later to the latest of those bases so every line starts together.
fn rebased(lines: &[Line], cutoff: Option<i64>) -> Vec<(String, Vec<f64>, Vec<f64>)> {
    let charts: Vec<(&Line, &BarSeries)> = lines.iter().filter_map(|l| l.chart.as_ref().ok().filter(|b| !b.ts.is_empty()).map(|b| (l, b))).collect();
    let base_ts = |b: &BarSeries| cutoff.and_then(|c| b.ts.iter().rev().find(|t| **t < c).copied()).unwrap_or(b.ts[0]);
    let Some(start) = charts.iter().map(|(_, b)| base_ts(b)).max() else {
        return Vec::new();
    };
    charts
        .into_iter()
        .filter_map(|(l, b)| {
            // The last bar at or before the common start, else the first after it.
            let first = b.ts.iter().rposition(|t| *t <= start).or_else(|| b.ts.iter().position(|t| *t >= start))?;
            let base = b.close[first];
            if !(base.is_finite() && base != 0.0) {
                return None;
            }
            let n = b.ts.len() - first;
            let step = n.div_ceil(MAX_POINTS).max(1);
            let mut idx: Vec<usize> = (first..b.ts.len()).step_by(step).collect();
            if idx.last() != Some(&(b.ts.len() - 1)) {
                idx.push(b.ts.len() - 1);
            }
            let x = idx.iter().map(|&i| b.ts[i] as f64).collect();
            let y = idx.iter().map(|&i| b.close[i] / base * 100.0).collect();
            Some((l.key.symbol.clone(), x, y))
        })
        .collect()
}

pub(crate) async fn compare(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    let range = req.arg("range").map(|r| r.trim().to_ascii_uppercase()).filter(|r| ACCEPTED.contains(&r.as_str())).unwrap_or_else(|| DEFAULT_RANGE.to_owned());
    let mut keys: Vec<SecurityKey> = match req.arg("securities").filter(|s| !s.trim().is_empty()) {
        Some(list) => list.split(',').filter_map(|k| k.trim().parse().ok()).collect(),
        None => req.security.iter().cloned().collect(),
    };
    let mut seen = Vec::new();
    keys.retain(|k| {
        let new = !seen.contains(k);
        seen.push(k.clone());
        new
    });
    let dropped = keys.len().saturating_sub(meridian_command::MAX_COMPARE);
    keys.truncate(meridian_command::MAX_COMPARE);
    let list = keys.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");

    let mut s = Screen::new(FUNCTION, TITLE, keys.first().map(ToString::to_string));
    let base = Action::new(FUNCTION, keys.first().map(ToString::to_string).as_deref()).arg("securities", list.clone());
    for r in RANGES {
        s.menu_item(r, base.clone().arg("range", *r), *r == range);
    }
    s.push(Block::Inputs {
        title: None,
        inputs: vec![
            Input { id: "securities".into(), label: "Securities".into(), value: list, kind: InputKind::Text, options: vec![] },
            Input { id: "range".into(), label: "Period".into(), value: range.clone(), kind: InputKind::Choice, options: RANGES.iter().map(|r| (*r).to_owned()).collect() },
        ],
    });
    if keys.is_empty() {
        s.status = ScreenStatus::NotAvailable { reason: "type the securities to compare, e.g. aapl vs msft".into() };
        return s;
    }
    if dropped > 0 {
        s.push(Block::Notice {
            level: NoticeLevel::Info,
            text: format!("Showing the first {} securities; {dropped} more left out", meridian_command::MAX_COMPARE),
        });
    }

    let mut tasks = tokio::task::JoinSet::new();
    for (i, k) in keys.iter().enumerate() {
        let (engine, key, range) = (engine.clone(), k.clone(), range.clone());
        tasks.spawn(async move { (i, load(engine, key, range).await) });
    }
    let mut slots: Vec<Option<Line>> = keys.iter().map(|_| None).collect();
    while let Some(done) = tasks.join_next().await {
        match done {
            Ok((i, line)) => slots[i] = Some(line),
            Err(e) => tracing::warn!(error = %e, "compare load task failed"),
        }
    }
    let lines: Vec<Line> = slots.into_iter().flatten().collect();
    for p in lines.iter().flat_map(|l| &l.sources) {
        s.source(p);
    }

    let today = nanos_to_date(engine.now());
    let series = rebased(&lines, range_cutoff(&range, today));
    if series.is_empty() {
        let reasons = lines.iter().filter_map(|l| l.chart.as_ref().err().map(|e| format!("{}: {e}", l.key.symbol))).collect::<Vec<_>>().join("; ");
        s.status = ScreenStatus::NotAvailable {
            reason: if reasons.is_empty() { "no price history for these securities".into() } else { format!("no price history — {reasons}") },
        };
        return s;
    }
    let label = meridian_command::plain::range_label(&range).unwrap_or(range.as_str());
    s.push(Block::Xy(XyChart {
        title: format!("Performance, {label}, rebased to 100"),
        x_label: "Date".into(),
        y_label: "Rebased".into(),
        series: series
            .into_iter()
            .enumerate()
            .map(|(i, (name, x, y))| XySeries { name, x, y, style: if i == 0 { Style::Emphasis } else { Style::Normal }, bars: false })
            .collect(),
        x_marker: None,
        height_rows: 12,
        x_categories: None,
    }));

    let month_ago = range_start("1M", today).unwrap_or(today);
    let year_ago = range_start("1Y", today).unwrap_or(today);
    let year_end = range_start("YTD", today).unwrap_or(today);
    let decimals = lines.iter().map(|l| engine.price_decimals(&l.key)).max().unwrap_or(2);
    let mut notices = Vec::new();
    let rows: Vec<Row> = lines
        .iter()
        .map(|l| {
            let ks = l.key.to_string();
            let daily = l.daily.as_ref().ok();
            let figures = l.figures.as_ref().ok();
            let mut prices: Vec<&str> = Vec::new();
            let mut valuation: Vec<&str> = Vec::new();
            let mut cells = vec![
                Cell::text(&l.key.symbol).styled(Style::Link),
                Cell::text(&l.name),
                cell("last price", l.last, false, &mut prices),
                cell("1D change", l.change_1d, true, &mut prices),
                cell("1M change", daily.and_then(|b| change_since(b, month_ago)), true, &mut prices),
                cell("YTD change", daily.and_then(|b| change_since(b, year_end)), true, &mut prices),
                cell("1Y change", daily.and_then(|b| change_since(b, year_ago)), true, &mut prices),
            ];
            cells.push(cell("market cap", figures.and_then(|m| m.market_cap), false, &mut valuation));
            cells.push(cell("P/E", figures.and_then(|m| m.pe), false, &mut valuation));
            cells.push(cell("net margin", figures.and_then(|m| m.net_margin), false, &mut valuation));
            cells.push(cell("dividend yield", figures.and_then(|m| m.div_yield), false, &mut valuation));
            if !prices.is_empty() {
                let why = match &l.daily {
                    Err(e) => e.clone(),
                    Ok(b) => match b.ts.first() {
                        Some(t) => format!("history starts {}", nanos_to_date(*t).format("%m/%d/%Y")),
                        None => "no price history".into(),
                    },
                };
                notices.push(format!("{}: {} NOT AVAILABLE — {why}", l.key.symbol, join_fields(&prices)));
            }
            if !valuation.is_empty() {
                let why = match &l.figures {
                    Err(e) => e.clone(),
                    Ok(_) => "not derivable from the latest annual statements (for example, negative earnings or no dividend reported)".into(),
                };
                notices.push(format!("{}: {} NOT AVAILABLE — {why}", l.key.symbol, join_fields(&valuation)));
            }
            Row::new(cells).security(&ks).action(Action::new("DES", Some(&ks)))
        })
        .collect();
    let pct = Format::ChangePercent { decimals: 2 };
    s.push(Block::Table(Table {
        title: Some("Side by side".into()),
        columns: vec![
            Column::text("Security", 9),
            Column::text("Name", 24),
            Column::num("Last", Format::Price { decimals }, 13).live(LiveField::Last),
            Column::num("1D %", pct, 13).live(LiveField::PctChange),
            Column::num("1M %", pct, 13),
            Column::num("YTD %", pct, 13),
            Column::num("1Y %", pct, 13),
            Column::num("Mkt Cap", Format::Large { decimals: 2 }, 13),
            Column::num("P/E", Format::Number { decimals: 1 }, 13),
            Column::num("Net Margin %", Format::Percent { decimals: 1 }, 13),
            Column::num("Div Yield %", Format::Percent { decimals: 2 }, 13),
        ],
        rows,
        page_size: None,
        numbered: true,
    }));
    for text in notices {
        s.push(Block::Notice { level: NoticeLevel::Warning, text });
    }
    s
}

/// A numeric cell, or NOT AVAILABLE with `label` noted in `missing`.
fn cell(label: &'static str, value: Option<f64>, signed: bool, missing: &mut Vec<&'static str>) -> Cell {
    match value {
        Some(x) if signed => Cell::signed(Some(x)),
        Some(x) => Cell::num(Some(x)),
        None => {
            missing.push(label);
            Cell::text(NA).styled(Style::Warning)
        }
    }
}

/// `a`, `a and b`, `a, b and c`.
fn join_fields(fields: &[&str]) -> String {
    match fields {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use meridian_types::{Adjustment, Bar};

    use super::*;

    fn series(closes: &[(NaiveDate, f64)]) -> BarSeries {
        let mut s = BarSeries::new(SecurityKey::equity("X"), BarInterval::Day, Adjustment::None, Provenance::synthetic(0));
        for (d, c) in closes {
            s.push(Bar { ts: date_to_nanos(*d), open: *c, high: *c, low: *c, close: *c, volume: 1.0 });
        }
        s
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn changes_use_the_last_close_on_or_before_the_cutoff() {
        let b = series(&[(d(2025, 12, 30), 90.0), (d(2025, 12, 31), 100.0), (d(2026, 1, 2), 104.0), (d(2026, 2, 2), 110.0)]);
        assert_eq!(change_since(&b, d(2025, 12, 31)), Some(10.000000000000009));
        // A cutoff on a holiday falls back to the previous close.
        assert_eq!(change_since(&b, d(2026, 1, 1)), Some(10.000000000000009));
        assert!((change_since(&b, d(2026, 1, 15)).unwrap() - (110.0 / 104.0 - 1.0) * 100.0).abs() < 1e-12);
        // Before the history starts: unknown, not zero.
        assert_eq!(change_since(&b, d(2025, 1, 1)), None);
    }

    fn line(symbol: &str, closes: &[(NaiveDate, f64)]) -> Line {
        Line {
            key: SecurityKey::equity(symbol),
            name: String::new(),
            chart: Ok(series(closes)),
            daily: Err("unused".into()),
            last: None,
            change_1d: None,
            figures: Err("unused".into()),
            sources: Vec::new(),
        }
    }

    #[test]
    fn lines_rebase_at_the_first_common_date() {
        let a = line("A", &[(d(2026, 1, 1), 50.0), (d(2026, 1, 2), 100.0), (d(2026, 1, 3), 110.0)]);
        let b = line("B", &[(d(2026, 1, 2), 20.0), (d(2026, 1, 3), 10.0)]);
        let r = rebased(&[a, b], None);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].0, "A");
        assert_eq!(r[0].2, vec![100.0, 110.00000000000001]);
        assert_eq!(r[1].2, vec![100.0, 50.0]);
        assert_eq!(r[0].1[0], date_to_nanos(d(2026, 1, 2)) as f64);
    }

    #[test]
    fn a_range_rebases_at_the_close_before_its_start_date() {
        // Like YTD: the base is the prior year-end close, so the line ends at
        // the same change the table shows.
        let a = line("A", &[(d(2025, 12, 30), 90.0), (d(2025, 12, 31), 100.0), (d(2026, 1, 2), 104.0), (d(2026, 2, 2), 110.0)]);
        let today = d(2026, 2, 3);
        let r = rebased(&[a], range_cutoff("YTD", today));
        assert_eq!(r[0].1[0], date_to_nanos(d(2025, 12, 31)) as f64);
        assert!((r[0].2.last().unwrap() - 110.0).abs() < 1e-9);
        assert_eq!(range_start("1M", today), Some(d(2026, 1, 3)));
        assert_eq!(range_start("MAX", today), None);
        assert_eq!(range_start("1D", today), None);
    }

    #[test]
    fn long_lines_are_thinned_but_keep_the_last_point() {
        let start = d(2000, 1, 1);
        let closes: Vec<(NaiveDate, f64)> = (0..1000).map(|i| (start + chrono::Duration::days(i), 100.0 + i as f64)).collect();
        let r = rebased(&[line("A", &closes)], None);
        assert!(r[0].1.len() <= MAX_POINTS + 1);
        assert_eq!(*r[0].2.last().unwrap(), 1099.0);
    }

    #[test]
    fn field_lists_read_naturally() {
        assert_eq!(join_fields(&["P/E"]), "P/E");
        assert_eq!(join_fields(&["P/E", "net margin"]), "P/E and net margin");
        assert_eq!(join_fields(&["a", "b", "c"]), "a, b and c");
    }

    #[test]
    fn intervals_by_range() {
        assert_eq!(chart_interval("1D"), BarInterval::Minute(5));
        assert_eq!(chart_interval("1Y"), BarInterval::Day);
        assert_eq!(chart_interval("MAX"), BarInterval::Week);
    }
}

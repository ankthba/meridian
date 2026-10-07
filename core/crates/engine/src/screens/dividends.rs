//! DVD — dividends and splits, merged from every source that offers them.
//!
//! Sources complement each other: one may give dividend events with ex-,
//! record and pay dates (e.g. a market data vendor's corporate actions),
//! another per-share totals by fiscal period and split disclosures from
//! financial statements. Each is fetched and cached on its own; a source
//! that fails leaves only its part NOT AVAILABLE.

use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::NaiveDate;
use meridian_provider::{Capability, ProviderError};
use meridian_types::{Dividend, DividendKind, Dividends, PeriodDividend, PeriodType, ReportedSplit, SecurityKey};

use super::{ScreenRequest, error_screen, require_security, stale_notice};
use crate::cache::{Fetched, ttl};
use crate::core::Engine;
use crate::error::{EngineError, EngineResult};
use crate::screen::{Block, Cell, Column, Field, Format, NoticeLevel, Row, Screen, ScreenStatus, Style, Table};

const TITLE: &str = "Dividends & Splits";
/// Fiscal years and quarters shown in the per-period tables.
const FISCAL_YEARS_SHOWN: usize = 10;
const FISCAL_QUARTERS_SHOWN: usize = 8;

impl Engine {
    /// Dividends from every provider that offers them for `key`, in routing
    /// order, each cached under its own key.
    pub async fn dividends_by_source(&self, key: &SecurityKey) -> Vec<(String, EngineResult<Fetched<Dividends>>)> {
        let mut out = Vec::new();
        for id in self.router().providers_for(Capability::Dividends, Some(key)) {
            let router = self.router().clone();
            let (k, p) = (key.clone(), id.clone());
            let ck = format!("{key}|{id}");
            let r = self.cached("dividends", &ck, ttl::HOLDERS, async move { router.dividends_from(&p, &k).await }).await;
            out.push((id.to_string(), r));
        }
        out
    }
}

/// `ratio:1` for splits, `1:n` for reverse splits.
pub(crate) fn fmt_ratio(r: f64) -> String {
    let trim = |x: f64| {
        let s = format!("{x:.4}");
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    };
    if r >= 1.0 { format!("{}:1", trim(r)) } else { format!("1:{}", trim(1.0 / r)) }
}

fn fmt_day(d: NaiveDate) -> String {
    d.format("%m/%d/%Y").to_string()
}

/// Notice for per-share values when a split disclosed in filings falls
/// inside `(from, to]`: values on either side are not comparable.
pub(crate) fn split_notice(splits: &[ReportedSplit], from: NaiveDate, to: NaiveDate) -> Option<Block> {
    let inside: Vec<String> = splits
        .iter()
        .filter(|s| s.overlaps(from, to))
        .map(|s| {
            let period = match s.period_start {
                Some(start) if start != s.period_end => format!("{}–{}", fmt_day(start), fmt_day(s.period_end)),
                _ => fmt_day(s.period_end),
            };
            let ratio = format!("{:.4}", s.ratio);
            format!("conversion ratio {} for {period}", ratio.trim_end_matches('0').trim_end_matches('.'))
        })
        .collect();
    (!inside.is_empty()).then(|| Block::Notice {
        level: NoticeLevel::Warning,
        text: format!(
            "Filings report a stock split in this window ({}). Per-share values are as reported; periods before the split are not adjusted.",
            inside.join("; ")
        ),
    })
}

/// One event per (ex-date, kind, amount) when several sources report it.
fn merge_events(sources: &[&Dividends]) -> Vec<Dividend> {
    let mut out: Vec<Dividend> = Vec::new();
    for d in sources.iter().flat_map(|s| &s.dividends) {
        let dup = out.iter().any(|e| e.ex_date == d.ex_date && e.kind == d.kind && (e.amount - d.amount).abs() < 1e-9);
        if !dup {
            out.push(d.clone());
        }
    }
    out.sort_by(|a, b| b.ex_date.cmp(&a.ex_date));
    out
}

fn failure_text(failures: &[&(String, EngineError)]) -> String {
    failures.iter().map(|(p, e)| format!("{p}: {}", e.user_message())).collect::<Vec<_>>().join("; ")
}

pub(crate) async fn dvd(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    let key = match require_security("DVD", TITLE, &req) {
        Ok(k) => k,
        Err(s) => return *s,
    };
    let ks = key.to_string();
    let (results, quote) = tokio::join!(engine.dividends_by_source(&key), engine.quote_row(&key));
    let mut ok: Vec<(String, Fetched<Dividends>)> = Vec::new();
    let mut failed: Vec<(String, EngineError)> = Vec::new();
    for (p, r) in results {
        match r {
            Ok(f) => ok.push((p, f)),
            Err(e) => failed.push((p, e)),
        }
    }
    if ok.is_empty() {
        return match failed.as_slice() {
            [] => error_screen("DVD", TITLE, Some(&key), &ProviderError::Unsupported { capability: Capability::Dividends }.into()),
            [(_, only)] => error_screen("DVD", TITLE, Some(&key), only),
            many => {
                let mut s = Screen::new("DVD", TITLE, Some(ks));
                let reason = failure_text(&many.iter().collect::<Vec<_>>());
                s.status = if many.iter().all(|(_, e)| e.is_unavailable()) {
                    ScreenStatus::NotAvailable { reason }
                } else {
                    ScreenStatus::Error { message: reason }
                };
                s
            }
        };
    }

    let mut s = Screen::new("DVD", format!("{ks} — {TITLE}"), Some(ks.clone()));
    for (_, f) in &ok {
        s.source(&f.value.provenance);
        stale_notice(&mut s, f);
    }
    let values: Vec<&Dividends> = ok.iter().map(|(_, f)| &f.value).collect();
    let events = merge_events(&values);
    let per_period: Vec<&PeriodDividend> = values.iter().find(|v| !v.per_period.is_empty()).map(|v| v.per_period.iter().collect()).unwrap_or_default();
    let reported_splits: &[ReportedSplit] = values.iter().find(|v| !v.reported_splits.is_empty()).map_or(&[], |v| &v.reported_splits);
    // NotFound means "this source has none", not a failure.
    let real_failures: Vec<&(String, EngineError)> =
        failed.iter().filter(|(_, e)| !matches!(e, EngineError::Provider(ProviderError::NotFound(_)) | EngineError::NotFound(_))).collect();
    let none_found: Vec<&(String, EngineError)> =
        failed.iter().filter(|(_, e)| matches!(e, EngineError::Provider(ProviderError::NotFound(_)) | EngineError::NotFound(_))).collect();
    // A source that answered with nothing at all (no events, periods or splits).
    let answered_empty = values.iter().any(|v| v.dividends.is_empty() && v.per_period.is_empty() && v.reported_splits.is_empty());
    let period_source_answered = values.iter().any(|v| !v.per_period.is_empty() || !v.reported_splits.is_empty());

    // --- Summary fields --------------------------------------------------
    let today = meridian_types::nanos_to_date(engine.now());
    let last = quote.map(|q| q.last).filter(|x| x.is_finite());
    if events.is_empty() {
        if let Some(fy) = per_period.iter().find(|p| p.period_type == PeriodType::Annual) {
            s.push(Block::Fields {
                title: None,
                columns: 3,
                fields: vec![
                    Field::text("Last Fiscal Year", format!("FY {} (ended {})", fy.fiscal_year, fmt_day(fy.period_end))),
                    Field::num("DPS Declared", fy.declared_per_share, Format::Number { decimals: 4 }),
                    Field::num("DPS Paid", fy.paid_per_share, Format::Number { decimals: 4 }),
                ],
            });
        }
    } else {
        let cash: Vec<&Dividend> = events.iter().filter(|x| !matches!(x.kind, DividendKind::Split)).collect();
        let year_ago = today - chrono::Duration::days(365);
        let ttm_rows: Vec<&&Dividend> = cash.iter().filter(|x| x.ex_date > year_ago).collect();
        let currencies: BTreeSet<&str> = ttm_rows.iter().map(|x| x.currency.as_str()).collect();
        let mut fields = Vec::new();
        if currencies.len() <= 1 {
            let ttm: f64 = ttm_rows.iter().map(|x| x.amount).sum();
            fields.push(Field::num("Div (TTM)", Some(ttm), Format::Number { decimals: 4 }));
            // The quote is in USD; a yield needs dividends known to be in USD.
            if let Some(px) = last
                && ttm > 0.0
                && currencies.iter().all(|c| *c == "USD")
            {
                fields.push(Field::num("Yield (TTM) %", Some(ttm / px * 100.0), Format::Percent { decimals: 2 }));
            }
        } else {
            fields.push(Field::text("Div (TTM)", "n/a — mixed currencies"));
        }
        if let Some(l) = cash.first() {
            fields.push(Field::num("Last Amount", Some(l.amount), Format::Number { decimals: 4 }));
            fields.push(Field::text("Last Ex-Date", l.ex_date.format("%m/%d/%Y").to_string()));
            fields.push(Field::opt_text("Frequency", l.frequency.clone()));
        }
        s.push(Block::Fields { title: None, columns: 3, fields });
    }

    // --- Events ------------------------------------------------------------
    if events.is_empty() {
        let (level, text) = if !answered_empty && !real_failures.is_empty() {
            (NoticeLevel::Warning, format!("Ex-dates, record and pay dates: NOT AVAILABLE — {}", failure_text(&real_failures)))
        } else {
            (NoticeLevel::Info, "No dividend or split events (ex-dates) reported for this security.".to_owned())
        };
        s.push(Block::Notice { level, text });
    } else {
        s.push(events_table(&events));
    }

    // --- Per fiscal period -------------------------------------------------
    if per_period.is_empty() {
        let notice = if period_source_answered {
            Some((NoticeLevel::Info, "Dividends per share by fiscal period: none reported.".to_owned()))
        } else if !real_failures.is_empty() {
            Some((NoticeLevel::Warning, format!("Dividends per share by fiscal period: NOT AVAILABLE — {}", failure_text(&real_failures))))
        } else if !none_found.is_empty() {
            Some((NoticeLevel::Info, format!("Dividends per share by fiscal period: none reported — {}", failure_text(&none_found))))
        } else {
            None
        };
        if let Some((level, text)) = notice {
            s.push(Block::Notice { level, text });
        }
    } else {
        push_period_tables(&mut s, &per_period, reported_splits);
    }

    // --- Splits disclosed in filings (no ex-dates) ---------------------------
    if !reported_splits.is_empty() && !events.iter().any(|e| matches!(e.kind, DividendKind::Split)) {
        let rows = reported_splits
            .iter()
            .map(|r| {
                let period = match r.period_start {
                    Some(start) if start != r.period_end => format!("{} – {}", fmt_day(start), fmt_day(r.period_end)),
                    _ => fmt_day(r.period_end),
                };
                Row::new(vec![Cell::num(Some(r.ratio)), Cell::text(period), Cell::text(fmt_day(r.filed))])
            })
            .collect();
        s.push(Block::Table(Table {
            title: Some("Stock Splits Reported in Filings (ratio as reported; no ex-dates)".into()),
            columns: vec![Column::num("Ratio", Format::Number { decimals: 4 }, 9), Column::text("Period", 25), Column::text("Filed", 11)],
            rows,
            page_size: None,
            numbered: false,
        }));
    }
    s
}

fn events_table(events: &[Dividend]) -> Block {
    let d = |o: Option<NaiveDate>| o.map(|d| d.format("%m/%d/%y").to_string()).unwrap_or_default();
    let rows = events
        .iter()
        .map(|x| {
            let (kind, amount, ccy) = match x.kind {
                DividendKind::Regular => ("Regular Cash", Cell::num(Some(x.amount)), x.currency.as_str()),
                DividendKind::Special => ("Special Cash", Cell::num(Some(x.amount)), x.currency.as_str()),
                DividendKind::Split => ("Stock Split", Cell::text(fmt_ratio(x.amount)).styled(Style::Emphasis), ""),
            };
            Row::new(vec![
                Cell::text(d(x.declared_date)),
                Cell::text(x.ex_date.format("%m/%d/%y").to_string()),
                Cell::text(d(x.record_date)),
                Cell::text(d(x.pay_date)),
                amount,
                Cell::text(ccy),
                Cell::text(kind),
            ])
        })
        .collect();
    Block::Table(Table {
        title: None,
        columns: vec![
            Column::text("Declared", 9),
            Column::text("Ex-Date", 9),
            Column::text("Record", 9),
            Column::text("Payable", 9),
            Column::num("Amount", Format::Number { decimals: 4 }, 9),
            Column::text("Ccy", 4),
            Column::text("Type", 14),
        ],
        rows,
        page_size: Some(20),
        numbered: false,
    })
}

fn push_period_tables(s: &mut Screen, per_period: &[&PeriodDividend], splits: &[ReportedSplit]) {
    let years: Vec<&&PeriodDividend> = per_period.iter().filter(|p| p.period_type == PeriodType::Annual).take(FISCAL_YEARS_SHOWN).collect();
    let quarters: Vec<&&PeriodDividend> =
        per_period.iter().filter(|p| p.period_type == PeriodType::Quarterly).take(FISCAL_QUARTERS_SHOWN).collect();
    // Window covered by the rows shown: the oldest period's start to the newest end.
    let span = |rows: &[&&PeriodDividend], days: i64| {
        let newest = rows.iter().map(|p| p.period_end).max()?;
        let oldest = rows.iter().map(|p| p.period_end).min()?;
        Some((oldest - chrono::Duration::days(days), newest))
    };
    let window = match (span(&years, 365), span(&quarters, 92)) {
        (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.max(b.1))),
        (a, b) => a.or(b),
    };
    if let Some((from, to)) = window
        && let Some(n) = split_notice(splits, from, to)
    {
        s.push(n);
    }
    let currency = per_period.first().map_or("USD", |p| p.currency.as_str()).to_owned();
    let table = |title: String, rows: &[&&PeriodDividend]| {
        Block::Table(Table {
            title: Some(title),
            columns: vec![
                Column::text("Period", 9),
                Column::text("Period End", 11),
                Column::num("Declared / Sh", Format::Number { decimals: 4 }, 14),
                Column::num("Paid / Sh", Format::Number { decimals: 4 }, 14),
            ],
            rows: rows
                .iter()
                .map(|p| {
                    let label = if p.fiscal_period == "FY" {
                        format!("FY {}", p.fiscal_year)
                    } else {
                        format!("{} {}", p.fiscal_period, p.fiscal_year % 100)
                    };
                    Row::new(vec![
                        Cell::text(label).styled(Style::Emphasis),
                        Cell::text(fmt_day(p.period_end)),
                        Cell::num(p.declared_per_share),
                        Cell::num(p.paid_per_share),
                    ])
                })
                .collect(),
            page_size: None,
            numbered: false,
        })
    };
    if !years.is_empty() {
        s.push(table(format!("Dividends per Share by Fiscal Year ({currency}, as reported in financial statements)"), &years));
    }
    if !quarters.is_empty() {
        s.push(table(format!("Dividends per Share by Fiscal Quarter ({currency}, as reported)"), &quarters));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn ratios_read_naturally() {
        assert_eq!(fmt_ratio(2.0), "2:1");
        assert_eq!(fmt_ratio(1.5), "1.5:1");
        assert_eq!(fmt_ratio(0.1), "1:10");
        assert_eq!(fmt_ratio(0.02), "1:50");
    }

    #[test]
    fn split_notice_only_inside_the_window() {
        let split = ReportedSplit { ratio: 4.0, period_start: Some(d(2020, 8, 28)), period_end: d(2020, 8, 28), filed: d(2020, 10, 30) };
        let n = split_notice(std::slice::from_ref(&split), d(2019, 9, 28), d(2021, 9, 25)).unwrap();
        let Block::Notice { level, text } = n else { panic!("notice expected") };
        assert_eq!(level, NoticeLevel::Warning);
        assert!(text.contains("conversion ratio 4 for 08/28/2020"), "{text}");
        assert!(text.ends_with("Per-share values are as reported; periods before the split are not adjusted."));
        assert!(split_notice(std::slice::from_ref(&split), d(2021, 1, 1), d(2022, 1, 1)).is_none());
        assert!(split_notice(&[], d(2019, 1, 1), d(2022, 1, 1)).is_none());
    }

    #[test]
    fn events_from_several_sources_are_merged_once() {
        let ev = |day, amount, kind| Dividend {
            declared_date: None,
            ex_date: d(2026, 1, day),
            record_date: None,
            pay_date: None,
            amount,
            currency: "USD".into(),
            frequency: None,
            kind,
        };
        let mk = |dividends| Dividends {
            key: SecurityKey::equity("X"),
            dividends,
            per_period: vec![],
            reported_splits: vec![],
            provenance: meridian_types::Provenance::synthetic(0),
        };
        let a = mk(vec![ev(2, 0.5, DividendKind::Regular), ev(9, 2.0, DividendKind::Split)]);
        let b = mk(vec![ev(2, 0.5, DividendKind::Regular), ev(5, 1.0, DividendKind::Special)]);
        let merged = merge_events(&[&a, &b]);
        let days: Vec<u32> = merged.iter().map(|e| chrono::Datelike::day(&e.ex_date)).collect();
        assert_eq!(days, vec![9, 5, 2]);
    }
}

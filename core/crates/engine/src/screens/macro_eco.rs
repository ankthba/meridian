//! ECO — economic calendar, series detail, and the Treasury curve.

use std::sync::Arc;

use chrono::Duration;
use meridian_provider::{CalendarRequest, CurveRequest, SeriesRequest};
use meridian_types::{EconomicEvent, EconomicSeries, Importance, YieldCurve, date_to_nanos, nanos_to_date};

use super::{ScreenRequest, error_screen, parse_date, stale_notice};
use crate::cache::ttl;
use crate::core::Engine;
use crate::error::EngineResult;
use crate::screen::{Action, Block, Cell, Column, Field, Format, Input, InputKind, NoticeLevel, Row, Screen, Style, Table, XyChart, XySeries};

const TITLE: &str = "Economic Calendar";

impl Engine {
    pub async fn economic_series(&self, id: &str) -> EngineResult<crate::cache::Fetched<EconomicSeries>> {
        let router = self.router().clone();
        let req = SeriesRequest { id: id.to_owned(), from: None, to: None };
        let f = self.cached("econ_series", id, ttl::SERIES, async move { router.economic_series(req).await }).await?;
        if !f.from_cache && !f.value.provenance.synthetic {
            let _ = self.stores().market.put_series(&f.value);
        }
        Ok(f)
    }

    pub async fn yield_curve(&self, name: &str, date: Option<chrono::NaiveDate>) -> EngineResult<crate::cache::Fetched<YieldCurve>> {
        let router = self.router().clone();
        let req = CurveRequest { name: name.to_owned(), date };
        let ck = format!("{name}|{date:?}");
        self.cached("curve", &ck, ttl::CURVE, async move { router.yield_curve(req).await }).await
    }
}

pub(crate) async fn eco(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    if let Some(id) = req.arg("series").map(str::to_owned) {
        return series(engine, &id).await;
    }
    if req.arg("view") == Some("curve") {
        return curve(engine).await;
    }
    let today = nanos_to_date(engine.now());
    let from = req.arg("from").and_then(parse_date).unwrap_or(today - Duration::days(3));
    let to = req.arg("to").and_then(parse_date).unwrap_or(today + Duration::days(7));
    let min_imp = match req.arg("importance") {
        Some("High") => Importance::High,
        Some("Medium") => Importance::Medium,
        _ => Importance::Low,
    };
    let router = engine.router().clone();
    let creq = CalendarRequest { from, to, countries: vec![] };
    let ck = format!("{from}|{to}");
    let events = match engine.cached("calendar", &ck, ttl::CALENDAR, async move { router.economic_calendar(creq).await }).await {
        Ok(e) => e,
        Err(e) => return error_screen("ECO", TITLE, None, &e),
    };
    let mut s = Screen::new("ECO", TITLE, None);
    if let Some(e) = events.value.first() {
        s.source(&e.provenance);
    }
    stale_notice(&mut s, &events);
    s.menu_item("Calendar", Action::new("ECO", None), true);
    s.menu_item("US Treasury Curve", Action::new("ECO", None).arg("view", "curve"), false);
    for (label, id) in [("CPI", "CPIAUCSL"), ("Unemployment", "UNRATE"), ("Payrolls", "PAYEMS"), ("Fed Funds", "FEDFUNDS"), ("10Y Yield", "DGS10")] {
        s.menu_item(label, Action::new("ECO", None).arg("series", id), false);
    }
    s.push(Block::Inputs {
        title: None,
        inputs: vec![
            Input { id: "from".into(), label: "From".into(), value: from.format("%m/%d/%Y").to_string(), kind: InputKind::Date, options: vec![] },
            Input { id: "to".into(), label: "To".into(), value: to.format("%m/%d/%Y").to_string(), kind: InputKind::Date, options: vec![] },
            Input {
                id: "importance".into(),
                label: "Importance".into(),
                value: match min_imp {
                    Importance::High => "High",
                    Importance::Medium => "Medium",
                    Importance::Low => "All",
                }
                .into(),
                kind: InputKind::Choice,
                options: vec!["All".into(), "Medium".into(), "High".into()],
            },
        ],
    });
    let mut list: Vec<&EconomicEvent> = events.value.iter().filter(|e| e.importance >= min_imp).collect();
    list.sort_by_key(|e| e.release_time);
    if list.iter().all(|e| e.consensus.is_none()) && !list.is_empty() {
        s.push(Block::Notice { level: NoticeLevel::Info, text: "Consensus forecasts are not available from the configured sources.".into() });
    }
    let rows: Vec<Row> = list
        .iter()
        .map(|e| {
            let imp = match e.importance {
                Importance::High => Cell::text("●●●").styled(Style::Down),
                Importance::Medium => Cell::text("●● ").styled(Style::Warning),
                Importance::Low => Cell::text("●  ").styled(Style::Muted),
            };
            let surprise = match (e.actual, e.consensus) {
                (Some(a), Some(c)) if a > c => Style::Up,
                (Some(a), Some(c)) if a < c => Style::Down,
                _ => Style::Emphasis,
            };
            let time = if e.time_known { Cell::num(Some(e.release_time as f64)) } else { Cell::text(nanos_to_date(e.release_time).format("%m/%d/%y").to_string()) };
            let mut r = Row::new(vec![
                time,
                Cell::text(&e.country),
                Cell::text(&e.event).styled(Style::Emphasis),
                Cell::text(e.period.clone().unwrap_or_default()),
                imp,
                Cell::num(e.actual).styled(surprise),
                Cell::num(e.consensus),
                Cell::num(e.prior),
                Cell::text(e.unit.clone().unwrap_or_default()).styled(Style::Muted),
            ]);
            if let Some(id) = &e.series_id {
                r = r.action(Action::new("ECO", None).arg("series", id.clone()));
            }
            r
        })
        .collect();
    s.push(Block::Table(Table {
        title: None,
        columns: vec![
            Column::num("Date Time", Format::DateTime, 16),
            Column::text("Ctry", 4),
            Column::text("Event", 34),
            Column::text("Period", 7),
            Column::text("Imp", 4),
            Column::num("Actual", Format::Number { decimals: 2 }, 9),
            Column::num("Survey", Format::Number { decimals: 2 }, 9),
            Column::num("Prior", Format::Number { decimals: 2 }, 9),
            Column::text("Unit", 6),
        ],
        rows,
        page_size: Some(24),
        numbered: true,
    }));
    s
}

async fn series(engine: Arc<Engine>, id: &str) -> Screen {
    let f = match engine.economic_series(id).await {
        Ok(f) => f,
        Err(e) => return error_screen("ECO", TITLE, None, &e),
    };
    let se = &f.value;
    let mut s = Screen::new("ECO", format!("{} ({})", se.title, se.id), None);
    s.source(&se.provenance);
    stale_notice(&mut s, &f);
    s.menu_item("Calendar", Action::new("ECO", None), false);
    let latest = se.latest();
    let obs: Vec<_> = se.observations.iter().filter(|o| o.value.is_some()).collect();
    let prev = obs.len().checked_sub(2).map(|i| obs[i]);
    let mut fields = vec![
        Field::text("Units", &se.units),
        Field::text("Frequency", &se.frequency),
        Field::opt_text("Seasonal Adj", se.seasonal_adjustment.clone()),
    ];
    if let Some(l) = latest {
        fields.push(Field::num("Latest", l.value, Format::Number { decimals: 2 }).styled(Style::Emphasis));
        fields.push(Field::text("Date", l.date.format("%m/%d/%Y").to_string()));
        if let Some(p) = prev {
            fields.push(Field::num("Change", l.value.zip(p.value).map(|(a, b)| a - b), Format::Change { decimals: 2 }));
        }
    }
    s.push(Block::Fields { title: None, columns: 3, fields });
    let tail: Vec<_> = obs.iter().rev().take(240).rev().collect();
    s.push(Block::Xy(XyChart {
        title: se.title.clone(),
        x_label: "Date".into(),
        y_label: se.units.clone(),
        series: vec![XySeries {
            name: se.id.clone(),
            x: tail.iter().map(|o| date_to_nanos(o.date) as f64).collect(),
            y: tail.iter().map(|o| o.value.unwrap_or(f64::NAN)).collect(),
            style: Style::Emphasis,
            bars: false,
        }],
        x_marker: None,
        height_rows: 12,
        x_categories: None,
    }));
    if let Some(n) = &se.notes {
        s.push(Block::Text { title: Some("Notes".into()), body: n.clone() });
    }
    let rows = obs
        .iter()
        .rev()
        .take(120)
        .map(|o| Row::new(vec![Cell::text(o.date.format("%m/%d/%Y").to_string()), Cell::num(o.value)]))
        .collect();
    s.push(Block::Table(Table {
        title: Some("Observations".into()),
        columns: vec![Column::text("Date", 12), Column::num("Value", Format::Number { decimals: 3 }, 12)],
        rows,
        page_size: Some(20),
        numbered: false,
    }));
    s
}

async fn curve(engine: Arc<Engine>) -> Screen {
    let f = match engine.yield_curve("UST", None).await {
        Ok(f) => f,
        Err(e) => return error_screen("ECO", "US Treasury Curve", None, &e),
    };
    let c = &f.value;
    let mut s = Screen::new("ECO", format!("US Treasury Par Yield Curve — {}", c.date.format("%m/%d/%Y")), None);
    s.source(&c.provenance);
    stale_notice(&mut s, &f);
    s.menu_item("Calendar", Action::new("ECO", None), false);
    s.menu_item("US Treasury Curve", Action::new("ECO", None).arg("view", "curve"), true);
    let pts: Vec<_> = c.points.iter().filter(|p| p.yield_pct.is_some()).collect();
    s.push(Block::Xy(XyChart {
        title: "Yield (%) by Tenor (years)".into(),
        x_label: "Years".into(),
        y_label: "%".into(),
        series: vec![XySeries {
            name: c.name.clone(),
            x: pts.iter().map(|p| p.years).collect(),
            y: pts.iter().map(|p| p.yield_pct.unwrap_or(f64::NAN)).collect(),
            style: Style::Emphasis,
            bars: false,
        }],
        x_marker: None,
        height_rows: 12,
        x_categories: None,
    }));
    let y = |t: &str| c.points.iter().find(|p| p.tenor == t).and_then(|p| p.yield_pct);
    let spread = |a: &str, b: &str| y(a).zip(y(b)).map(|(a, b)| (a - b) * 100.0);
    s.push(Block::Fields {
        title: Some("Spreads (bp)".into()),
        columns: 3,
        fields: vec![
            Field::num("2s10s", spread("10 Yr", "2 Yr"), Format::Change { decimals: 1 }),
            Field::num("3m10y", spread("10 Yr", "3 Mo"), Format::Change { decimals: 1 }),
            Field::num("5s30s", spread("30 Yr", "5 Yr"), Format::Change { decimals: 1 }),
        ],
    });
    let rows = c
        .points
        .iter()
        .map(|p| Row::new(vec![Cell::text(&p.tenor).styled(Style::Emphasis), Cell::num(p.yield_pct)]))
        .collect();
    s.push(Block::Table(Table {
        title: None,
        columns: vec![Column::text("Tenor", 8), Column::num("Yield %", Format::Number { decimals: 3 }, 9)],
        rows,
        page_size: None,
        numbered: false,
    }));
    s
}

//! ALRT — create, list, and delete alerts; recent alert events.

use std::sync::Arc;

use meridian_alerts::{AlertCondition, AlertRepeat, AlertRule};
use meridian_types::SecurityKey;

use super::ScreenRequest;
use crate::core::Engine;
use crate::screen::{Action, Block, Cell, Column, Format, Input, InputKind, NoticeLevel, Row, Screen, Style, Table};

const TITLE: &str = "Alerts";
const CONDITIONS: &[&str] = &["Price Above", "Price Below", "% Chg Above", "% Chg Below", "Volume Above", "News Keyword"];
const REPEATS: &[&str] = &["Once", "Every Cross", "Every 15 Min"];

fn build_rule(req: &ScreenRequest) -> Result<Option<AlertRule>, String> {
    if req.arg("create").is_none() {
        return Ok(None);
    }
    let sec = req.arg("security").map(str::trim).filter(|s| !s.is_empty()).ok_or("enter a security, e.g. AAPL US Equity")?;
    let security: SecurityKey = sec.parse().map_err(|_| format!("not a security key: {sec}"))?;
    let cond = req.arg("condition").unwrap_or("Price Above");
    let value = req.arg("value").unwrap_or("").trim();
    let num = || value.parse::<f64>().map_err(|_| format!("enter a number for {cond}"));
    let condition = match cond {
        "Price Above" => AlertCondition::PriceAbove { level: num()? },
        "Price Below" => AlertCondition::PriceBelow { level: num()? },
        "% Chg Above" => AlertCondition::PctChangeAbove { pct: num()? },
        "% Chg Below" => AlertCondition::PctChangeBelow { pct: num()? },
        "Volume Above" => AlertCondition::VolumeAbove { volume: num()? },
        "News Keyword" if !value.is_empty() => AlertCondition::NewsKeyword { keyword: value.to_owned() },
        "News Keyword" => return Err("enter a keyword".into()),
        other => return Err(format!("unknown condition {other}")),
    };
    let repeat = match req.arg("repeat").unwrap_or("Once") {
        "Every Cross" => AlertRepeat::EveryCross,
        "Every 15 Min" => AlertRepeat::Cooldown { seconds: 900 },
        _ => AlertRepeat::Once,
    };
    let note = req.arg("note").map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned);
    Ok(Some(AlertRule { id: 0, security, condition, repeat, note, enabled: true }))
}

pub(crate) async fn alrt(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    let mut s = Screen::new("ALRT", TITLE, req.security.as_ref().map(ToString::to_string));
    if let Some(id) = req.arg("delete").and_then(|x| x.parse::<i64>().ok()) {
        if let Err(e) = engine.delete_alert(id) {
            s.push(Block::Notice { level: NoticeLevel::Error, text: e.user_message() });
        }
    }
    if let Some(id) = req.arg("toggle").and_then(|x| x.parse::<i64>().ok())
        && let Some(mut r) = engine.alert_rules().into_iter().find(|r| r.id == id)
    {
        r.enabled = !r.enabled;
        let _ = engine.save_alert(&r);
    }
    match build_rule(&req) {
        Ok(Some(rule)) => match engine.save_alert(&rule) {
            Ok(_) => s.push(Block::Notice { level: NoticeLevel::Info, text: format!("Alert created: {} {}", rule.security, rule.condition.describe()) }),
            Err(e) => s.push(Block::Notice { level: NoticeLevel::Error, text: e.user_message() }),
        },
        Ok(None) => {}
        Err(msg) => s.push(Block::Notice { level: NoticeLevel::Error, text: msg }),
    }
    let default_sec = req.security.as_ref().map(ToString::to_string).unwrap_or_default();
    s.push(Block::Inputs {
        title: Some("New Alert".into()),
        inputs: vec![
            Input { id: "security".into(), label: "Security".into(), value: default_sec, kind: InputKind::Text, options: vec![] },
            Input {
                id: "condition".into(),
                label: "Condition".into(),
                value: "Price Above".into(),
                kind: InputKind::Choice,
                options: CONDITIONS.iter().map(|c| (*c).to_string()).collect(),
            },
            Input { id: "value".into(), label: "Value".into(), value: String::new(), kind: InputKind::Text, options: vec![] },
            Input {
                id: "repeat".into(),
                label: "Repeat".into(),
                value: "Once".into(),
                kind: InputKind::Choice,
                options: REPEATS.iter().map(|c| (*c).to_string()).collect(),
            },
            Input { id: "note".into(), label: "Note".into(), value: String::new(), kind: InputKind::Text, options: vec![] },
        ],
    });
    s.menu_item("Create Alert", Action::new("ALRT", None).arg("create", "1"), false);

    let rules = engine.alert_rules();
    let rows: Vec<Row> = rules
        .iter()
        .map(|r| {
            let status = if r.enabled { Cell::text("ACTIVE").styled(Style::Up) } else { Cell::text("OFF").styled(Style::Muted) };
            let repeat = match r.repeat {
                AlertRepeat::Once => "Once".to_string(),
                AlertRepeat::EveryCross => "Every cross".to_string(),
                AlertRepeat::Cooldown { seconds } => format!("Every {} min", seconds / 60),
            };
            Row::new(vec![
                Cell::text(r.security.to_string()).styled(Style::Emphasis),
                Cell::text(r.condition.describe()),
                Cell::text(repeat),
                status,
                Cell::text(r.note.clone().unwrap_or_default()),
            ])
            .action(Action::new("ALRT", None).arg("toggle", r.id.to_string()))
        })
        .collect();
    if rows.is_empty() {
        s.push(Block::Notice { level: NoticeLevel::Info, text: "No alerts yet. Fill in New Alert, then select Create Alert.".into() });
    } else {
        s.push(Block::Table(Table {
            title: Some("Alerts (select to enable/disable)".into()),
            columns: vec![
                Column::text("Security", 20),
                Column::text("Condition", 30),
                Column::text("Repeat", 13),
                Column::text("Status", 7),
                Column::text("Note", 24),
            ],
            rows,
            page_size: Some(15),
            numbered: true,
        }));
    }
    if let Ok(events) = engine.stores().app.alert_events(30)
        && !events.is_empty()
    {
        let rows = events
            .iter()
            .map(|e| Row::new(vec![Cell::num(Some(e.fired_at as f64)), Cell::text(&e.message)]))
            .collect();
        s.push(Block::Table(Table {
            title: Some("Recent Triggers".into()),
            columns: vec![Column::num("Time", Format::DateTime, 16), Column::text("Alert", 80)],
            rows,
            page_size: Some(10),
            numbered: false,
        }));
    }
    s
}

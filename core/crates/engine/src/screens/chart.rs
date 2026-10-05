//! GP / GIP — chart screens and the chart data service.

use std::sync::Arc;

use meridian_analytics::indicators;
use meridian_types::{BarInterval, BarSeries, SecurityKey};
use serde::{Deserialize, Serialize};

use super::{ScreenRequest, parse_range, require_security};
use crate::core::Engine;
use crate::error::{EngineError, EngineResult};
use crate::screen::{Action, Block, ChartSpec, ChartStyle, Input, InputKind, Screen, SourceBadge};

/// One study, parsed from `NAME[:p1[:p2[:p3]]]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Study {
    Sma(usize),
    Ema(usize),
    Bollinger(usize, f64),
    Vwap,
    Rsi(usize),
    Macd(usize, usize, usize),
    Atr(usize),
    Stochastic(usize, usize),
    Obv,
}

impl Study {
    #[must_use]
    pub fn parse(s: &str) -> Option<Study> {
        let mut parts = s.trim().split(':');
        let name = parts.next()?.to_ascii_uppercase();
        let p: Vec<f64> = parts.filter_map(|x| x.trim().parse().ok()).collect();
        let u = |i: usize, d: usize| p.get(i).map_or(d, |v| (*v as usize).max(1));
        Some(match name.as_str() {
            "SMA" | "MA" => Study::Sma(u(0, 50)),
            "EMA" => Study::Ema(u(0, 20)),
            "BB" | "BOLL" => Study::Bollinger(u(0, 20), p.get(1).copied().unwrap_or(2.0)),
            "VWAP" => Study::Vwap,
            "RSI" => Study::Rsi(u(0, 14)),
            "MACD" => Study::Macd(u(0, 12), u(1, 26), u(2, 9)),
            "ATR" => Study::Atr(u(0, 14)),
            "STOCH" => Study::Stochastic(u(0, 14), u(1, 3)),
            "OBV" => Study::Obv,
            _ => return None,
        })
    }

    fn overlay(&self) -> bool {
        matches!(self, Study::Sma(_) | Study::Ema(_) | Study::Bollinger(..) | Study::Vwap)
    }
}

/// A computed study line. `pane` 0 = price overlay; 1.. = lower panes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StudyLine {
    pub name: String,
    pub pane: u8,
    pub values: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartData {
    pub series: BarSeries,
    pub studies: Vec<StudyLine>,
    pub price_decimals: u8,
    pub sources: Vec<SourceBadge>,
    pub stale: bool,
}

fn compute_studies(s: &BarSeries, studies: &[Study]) -> Vec<StudyLine> {
    let mut out = Vec::new();
    let mut pane = 0u8;
    for st in studies {
        let p = if st.overlay() {
            0
        } else {
            pane += 1;
            pane
        };
        match st {
            Study::Sma(n) => out.push(StudyLine { name: format!("SMA({n})"), pane: p, values: indicators::sma(&s.close, *n) }),
            Study::Ema(n) => out.push(StudyLine { name: format!("EMA({n})"), pane: p, values: indicators::ema(&s.close, *n) }),
            Study::Bollinger(n, k) => {
                let b = indicators::bollinger(&s.close, *n, *k);
                out.push(StudyLine { name: format!("BB Upper({n},{k})"), pane: p, values: b.upper });
                out.push(StudyLine { name: format!("BB Mid({n})"), pane: p, values: b.middle });
                out.push(StudyLine { name: format!("BB Lower({n},{k})"), pane: p, values: b.lower });
            }
            Study::Vwap => out.push(StudyLine { name: "VWAP".into(), pane: p, values: indicators::vwap(&s.high, &s.low, &s.close, &s.volume) }),
            Study::Rsi(n) => out.push(StudyLine { name: format!("RSI({n})"), pane: p, values: indicators::rsi(&s.close, *n) }),
            Study::Macd(f, sl, sig) => {
                let m = indicators::macd(&s.close, *f, *sl, *sig);
                out.push(StudyLine { name: format!("MACD({f},{sl})"), pane: p, values: m.macd });
                out.push(StudyLine { name: format!("Signal({sig})"), pane: p, values: m.signal });
                out.push(StudyLine { name: "Hist".into(), pane: p, values: m.hist });
            }
            Study::Atr(n) => out.push(StudyLine { name: format!("ATR({n})"), pane: p, values: indicators::atr(&s.high, &s.low, &s.close, *n) }),
            Study::Stochastic(k, d) => {
                let (kv, dv) = indicators::stochastic(&s.high, &s.low, &s.close, *k, *d);
                out.push(StudyLine { name: format!("%K({k})"), pane: p, values: kv });
                out.push(StudyLine { name: format!("%D({d})"), pane: p, values: dv });
            }
            Study::Obv => out.push(StudyLine { name: "OBV".into(), pane: p, values: indicators::obv(&s.close, &s.volume) }),
        }
    }
    out
}

/// Bars needed before the visible range so studies are warmed up.
fn warmup_bars(studies: &[Study]) -> i64 {
    studies
        .iter()
        .map(|s| match s {
            Study::Sma(n) | Study::Ema(n) | Study::Bollinger(n, _) | Study::Rsi(n) | Study::Atr(n) => *n as i64 * 3,
            Study::Macd(_, sl, sig) => (*sl + *sig) as i64 * 3,
            Study::Stochastic(k, d) => (*k + *d) as i64 * 2,
            Study::Vwap | Study::Obv => 0,
        })
        .max()
        .unwrap_or(0)
}

impl Engine {
    /// Bars plus computed studies for a chart.
    pub async fn chart_data(&self, key: &SecurityKey, interval: BarInterval, range: &str, studies: &[Study]) -> EngineResult<ChartData> {
        let now = self.now();
        let (from, to) = parse_range(range, now);
        let warm = warmup_bars(studies) * interval.seconds() * 1_000_000_000 * if interval.is_intraday() { 1 } else { 2 };
        let fetch_from = from.map(|f| f - warm);
        // Weekly/monthly charts aggregate daily bars (providers vary in
        // what they offer natively).
        let fetch_iv = if matches!(interval, BarInterval::Week | BarInterval::Month) { BarInterval::Day } else { interval };
        let fetched = self.bars(key, fetch_iv, fetch_from, to).await?;
        let full = if fetch_iv == interval {
            fetched.value
        } else {
            let period = if interval == BarInterval::Week { super::hp::Period::Weekly } else { super::hp::Period::Monthly };
            let mut agg = BarSeries::new(key.clone(), interval, fetched.value.adjustment, fetched.value.provenance.clone());
            for b in super::hp::aggregate(&fetched.value, period) {
                agg.push(b);
            }
            agg
        };
        if full.is_empty() {
            return Err(EngineError::NotFound(format!("no {interval} history for {key}")));
        }
        let lines = compute_studies(&full, studies);
        // Trim warm-up bars from the output.
        let start = from.map_or(0, |f| full.ts.partition_point(|t| *t < f));
        let series = full.slice_time(full.ts[start], i64::MAX);
        let studies = lines.into_iter().map(|l| StudyLine { values: l.values[start..].to_vec(), ..l }).collect();
        Ok(ChartData {
            sources: vec![SourceBadge::from_provenance(&series.provenance)],
            series,
            studies,
            price_decimals: self.price_decimals(key),
            stale: fetched.stale,
        })
    }
}

const RANGES: &[&str] = &["1D", "5D", "1M", "3M", "6M", "YTD", "1Y", "5Y", "10Y", "MAX"];

fn default_interval(function: &str, range: &str) -> BarInterval {
    match (function, range) {
        (_, "1D") => BarInterval::Minute(1),
        (_, "5D") => BarInterval::Minute(5),
        ("GIP", "1M") => BarInterval::Minute(30),
        ("GIP", _) => BarInterval::Hour(1),
        (_, "5Y" | "10Y" | "MAX") => BarInterval::Week,
        _ => BarInterval::Day,
    }
}

pub(crate) async fn gp(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    let function = req.function.clone();
    let title = if function == "GIP" { "Intraday Price Graph" } else { "Historical Price Graph" };
    let key = match require_security(&function, title, &req) {
        Ok(k) => k,
        Err(s) => return *s,
    };
    let ks = key.to_string();
    let default_range = if function == "GIP" { "1D" } else { "1Y" };
    let range = req.arg("range").unwrap_or(default_range).to_ascii_uppercase();
    let interval = req.arg("interval").and_then(BarInterval::from_code).unwrap_or_else(|| default_interval(&function, &range));
    let style = match req.arg("style") {
        Some("Line") => ChartStyle::Line,
        Some("Mountain") => ChartStyle::Mountain,
        Some("Bars") => ChartStyle::Bars,
        _ => ChartStyle::Candles,
    };
    let studies_arg = req.arg("studies").unwrap_or(if function == "GIP" { "VWAP" } else { "SMA:50,SMA:200" }).to_owned();
    let studies: Vec<String> = studies_arg.split(',').map(str::trim).filter(|s| Study::parse(s).is_some()).map(str::to_owned).collect();

    let mut s = Screen::new(&function, format!("{ks} — {title}"), Some(ks.clone()));
    if let Some(p) = engine.quote_provenance(&key) {
        s.source(&p);
    }
    let base = Action::new(&function, Some(&ks)).with_args(&req.args);
    let ranges: &[&str] = if function == "GIP" { &["1D", "5D", "1M"] } else { RANGES };
    for r in ranges {
        let mut a = base.clone().arg("range", *r);
        a.args.retain(|(k, _)| k != "interval");
        s.menu_item(r, a, *r == range);
    }
    s.push(Block::Inputs {
        title: None,
        inputs: vec![
            Input {
                id: "style".into(),
                label: "Style".into(),
                value: match style {
                    ChartStyle::Line => "Line",
                    ChartStyle::Mountain => "Mountain",
                    ChartStyle::Bars => "Bars",
                    ChartStyle::Candles => "Candles",
                }
                .into(),
                kind: InputKind::Choice,
                options: vec!["Candles".into(), "Bars".into(), "Line".into(), "Mountain".into()],
            },
            Input {
                id: "studies".into(),
                label: "Studies".into(),
                value: studies.join(","),
                kind: InputKind::Text,
                options: vec![],
            },
            Input { id: "interval".into(), label: "Interval".into(), value: interval.code(), kind: InputKind::Text, options: vec![] },
        ],
    });
    s.push(Block::Chart(ChartSpec { security: ks, interval: interval.code(), range, style, indicators: studies, height_rows: 0 }));
    s
}

/// Columnar encoding for the FFI: `[n: u32][origin: f64]` then
/// `ts: i64[n]`, `open/high/low/close: f32[n]` relative to origin,
/// `volume: f32[n]`, little-endian.
#[must_use]
pub fn pack_bars(series: &BarSeries) -> (f64, Vec<u8>) {
    let n = series.len();
    let origin = series.close.last().copied().unwrap_or(0.0);
    let mut out = Vec::with_capacity(12 + n * (8 + 4 * 5));
    out.extend_from_slice(&u32::try_from(n).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(&origin.to_le_bytes());
    for t in &series.ts {
        out.extend_from_slice(&t.to_le_bytes());
    }
    for col in [&series.open, &series.high, &series.low, &series.close] {
        for v in col {
            out.extend_from_slice(&((v - origin) as f32).to_le_bytes());
        }
    }
    for v in &series.volume {
        out.extend_from_slice(&(*v as f32).to_le_bytes());
    }
    (origin, out)
}

/// f32 little-endian encoding; overlay studies are relative to `origin`.
#[must_use]
pub fn pack_values(values: &[f64], origin: f64) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for v in values {
        out.extend_from_slice(&((v - origin) as f32).to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn study_parsing() {
        assert_eq!(Study::parse("SMA:20"), Some(Study::Sma(20)));
        assert_eq!(Study::parse("bb:20:2.5"), Some(Study::Bollinger(20, 2.5)));
        assert_eq!(Study::parse("MACD"), Some(Study::Macd(12, 26, 9)));
        assert_eq!(Study::parse("nope"), None);
    }

    #[test]
    fn panes_assigned() {
        use meridian_types::{Adjustment, Bar, Provenance};
        let mut s = BarSeries::new(SecurityKey::equity("X"), BarInterval::Day, Adjustment::None, Provenance::synthetic(0));
        for i in 0..60 {
            let c = 100.0 + f64::from(i);
            s.push(Bar { ts: i64::from(i), open: c, high: c + 1.0, low: c - 1.0, close: c, volume: 1.0 });
        }
        let lines = compute_studies(&s, &[Study::Sma(5), Study::Rsi(14), Study::Macd(12, 26, 9)]);
        assert_eq!(lines[0].pane, 0);
        assert_eq!(lines[1].pane, 1);
        assert!(lines[2..].iter().all(|l| l.pane == 2));
        assert!(lines.iter().all(|l| l.values.len() == 60));
    }

    #[test]
    fn pack_layout() {
        use meridian_types::{Adjustment, Bar, Provenance};
        let mut s = BarSeries::new(SecurityKey::equity("X"), BarInterval::Day, Adjustment::None, Provenance::synthetic(0));
        s.push(Bar { ts: 7, open: 1.0, high: 2.0, low: 0.5, close: 1.5, volume: 9.0 });
        let (origin, b) = pack_bars(&s);
        assert_eq!(origin, 1.5);
        assert_eq!(b.len(), 4 + 8 + 8 + 4 * 5);
        assert_eq!(i64::from_le_bytes(b[12..20].try_into().unwrap()), 7);
    }
}

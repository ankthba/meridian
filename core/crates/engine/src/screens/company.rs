//! FA, EE, ERN, ANR, HDS, DVD — company data screens.

use std::sync::Arc;

use meridian_provider::FundamentalsRequest;
use meridian_types::{
    DividendKind, EstimateMetric, Fundamentals, HolderKind, PeriodType, SecurityKey, Statement, StatementKind,
};

use super::{ScreenRequest, error_screen, require_security, stale_notice};
use crate::cache::{Fetched, ttl};
use crate::core::Engine;
use crate::error::EngineResult;
use crate::screen::{
    Action, Block, Cell, Column, Field, Format, Input, InputKind, NoticeLevel, Row, Screen, Style, Table, XyChart, XySeries,
};

impl Engine {
    pub async fn fundamentals(&self, key: &SecurityKey, period_type: PeriodType, periods: usize) -> EngineResult<Fetched<Fundamentals>> {
        let router = self.router().clone();
        let req = FundamentalsRequest { key: key.clone(), period_type, periods };
        let ck = format!("{key}|{period_type:?}|{periods}");
        let f = self.cached("fundamentals", &ck, ttl::FUNDAMENTALS, async move { router.fundamentals(req).await }).await?;
        if !f.from_cache && !f.value.provenance.synthetic {
            let _ = self.stores().market.put_fundamentals(&f.value);
        }
        Ok(f)
    }
}

// --- FA -------------------------------------------------------------------

const FA_TITLE: &str = "Financial Analysis";

fn statements_of(f: &Fundamentals, kind: StatementKind, limit: usize) -> Vec<&Statement> {
    let mut v: Vec<&Statement> = f.statements.iter().filter(|s| s.kind == kind).collect();
    v.sort_by_key(|s| s.period_end);
    let skip = v.len().saturating_sub(limit);
    v.into_iter().skip(skip).collect()
}

fn period_label(st: &Statement) -> String {
    if st.fiscal_period == "FY" { format!("FY {}", st.fiscal_year) } else { format!("{} {}", st.fiscal_period, st.fiscal_year % 100) }
}

fn is_per_share(code: &str) -> bool {
    code.starts_with("eps") || code.contains("per_share") || code.ends_with("_ps")
}

pub(crate) async fn fa(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    let key = match require_security("FA", FA_TITLE, &req) {
        Ok(k) => k,
        Err(s) => return *s,
    };
    let ks = key.to_string();
    let stmt = req.arg("stmt").unwrap_or("IS").to_ascii_uppercase();
    let per = if req.arg("per").is_some_and(|p| p.starts_with('Q')) { PeriodType::Quarterly } else { PeriodType::Annual };
    let n = if per == PeriodType::Annual { 10 } else { 12 };
    let f = match engine.fundamentals(&key, per, n).await {
        Ok(f) => f,
        Err(e) => return error_screen("FA", FA_TITLE, Some(&key), &e),
    };
    let mut s = Screen::new("FA", format!("{ks} — {FA_TITLE}"), Some(ks.clone()));
    s.source(&f.value.provenance);
    stale_notice(&mut s, &f);
    let base = Action::new("FA", Some(&ks)).with_args(&req.args);
    for (label, code) in [("Income Statement", "IS"), ("Balance Sheet", "BS"), ("Cash Flow", "CF"), ("Ratios", "RATIOS")] {
        s.menu_item(label, base.clone().arg("stmt", code), stmt == code);
    }
    s.push(Block::Inputs {
        title: None,
        inputs: vec![Input {
            id: "per".into(),
            label: "Periodicity".into(),
            value: if per == PeriodType::Annual { "Annual" } else { "Quarterly" }.into(),
            kind: InputKind::Choice,
            options: vec!["Annual".into(), "Quarterly".into()],
        }],
    });

    if stmt == "RATIOS" {
        s.push(ratios_table(&f.value, n));
        return s;
    }
    let kind = match stmt.as_str() {
        "BS" => StatementKind::Balance,
        "CF" => StatementKind::CashFlow,
        _ => StatementKind::Income,
    };
    let sts = statements_of(&f.value, kind, n);
    if sts.is_empty() {
        s.push(Block::Notice { level: NoticeLevel::Info, text: format!("No {} data", kind.title()) });
        return s;
    }
    let currency = sts.last().map_or("USD", |x| x.currency.as_str()).to_owned();
    // Row order: the most recent statement's line order, then any lines
    // only present in earlier periods.
    let mut order: Vec<(String, String, u8)> = Vec::new();
    for st in sts.iter().rev() {
        for l in &st.lines {
            if !order.iter().any(|(c, _, _)| *c == l.code) {
                order.push((l.code.clone(), l.label.clone(), l.depth));
            }
        }
    }
    let mut columns = vec![Column::text(&format!("In Millions of {currency} except per share"), 34)];
    for st in &sts {
        columns.push(Column::num(&period_label(st), Format::Number { decimals: 1 }, 11));
    }
    let rows = order
        .iter()
        .map(|(code, label, depth)| {
            let mut cells = vec![Cell::text(label)];
            for st in &sts {
                let v = st.lines.iter().find(|l| &l.code == code).and_then(|l| l.value);
                let shown = if is_per_share(code) { v } else { v.map(|x| x / 1e6) };
                let mut c = Cell::num(shown);
                if is_per_share(code) {
                    c.text = shown.map(|x| format!("{x:.2}"));
                    c.value = None;
                }
                if shown.is_some_and(|x| x < 0.0) {
                    c = c.styled(Style::Down);
                }
                cells.push(c);
            }
            let mut r = Row::new(cells).depth(*depth);
            if *depth == 0 && matches!(code.as_str(), "revenue" | "gross_profit" | "operating_income" | "net_income" | "total_assets" | "total_liabilities" | "total_equity" | "cfo" | "fcf") {
                r = r.emphasis();
            }
            r
        })
        .collect();
    s.push(Block::Table(Table { title: Some(kind.title().into()), columns, rows, page_size: Some(30), numbered: false }));
    s
}

fn ratios_table(f: &Fundamentals, n: usize) -> Block {
    let inc = statements_of(f, StatementKind::Income, n);
    let bal = statements_of(f, StatementKind::Balance, n + 1);
    let cf = statements_of(f, StatementKind::CashFlow, n);
    let find_bal = |end: chrono::NaiveDate| bal.iter().find(|b| b.period_end == end).copied();
    let prev_bal = |end: chrono::NaiveDate| bal.iter().rev().find(|b| b.period_end < end).copied();
    let prev_inc = |i: usize| (i > 0).then(|| inc[i - 1]);
    let ratio = |a: Option<f64>, b: Option<f64>| match (a, b) {
        (Some(a), Some(b)) if b != 0.0 => Some(a / b * 100.0),
        _ => None,
    };
    let growth = |a: Option<f64>, b: Option<f64>| match (a, b) {
        (Some(a), Some(b)) if b != 0.0 => Some((a / b - 1.0) * 100.0),
        _ => None,
    };
    let mut columns = vec![Column::text("Ratios", 30)];
    for st in &inc {
        columns.push(Column::num(&period_label(st), Format::Number { decimals: 2 }, 11));
    }
    type RatioFn<'a> = Box<dyn Fn(usize) -> Option<f64> + 'a>;
    let defs: Vec<(&str, RatioFn)> = vec![
        ("Gross Margin %", Box::new(|i| ratio(inc[i].value("gross_profit"), inc[i].value("revenue")))),
        ("Operating Margin %", Box::new(|i| ratio(inc[i].value("operating_income"), inc[i].value("revenue")))),
        ("Net Margin %", Box::new(|i| ratio(inc[i].value("net_income"), inc[i].value("revenue")))),
        ("Revenue Growth % (YoY)", Box::new(|i| {
            let prev = if inc[i].period_type == PeriodType::Quarterly { (i >= 4).then(|| inc[i - 4]) } else { prev_inc(i) };
            growth(inc[i].value("revenue"), prev.and_then(|p| p.value("revenue")))
        })),
        ("EPS Growth % (YoY)", Box::new(|i| {
            let prev = if inc[i].period_type == PeriodType::Quarterly { (i >= 4).then(|| inc[i - 4]) } else { prev_inc(i) };
            growth(inc[i].value("eps_diluted"), prev.and_then(|p| p.value("eps_diluted")))
        })),
        ("Return on Equity %", Box::new(|i| {
            let end = inc[i].period_end;
            let eq = find_bal(end).and_then(|b| b.value("total_equity"));
            let peq = prev_bal(end).and_then(|b| b.value("total_equity"));
            let avg = match (eq, peq) {
                (Some(a), Some(b)) => Some(f64::midpoint(a, b)),
                (Some(a), None) => Some(a),
                _ => None,
            };
            let mult = if inc[i].period_type == PeriodType::Quarterly { 4.0 } else { 1.0 };
            ratio(inc[i].value("net_income").map(|x| x * mult), avg)
        })),
        ("Return on Assets %", Box::new(|i| {
            let mult = if inc[i].period_type == PeriodType::Quarterly { 4.0 } else { 1.0 };
            ratio(inc[i].value("net_income").map(|x| x * mult), find_bal(inc[i].period_end).and_then(|b| b.value("total_assets")))
        })),
        ("Debt / Equity %", Box::new(|i| {
            let b = find_bal(inc[i].period_end)?;
            let debt = match (b.value("long_term_debt"), b.value("short_term_debt")) {
                (None, None) => None,
                (a, c) => Some(a.unwrap_or(0.0) + c.unwrap_or(0.0)),
            };
            ratio(debt, b.value("total_equity"))
        })),
        ("Current Ratio", Box::new(|i| {
            let b = find_bal(inc[i].period_end)?;
            ratio(b.value("total_current_assets"), b.value("total_current_liabilities")).map(|x| x / 100.0)
        })),
        ("FCF Margin %", Box::new(|i| {
            let c = cf.iter().find(|c| c.period_end == inc[i].period_end)?;
            ratio(c.value("fcf"), inc[i].value("revenue"))
        })),
    ];
    let rows = defs
        .iter()
        .map(|(label, f)| {
            let mut cells = vec![Cell::text(*label)];
            for i in 0..inc.len() {
                cells.push(Cell::num(f(i)));
            }
            Row::new(cells)
        })
        .collect();
    Block::Table(Table { title: Some("Key Ratios (computed from reported statements)".into()), columns, rows, page_size: None, numbered: false })
}

// --- EE -------------------------------------------------------------------

pub(crate) async fn ee(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    const T: &str = "Earnings Estimates";
    let key = match require_security("EE", T, &req) {
        Ok(k) => k,
        Err(s) => return *s,
    };
    let ks = key.to_string();
    let router = engine.router().clone();
    let k2 = key.clone();
    let est = match engine.cached("estimates", &ks, ttl::ESTIMATES, async move { router.estimates(&k2).await }).await {
        Ok(e) => e,
        Err(e) => return error_screen("EE", T, Some(&key), &e),
    };
    let metric = match req.arg("metric").unwrap_or("EPS") {
        "REV" | "Revenue" => EstimateMetric::Revenue,
        "EBITDA" => EstimateMetric::Ebitda,
        _ => EstimateMetric::Eps,
    };
    let mut s = Screen::new("EE", format!("{ks} — {T}"), Some(ks.clone()));
    s.source(&est.value.provenance);
    stale_notice(&mut s, &est);
    for (label, m, code) in [("EPS", EstimateMetric::Eps, "EPS"), ("Revenue", EstimateMetric::Revenue, "REV"), ("EBITDA", EstimateMetric::Ebitda, "EBITDA")] {
        s.menu_item(label, Action::new("EE", Some(&ks)).arg("metric", code), m == metric);
    }
    let (fmt, scale) = if metric == EstimateMetric::Eps { (Format::Number { decimals: 2 }, 1.0) } else { (Format::Number { decimals: 1 }, 1e6) };
    let mut list: Vec<_> = est.value.estimates.iter().filter(|e| e.metric == metric).collect();
    list.sort_by_key(|e| (e.period_type == PeriodType::Annual, e.period_end));
    if list.is_empty() {
        s.push(Block::Notice { level: NoticeLevel::Info, text: format!("No {} estimates", metric.label()) });
        return s;
    }
    let sc = |v: Option<f64>| v.map(|x| x / scale);
    let rows: Vec<Row> = list
        .iter()
        .map(|e| {
            Row::new(vec![
                Cell::text(&e.fiscal_label).styled(Style::Emphasis),
                Cell::text(e.period_end.format("%m/%Y").to_string()),
                Cell::num(sc(e.mean)),
                Cell::num(sc(e.median)),
                Cell::num(sc(e.high)),
                Cell::num(sc(e.low)),
                Cell::num(e.count.map(f64::from)),
                Cell::num(sc(e.actual)),
            ])
        })
        .collect();
    let unit = if metric == EstimateMetric::Eps { "" } else { " (M)" };
    s.push(Block::Table(Table {
        title: Some(format!("{}{unit} — Consensus", metric.label())),
        columns: vec![
            Column::text("Period", 10),
            Column::text("End", 8),
            Column::num("Mean", fmt, 11),
            Column::num("Median", fmt, 11),
            Column::num("High", fmt, 11),
            Column::num("Low", fmt, 11),
            Column::num("# Est", Format::Integer, 6),
            Column::num("Actual", fmt, 11),
        ],
        rows,
        page_size: None,
        numbered: false,
    }));
    s
}

// --- ERN ------------------------------------------------------------------

pub(crate) async fn ern(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    const T: &str = "Earnings History";
    let key = match require_security("ERN", T, &req) {
        Ok(k) => k,
        Err(s) => return *s,
    };
    let ks = key.to_string();
    let router = engine.router().clone();
    let k2 = key.clone();
    let h = match engine.cached("earnings", &ks, ttl::ESTIMATES, async move { router.earnings(&k2).await }).await {
        Ok(h) => h,
        Err(e) => return error_screen("ERN", T, Some(&key), &e),
    };
    let mut s = Screen::new("ERN", format!("{ks} — {T}"), Some(ks.clone()));
    s.source(&h.value.provenance);
    stale_notice(&mut s, &h);
    let mut recs = h.value.records.clone();
    recs.sort_by_key(|r| r.period_end);
    let beats = recs.iter().filter(|r| r.eps_surprise_pct().is_some_and(|x| x > 0.0)).count();
    let with_est = recs.iter().filter(|r| r.eps_surprise_pct().is_some()).count();
    if with_est > 0 {
        s.push(Block::Fields {
            title: None,
            columns: 3,
            fields: vec![
                Field::num("Quarters", Some(recs.len() as f64), Format::Integer),
                Field::num("EPS Beats", Some(beats as f64), Format::Integer),
                Field::num("Beat Rate %", Some(beats as f64 / with_est as f64 * 100.0), Format::Percent { decimals: 0 }),
            ],
        });
    } else {
        s.push(Block::Notice { level: NoticeLevel::Info, text: "Consensus estimates are not available from the configured sources; actuals only.".into() });
    }
    s.push(Block::Xy(XyChart {
        title: "EPS: Actual vs Estimate".into(),
        x_label: "Quarter".into(),
        y_label: "EPS".into(),
        series: vec![
            XySeries { name: "Estimate".into(), x: (0..recs.len()).map(|i| i as f64).collect(), y: recs.iter().map(|r| r.eps_estimate.unwrap_or(f64::NAN)).collect(), style: Style::Muted, bars: true },
            XySeries { name: "Actual".into(), x: (0..recs.len()).map(|i| i as f64).collect(), y: recs.iter().map(|r| r.eps_actual.unwrap_or(f64::NAN)).collect(), style: Style::Emphasis, bars: true },
        ],
        x_marker: None,
        height_rows: 8,
    }));
    let rows: Vec<Row> = recs
        .iter()
        .rev()
        .map(|r| {
            Row::new(vec![
                Cell::text(&r.fiscal_label).styled(Style::Emphasis),
                Cell::text(r.announce_date.map(|d| d.format("%m/%d/%y").to_string()).unwrap_or_default()),
                Cell::num(r.eps_estimate),
                Cell::num(r.eps_actual),
                Cell::signed(r.eps_surprise_pct()),
                Cell::num(r.revenue_estimate.map(|x| x / 1e6)),
                Cell::num(r.revenue_actual.map(|x| x / 1e6)),
                Cell::signed(r.revenue_surprise_pct()),
            ])
        })
        .collect();
    s.push(Block::Table(Table {
        title: None,
        columns: vec![
            Column::text("Period", 10),
            Column::text("Announce", 9),
            Column::num("EPS Est", Format::Number { decimals: 2 }, 9),
            Column::num("EPS Act", Format::Number { decimals: 2 }, 9),
            Column::num("Surp %", Format::ChangePercent { decimals: 1 }, 8),
            Column::num("Rev Est (M)", Format::Number { decimals: 0 }, 12),
            Column::num("Rev Act (M)", Format::Number { decimals: 0 }, 12),
            Column::num("Surp %", Format::ChangePercent { decimals: 1 }, 8),
        ],
        rows,
        page_size: Some(20),
        numbered: false,
    }));
    s
}

// --- ANR ------------------------------------------------------------------

pub(crate) async fn anr(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    const T: &str = "Analyst Recommendations";
    let key = match require_security("ANR", T, &req) {
        Ok(k) => k,
        Err(s) => return *s,
    };
    let ks = key.to_string();
    let router = engine.router().clone();
    let k2 = key.clone();
    let (recs, quote) = tokio::join!(
        engine.cached("recommendations", &ks, ttl::ESTIMATES, async move { router.recommendations(&k2).await }),
        engine.quote_row(&key)
    );
    let r = match recs {
        Ok(r) => r,
        Err(e) => return error_screen("ANR", T, Some(&key), &e),
    };
    let mut s = Screen::new("ANR", format!("{ks} — {T}"), Some(ks.clone()));
    s.source(&r.value.provenance);
    stale_notice(&mut s, &r);
    let v = &r.value;
    let last = quote.map(|q| q.last).filter(|x| x.is_finite());
    let mut fields = vec![
        Field::num("Consensus (1 Sell–5 Buy)", v.consensus_score(), Format::Number { decimals: 2 }).styled(Style::Emphasis),
        Field::num("Analysts", Some(f64::from(v.total())), Format::Integer),
        Field::text("As of", v.as_of.format("%m/%d/%Y").to_string()),
        Field::num("Target Mean", v.target_mean, Format::Number { decimals: 2 }),
        Field::num("Target High", v.target_high, Format::Number { decimals: 2 }),
        Field::num("Target Low", v.target_low, Format::Number { decimals: 2 }),
    ];
    if let (Some(t), Some(px)) = (v.target_mean, last) {
        fields.push(Field::num("Last Price", Some(px), Format::Number { decimals: 2 }));
        fields.push(Field::num("Implied Return %", Some((t / px - 1.0) * 100.0), Format::ChangePercent { decimals: 1 }));
    }
    s.push(Block::Fields { title: None, columns: 3, fields });
    let labels = ["Strong Buy", "Buy", "Hold", "Sell", "Strong Sell"];
    let counts = [v.strong_buy, v.buy, v.hold, v.sell, v.strong_sell];
    s.push(Block::Xy(XyChart {
        title: "Rating Distribution".into(),
        x_label: labels.join(" | "),
        y_label: "Analysts".into(),
        series: vec![XySeries { name: "Count".into(), x: (0..5).map(f64::from).collect(), y: counts.iter().map(|c| f64::from(*c)).collect(), style: Style::Emphasis, bars: true }],
        x_marker: None,
        height_rows: 6,
    }));
    if !v.ratings.is_empty() {
        let mut ratings = v.ratings.clone();
        ratings.sort_by(|a, b| b.date.cmp(&a.date));
        let rows = ratings
            .iter()
            .map(|a| {
                let style = match a.score {
                    Some(4 | 5) => Style::Up,
                    Some(1 | 2) => Style::Down,
                    _ => Style::Normal,
                };
                Row::new(vec![
                    Cell::text(&a.firm),
                    Cell::text(a.analyst.clone().unwrap_or_default()),
                    Cell::text(&a.rating).styled(style),
                    Cell::num(a.target_price),
                    Cell::text(a.date.format("%m/%d/%y").to_string()),
                ])
            })
            .collect();
        s.push(Block::Table(Table {
            title: Some("Broker Ratings".into()),
            columns: vec![
                Column::text("Firm", 28),
                Column::text("Analyst", 20),
                Column::text("Rating", 14),
                Column::num("Target", Format::Number { decimals: 2 }, 9),
                Column::text("Date", 9),
            ],
            rows,
            page_size: Some(18),
            numbered: false,
        }));
    }
    s
}

// --- HDS ------------------------------------------------------------------

pub(crate) async fn hds(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    const T: &str = "Holders";
    let key = match require_security("HDS", T, &req) {
        Ok(k) => k,
        Err(s) => return *s,
    };
    let ks = key.to_string();
    let router = engine.router().clone();
    let k2 = key.clone();
    let h = match engine.cached("holders", &ks, ttl::HOLDERS, async move { router.holders(&k2).await }).await {
        Ok(h) => h,
        Err(e) => return error_screen("HDS", T, Some(&key), &e),
    };
    let filter = req.arg("type").unwrap_or("All").to_owned();
    let mut s = Screen::new("HDS", format!("{ks} — {T}"), Some(ks.clone()));
    s.source(&h.value.provenance);
    stale_notice(&mut s, &h);
    for label in ["All", "Institutions", "Funds", "Insiders"] {
        s.menu_item(label, Action::new("HDS", Some(&ks)).arg("type", label), filter == label);
    }
    let want = |k: HolderKind| match filter.as_str() {
        "Institutions" => k == HolderKind::Institution,
        "Funds" => k == HolderKind::Fund,
        "Insiders" => k == HolderKind::Insider,
        _ => true,
    };
    let mut list: Vec<_> = h.value.holders.iter().filter(|x| want(x.kind)).collect();
    list.sort_by(|a, b| b.shares.total_cmp(&a.shares));
    let total_pct: f64 = list.iter().filter_map(|x| x.pct_outstanding).sum();
    s.push(Block::Fields {
        title: None,
        columns: 2,
        fields: vec![
            Field::num("Holders Shown", Some(list.len() as f64), Format::Integer),
            Field::num("% Out (shown)", Some(total_pct), Format::Percent { decimals: 2 }),
        ],
    });
    let rows = list
        .iter()
        .map(|x| {
            Row::new(vec![
                Cell::text(&x.name),
                Cell::text(match x.kind {
                    HolderKind::Institution => "Inst",
                    HolderKind::Fund => "Fund",
                    HolderKind::Insider => "Insider",
                }),
                Cell::num(Some(x.shares)),
                Cell::num(x.pct_outstanding),
                Cell::num(x.market_value),
                Cell::signed(x.change_shares),
                Cell::text(x.filing_date.map(|d| d.format("%m/%d/%y").to_string()).unwrap_or_default()),
            ])
        })
        .collect();
    s.push(Block::Table(Table {
        title: None,
        columns: vec![
            Column::text("Holder", 34),
            Column::text("Type", 7),
            Column::num("Shares", Format::Large { decimals: 2 }, 10),
            Column::num("% Out", Format::Percent { decimals: 2 }, 7),
            Column::num("Mkt Val", Format::Large { decimals: 2 }, 10),
            Column::num("Chg", Format::Large { decimals: 2 }, 10),
            Column::text("Filed", 9),
        ],
        rows,
        page_size: Some(20),
        numbered: true,
    }));
    s
}

// --- DVD ------------------------------------------------------------------

pub(crate) async fn dvd(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    const T: &str = "Dividends & Splits";
    let key = match require_security("DVD", T, &req) {
        Ok(k) => k,
        Err(s) => return *s,
    };
    let ks = key.to_string();
    let router = engine.router().clone();
    let k2 = key.clone();
    let (d, quote) = tokio::join!(
        engine.cached("dividends", &ks, ttl::HOLDERS, async move { router.dividends(&k2).await }),
        engine.quote_row(&key)
    );
    let d = match d {
        Ok(d) => d,
        Err(e) => return error_screen("DVD", T, Some(&key), &e),
    };
    let mut s = Screen::new("DVD", format!("{ks} — {T}"), Some(ks.clone()));
    s.source(&d.value.provenance);
    stale_notice(&mut s, &d);
    let mut list = d.value.dividends.clone();
    list.sort_by(|a, b| b.ex_date.cmp(&a.ex_date));
    let cash: Vec<_> = list.iter().filter(|x| !matches!(x.kind, DividendKind::Split)).collect();
    let now = meridian_types::nanos_to_date(engine.now());
    let year_ago = now - chrono::Duration::days(365);
    let ttm: f64 = cash.iter().filter(|x| x.ex_date > year_ago).map(|x| x.amount).sum();
    let last = quote.map(|q| q.last).filter(|x| x.is_finite());
    let mut fields = vec![Field::num("Div (TTM)", Some(ttm), Format::Number { decimals: 4 })];
    if let Some(px) = last
        && ttm > 0.0
    {
        fields.push(Field::num("Yield (TTM) %", Some(ttm / px * 100.0), Format::Percent { decimals: 2 }));
    }
    if let Some(l) = cash.first() {
        fields.push(Field::num("Last Amount", Some(l.amount), Format::Number { decimals: 4 }));
        fields.push(Field::text("Last Ex-Date", l.ex_date.format("%m/%d/%Y").to_string()));
        fields.push(Field::opt_text("Frequency", l.frequency.clone()));
    }
    s.push(Block::Fields { title: None, columns: 3, fields });
    let rows = list
        .iter()
        .map(|x| {
            let (kind, amount) = match x.kind {
                DividendKind::Regular => ("Regular Cash", Cell::num(Some(x.amount))),
                DividendKind::Special => ("Special Cash", Cell::num(Some(x.amount))),
                DividendKind::Split => ("Stock Split", Cell::text(format!("{}:1", x.amount)).styled(Style::Emphasis)),
            };
            let d = |o: Option<chrono::NaiveDate>| o.map(|d| d.format("%m/%d/%y").to_string()).unwrap_or_default();
            Row::new(vec![
                Cell::text(d(x.declared_date)),
                Cell::text(x.ex_date.format("%m/%d/%y").to_string()),
                Cell::text(d(x.record_date)),
                Cell::text(d(x.pay_date)),
                amount,
                Cell::text(kind),
            ])
        })
        .collect();
    s.push(Block::Table(Table {
        title: None,
        columns: vec![
            Column::text("Declared", 9),
            Column::text("Ex-Date", 9),
            Column::text("Record", 9),
            Column::text("Payable", 9),
            Column::num("Amount", Format::Number { decimals: 4 }, 9),
            Column::text("Type", 14),
        ],
        rows,
        page_size: Some(20),
        numbered: false,
    }));
    s
}

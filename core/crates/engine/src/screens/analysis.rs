//! EQS, RV, CORR, PORT, BTST — analytics screens.

use std::collections::HashMap;
use std::sync::Arc;

use meridian_analytics::backtest::{BacktestConfig, Direction, Strategy, run_backtest};
use meridian_analytics::{returns, risk, stats};
use meridian_store::Transaction;
use meridian_types::{AssetClass, BarInterval, Fundamentals, MarketSector, NANOS_PER_DAY, PeriodType, SecurityKey, StatementKind, nanos_to_date};

use super::{ScreenRequest, parse_range, require_security};
use crate::config::DataMode;
use crate::core::Engine;
use crate::portfolio::{Holding, Ledger};
use crate::screen::{
    Action, Block, Cell, Column, Field, Format, HeatMap, Input, InputKind, NoticeLevel, Row, Screen, Style, Table, XyChart, XySeries,
};
use crate::universe::DEFAULT_WATCHLIST;

/// Valuation and quality metrics for one company.
#[derive(Debug, Clone, Default)]
pub struct Metrics {
    pub key: Option<SecurityKey>,
    pub name: String,
    pub sector: String,
    pub industry: String,
    pub price: Option<f64>,
    pub market_cap: Option<f64>,
    pub pe: Option<f64>,
    pub ps: Option<f64>,
    pub net_margin: Option<f64>,
    pub rev_growth: Option<f64>,
    pub roe: Option<f64>,
    pub div_yield: Option<f64>,
    pub ret_1y: Option<f64>,
}

impl Metrics {
    /// Fills the valuation and quality figures from annual statements,
    /// using `self.price` for market cap, P/E, P/S and dividend yield.
    pub(crate) fn apply_fundamentals(&mut self, f: &Fundamentals) {
        let mut inc: Vec<_> = f.statements.iter().filter(|s| s.kind == StatementKind::Income).collect();
        inc.sort_by_key(|s| s.period_end);
        let bal = f.statements.iter().filter(|s| s.kind == StatementKind::Balance).max_by_key(|s| s.period_end);
        let Some(last) = inc.last() else { return };
        let rev = last.value("revenue");
        let ni = last.value("net_income");
        let eps = last.value("eps_diluted");
        let shares = last.value("shares_diluted");
        if let (Some(px), Some(sh)) = (self.price, shares) {
            self.market_cap = Some(px * sh);
        }
        if let (Some(px), Some(e)) = (self.price, eps)
            && e > 0.0
        {
            self.pe = Some(px / e);
        }
        if let (Some(mc), Some(r)) = (self.market_cap, rev)
            && r > 0.0
        {
            self.ps = Some(mc / r);
        }
        if let (Some(n), Some(r)) = (ni, rev)
            && r != 0.0
        {
            self.net_margin = Some(n / r * 100.0);
        }
        if inc.len() >= 2
            && let (Some(r1), Some(r0)) = (rev, inc[inc.len() - 2].value("revenue"))
            && r0 != 0.0
        {
            self.rev_growth = Some((r1 / r0 - 1.0) * 100.0);
        }
        if let (Some(n), Some(eq)) = (ni, bal.and_then(|b| b.value("total_equity")))
            && eq > 0.0
        {
            self.roe = Some(n / eq * 100.0);
        }
        if let (Some(dp), Some(sh), Some(px)) =
            (f.statements.iter().filter(|s| s.kind == StatementKind::CashFlow).max_by_key(|s| s.period_end).and_then(|c| c.value("dividends_paid")), shares, self.price)
            && sh > 0.0
            && px > 0.0
        {
            self.div_yield = Some(dp.abs() / sh / px * 100.0);
        }
    }
}

impl Engine {
    /// Computes [`Metrics`] from fundamentals, quote, and history.
    pub async fn company_metrics(&self, key: &SecurityKey) -> Metrics {
        let now = self.now();
        let (inst, quote, fund, bars) = tokio::join!(
            self.instrument(key),
            self.quote_row(key),
            self.fundamentals(key, PeriodType::Annual, 3),
            self.bars(key, BarInterval::Day, Some(now - 370 * NANOS_PER_DAY), now + NANOS_PER_DAY),
        );
        let mut m = Metrics { key: Some(key.clone()), ..Default::default() };
        if let Ok(i) = inst {
            m.name = i.name;
            m.sector = i.sector.unwrap_or_default();
            m.industry = i.industry.unwrap_or_default();
        }
        m.price = quote.map(|q| q.last).filter(|x| x.is_finite());
        if let Ok(b) = &bars
            && let (Some(f), Some(l)) = (b.value.close.first(), b.value.close.last())
        {
            m.ret_1y = Some((l / f - 1.0) * 100.0);
            m.price = m.price.or(Some(*l));
        }
        if let Ok(f) = fund {
            m.apply_fundamentals(&f.value);
        }
        m
    }

    /// Equity universe for screening and peers. In LIVE mode only
    /// securities the user tracks (watchlists) are screened, because there
    /// is no market-wide fundamentals feed in the configured sources.
    pub fn screening_universe(&self) -> Vec<SecurityKey> {
        let mut keys: Vec<SecurityKey> = match self.mode() {
            DataMode::Mock => self
                .universe()
                .into_iter()
                .filter(|i| i.asset_class == AssetClass::Equity && i.key.sector == MarketSector::Equity && !i.key.symbol.starts_with("ZQ"))
                .map(|i| i.key)
                .collect(),
            DataMode::Live => {
                let mut v: Vec<SecurityKey> = DEFAULT_WATCHLIST.iter().filter_map(|s| s.parse().ok()).collect();
                if let Ok(lists) = self.stores().app.watchlists() {
                    for l in lists {
                        v.extend(l.securities.iter().filter_map(|s| s.parse().ok()));
                    }
                }
                v.retain(|k: &SecurityKey| k.sector == MarketSector::Equity);
                v
            }
        };
        keys.sort();
        keys.dedup();
        keys
    }

    async fn metrics_for(self: &Arc<Self>, keys: Vec<SecurityKey>) -> Vec<Metrics> {
        let mut set = tokio::task::JoinSet::new();
        for k in keys {
            let e = self.clone();
            set.spawn(async move { e.company_metrics(&k).await });
        }
        let mut out = Vec::new();
        while let Some(r) = set.join_next().await {
            if let Ok(m) = r {
                out.push(m);
            }
        }
        out
    }
}

fn metric_columns() -> Vec<Column> {
    vec![
        Column::text("Security", 16),
        Column::text("Name", 24),
        Column::num("Mkt Cap", Format::Large { decimals: 2 }, 9),
        Column::num("P/E", Format::Number { decimals: 1 }, 7),
        Column::num("P/S", Format::Number { decimals: 1 }, 6),
        Column::num("Net Mgn %", Format::Number { decimals: 1 }, 9),
        Column::num("Rev Gr %", Format::ChangePercent { decimals: 1 }, 9),
        Column::num("ROE %", Format::Number { decimals: 1 }, 7),
        Column::num("Div Yld %", Format::Number { decimals: 2 }, 9),
        Column::num("1Y Ret %", Format::ChangePercent { decimals: 1 }, 9),
    ]
}

fn metric_row(m: &Metrics) -> Row {
    let ks = m.key.as_ref().map(ToString::to_string).unwrap_or_default();
    Row::new(vec![
        Cell::text(&ks).styled(Style::Link),
        Cell::text(&m.name),
        Cell::num(m.market_cap),
        Cell::num(m.pe),
        Cell::num(m.ps),
        Cell::num(m.net_margin),
        Cell::signed(m.rev_growth),
        Cell::num(m.roe),
        Cell::num(m.div_yield),
        Cell::signed(m.ret_1y),
    ])
    .security(&ks)
    .action(Action::new("DES", Some(&ks)))
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    v.retain(|x| x.is_finite());
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(stats::percentile(&v, 0.5))
}

// --- EQS ------------------------------------------------------------------

pub(crate) async fn eqs(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    let num = |id: &str| req.arg(id).and_then(|x| x.trim().parse::<f64>().ok());
    let sector = req.arg("sector").unwrap_or("All").to_owned();
    let universe = engine.screening_universe();
    let all = engine.metrics_for(universe).await;
    let mut sectors: Vec<String> = all.iter().map(|m| m.sector.clone()).filter(|s| !s.is_empty()).collect();
    sectors.sort();
    sectors.dedup();

    let mut s = Screen::new("EQS", "Equity Screening", None);
    if engine.mode() == DataMode::Live {
        s.push(Block::Notice {
            level: NoticeLevel::Info,
            text: "LIVE universe = your watchlists (no market-wide fundamentals feed is configured).".into(),
        });
    }
    let input = |id: &str, label: &str| Input {
        id: id.into(),
        label: label.into(),
        value: req.arg(id).unwrap_or("").into(),
        kind: InputKind::Number,
        options: vec![],
    };
    s.push(Block::Inputs {
        title: Some("Criteria".into()),
        inputs: vec![
            Input {
                id: "sector".into(),
                label: "Sector".into(),
                value: sector.clone(),
                kind: InputKind::Choice,
                options: std::iter::once("All".to_string()).chain(sectors).collect(),
            },
            input("min_cap_b", "Min Mkt Cap ($B)"),
            input("max_pe", "Max P/E"),
            input("min_growth", "Min Rev Growth %"),
            input("min_margin", "Min Net Margin %"),
            input("min_yield", "Min Div Yield %"),
            input("min_ret", "Min 1Y Return %"),
        ],
    });
    let pass = |m: &Metrics| -> bool {
        let ge = |v: Option<f64>, min: Option<f64>| min.is_none_or(|t| v.is_some_and(|x| x >= t));
        (sector == "All" || m.sector == sector)
            && ge(m.market_cap.map(|c| c / 1e9), num("min_cap_b"))
            && num("max_pe").is_none_or(|t| m.pe.is_some_and(|x| x <= t))
            && ge(m.rev_growth, num("min_growth"))
            && ge(m.net_margin, num("min_margin"))
            && ge(m.div_yield, num("min_yield"))
            && ge(m.ret_1y, num("min_ret"))
    };
    let mut hits: Vec<&Metrics> = all.iter().filter(|m| pass(m)).collect();
    hits.sort_by(|a, b| b.market_cap.unwrap_or(0.0).total_cmp(&a.market_cap.unwrap_or(0.0)));
    s.push(Block::Fields {
        title: None,
        columns: 2,
        fields: vec![
            Field::num("Universe", Some(all.len() as f64), Format::Integer),
            Field::num("Matches", Some(hits.len() as f64), Format::Integer).styled(Style::Emphasis),
        ],
    });
    s.push(Block::Table(Table { title: None, columns: metric_columns(), rows: hits.iter().map(|m| metric_row(m)).collect(), page_size: Some(25), numbered: true }));
    s
}

// --- RV -------------------------------------------------------------------

pub(crate) async fn rv(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    const T: &str = "Relative Valuation";
    let key = match require_security("RV", T, &req) {
        Ok(k) => k,
        Err(s) => return *s,
    };
    let target = engine.company_metrics(&key).await;
    let universe = engine.screening_universe();
    let peers: Vec<SecurityKey> = {
        let infos: Vec<_> = engine.universe().into_iter().filter(|i| universe.contains(&i.key) && i.key != key).collect();
        let mut same_ind: Vec<SecurityKey> = infos.iter().filter(|i| !target.industry.is_empty() && i.industry.as_deref() == Some(&target.industry)).map(|i| i.key.clone()).collect();
        let same_sec: Vec<SecurityKey> = infos.iter().filter(|i| !target.sector.is_empty() && i.sector.as_deref() == Some(&target.sector)).map(|i| i.key.clone()).collect();
        for k in same_sec {
            if !same_ind.contains(&k) {
                same_ind.push(k);
            }
        }
        same_ind.truncate(10);
        same_ind
    };
    let mut s = Screen::new("RV", format!("{key} — {T}"), Some(key.to_string()));
    if peers.is_empty() {
        s.push(Block::Notice { level: NoticeLevel::Info, text: "No peers found in the screening universe".into() });
    }
    let mut ms = engine.metrics_for(peers).await;
    ms.sort_by(|a, b| b.market_cap.unwrap_or(0.0).total_cmp(&a.market_cap.unwrap_or(0.0)));
    let mut rows = vec![metric_row(&target).emphasis()];
    rows.extend(ms.iter().map(metric_row));
    let mut all = vec![target.clone()];
    all.extend(ms.iter().cloned());
    let med = |f: fn(&Metrics) -> Option<f64>| median(all.iter().filter_map(f).collect());
    rows.push(
        Row::new(vec![
            Cell::text("Median").styled(Style::Emphasis),
            Cell::text(format!("{} companies", all.len())),
            Cell::num(med(|m| m.market_cap)),
            Cell::num(med(|m| m.pe)),
            Cell::num(med(|m| m.ps)),
            Cell::num(med(|m| m.net_margin)),
            Cell::signed(med(|m| m.rev_growth)),
            Cell::num(med(|m| m.roe)),
            Cell::num(med(|m| m.div_yield)),
            Cell::signed(med(|m| m.ret_1y)),
        ])
        .emphasis(),
    );
    s.push(Block::Fields {
        title: None,
        columns: 3,
        fields: vec![
            Field::text("Sector", &target.sector),
            Field::text("Industry", &target.industry),
            Field::num("P/E vs Median", target.pe.zip(med(|m| m.pe)).map(|(a, b)| (a / b - 1.0) * 100.0), Format::ChangePercent { decimals: 1 }),
        ],
    });
    s.push(Block::Table(Table { title: Some("Peers".into()), columns: metric_columns(), rows, page_size: None, numbered: false }));
    s.menu_item("Correlation", Action::new("CORR", Some(&key.to_string())), false);
    s
}

// --- CORR -----------------------------------------------------------------

pub(crate) async fn corr(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    let range = req.arg("range").unwrap_or("1Y").to_owned();
    let list: Vec<SecurityKey> = if let Some(txt) = req.arg("securities").filter(|s| !s.trim().is_empty()) {
        txt.split(',').filter_map(|x| x.trim().parse().ok()).collect()
    } else {
        let mut v: Vec<SecurityKey> = req.security.iter().cloned().collect();
        v.extend(DEFAULT_WATCHLIST.iter().filter_map(|s| s.parse::<SecurityKey>().ok()).filter(|k| k.sector == MarketSector::Equity).take(8));
        v.dedup();
        v
    };
    let mut s = Screen::new("CORR", "Correlation Matrix", req.security.as_ref().map(ToString::to_string));
    s.push(Block::Inputs {
        title: None,
        inputs: vec![
            Input {
                id: "securities".into(),
                label: "Securities".into(),
                value: list.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "),
                kind: InputKind::Text,
                options: vec![],
            },
            Input { id: "range".into(), label: "Period".into(), value: range.clone(), kind: InputKind::Choice, options: vec!["3M".into(), "6M".into(), "1Y".into(), "3Y".into(), "5Y".into()] },
        ],
    });
    let (from, to) = parse_range(&range, engine.now());
    let mut series: Vec<(String, HashMap<chrono::NaiveDate, f64>)> = Vec::new();
    let mut missing = Vec::new();
    for k in &list {
        match engine.bars(k, BarInterval::Day, from, to).await {
            Ok(b) => {
                if series.is_empty() {
                    s.source(&b.value.provenance);
                }
                let closes: HashMap<_, _> = b.value.ts.iter().zip(&b.value.close).map(|(t, c)| (nanos_to_date(*t), *c)).collect();
                series.push((k.symbol.clone(), closes));
            }
            Err(_) => missing.push(k.to_string()),
        }
    }
    if !missing.is_empty() {
        s.push(Block::Notice { level: NoticeLevel::Warning, text: format!("No history for: {}", missing.join(", ")) });
    }
    if series.len() < 2 {
        s.status = crate::screen::ScreenStatus::NotAvailable { reason: "need history for at least two securities".into() };
        return s;
    }
    // Align on common dates (crypto trades weekends; equities don't).
    let mut dates: Vec<chrono::NaiveDate> = series[0].1.keys().copied().filter(|d| series.iter().all(|(_, m)| m.contains_key(d))).collect();
    dates.sort();
    let rets: Vec<Vec<f64>> = series.iter().map(|(_, m)| returns::log_returns(&dates.iter().map(|d| m[d]).collect::<Vec<_>>())).collect();
    let mat = stats::correlation_matrix(&rets);
    let labels: Vec<String> = series.iter().map(|(n, _)| n.clone()).collect();
    let n = labels.len();
    s.push(Block::Heat(HeatMap {
        title: format!("Daily log-return correlation, {} common days", dates.len().saturating_sub(1)),
        row_labels: labels.clone(),
        col_labels: labels.clone(),
        values: mat.iter().flatten().copied().collect(),
        format: Format::Number { decimals: 2 },
        diverging: true,
    }));
    let vols: Vec<f64> = rets.iter().map(|r| returns::annualized_volatility(r, 252.0) * 100.0).collect();
    let mut columns = vec![Column::text("", 8)];
    columns.extend(labels.iter().map(|l| Column::num(l, Format::Number { decimals: 2 }, 7)));
    columns.push(Column::num("Ann Vol %", Format::Number { decimals: 1 }, 9));
    let rows = (0..n)
        .map(|i| {
            let mut cells = vec![Cell::text(&labels[i]).styled(Style::Emphasis)];
            cells.extend((0..n).map(|j| Cell::num(Some(mat[i][j])).styled(if i == j { Style::Muted } else { Style::Normal })));
            cells.push(Cell::num(Some(vols[i])));
            Row::new(cells)
        })
        .collect();
    s.push(Block::Table(Table { title: None, columns, rows, page_size: None, numbered: false }));
    s
}

// --- PORT -----------------------------------------------------------------

/// Most transactions PORT lists (the table crosses the FFI as records).
const MAX_TX_ROWS: usize = 500;

/// Names a few items and counts the rest: `AAPL, MSFT and 3 more`.
fn some_of(items: &[String], n: usize) -> String {
    if items.len() <= n {
        items.join(", ")
    } else {
        format!("{} and {} more", items[..n].join(", "), items.len() - n)
    }
}

pub(crate) async fn port(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    const T: &str = "Portfolio & Risk";
    let store = &engine.stores().app;
    let mut s = Screen::new("PORT", T, None);
    let mut portfolios = store.portfolios().unwrap_or_default();
    if portfolios.is_empty()
        && let Ok(id) = store.create_portfolio("Main", "USD")
    {
        portfolios.push(meridian_store::Portfolio { id, name: "Main".into(), base_currency: "USD".into() });
    }
    let pid = req.arg("portfolio").and_then(|x| x.parse::<i64>().ok()).or_else(|| portfolios.first().map(|p| p.id)).unwrap_or(0);
    let base = portfolios.iter().find(|p| p.id == pid).map_or_else(|| "USD".to_owned(), |p| p.base_currency.clone());
    // Add/delete transactions from inputs.
    if req.arg("add").is_some() {
        let sec = req.arg("security").unwrap_or("").trim().to_owned();
        let qty = req.arg("qty").and_then(|x| x.trim().parse::<f64>().ok());
        let px = req.arg("price").and_then(|x| x.trim().parse::<f64>().ok());
        let date = req.arg("date").and_then(super::parse_date).unwrap_or_else(|| nanos_to_date(engine.now()));
        match (sec.parse::<SecurityKey>(), qty, px) {
            (Ok(k), Some(q), Some(p)) if q != 0.0 && p > 0.0 => {
                let fees = req.arg("fees").and_then(|x| x.trim().parse().ok()).unwrap_or(0.0);
                let _ = store.add_transaction(&Transaction::trade(pid, &k.to_string(), &date.format("%Y-%m-%d").to_string(), q, p, fees));
            }
            _ => s.push(Block::Notice { level: NoticeLevel::Error, text: "Enter a security key, a non-zero quantity, and a price".into() }),
        }
    }
    if let Some(id) = req.arg("delete_tx").and_then(|x| x.parse::<i64>().ok()) {
        let _ = store.delete_transaction(id);
    }
    for p in &portfolios {
        s.menu_item(&p.name, Action::new("PORT", None).arg("portfolio", p.id.to_string()), p.id == pid);
    }
    s.push(Block::Inputs {
        title: Some("Add Transaction (negative quantity = sell)".into()),
        inputs: vec![
            Input { id: "security".into(), label: "Security".into(), value: String::new(), kind: InputKind::Text, options: vec![] },
            Input { id: "date".into(), label: "Trade Date".into(), value: nanos_to_date(engine.now()).format("%m/%d/%Y").to_string(), kind: InputKind::Date, options: vec![] },
            Input { id: "qty".into(), label: "Quantity".into(), value: String::new(), kind: InputKind::Number, options: vec![] },
            Input { id: "price".into(), label: "Price".into(), value: String::new(), kind: InputKind::Number, options: vec![] },
            Input { id: "fees".into(), label: "Fees".into(), value: "0".into(), kind: InputKind::Number, options: vec![] },
        ],
    });
    s.menu_item("Add Transaction", Action::new("PORT", None).arg("portfolio", pid.to_string()).arg("add", "1"), false);
    // The app intercepts IMPORT and opens a file picker for this portfolio.
    s.menu_item("Import from broker…", Action::new("IMPORT", None).arg("portfolio", pid.to_string()), false);

    let txs = store.transactions(pid).unwrap_or_default();
    if txs.is_empty() {
        s.push(Block::Notice {
            level: NoticeLevel::Info,
            text: "This portfolio is empty. Choose Import from broker… to load a CSV export from Robinhood, Fidelity, Charles Schwab or Vanguard, or a positions file; or add a transaction above.".into(),
        });
        return s;
    }
    let ledger = crate::portfolio::ledger(&txs, &base);
    let open: Vec<&Holding> = ledger.open().collect();
    let mut prices = HashMap::new();
    let mut sectors = HashMap::new();
    for h in &open {
        let Ok(k) = h.security.parse::<SecurityKey>() else { continue };
        if let Some(r) = engine.quote_row(&k).await.filter(|r| r.last.is_finite()) {
            prices.insert(h.security.clone(), r.last);
        }
        let sec = engine.instrument(&k).await.ok().and_then(|i| i.sector.or(Some(i.asset_class.label().to_string()))).unwrap_or_else(|| "Other".into());
        sectors.insert(h.security.clone(), sec);
        if let Some(p) = engine.quote_provenance(&k) {
            s.source(&p);
        }
    }
    let mv_of = |h: &Holding| prices.get(&h.security).map(|px| px * h.quantity);
    let total_mv: f64 = open.iter().filter_map(|h| mv_of(h)).sum();
    let total_cost: f64 = open.iter().filter(|h| h.cost_known).map(|h| h.cost).sum();
    let unrealized: f64 = open.iter().filter(|h| h.cost_known).filter_map(|h| mv_of(h).map(|mv| mv - h.cost)).sum();
    let realized = ledger.realized();
    let total_return = unrealized + realized + ledger.dividends + ledger.interest - ledger.fees;
    s.push(Block::Fields {
        title: None,
        columns: 3,
        fields: vec![
            Field::num("Market value", Some(total_mv), Format::Number { decimals: 2 }).styled(Style::Emphasis),
            Field::num("Cost basis", Some(total_cost), Format::Number { decimals: 2 }),
            Field::num("Unrealized P&L", Some(unrealized), Format::Change { decimals: 2 }),
            Field::num("Realized P&L", Some(realized), Format::Change { decimals: 2 }),
            Field::num("Dividends", Some(ledger.dividends), Format::Number { decimals: 2 }),
            Field::num("Interest", Some(ledger.interest), Format::Number { decimals: 2 }),
            Field::num("Fees and taxes", Some(ledger.fees), Format::Number { decimals: 2 }),
            Field::num("Total return", Some(total_return), Format::Change { decimals: 2 }).styled(Style::Emphasis),
            Field::num("Net deposits", Some(ledger.deposits - ledger.withdrawals), Format::Number { decimals: 2 }),
        ],
    });
    port_notices(&mut s, &ledger, &open, &prices);
    if !open.is_empty() {
        let rows: Vec<Row> = open
            .iter()
            .map(|h| {
                let k = &h.security;
                let px = prices.get(k).copied();
                let mv = px.map(|x| x * h.quantity);
                let pnl = mv.filter(|_| h.cost_known).map(|m| m - h.cost);
                Row::new(vec![
                    Cell::text(k).styled(Style::Link),
                    Cell::num(Some(h.quantity)),
                    Cell::num(h.average_cost()),
                    Cell::num(px),
                    Cell::num(mv),
                    Cell::signed(pnl),
                    Cell::signed(pnl.filter(|_| h.cost.abs() > 0.0).map(|x| x / h.cost.abs() * 100.0)),
                    Cell::num(Some(h.income)),
                    Cell::num(mv.filter(|_| total_mv > 0.0).map(|m| m / total_mv * 100.0)),
                    Cell::text(sectors.get(k).cloned().unwrap_or_default()),
                ])
                .security(k)
                .action(Action::new("DES", Some(k)))
            })
            .collect();
        s.push(Block::Table(Table {
            title: Some("Positions".into()),
            columns: vec![
                Column::text("Security", 18),
                Column::num("Qty", Format::Number { decimals: 2 }, 9),
                Column::num("Avg Cost", Format::Number { decimals: 2 }, 9),
                Column::num("Last", Format::Number { decimals: 2 }, 9).live(crate::screen::LiveField::Last),
                Column::num("Mkt Val", Format::Number { decimals: 0 }, 11),
                Column::num("P&L", Format::Change { decimals: 0 }, 10),
                Column::num("P&L %", Format::ChangePercent { decimals: 2 }, 8),
                Column::num("Income", Format::Number { decimals: 2 }, 9),
                Column::num("Weight %", Format::Number { decimals: 1 }, 8),
                Column::text("Sector", 18),
            ],
            rows,
            page_size: Some(20),
            numbered: true,
        }));
    }
    if total_mv > 0.0 {
        port_exposure_and_risk(&engine, &mut s, &open, &prices, &sectors, total_mv).await;
    }
    port_transactions(&mut s, &txs, pid);
    s
}

/// Warnings about what the totals leave out.
fn port_notices(s: &mut Screen, ledger: &Ledger, open: &[&Holding], prices: &HashMap<String, f64>) {
    let warn = |s: &mut Screen, text: String| s.push(Block::Notice { level: NoticeLevel::Warning, text });
    let unpriced: Vec<String> = open.iter().filter(|h| !prices.contains_key(&h.security)).map(|h| h.security.clone()).collect();
    if !unpriced.is_empty() {
        warn(s, format!("No current price for {}: market value and unrealized P&L leave them out.", some_of(&unpriced, 4)));
    }
    let no_cost: Vec<String> = open.iter().filter(|h| !h.cost_known).map(|h| h.security.clone()).collect();
    if !no_cost.is_empty() {
        warn(s, format!("Cost basis unknown for {} (shares arrived without one): cost and P&L leave them out.", some_of(&no_cost, 4)));
    }
    let issues: Vec<String> = ledger.holdings.iter().flat_map(|h| h.issues.iter().map(move |i| format!("{} {i}", h.security))).collect();
    if !issues.is_empty() {
        warn(s, format!("History gaps (imports cover only the exported date range): {}.", some_of(&issues, 3)));
    }
    if ledger.other > 0 {
        s.push(Block::Notice { level: NoticeLevel::Info, text: format!("{} transactions of kind Other are listed below but not counted.", ledger.other) });
    }
    for (ccy, n) in &ledger.other_currency {
        warn(s, format!("{n} transactions in {ccy} are listed below but not counted (no currency conversion)."));
    }
}

async fn port_exposure_and_risk(engine: &Arc<Engine>, s: &mut Screen, open: &[&Holding], prices: &HashMap<String, f64>, sectors: &HashMap<String, String>, total_mv: f64) {
    // Sector exposure.
    let mut exposure: HashMap<String, f64> = HashMap::new();
    for h in open {
        if let Some(px) = prices.get(&h.security) {
            *exposure.entry(sectors.get(&h.security).cloned().unwrap_or_default()).or_default() += px * h.quantity;
        }
    }
    let mut ex: Vec<_> = exposure.into_iter().collect();
    ex.sort_by(|a, b| b.1.total_cmp(&a.1));
    s.push(Block::Xy(XyChart {
        title: "Exposure by Sector (%)".into(),
        x_label: ex.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(" | "),
        y_label: "%".into(),
        series: vec![XySeries {
            name: "Weight".into(),
            x: (0..ex.len()).map(|i| i as f64).collect(),
            y: ex.iter().map(|(_, v)| v / total_mv * 100.0).collect(),
            style: Style::Emphasis,
            bars: true,
        }],
        x_marker: None,
        height_rows: 6,
        x_categories: None,
    }));

    // Risk from 1Y of daily history on current weights.
    let now = engine.now();
    let from = Some(now - 366 * NANOS_PER_DAY);
    let to = now + NANOS_PER_DAY;
    let mut closes: Vec<(f64, HashMap<chrono::NaiveDate, f64>)> = Vec::new();
    for h in open {
        let (Ok(key), Some(px)) = (h.security.parse::<SecurityKey>(), prices.get(&h.security)) else { continue };
        if let Ok(b) = engine.bars(&key, BarInterval::Day, from, to).await {
            let m: HashMap<_, _> = b.value.ts.iter().zip(&b.value.close).map(|(t, c)| (nanos_to_date(*t), *c)).collect();
            closes.push((px * h.quantity / total_mv, m));
        }
    }
    let factor_keys = [("Market (SPY)", "SPY US Equity"), ("Size (IWM)", "IWM US Equity"), ("Rates (TLT)", "TLT US Equity"), ("Gold (GLD)", "GLD US Equity")];
    let mut factors: Vec<(&str, HashMap<chrono::NaiveDate, f64>)> = Vec::new();
    for (name, k) in factor_keys {
        if let Ok(key) = k.parse::<SecurityKey>()
            && let Ok(b) = engine.bars(&key, BarInterval::Day, from, to).await
        {
            factors.push((name, b.value.ts.iter().zip(&b.value.close).map(|(t, c)| (nanos_to_date(*t), *c)).collect()));
        }
    }
    if closes.is_empty() {
        s.push(Block::Notice { level: NoticeLevel::Warning, text: "No price history for holdings; risk metrics unavailable".into() });
        return;
    }
    let mut dates: Vec<chrono::NaiveDate> = closes[0].1.keys().copied().filter(|d| closes.iter().all(|(_, m)| m.contains_key(d))).collect();
    dates.sort();
    if dates.len() < 30 {
        s.push(Block::Notice { level: NoticeLevel::Warning, text: "Too little common history for risk metrics".into() });
        return;
    }
    let mut port_ret = vec![0.0; dates.len() - 1];
    for (w, m) in &closes {
        let r = returns::simple_returns(&dates.iter().map(|d| m[d]).collect::<Vec<_>>());
        for (i, x) in r.iter().enumerate() {
            port_ret[i] += w * x;
        }
    }
    let nav: Vec<f64> = std::iter::once(1.0)
        .chain(port_ret.iter().scan(1.0, |acc, r| {
            *acc *= 1.0 + r;
            Some(*acc)
        }))
        .collect();
    let vol = returns::annualized_volatility(&port_ret, 252.0);
    let var95 = risk::var_historical(&port_ret, 0.95);
    let var99 = risk::var_historical(&port_ret, 0.99);
    let cvar95 = risk::cvar_historical(&port_ret, 0.95);
    let pvar95 = risk::var_parametric(stats::mean(&port_ret), stats::std_dev(&port_ret), 0.95);
    let dd = returns::max_drawdown(&nav);
    let mut risk_fields = vec![
        Field::num("Ann. Volatility %", Some(vol * 100.0), Format::Number { decimals: 2 }),
        Field::num("1D VaR 95% (hist)", Some(var95 * total_mv), Format::Number { decimals: 0 }),
        Field::num("1D VaR 99% (hist)", Some(var99 * total_mv), Format::Number { decimals: 0 }),
        Field::num("1D CVaR 95%", Some(cvar95 * total_mv), Format::Number { decimals: 0 }),
        Field::num("1D VaR 95% (param)", Some(pvar95 * total_mv), Format::Number { decimals: 0 }),
        Field::num("Sharpe (rf=0)", Some(returns::sharpe(&port_ret, 0.0, 252.0)), Format::Number { decimals: 2 }),
        Field::num("Max Drawdown %", Some(dd.max_drawdown * 100.0), Format::Number { decimals: 2 }),
        Field::num("Days of History", Some(port_ret.len() as f64), Format::Integer),
    ];
    // Factor betas on ETF proxies (multivariate OLS on aligned dates).
    let fdates: Vec<_> = dates.iter().copied().filter(|d| factors.iter().all(|(_, m)| m.contains_key(d))).collect();
    if factors.len() == factor_keys.len() && fdates.len() > 30 {
        let pr = {
            let mut v = vec![0.0; fdates.len() - 1];
            for (w, m) in &closes {
                let r = returns::simple_returns(&fdates.iter().map(|d| m[d]).collect::<Vec<_>>());
                for (i, x) in r.iter().enumerate() {
                    v[i] += w * x;
                }
            }
            v
        };
        let xs: Vec<Vec<f64>> = factors.iter().map(|(_, m)| returns::simple_returns(&fdates.iter().map(|d| m[d]).collect::<Vec<_>>())).collect();
        risk_fields.push(Field::num("Beta vs SPY", Some(stats::beta(&pr, &xs[0])), Format::Number { decimals: 2 }));
        if let Ok(o) = stats::ols(&pr, &xs) {
            for (i, (name, _)) in factors.iter().enumerate() {
                risk_fields.push(Field::num(&format!("β {name}"), o.coefficients.get(i + 1).copied(), Format::Number { decimals: 2 }));
            }
            risk_fields.push(Field::num("Factor R²", Some(o.r_squared), Format::Number { decimals: 2 }));
        }
    }
    s.push(Block::Fields { title: Some("Risk (1Y daily, current weights; factors are ETF proxies)".into()), columns: 3, fields: risk_fields });
    s.push(Block::Xy(XyChart {
        title: "Backcast NAV (current weights)".into(),
        x_label: "Date".into(),
        y_label: "NAV".into(),
        series: vec![XySeries {
            name: "Portfolio".into(),
            x: dates.iter().map(|d| meridian_types::date_to_nanos(*d) as f64).collect(),
            y: nav,
            style: Style::Emphasis,
            bars: false,
        }],
        x_marker: None,
        height_rows: 8,
        x_categories: None,
    }));
}

/// IMPORT: what can be imported and from where. The app intercepts this
/// function to open a file picker; the screen is the explanation shown
/// beside it (and when the function is opened directly).
pub(crate) async fn import(_engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    let mut s = Screen::new("IMPORT", "Import from broker", None);
    let back = req.arg("portfolio").map_or_else(|| Action::new("PORT", None), |p| Action::new("PORT", None).arg("portfolio", p));
    s.menu_item("Back to portfolio", back, false);
    s.push(Block::Text {
        title: None,
        body: "Choose a CSV file exported from your broker. Meridian reads it on this Mac and shows a preview before saving anything; rows already imported are skipped. Options, short sales, mergers, bonds and workplace 401(k) rows are listed as not imported so you can add them by hand.".into(),
    });
    let rows = [
        ("Robinhood", "Account → Reports and statements → Account activity report (CSV)"),
        ("Fidelity", "Activity & Orders → Download as CSV (History_for_Account or Accounts_History)"),
        ("Charles Schwab", "History → Transactions → Export → CSV"),
        ("Vanguard", "Transaction history → Download → spreadsheet-compatible CSV (OfxDownload.csv)"),
        ("Positions file", "A holdings download with Symbol and Quantity columns (Fidelity, Schwab, E*TRADE and others); recorded on the import day"),
        ("Any other CSV", "Map its columns to date, symbol, type, quantity, price and amount"),
    ]
    .into_iter()
    .map(|(what, how)| Row::new(vec![Cell::text(what).styled(Style::Emphasis), Cell::text(how)]))
    .collect();
    s.push(Block::Table(Table {
        title: Some("Supported files".into()),
        columns: vec![Column::text("Source", 16), Column::text("Where to get it", 72)],
        rows,
        page_size: None,
        numbered: false,
    }));
    s
}

/// The transactions table, newest first, capped at [`MAX_TX_ROWS`].
fn port_transactions(s: &mut Screen, txs: &[Transaction], pid: i64) {
    let trows = txs
        .iter()
        .rev()
        .take(MAX_TX_ROWS)
        .map(|t| {
            let qty = (t.quantity != 0.0).then_some(t.quantity);
            Row::new(vec![
                Cell::text(&t.trade_date),
                Cell::text(t.kind.label()),
                Cell::text(t.security.clone().unwrap_or_default()),
                Cell::signed(qty),
                Cell::num(t.price),
                Cell::signed(crate::portfolio::cash_flow(t)),
                Cell::num((t.fees != 0.0).then_some(t.fees)),
                Cell::text(t.note.clone().unwrap_or_default()).styled(Style::Muted),
            ])
            .action(Action::new("PORT", None).arg("portfolio", pid.to_string()).arg("delete_tx", t.id.to_string()))
        })
        .collect();
    let title = if txs.len() > MAX_TX_ROWS {
        format!("Transactions (latest {MAX_TX_ROWS} of {}; select to delete)", txs.len())
    } else {
        "Transactions (select to delete)".to_owned()
    };
    s.push(Block::Table(Table {
        title: Some(title),
        columns: vec![
            Column::text("Date", 11),
            Column::text("Kind", 12),
            Column::text("Security", 18),
            Column::num("Qty", Format::Change { decimals: 2 }, 9),
            Column::num("Price", Format::Number { decimals: 2 }, 9),
            Column::num("Amount", Format::Change { decimals: 2 }, 11),
            Column::num("Fees", Format::Number { decimals: 2 }, 7),
            Column::text("Description", 28),
        ],
        rows: trows,
        page_size: Some(10),
        numbered: false,
    }));
}

// --- BTST -----------------------------------------------------------------

const STRATS: &[&str] = &["SMA Cross", "RSI Reversion", "Breakout", "MACD Cross", "Buy & Hold"];

pub(crate) async fn btst(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    const T: &str = "Backtest";
    let key = match require_security("BTST", T, &req) {
        Ok(k) => k,
        Err(s) => return *s,
    };
    let ks = key.to_string();
    let n = |id: &str, d: f64| req.arg(id).and_then(|x| x.trim().parse::<f64>().ok()).unwrap_or(d);
    let strat_name = req.arg("strategy").filter(|x| STRATS.contains(x)).unwrap_or("SMA Cross").to_owned();
    let p1 = n("p1", match strat_name.as_str() {
        "RSI Reversion" => 14.0,
        "Breakout" => 55.0,
        "MACD Cross" => 12.0,
        _ => 50.0,
    });
    let p2 = n("p2", match strat_name.as_str() {
        "RSI Reversion" => 30.0,
        "MACD Cross" => 26.0,
        _ => 200.0,
    });
    let p3 = n("p3", match strat_name.as_str() {
        "RSI Reversion" => 70.0,
        "MACD Cross" => 9.0,
        _ => 0.0,
    });
    let strategy = match strat_name.as_str() {
        "RSI Reversion" => Strategy::RsiReversion { period: p1 as usize, lower: p2, upper: p3 },
        "Breakout" => Strategy::Breakout { lookback: p1 as usize },
        "MACD Cross" => Strategy::MacdCross { fast: p1 as usize, slow: p2 as usize, signal: p3 as usize },
        "Buy & Hold" => Strategy::BuyAndHold,
        _ => Strategy::SmaCross { fast: p1 as usize, slow: p2 as usize },
    };
    let range = req.arg("range").unwrap_or("5Y").to_owned();
    let cfg = BacktestConfig {
        initial_capital: n("capital", 100_000.0),
        commission_bps: n("commission", 1.0),
        slippage_bps: n("slippage", 2.0),
        allow_short: req.arg("short") == Some("Yes"),
        periods_per_year: 252.0,
    };
    let mut s = Screen::new("BTST", format!("{ks} — {T}"), Some(ks.clone()));
    s.push(Block::Inputs {
        title: Some("Strategy".into()),
        inputs: vec![
            Input { id: "strategy".into(), label: "Strategy".into(), value: strat_name.clone(), kind: InputKind::Choice, options: STRATS.iter().map(|x| (*x).into()).collect() },
            Input { id: "p1".into(), label: "Param 1".into(), value: format!("{p1}"), kind: InputKind::Number, options: vec![] },
            Input { id: "p2".into(), label: "Param 2".into(), value: format!("{p2}"), kind: InputKind::Number, options: vec![] },
            Input { id: "p3".into(), label: "Param 3".into(), value: format!("{p3}"), kind: InputKind::Number, options: vec![] },
            Input { id: "range".into(), label: "Period".into(), value: range.clone(), kind: InputKind::Choice, options: vec!["1Y".into(), "3Y".into(), "5Y".into(), "10Y".into(), "20Y".into()] },
            Input { id: "commission".into(), label: "Commission bp".into(), value: format!("{}", cfg.commission_bps), kind: InputKind::Number, options: vec![] },
            Input { id: "slippage".into(), label: "Slippage bp".into(), value: format!("{}", cfg.slippage_bps), kind: InputKind::Number, options: vec![] },
            Input { id: "short".into(), label: "Allow Short".into(), value: if cfg.allow_short { "Yes" } else { "No" }.into(), kind: InputKind::Choice, options: vec!["No".into(), "Yes".into()] },
        ],
    });
    s.push(Block::Notice {
        level: NoticeLevel::Info,
        text: "Params — SMA Cross: fast, slow · RSI: period, lower, upper · Breakout: lookback · MACD: fast, slow, signal. Signals on close fill at the next close.".into(),
    });
    let (from, to) = parse_range(&range, engine.now());
    let bars = match engine.bars(&key, BarInterval::Day, from, to).await {
        Ok(b) => b,
        Err(e) => return super::error_screen("BTST", T, Some(&key), &e),
    };
    s.source(&bars.value.provenance);
    let res = match run_backtest(&bars.value.ts, &bars.value.close, &strategy, &cfg) {
        Ok(r) => r,
        Err(e) => {
            s.push(Block::Notice { level: NoticeLevel::Error, text: e.to_string() });
            return s;
        }
    };
    let wins = res.trades.iter().filter(|t| !t.is_open && t.pnl > 0.0).count();
    s.push(Block::Fields {
        title: Some("Results".into()),
        columns: 3,
        fields: vec![
            Field::num("Total Return %", Some(res.total_return * 100.0), Format::ChangePercent { decimals: 2 }).styled(Style::Emphasis),
            Field::num("Buy & Hold %", Some(res.benchmark_return * 100.0), Format::ChangePercent { decimals: 2 }),
            Field::num("CAGR %", Some(res.cagr * 100.0), Format::ChangePercent { decimals: 2 }),
            Field::num("Ann. Vol %", Some(res.annualized_vol * 100.0), Format::Number { decimals: 2 }),
            Field::num("Sharpe", Some(res.sharpe), Format::Number { decimals: 2 }),
            Field::num("Max Drawdown %", Some(res.max_drawdown * 100.0), Format::Number { decimals: 2 }),
            Field::num("Trades", Some(res.num_trades as f64), Format::Integer),
            Field::num("Win Rate %", Some(res.win_rate * 100.0), Format::Number { decimals: 1 }),
            Field::num("Exposure %", Some(res.exposure_pct * 100.0), Format::Number { decimals: 1 }),
        ],
    });
    let first = bars.value.close.first().copied().unwrap_or(1.0);
    s.push(Block::Xy(XyChart {
        title: "Equity Curve vs Buy & Hold (growth of $1)".into(),
        x_label: "Date".into(),
        y_label: "x".into(),
        series: vec![
            XySeries {
                name: "Strategy".into(),
                x: bars.value.ts.iter().map(|t| *t as f64).collect(),
                y: res.equity_curve.iter().map(|e| e / cfg.initial_capital).collect(),
                style: Style::Emphasis,
                bars: false,
            },
            XySeries {
                name: "Buy & Hold".into(),
                x: bars.value.ts.iter().map(|t| *t as f64).collect(),
                y: bars.value.close.iter().map(|c| c / first).collect(),
                style: Style::Muted,
                bars: false,
            },
        ],
        x_marker: None,
        height_rows: 12,
        x_categories: None,
    }));
    let rows = res
        .trades
        .iter()
        .rev()
        .take(200)
        .map(|t| {
            Row::new(vec![
                Cell::text(nanos_to_date(bars.value.ts[t.entry_index]).format("%m/%d/%y").to_string()),
                Cell::text(nanos_to_date(bars.value.ts[t.exit_index.min(bars.value.len() - 1)]).format("%m/%d/%y").to_string()),
                Cell::text(match t.direction {
                    Direction::Long => "Long",
                    Direction::Short => "Short",
                }),
                Cell::num(Some(t.entry_price)),
                Cell::num(Some(t.exit_price)),
                Cell::signed(Some(t.pnl)),
                Cell::signed(Some(t.return_pct * 100.0)),
                Cell::text(if t.is_open { "open" } else { "" }).styled(Style::Muted),
            ])
        })
        .collect();
    s.push(Block::Table(Table {
        title: Some(format!("Trades ({wins} closed winners)")),
        columns: vec![
            Column::text("Entry", 9),
            Column::text("Exit", 9),
            Column::text("Side", 6),
            Column::num("Entry Px", Format::Number { decimals: 2 }, 9),
            Column::num("Exit Px", Format::Number { decimals: 2 }, 9),
            Column::num("P&L", Format::Change { decimals: 0 }, 10),
            Column::num("Ret %", Format::ChangePercent { decimals: 2 }, 8),
            Column::text("", 5),
        ],
        rows,
        page_size: Some(15),
        numbered: false,
    }));
    s
}

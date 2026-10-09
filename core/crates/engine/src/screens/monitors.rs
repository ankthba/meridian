//! W, WEI, CRYP, FXC, MOST — monitors. Tables bind live columns to the
//! stream hub; Swift overlays hot-row values onto them each frame.

use std::sync::Arc;

use meridian_provider::Capability;
use meridian_types::{AssetClass, MarketSector, SecurityKey};

use super::ScreenRequest;
use crate::core::Engine;
use crate::screen::{
    Action, Block, Cell, Column, Format, Input, InputKind, LiveField, NoticeLevel, Row, Screen, Style, Table,
};
use crate::universe::{CRYPTO, FX_MATRIX, WORLD_INDICES, index_proxy, usd_pair};

fn live_columns(dec: u8) -> Vec<Column> {
    vec![
        Column::num("Last", Format::Price { decimals: dec }, 11).live(LiveField::Last),
        Column::num("Net Chg", Format::Change { decimals: dec }, 9).live(LiveField::NetChange),
        Column::num("% Chg", Format::ChangePercent { decimals: 2 }, 8).live(LiveField::PctChange),
        Column::num("Bid", Format::Price { decimals: dec }, 10).live(LiveField::Bid),
        Column::num("Ask", Format::Price { decimals: dec }, 10).live(LiveField::Ask),
        Column::num("High", Format::Price { decimals: dec }, 10).live(LiveField::High),
        Column::num("Low", Format::Price { decimals: dec }, 10).live(LiveField::Low),
        Column::num("Volume", Format::Large { decimals: 2 }, 9).live(LiveField::Volume),
        Column::num("Time", Format::Time, 8).live(LiveField::Time),
    ]
}

fn live_cells(n: usize) -> Vec<Cell> {
    (0..n).map(|_| Cell::empty()).collect()
}

fn badge_from_quotes(engine: &Engine, s: &mut Screen, keys: &[SecurityKey]) {
    let mut seen = std::collections::HashSet::new();
    for k in keys {
        if let Some(p) = engine.quote_provenance(k)
            && seen.insert(p.provider.clone())
        {
            s.source(&p);
        }
    }
}

// --- W --------------------------------------------------------------------

pub(crate) async fn worksheet(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    let lists = match engine.stores().app.watchlists() {
        Ok(l) => l,
        Err(e) => return super::error_screen("W", "Worksheet", None, &e.into()),
    };
    let chosen = req.arg("list").and_then(|x| x.parse::<i64>().ok()).or_else(|| lists.first().map(|l| l.id));
    let Some(list) = lists.iter().find(|l| Some(l.id) == chosen) else {
        return Screen::not_available("W", "Worksheet", None, "no watchlists; create one with the Name input");
    };
    // Mutations through inputs: add=<security>, remove=<security>, new=<name>.
    let mut securities = list.securities.clone();
    let mut changed = false;
    if let Some(add) = req.arg("add").filter(|a| !a.trim().is_empty())
        && let Ok(k) = add.parse::<SecurityKey>()
    {
        let ks = k.to_string();
        if !securities.contains(&ks) {
            securities.push(ks);
            changed = true;
        }
    }
    if let Some(rm) = req.arg("remove") {
        let before = securities.len();
        securities.retain(|s| s != rm);
        changed |= securities.len() != before;
    }
    if changed {
        let _ = engine.stores().app.set_watchlist_items(list.id, &securities);
    }
    if let Some(name) = req.arg("new").filter(|n| !n.trim().is_empty())
        && let Ok(id) = engine.stores().app.create_watchlist(name.trim(), engine.now())
    {
        let mut a = req.clone();
        a.args = vec![("list".into(), id.to_string())];
        let mut s = Box::pin(worksheet(engine, a.clone())).await;
        s.args = a.args;
        return s;
    }

    let keys: Vec<SecurityKey> = securities.iter().filter_map(|s| s.parse().ok()).collect();
    let mut s = Screen::new("W", format!("Worksheet — {}", list.name), None);
    for l in &lists {
        s.menu_item(&l.name, Action::new("W", None).arg("list", l.id.to_string()), l.id == list.id);
    }
    s.push(Block::Inputs {
        title: None,
        inputs: vec![
            Input { id: "add".into(), label: "Add Security".into(), value: String::new(), kind: InputKind::Text, options: vec![] },
            Input { id: "new".into(), label: "New List".into(), value: String::new(), kind: InputKind::Text, options: vec![] },
        ],
    });
    badge_from_quotes(&engine, &mut s, &keys);
    for text in engine.setup_notes("Prices", Capability::Quotes, &keys) {
        s.push(Block::Notice { level: NoticeLevel::Warning, text });
    }
    let mut columns = vec![Column::text("Security", 20), Column::text("Name", 24)];
    columns.extend(live_columns(2));
    let mut rows = Vec::new();
    for k in &keys {
        let name = engine.instrument(k).await.map(|i| i.name).unwrap_or_default();
        let ks = k.to_string();
        let mut cells = vec![Cell::text(&ks).styled(Style::Link), Cell::text(name)];
        cells.extend(live_cells(9));
        rows.push(Row::new(cells).security(&ks).action(Action::new("DES", Some(&ks))));
    }
    s.push(Block::Table(Table { title: None, columns, rows, page_size: Some(30), numbered: true }));
    s
}

// --- WEI ------------------------------------------------------------------

pub(crate) async fn wei(engine: Arc<Engine>, _req: ScreenRequest) -> Screen {
    let keys: Vec<SecurityKey> = WORLD_INDICES.iter().map(|(sym, _, _)| SecurityKey::index(sym)).collect();
    let covered: Vec<bool> = keys.iter().map(|k| engine.router().supports(meridian_provider::Capability::Quotes, Some(k))).collect();
    if !covered.iter().any(|c| *c) {
        return wei_proxies(&engine);
    }
    let mut s = Screen::new("WEI", "World Equity Indices", None);
    badge_from_quotes(&engine, &mut s, &keys);
    let mut columns = vec![Column::text("Index", 28)];
    columns.extend(live_columns(2).into_iter().filter(|c| !matches!(c.live, Some(LiveField::Bid | LiveField::Ask | LiveField::Volume))));
    let ncells = columns.len() - 1;
    let mut rows = Vec::new();
    let mut region = "";
    for ((sym, name, reg), ok) in WORLD_INDICES.iter().zip(&covered) {
        if *reg != region {
            region = reg;
            let mut cells = vec![Cell::text(*reg).styled(Style::Emphasis)];
            cells.extend(live_cells(ncells));
            rows.push(Row::new(cells).emphasis());
        }
        let ks = SecurityKey::index(sym).to_string();
        let mut cells = vec![Cell::text(*name)];
        if *ok {
            cells.extend(live_cells(ncells));
            rows.push(Row::new(cells).security(&ks).action(Action::new("GP", Some(&ks))).depth(1));
        } else {
            cells.push(Cell::text("n/a").styled(Style::Muted));
            cells.extend(live_cells(ncells - 1));
            rows.push(Row::new(cells).depth(1));
        }
    }
    s.push(Block::Table(Table { title: None, columns, rows, page_size: None, numbered: false }));
    s
}

/// Shown on WEI when index levels come from US-listed ETF proxies.
pub(crate) const WEI_PROXY_NOTICE: &str =
    "Index levels aren't available from free sources; rows show ETF proxies, whose % change approximates the index's.";

/// WEI when no source provides index levels: each index with a liquid
/// US-listed ETF proxy, quoted through the equity path.
fn wei_proxies(engine: &Engine) -> Screen {
    const T: &str = "World Equity Indices";
    let etf = |sym: &str| index_proxy(sym).map(|(ticker, name)| (SecurityKey::equity(ticker), ticker, name));
    let quoted = |k: &SecurityKey| engine.router().supports(meridian_provider::Capability::Quotes, Some(k));
    let keys: Vec<SecurityKey> =
        WORLD_INDICES.iter().filter_map(|(sym, _, _)| etf(sym)).map(|(k, _, _)| k).filter(|k| quoted(k)).collect();
    if keys.is_empty() {
        return Screen::not_available(
            "WEI",
            T,
            None,
            "no configured data source provides index levels (index data is separately licensed) or quotes for the US-listed ETFs used as proxies; add Alpaca in Settings → Setup",
        );
    }
    let mut s = Screen::new("WEI", T, None);
    s.push(Block::Notice { level: NoticeLevel::Info, text: WEI_PROXY_NOTICE.into() });
    badge_from_quotes(engine, &mut s, &keys);
    let mut columns = vec![Column::text("Index", 28), Column::text("Proxy", 50)];
    columns.extend(
        live_columns(2)
            .into_iter()
            .filter(|c| !matches!(c.live, Some(LiveField::Bid | LiveField::Ask | LiveField::Volume)))
            .map(|c| if c.live == Some(LiveField::Last) { Column { title: "ETF Last".into(), ..c } } else { c }),
    );
    let nlive = columns.len() - 2;
    let mut rows = Vec::new();
    let mut region = "";
    for (sym, name, reg) in WORLD_INDICES {
        if *reg != region {
            region = reg;
            let mut cells = vec![Cell::text(*reg).styled(Style::Emphasis), Cell::empty()];
            cells.extend(live_cells(nlive));
            rows.push(Row::new(cells).emphasis());
        }
        let mut cells = vec![Cell::text(*name)];
        match etf(sym) {
            Some((k, ticker, etf_name)) if quoted(&k) => {
                let ks = k.to_string();
                cells.push(Cell::text(format!("{ticker:<5} {etf_name}")).styled(Style::Link));
                cells.extend(live_cells(nlive));
                rows.push(Row::new(cells).security(&ks).action(Action::new("GP", Some(&ks))).depth(1));
            }
            other => {
                let why = if other.is_some() { "n/a — no quote source" } else { "n/a — no proxy" };
                cells.push(Cell::text(why).styled(Style::Muted));
                cells.extend(live_cells(nlive));
                rows.push(Row::new(cells).depth(1));
            }
        }
    }
    s.push(Block::Table(Table { title: None, columns, rows, page_size: None, numbered: false }));
    s
}

// --- CRYP -----------------------------------------------------------------

pub(crate) async fn cryp(engine: Arc<Engine>, _req: ScreenRequest) -> Screen {
    let keys: Vec<SecurityKey> = CRYPTO.iter().map(|(sym, _)| SecurityKey::currency(sym)).collect();
    if !keys.iter().any(|k| engine.router().supports(meridian_provider::Capability::Quotes, Some(k))) {
        return Screen::not_available("CRYP", "Crypto Monitor", None, engine.unavailable_hint());
    }
    let mut s = Screen::new("CRYP", "Crypto Monitor", None);
    badge_from_quotes(&engine, &mut s, &keys);
    let mut columns = vec![Column::text("Asset", 16), Column::text("Pair", 10)];
    columns.extend(live_columns(2));
    let rows = CRYPTO
        .iter()
        .map(|(sym, name)| {
            let ks = SecurityKey::currency(sym).to_string();
            let mut cells = vec![Cell::text(*name).styled(Style::Emphasis), Cell::text(*sym)];
            cells.extend(live_cells(9));
            Row::new(cells).security(&ks).action(Action::new("GP", Some(&ks)))
        })
        .collect();
    s.push(Block::Table(Table { title: None, columns, rows, page_size: None, numbered: true }));
    s
}

// --- FXC ------------------------------------------------------------------

pub(crate) async fn fxc(engine: Arc<Engine>, _req: ScreenRequest) -> Screen {
    let pairs: Vec<(String, Option<(SecurityKey, bool)>)> = FX_MATRIX.iter().map(|c| ((*c).to_string(), usd_pair(c))).collect();
    let keys: Vec<SecurityKey> = pairs.iter().filter_map(|(_, p)| p.as_ref().map(|(k, _)| k.clone())).collect();
    let quotes = engine.quotes(&keys).await;
    if quotes.is_empty() {
        return Screen::not_available("FXC", "FX Cross Rates", None, engine.unavailable_hint());
    }
    // usd_per[c] = USD value of one unit of currency c.
    let mut usd_per: Vec<Option<f64>> = Vec::new();
    for (ccy, pair) in &pairs {
        let v = match pair {
            None => Some(1.0),
            Some((k, usd_per_unit)) => quotes.iter().find(|q| &q.key == k).and_then(|q| q.last.or(q.mid())).map(|px| if *usd_per_unit { px } else { 1.0 / px }),
        };
        let _ = ccy;
        usd_per.push(v);
    }
    let n = pairs.len();
    let mut values = Vec::with_capacity(n * n);
    for i in 0..n {
        for j in 0..n {
            // Price of 1 unit of row currency i in column currency j.
            values.push(match (usd_per[i], usd_per[j]) {
                (Some(a), Some(b)) if b != 0.0 => a / b,
                _ => f64::NAN,
            });
        }
    }
    let mut s = Screen::new("FXC", "FX Cross Rates", None);
    let mut seen = std::collections::HashSet::new();
    for q in &quotes {
        if seen.insert(q.provenance.provider.clone()) {
            s.source(&q.provenance);
        }
    }
    let missing: Vec<&str> = pairs.iter().zip(&usd_per).filter(|(_, v)| v.is_none()).map(|((c, _), _)| c.as_str()).collect();
    if !missing.is_empty() {
        s.push(Block::Notice { level: NoticeLevel::Warning, text: format!("No rate available for: {}", missing.join(", ")) });
    }
    let labels: Vec<String> = pairs.iter().map(|(c, _)| c.clone()).collect();
    let mut columns = vec![Column::text("", 5)];
    for l in &labels {
        columns.push(Column::num(l, Format::Number { decimals: 4 }, 10));
    }
    let rows = (0..n)
        .map(|i| {
            let mut cells = vec![Cell::text(&labels[i]).styled(Style::Emphasis)];
            for j in 0..n {
                let v = values[i * n + j];
                cells.push(if i == j { Cell::text("—").styled(Style::Muted) } else { Cell::num((!v.is_nan()).then_some(v)) });
            }
            Row::new(cells)
        })
        .collect();
    s.push(Block::Table(Table { title: Some("Units of column currency per 1 row currency".into()), columns, rows, page_size: None, numbered: false }));
    s.refresh_ms = Some(5_000);
    s
}

// --- MOST -----------------------------------------------------------------

pub(crate) async fn most(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    let universe: Vec<SecurityKey> = engine
        .universe()
        .into_iter()
        .filter(|i| matches!(i.asset_class, AssetClass::Equity | AssetClass::Etf) && i.key.sector == MarketSector::Equity)
        .map(|i| i.key)
        .take(500)
        .collect();
    if universe.is_empty() || engine.mode() == crate::config::DataMode::Live {
        return Screen::not_available(
            "MOST",
            "Most Active",
            None,
            "requires a market-wide quote feed; the configured sources only quote requested symbols",
        );
    }
    let quotes = engine.quotes(&universe).await;
    let sort = req.arg("by").unwrap_or("Volume").to_owned();
    let mut list: Vec<_> = quotes.iter().collect();
    match sort.as_str() {
        "Gainers" => list.sort_by(|a, b| b.pct_change().unwrap_or(f64::MIN).total_cmp(&a.pct_change().unwrap_or(f64::MIN))),
        "Losers" => list.sort_by(|a, b| a.pct_change().unwrap_or(f64::MAX).total_cmp(&b.pct_change().unwrap_or(f64::MAX))),
        _ => list.sort_by(|a, b| b.volume.unwrap_or(0.0).total_cmp(&a.volume.unwrap_or(0.0))),
    }
    let mut s = Screen::new("MOST", "Most Active", None);
    if let Some(q) = quotes.first() {
        s.source(&q.provenance);
    }
    for by in ["Volume", "Gainers", "Losers"] {
        s.menu_item(by, Action::new("MOST", None).arg("by", by), by == sort);
    }
    let mut columns = vec![Column::text("Security", 18)];
    columns.extend(live_columns(2));
    let rows = list
        .iter()
        .take(50)
        .map(|q| {
            let ks = q.key.to_string();
            let mut cells = vec![Cell::text(&ks).styled(Style::Link)];
            cells.extend(live_cells(9));
            Row::new(cells).security(&ks).action(Action::new("DES", Some(&ks)))
        })
        .collect();
    s.push(Block::Table(Table { title: None, columns, rows, page_size: Some(25), numbered: true }));
    s
}

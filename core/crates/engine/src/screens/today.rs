//! TODAY — the home screen: portfolio value and today's change, a market
//! strip, holdings with live prices, what's coming up, new filings, and
//! news on holdings.
//!
//! Every section is fetched concurrently and degrades on its own: a missing
//! source leaves a NOT AVAILABLE note naming it, never a stand-in value.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::NaiveDate;
use meridian_provider::{NewsQuery, NewsScope, ProviderError, SeriesRequest};
use meridian_stream::row::QuoteRow;
use meridian_types::{NANOS_PER_DAY, NANOS_PER_SEC, NewsItem, Provenance, SecurityKey};

use super::ScreenRequest;
use super::calendar::{CalendarData, CalendarRow, Kinds, SectionStatus, calendar_columns, source_name};
use super::inbox::{Inbox, inbox_columns, inbox_row};
use super::parts::{FILL_IN_MS, Part};
use super::scope::{Holding, Scope, Window, is_company_key, new_york_date};
use crate::cache::ttl;
use crate::core::Engine;
use crate::error::{EngineError, EngineResult};
use crate::screen::{
    Action, Block, Cell, Column, Field, Format, LiveField, NoticeLevel, Row, Screen, Style, Table,
};

const TITLE: &str = "Today";
/// Rows in "Coming up", "New filings" and "News on your holdings".
const COMING_UP_ROWS: usize = 8;
const NEW_FILINGS_ROWS: usize = 5;
const NEWS_ROWS: usize = 8;
/// Securities whose company news is fetched (largest positions first).
const NEWS_SECURITIES: usize = 10;
/// How long holdings news is reused before asking the sources again (the
/// screen refreshes every minute; Finnhub company news costs one call per
/// symbol against 60 calls/minute).
const NEWS_TTL_NANOS: i64 = 5 * 60 * NANOS_PER_SEC;
/// Window for "New filings".
const FILING_DAYS: i64 = 30;

/// The market strip: (security, label).
const STRIP: [(&str, &str); 3] = [
    ("SPY US Equity", "S&P 500 (SPY)"),
    ("QQQ US Equity", "Nasdaq 100 (QQQ)"),
    ("BTCUSD Curncy", "Bitcoin (BTC)"),
];
/// Ten-year Treasury yield: the Treasury's own daily par curve tenor, then
/// FRED's constant-maturity series.
const TEN_YEAR_SERIES: [&str; 2] = ["UST:10 Yr", "DGS10"];

fn finite(x: f64) -> Option<f64> {
    x.is_finite().then_some(x)
}

/// Latest and previous daily value of the ten-year yield.
pub(crate) struct TenYear {
    level: f64,
    change: Option<f64>,
    date: NaiveDate,
    provenance: Provenance,
}

impl Engine {
    /// Quote rows for `keys`: stream state where present, else one REST
    /// batch (which also seeds stream state).
    async fn quote_rows(&self, keys: &[SecurityKey]) -> HashMap<SecurityKey, QuoteRow> {
        let mut out = HashMap::new();
        let mut missing = Vec::new();
        for k in keys {
            let id = self.hub().registry().intern(k);
            match self.hub().state().snapshot(id) {
                Some(r) => {
                    out.insert(k.clone(), r);
                }
                None => missing.push(k.clone()),
            }
        }
        if !missing.is_empty() {
            self.quotes(&missing).await;
            for k in missing {
                let id = self.hub().registry().intern(&k);
                if let Some(r) = self.hub().state().snapshot(id) {
                    out.insert(k, r);
                }
            }
        }
        out
    }

    async fn ten_year(&self, today: NaiveDate) -> Result<TenYear, EngineError> {
        let mut last_err = None;
        for id in TEN_YEAR_SERIES {
            let router = self.router();
            let req = SeriesRequest { id: id.to_owned(), from: Some(today - chrono::Duration::days(30)), to: None };
            match self.cached("strip_series", id, ttl::CURVE, async move { router.economic_series(req).await }).await {
                Ok(f) => {
                    let obs: Vec<(NaiveDate, f64)> =
                        f.value.observations.iter().filter_map(|o| o.value.map(|v| (o.date, v))).collect();
                    let Some(&(date, level)) = obs.last() else { continue };
                    let change = obs.len().checked_sub(2).map(|i| level - obs[i].1);
                    return Ok(TenYear { level, change, date, provenance: f.value.provenance });
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| EngineError::NotAvailable {
            what: "ten-year yield".into(),
            reason: "the sources returned no observations".into(),
        }))
    }

    /// Recent company news for `keys`, newest first, de-duplicated by
    /// headline across sources. Reused for five minutes.
    async fn holdings_news_items(&self, keys: &[SecurityKey]) -> EngineResult<Vec<NewsItem>> {
        let now = self.now();
        let ck = keys.iter().map(ToString::to_string).collect::<Vec<_>>().join(",");
        if let Some((at, items)) = self.holdings_news.lock().get(&ck)
            && now - at < NEWS_TTL_NANOS
        {
            return Ok(items.clone());
        }
        let q = NewsQuery {
            scope: NewsScope::Company,
            keys: keys.to_vec(),
            text: None,
            from: Some(now - 3 * NANOS_PER_DAY),
            to: None,
            limit: 60,
        };
        let items = self.news_items(q).await?;
        let mut seen = HashSet::new();
        let norm = |h: &str| h.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
        let deduped: Vec<NewsItem> = items.into_iter().filter(|n| seen.insert(norm(&n.headline))).collect();
        let mut cache = self.holdings_news.lock();
        if cache.len() > 32 {
            cache.clear();
        }
        cache.insert(ck, (now, deduped.clone()));
        Ok(deduped)
    }
}

fn live_num(title: &str, format: Format, width: u16, f: LiveField) -> Column {
    Column::num(title, format, width).live(f)
}

/// Holding or watchlist row values at load time.
struct Line {
    key: SecurityKey,
    name: String,
    row: Option<QuoteRow>,
    holding: Option<Holding>,
}

impl Line {
    fn last(&self) -> Option<f64> {
        self.row.as_ref().and_then(|r| finite(r.last))
    }

    fn value(&self) -> Option<f64> {
        Some(self.holding.as_ref()?.qty * self.last()?)
    }
}

fn no_quote_reason(engine: &Engine, keys: &[&SecurityKey]) -> String {
    let names: Vec<&str> = keys.iter().map(|k| k.symbol.as_str()).collect();
    let source = keys.first().and_then(|k| engine.quote_provenance(k)).map(|p| source_name(p.provider.as_str()));
    match source {
        Some(s) => format!("{}: no quote returned by {s}", names.join(", ")),
        None => format!("{}: no configured source quotes {}", names.join(", "), if keys.len() == 1 { "it" } else { "them" }),
    }
}

/// TODAY's slower sections (the 10-year yield, the calendar, SEC filings,
/// news), kept on the engine between calls; see [`super::parts`].
#[derive(Default)]
pub(crate) struct TodayParts {
    ten_year: Part<Result<TenYear, EngineError>>,
    calendar: Part<CalendarData>,
    inbox: Part<Inbox>,
    news: Part<Option<EngineResult<Vec<NewsItem>>>>,
}

pub(crate) async fn today(engine: Arc<Engine>, _req: ScreenRequest) -> Screen {
    let now = engine.now();
    let today = new_york_date(now);
    let holdings = engine.holdings(today);
    let watchlist = engine.stores().app.watchlists().unwrap_or_default().into_iter().next();
    let (table_title, table_keys): (String, Vec<SecurityKey>) = if holdings.is_empty() {
        match &watchlist {
            Some(w) => (format!("Watchlist — {}", w.name), w.securities.iter().filter_map(|s| s.parse().ok()).collect()),
            None => ("Watchlist".into(), Vec::new()),
        }
    } else {
        ("Holdings".into(), holdings.iter().map(|h| h.key.clone()).collect())
    };
    let scope_keys = engine.scope_keys(Scope::Mine, today);
    let mut by_cost: Vec<&Holding> = holdings.iter().filter(|h| is_company_key(&h.key)).collect();
    by_cost.sort_by(|a, b| b.cost.abs().total_cmp(&a.cost.abs()));
    let news_keys: Vec<SecurityKey> = if by_cost.is_empty() {
        table_keys.iter().filter(|k| is_company_key(k)).take(NEWS_SECURITIES).cloned().collect()
    } else {
        by_cost.iter().take(NEWS_SECURITIES).map(|h| h.key.clone()).collect()
    };
    let strip_keys: Vec<SecurityKey> = STRIP.iter().filter_map(|(k, _)| k.parse().ok()).collect();
    let mut quote_keys = table_keys.clone();
    quote_keys.extend(strip_keys.iter().cloned());

    let parts = &engine.today;
    let (rt, wait_all) = (engine.handle(), engine.deterministic);
    let (day_sig, scope_sig, news_sig) = (today.to_string(), format!("{today}|{scope_keys:?}"), format!("{news_keys:?}"));
    let (quotes, ten_year, calendar, inbox, news) = tokio::join!(
        engine.quote_rows(&quote_keys),
        parts.ten_year.get(rt, wait_all, &day_sig, || {
            let e = engine.clone();
            async move { e.ten_year(today).await }
        }),
        parts.calendar.get(rt, wait_all, &scope_sig, || {
            let (e, scope) = (engine.clone(), scope_keys.clone());
            async move { e.calendar_data(&Window::parse(None, today), Some(&scope), Kinds::ALL, true).await }
        }),
        parts.inbox.get(rt, wait_all, &scope_sig, || {
            let (e, scope) = (engine.clone(), scope_keys.clone());
            async move { e.filings_inbox(&scope, today - chrono::Duration::days(FILING_DAYS)).await }
        }),
        parts.news.get(rt, wait_all, &news_sig, || {
            let (e, keys) = (engine.clone(), news_keys.clone());
            async move { if keys.is_empty() { None } else { Some(e.holdings_news_items(&keys).await) } }
        }),
    );
    let loading = ten_year.is_none() || calendar.is_none() || inbox.is_none() || news.is_none();

    let mut s = Screen::new("TODAY", TITLE, None);
    s.refresh_ms = Some(60_000);
    if holdings.is_empty() {
        s.menu_item("Import portfolio", Action::new("IMPORT", None), false);
    }
    s.menu_item("Portfolio", Action::new("PORT", None), false);
    s.menu_item("Calendar", Action::new("CALENDAR", None), false);
    s.menu_item("Filings", Action::new("FILINGS", None), false);
    let mut seen_providers = HashSet::new();
    for k in table_keys.iter().chain(&strip_keys) {
        if let Some(p) = engine.quote_provenance(k)
            && seen_providers.insert(p.provider.clone())
        {
            s.source(&p);
        }
    }

    // --- (a) Portfolio summary --------------------------------------------
    let lines: Vec<Line> = table_keys
        .iter()
        .map(|k| Line {
            key: k.clone(),
            name: engine.known_name(k),
            row: quotes.get(k).copied(),
            holding: holdings.iter().find(|h| &h.key == k).cloned(),
        })
        .collect();
    if holdings.is_empty() {
        let fallback = watchlist.as_ref().map_or(String::new(), |w| format!(" Showing your {} watchlist instead.", w.name));
        s.push(Block::Notice {
            level: NoticeLevel::Info,
            text: format!("No portfolio yet. Import your portfolio to see its value and today's change.{fallback}"),
        });
    } else {
        let unpriced: Vec<&SecurityKey> = lines.iter().filter(|l| l.last().is_none()).map(|l| &l.key).collect();
        let total: f64 = lines.iter().filter_map(Line::value).sum();
        let changes: Vec<Option<f64>> = lines
            .iter()
            .map(|l| {
                let h = l.holding.as_ref()?;
                h.day_change(l.last(), l.row.as_ref().and_then(|r| finite(r.prev_close)))
            })
            .collect();
        let all_changes = changes.iter().all(Option::is_some);
        // Unknown (not zero) when no position has a known change.
        let change: Option<f64> = changes.iter().any(Option::is_some).then(|| changes.iter().flatten().sum());
        let pct = change.and_then(|c| {
            let base = total - c;
            (base.abs() > 1e-9).then(|| c / base * 100.0)
        });
        let fields = vec![
            Field::num("Value", (unpriced.len() < lines.len()).then_some(total), Format::Number { decimals: 2 }).styled(Style::Emphasis),
            Field::num("Today", change, Format::Change { decimals: 2 }),
            Field::num("Today %", pct, Format::ChangePercent { decimals: 2 }),
            Field::num("Positions", Some(lines.len() as f64), Format::Integer),
        ];
        s.push(Block::Fields { title: Some("Portfolio".into()), columns: 4, fields });
        if !unpriced.is_empty() {
            s.push(Block::Notice {
                level: NoticeLevel::Warning,
                text: format!("Value and today's change leave out {}", no_quote_reason(&engine, &unpriced)),
            });
        } else if !all_changes {
            let missing: Vec<&str> =
                lines.iter().zip(&changes).filter(|(_, c)| c.is_none()).map(|(l, _)| l.key.symbol.as_str()).collect();
            s.push(Block::Notice {
                level: NoticeLevel::Warning,
                text: format!("Today's change leaves out {}: no previous close", missing.join(", ")),
            });
        }
    }

    // --- (b) Market strip ----------------------------------------------------
    let mut strip_rows = Vec::new();
    let mut strip_missing: Vec<&SecurityKey> = Vec::new();
    for (k, (_, label)) in strip_keys.iter().zip(STRIP) {
        match quotes.get(k).filter(|r| r.last.is_finite()) {
            Some(r) => {
                let ks = k.to_string();
                strip_rows.push(
                    Row::new(vec![
                        Cell::text(label).styled(Style::Emphasis),
                        Cell::num(finite(r.last)),
                        Cell::signed(finite(r.net_change)),
                        Cell::signed(finite(r.pct_change)),
                    ])
                    .security(&ks)
                    .action(Action::new("GP", Some(&ks))),
                );
            }
            None => strip_missing.push(k),
        }
    }
    match ten_year.as_deref() {
        None => {}
        Some(Ok(t)) => {
            s.source(&t.provenance);
            strip_rows.insert(
                2.min(strip_rows.len()),
                Row::new(vec![
                    Cell::text(format!("10-year Treasury, % · {}", t.date.format("%m/%d"))).styled(Style::Emphasis),
                    Cell::num(Some(t.level)),
                    Cell::signed(t.change),
                    Cell::empty(),
                ])
                .action(Action::new("ECO", None).arg("view", "curve")),
            );
        }
        Some(Err(e)) => s.push(Block::Notice {
            level: NoticeLevel::Warning,
            text: format!("10-year Treasury yield: NOT AVAILABLE — {}", e.user_message()),
        }),
    }
    if !strip_missing.is_empty() {
        s.push(Block::Notice { level: NoticeLevel::Warning, text: format!("Markets: NOT AVAILABLE — {}", no_quote_reason(&engine, &strip_missing)) });
    }
    if !strip_rows.is_empty() {
        s.push(Block::Table(Table {
            title: Some("Markets".into()),
            columns: vec![
                Column::text("", 30),
                live_num("Last", Format::Price { decimals: 2 }, 11, LiveField::Last),
                live_num("Change", Format::Change { decimals: 2 }, 9, LiveField::NetChange),
                live_num("Change %", Format::ChangePercent { decimals: 2 }, 9, LiveField::PctChange),
            ],
            rows: strip_rows,
            page_size: None,
            numbered: false,
        }));
    }

    // --- (c) Holdings (or the watchlist) -------------------------------------
    if lines.is_empty() {
        s.push(Block::Notice { level: NoticeLevel::Info, text: "Your watchlist is empty.".into() });
    } else {
        let total: f64 = lines.iter().filter_map(Line::value).sum();
        let mut ordered: Vec<&Line> = lines.iter().collect();
        if !holdings.is_empty() {
            ordered.sort_by(|a, b| b.value().unwrap_or(f64::MIN).total_cmp(&a.value().unwrap_or(f64::MIN)));
        }
        let mut columns = vec![
            Column::text("Security", 9),
            Column::text("Name", 26),
            live_num("Last", Format::Price { decimals: 2 }, 11, LiveField::Last),
            live_num("Today %", Format::ChangePercent { decimals: 2 }, 9, LiveField::PctChange),
        ];
        if !holdings.is_empty() {
            columns.push(Column::num("Value", Format::Number { decimals: 2 }, 13));
            columns.push(Column::num("Weight %", Format::Number { decimals: 1 }, 9));
        }
        let rows = ordered
            .iter()
            .map(|l| {
                let ks = l.key.to_string();
                let mut cells = vec![
                    Cell::text(&l.key.symbol).styled(Style::Link),
                    Cell::text(&l.name),
                    Cell::num(l.last()),
                    Cell::signed(l.row.as_ref().and_then(|r| finite(r.pct_change))),
                ];
                if !holdings.is_empty() {
                    let v = l.value();
                    cells.push(Cell::num(v));
                    cells.push(Cell::num(v.filter(|_| total.abs() > 1e-9).map(|v| v / total * 100.0)));
                }
                Row::new(cells).security(&ks).action(Action::new("DES", Some(&ks)))
            })
            .collect();
        s.push(Block::Table(Table { title: Some(table_title), columns, rows, page_size: Some(15), numbered: true }));
    }

    if loading {
        s.refresh_ms = Some(FILL_IN_MS);
    }
    if ten_year.is_none() {
        s.push(Block::Notice { level: NoticeLevel::Info, text: "10-year Treasury yield: loading…".into() });
    }

    // --- (d) Coming up --------------------------------------------------------
    if let Some(calendar) = &calendar {
        for p in &calendar.provenance {
            s.source(p);
        }
        for n in calendar.notices() {
            if let Block::Notice { level, text } = n {
                s.push(Block::Notice { level, text: format!("Coming up — {text}") });
            }
        }
        let upcoming: Vec<&CalendarRow> = calendar.rows.iter().filter(|r| r.date >= today).take(COMING_UP_ROWS).collect();
        if upcoming.is_empty() {
            if calendar.sections.iter().any(|x| matches!(x.status, SectionStatus::Loaded { .. })) {
                s.push(Block::Notice { level: NoticeLevel::Info, text: "Coming up: nothing scheduled in the next 10 days".into() });
            }
        } else {
            s.push(Block::Table(Table {
                title: Some("Coming up".into()),
                columns: calendar_columns(),
                rows: upcoming.iter().map(|r| r.to_row()).collect(),
                page_size: None,
                numbered: false,
            }));
        }
    } else {
        s.push(Block::Notice { level: NoticeLevel::Info, text: "Coming up: loading…".into() });
    }

    // --- (e) New filings ------------------------------------------------------
    if let Some(inbox) = &inbox {
        if let Some(p) = &inbox.provenance {
            s.source(p);
        }
        if let Some(reason) = &inbox.unavailable {
            s.push(Block::Notice { level: NoticeLevel::Warning, text: format!("New filings: NOT AVAILABLE — {reason}") });
        } else {
            for n in &inbox.notes {
                if let Block::Notice { level: NoticeLevel::Warning, text } = n {
                    s.push(Block::Notice { level: NoticeLevel::Warning, text: format!("New filings — {text}") });
                }
            }
            let unread: Vec<Row> = inbox.items.iter().filter(|i| i.unread).take(NEW_FILINGS_ROWS).map(inbox_row).collect();
            if unread.is_empty() {
                s.push(Block::Notice { level: NoticeLevel::Info, text: format!("New filings: none unread in the last {FILING_DAYS} days") });
            } else {
                s.push(Block::Table(Table {
                    title: Some(format!("New filings · {} unread", inbox.unread())),
                    columns: inbox_columns(),
                    rows: unread,
                    page_size: None,
                    numbered: false,
                }));
            }
        }
    } else {
        s.push(Block::Notice { level: NoticeLevel::Info, text: "New filings: loading…".into() });
    }

    // --- (f) News on holdings -------------------------------------------------
    let news_title = if holdings.is_empty() { "News on your watchlist" } else { "News on your holdings" };
    match news.as_deref() {
        None => s.push(Block::Notice { level: NoticeLevel::Info, text: format!("{news_title}: loading…") }),
        Some(None) => s.push(Block::Notice { level: NoticeLevel::Info, text: format!("{news_title}: no stocks to follow") }),
        Some(Some(Err(e))) => {
            let reason = match e {
                EngineError::Provider(ProviderError::Unsupported { .. }) => {
                    format!("no configured source provides company news ({})", engine.unavailable_hint())
                }
                e => e.user_message(),
            };
            s.push(Block::Notice { level: NoticeLevel::Warning, text: format!("{news_title}: NOT AVAILABLE — {reason}") });
        }
        Some(Some(Ok(items))) if items.is_empty() => {
            s.push(Block::Notice { level: NoticeLevel::Info, text: format!("{news_title}: no stories in the last 3 days") });
        }
        Some(Some(Ok(items))) => {
            let shown: Vec<&NewsItem> = items.iter().take(NEWS_ROWS).collect();
            for it in shown.iter().take(5) {
                s.source(&it.provenance);
            }
            let rows = shown
                .iter()
                .map(|it| {
                    Row::new(vec![
                        Cell::num(Some(it.published_at as f64)),
                        Cell::text(&it.source).styled(Style::Muted),
                        Cell::text(&it.headline).styled(Style::Emphasis),
                        Cell::text(it.tickers.iter().take(3).cloned().collect::<Vec<_>>().join(" ")),
                    ])
                    .action(Action::new("N", None).arg("story", it.id.clone()))
                })
                .collect();
            s.push(Block::Table(Table {
                title: Some(news_title.into()),
                columns: vec![
                    Column::num("Time", Format::DateTime, 16),
                    Column::text("Source", 14),
                    Column::text("Headline", 80),
                    Column::text("Tickers", 14),
                ],
                rows,
                page_size: None,
                numbered: false,
            }));
        }
    }
    s
}

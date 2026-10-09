//! LIVE-mode behaviour of TODAY, CALENDAR and FILINGS with stand-in
//! providers (no network): each section degrades on its own with a NOT
//! AVAILABLE note naming the problem, and sections load concurrently. The
//! stand-ins carry made-up ids and obviously-test values only.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::NaiveDate;
use meridian_engine::screen::{Block, NoticeLevel, Screen, ScreenStatus, Table};
use meridian_engine::screens::ScreenRequest;
use meridian_engine::{DataMode, Engine, EngineConfig, NullEvents};
use meridian_provider::{
    AiPolicy, CachePolicy, CalendarRequest, Capabilities, Capability, CapabilityEntry, EventCalendarRequest,
    FilingsRequest, NewsQuery, Provider, ProviderError, ProviderResult,
};
use meridian_store::Transaction;
use meridian_types::{
    AssetClass, DataDelay, Dividend, DividendCalendar, DividendEvent, DividendKind, EconomicEvent, FeedSource, Filing,
    FilingsPage, Importance, NewsItem, NewsPage, Provenance, ProviderId, Quote, SecurityKey, date_to_nanos,
};
use parking_lot::Mutex;

/// 2026-10-05 16:00 UTC (12:00 New York, a Monday).
const CLOCK: i64 = 1_791_216_000_000_000_000;

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

fn caps(list: &[Capability]) -> Capabilities {
    Capabilities {
        entries: list
            .iter()
            .map(|c| CapabilityEntry {
                capability: *c,
                asset_classes: vec![AssetClass::Equity, AssetClass::Etf, AssetClass::Crypto, AssetClass::Economic],
                delay: DataDelay::EndOfDay,
                source: FeedSource::Aggregated,
                history: None,
            })
            .collect(),
        rate_limit: None,
        max_stream_symbols: None,
        cache_policy: CachePolicy::Unrestricted,
        attribution: None,
        display_allowed: true,
        ai_policy: AiPolicy::Allowed,
        requires_credentials: false,
        terms_note: String::new(),
        docs_url: String::new(),
    }
}

fn prov(id: &str) -> Provenance {
    Provenance {
        provider: ProviderId::new(id),
        synthetic: false,
        delay: DataDelay::EndOfDay,
        source: FeedSource::Aggregated,
        as_of: CLOCK,
        source_ref: None,
        attribution: None,
    }
}

/// A stand-in source. Everything it answers is configured; what it isn't
/// given it reports as unsupported or not found.
#[derive(Default)]
struct Stand {
    id: &'static str,
    caps: Option<Capabilities>,
    /// symbol → (last, previous close).
    quotes: HashMap<&'static str, (f64, f64)>,
    dividends: Vec<DividendEvent>,
    releases: Vec<EconomicEvent>,
    /// symbol → filings; symbols not listed are NotFound.
    filings: HashMap<&'static str, Vec<Filing>>,
    /// symbol → error for filings.
    filing_errors: HashMap<&'static str, ProviderError>,
    /// Error for every earnings calendar call.
    earnings_error: Option<ProviderError>,
    news: Vec<NewsItem>,
    /// Added to every data call (to check sections load concurrently).
    delay: Duration,
    /// Keys of each calendar request, for assertions.
    seen: Mutex<Vec<Vec<SecurityKey>>>,
    /// Economic calendar calls, for assertions.
    macro_calls: AtomicUsize,
}

impl Stand {
    async fn wait(&self) {
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
    }
}

#[async_trait]
impl Provider for Stand {
    fn id(&self) -> ProviderId {
        ProviderId::new(self.id)
    }
    fn capabilities(&self) -> &Capabilities {
        self.caps.as_ref().expect("caps")
    }
    async fn quotes(&self, keys: &[SecurityKey]) -> ProviderResult<Vec<Quote>> {
        Ok(keys
            .iter()
            .filter_map(|k| {
                let (last, prev) = self.quotes.get(k.symbol.as_str())?;
                let mut q = Quote::empty(k.clone(), prov(self.id));
                q.last = Some(*last);
                q.prev_close = Some(*prev);
                q.ts_event = CLOCK;
                q.ts_recv = CLOCK;
                Some(q)
            })
            .collect())
    }
    async fn earnings_calendar(&self, _req: &EventCalendarRequest) -> ProviderResult<meridian_types::EarningsCalendar> {
        self.wait().await;
        Err(self.earnings_error.clone().unwrap_or(ProviderError::Unsupported { capability: Capability::EarningsCalendar }))
    }
    async fn dividend_calendar(&self, req: &EventCalendarRequest) -> ProviderResult<DividendCalendar> {
        self.wait().await;
        self.seen.lock().push(req.keys.clone());
        let events = self
            .dividends
            .iter()
            .filter(|e| req.keys.is_empty() || req.keys.contains(&e.key))
            .filter(|e| e.dividend.ex_date >= req.from && e.dividend.ex_date <= req.to)
            .cloned()
            .collect();
        Ok(DividendCalendar { events, provenance: prov(self.id) })
    }
    async fn economic_calendar(&self, _req: &CalendarRequest) -> ProviderResult<Vec<EconomicEvent>> {
        self.macro_calls.fetch_add(1, Ordering::SeqCst);
        self.wait().await;
        Ok(self.releases.clone())
    }
    async fn filings(&self, req: &FilingsRequest) -> ProviderResult<FilingsPage> {
        self.wait().await;
        let key = req.key.clone().expect("company filings");
        if let Some(e) = self.filing_errors.get(key.symbol.as_str()) {
            return Err(e.clone());
        }
        match self.filings.get(key.symbol.as_str()) {
            Some(f) => Ok(FilingsPage { key: Some(key), filings: f.clone() }),
            None => Err(ProviderError::NotFound(format!("no filer for {key}"))),
        }
    }
    async fn news(&self, _q: &NewsQuery) -> ProviderResult<NewsPage> {
        self.wait().await;
        Ok(NewsPage { items: self.news.clone(), next: None })
    }
}

fn engine(providers: Vec<Stand>) -> Arc<Engine> {
    engine_with(providers.into_iter().map(|p| Arc::new(p) as Arc<dyn Provider>).collect())
}

fn engine_with(providers: Vec<Arc<dyn Provider>>) -> Arc<Engine> {
    Engine::new(&EngineConfig::test(DataMode::Live, CLOCK), providers, Arc::new(NullEvents)).expect("engine")
}

fn screen(e: &Arc<Engine>, f: &str, args: &[(&str, &str)]) -> Screen {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let req = ScreenRequest { function: f.into(), security: None, args: args.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect() };
    rt.block_on(e.screen(req))
}

fn notices(s: &Screen) -> Vec<(NoticeLevel, String)> {
    s.blocks
        .iter()
        .filter_map(|b| match b {
            Block::Notice { level, text } => Some((*level, text.clone())),
            _ => None,
        })
        .collect()
}

fn has_notice(s: &Screen, level: NoticeLevel, needle: &str) -> bool {
    notices(s).iter().any(|(l, t)| *l == level && t.contains(needle))
}

fn table<'a>(s: &'a Screen, prefix: &str) -> Option<&'a Table> {
    s.blocks.iter().find_map(|b| match b {
        Block::Table(t) if t.title.as_deref().is_some_and(|x| x.starts_with(prefix)) => Some(t),
        _ => None,
    })
}

fn texts(t: &Table, col: usize) -> Vec<String> {
    t.rows.iter().map(|r| r.cells[col].text.clone().unwrap_or_default()).collect()
}

fn ex_div(sym: &str, ex: NaiveDate, amount: f64) -> DividendEvent {
    DividendEvent {
        key: SecurityKey::equity(sym),
        dividend: Dividend {
            declared_date: None,
            ex_date: ex,
            record_date: None,
            pay_date: Some(ex + chrono::Duration::days(14)),
            amount,
            currency: "USD".into(),
            frequency: None,
            kind: DividendKind::Regular,
        },
    }
}

fn release(name: &str, date: NaiveDate, importance: Importance) -> EconomicEvent {
    EconomicEvent {
        release_time: date_to_nanos(date),
        time_known: false,
        country: "US".into(),
        event: name.into(),
        period: None,
        actual: None,
        consensus: None,
        prior: None,
        unit: None,
        importance,
        series_id: Some("TESTSERIES".into()),
        provenance: prov("stand-macro"),
    }
}

fn filing(acc: &str, form: &str, filed: NaiveDate, items: &[&str]) -> Filing {
    Filing {
        cik: 9_999_999,
        company: "Test Filer".into(),
        accession: acc.into(),
        form: form.into(),
        filed,
        accepted_at: None,
        period_of_report: None,
        primary_document: None,
        primary_doc_url: None,
        description: None,
        size_bytes: None,
        items: items.iter().map(|i| (*i).to_owned()).collect(),
        provenance: prov("stand-filings"),
    }
}

// ---------------------------------------------------------------------------
// CALENDAR
// ---------------------------------------------------------------------------

#[test]
fn calendar_kinds_degrade_on_their_own() {
    let corp = Stand {
        id: "stand-corp",
        caps: Some(caps(&[Capability::DividendCalendar])),
        dividends: vec![ex_div("AAPL", d(2026, 10, 9), 0.26), ex_div("JPM", d(2026, 10, 30), 1.4), ex_div("ZZZT", d(2026, 10, 8), 9.0)],
        ..Stand::default()
    };
    let macro_src = Stand {
        id: "stand-macro",
        caps: Some(caps(&[Capability::EconomicCalendar])),
        releases: vec![release("Big Release", d(2026, 10, 7), Importance::High), release("Small Release", d(2026, 10, 8), Importance::Medium)],
        ..Stand::default()
    };
    // No source offers an earnings calendar.
    let e = engine(vec![corp, macro_src]);
    let s = screen(&e, "CALENDAR", &[]);
    assert!(matches!(s.status, ScreenStatus::Ok), "{:?}", s.status);
    assert!(has_notice(&s, NoticeLevel::Warning, "Earnings: NOT AVAILABLE — no configured source provides a earnings calendar"), "{:?}", notices(&s));
    let t = table(&s, "Next 10 days").expect("rows");
    // Default watchlist only (AAPL is on it, ZZZT isn't); JPM's ex-date is outside the 10 days.
    assert_eq!(texts(t, 3), ["US", "AAPL"]);
    // No reference data is loaded here, so the name is the symbol.
    assert_eq!(texts(t, 4), ["Big Release", "AAPL"]);
    assert_eq!(t.rows[1].action.as_ref().unwrap().function, "DVD");
    assert_eq!(t.rows[0].action.as_ref().unwrap().args, [("series".to_owned(), "TESTSERIES".to_owned())]);
    assert!(texts(t, 5)[1].starts_with("0.26 USD · pays 10/23"), "{:?}", texts(t, 5));

    // All macro releases on request; dividends only.
    let all = screen(&e, "CALENDAR", &[("importance", "all")]);
    assert_eq!(table(&all, "Next 10 days").unwrap().rows.len(), 3);
    let divs = screen(&e, "CALENDAR", &[("kind", "dividends"), ("range", "month")]);
    let t = table(&divs, "This month").unwrap();
    assert_eq!(texts(t, 3), ["AAPL", "JPM"]);
    assert!(notices(&divs).is_empty(), "the other kinds are off, not unavailable: {:?}", notices(&divs));
}

#[test]
fn calendar_reports_the_sources_own_error_and_asks_only_for_stocks() {
    let corp = Arc::new(Stand {
        id: "stand-corp",
        caps: Some(caps(&[Capability::DividendCalendar, Capability::EarningsCalendar])),
        earnings_error: Some(ProviderError::Unauthorized("Test source API key not set — add it in Settings".into())),
        ..Stand::default()
    });
    let e = engine_with(vec![corp.clone()]);
    let s = screen(&e, "CALENDAR", &[]);
    assert!(has_notice(&s, NoticeLevel::Warning, "Earnings: NOT AVAILABLE — Test source API key not set"), "{:?}", notices(&s));
    assert!(has_notice(&s, NoticeLevel::Warning, "Macro: NOT AVAILABLE"), "{:?}", notices(&s));
    // An empty answer from a source that loaded: "nothing scheduled".
    assert!(has_notice(&s, NoticeLevel::Info, "Nothing scheduled"), "{:?}", notices(&s));
    // Crypto and FX on the default watchlist are not sent to the corporate-actions source.
    let seen = corp.seen.lock().clone();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].iter().any(|k| k.symbol == "AAPL"));
    assert!(seen[0].iter().all(|k| k.sector == meridian_types::MarketSector::Equity), "{:?}", seen[0]);
    // "All securities" asks without keys.
    let _ = screen(&e, "CALENDAR", &[("scope", "all")]);
    assert!(corp.seen.lock().last().unwrap().is_empty());
}

#[test]
fn calendar_shows_the_other_kinds_while_a_slow_one_loads() {
    // The wall clock (no fixed clock): screens show what they have after the
    // first paint instead of waiting for every source.
    let config = EngineConfig { fixed_clock: None, ..EngineConfig::test(DataMode::Live, CLOCK) };
    let today = meridian_engine::screens::new_york_date(meridian_types::Clock::now(&meridian_types::SystemClock));
    let corp = Stand {
        id: "stand-corp",
        caps: Some(caps(&[Capability::DividendCalendar])),
        dividends: vec![ex_div("AAPL", today + chrono::Duration::days(1), 0.26)],
        ..Stand::default()
    };
    let slow_macro = Arc::new(Stand {
        id: "stand-macro",
        caps: Some(caps(&[Capability::EconomicCalendar])),
        releases: vec![release("Big Release", today + chrono::Duration::days(2), Importance::High)],
        delay: Duration::from_millis(1_500),
        ..Stand::default()
    });
    let e = Engine::new(&config, vec![Arc::new(corp), slow_macro.clone()], Arc::new(NullEvents)).expect("engine");

    let s = screen(&e, "CALENDAR", &[]);
    assert_eq!(texts(table(&s, "Next 10 days").expect("dividends while macro loads"), 3), ["AAPL"]);
    assert!(has_notice(&s, NoticeLevel::Info, "Macro: loading…"), "{:?}", notices(&s));
    assert!(has_notice(&s, NoticeLevel::Warning, "Earnings: NOT AVAILABLE"), "{:?}", notices(&s));
    assert_eq!(s.refresh_ms, Some(1_000), "asks to be refreshed to fill in");
    // The menus and the sources line work meanwhile.
    assert!(s.blocks.iter().any(|b| matches!(b, Block::Inputs { inputs, .. } if inputs.len() == 4)));
    assert!(s.blocks.iter().any(|b| matches!(b, Block::Fields { title: Some(t), fields, .. } if t == "Sources" && fields.len() == 1)));

    // A refresh while it loads shares the fetch already running.
    let again = screen(&e, "CALENDAR", &[]);
    assert!(has_notice(&again, NoticeLevel::Info, "Macro: loading…"), "{:?}", notices(&again));
    assert_eq!(slow_macro.macro_calls.load(Ordering::SeqCst), 1);

    std::thread::sleep(Duration::from_millis(1_500));
    let done = screen(&e, "CALENDAR", &[]);
    assert_eq!(texts(table(&done, "Next 10 days").unwrap(), 3), ["AAPL", "US"]);
    assert!(!has_notice(&done, NoticeLevel::Info, "loading"), "{:?}", notices(&done));
    assert_eq!(done.refresh_ms, None, "nothing left to fill in");
    assert_eq!(slow_macro.macro_calls.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// FILINGS
// ---------------------------------------------------------------------------

fn filings_source() -> Stand {
    let mut filings = HashMap::new();
    filings.insert(
        "AAPL",
        vec![
            filing("0000000001-26-000004", "4", d(2026, 10, 2), &[]),
            filing("0000000001-26-000003", "8-K", d(2026, 9, 30), &["2.02", "9.01"]),
            filing("0000000001-26-000002", "10-K", d(2026, 9, 20), &[]),
            filing("0000000001-25-000001", "10-K", d(2025, 9, 20), &[]),
        ],
    );
    let mut filing_errors = HashMap::new();
    filing_errors.insert("MSFT", ProviderError::Network("test network failure".into()));
    Stand { id: "stand-filings", caps: Some(caps(&[Capability::Filings])), filings, filing_errors, ..Stand::default() }
}

#[test]
fn filings_inbox_describes_marks_and_degrades() {
    let e = engine(vec![filings_source()]);
    let s = screen(&e, "FILINGS", &[]);
    assert!(matches!(s.status, ScreenStatus::Ok), "{:?}", s.status);
    let t = table(&s, "Last 30 days").expect("inbox");
    // Newest first; last year's 10-K is outside the 30 days.
    assert_eq!(texts(t, 2), ["4", "8-K", "10-K"]);
    assert_eq!(texts(t, 4), ["Insider transaction", "Results of Operations and Financial Condition", "Annual report"]);
    assert_eq!(texts(t, 0), ["●", "●", "●"]);
    // The 10-K opens on what changed versus last year's; the others on the document.
    let args = |i: usize| t.rows[i].action.clone().unwrap().args;
    assert!(args(2).contains(&("diff".to_owned(), "1".to_owned())));
    assert!(!args(0).iter().any(|(k, _)| k == "diff"));
    // MSFT failed; the funds have no filings.
    assert!(has_notice(&s, NoticeLevel::Warning, "Filings for MSFT: NOT AVAILABLE — Network error: test network failure"), "{:?}", notices(&s));
    assert!(has_notice(&s, NoticeLevel::Info, "No SEC filings found for"), "{:?}", notices(&s));

    // Mark one unread after marking all read.
    let mark = s.menu.iter().find(|m| m.label == "Mark all read").unwrap().action.clone();
    let args: Vec<(&str, &str)> = mark.args.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let read = screen(&e, "FILINGS", &args);
    assert!(table(&read, "Last 30 days").unwrap().title.as_deref().unwrap().contains("0 unread of 3"));
    let back = screen(&e, "FILINGS", &[("unread", "0000000001-26-000003")]);
    assert!(table(&back, "Last 30 days").unwrap().title.as_deref().unwrap().contains("1 unread of 3"));
}

#[test]
fn filings_unavailable_when_every_company_fails() {
    let mut src = filings_source();
    src.filings.clear();
    for sym in ["AAPL", "MSFT", "NVDA", "AMZN", "GOOGL", "META", "TSLA", "JPM", "XOM", "SPY", "QQQ"] {
        src.filing_errors.insert(sym, ProviderError::Unauthorized("Test filings source needs a contact email — add it in Settings".into()));
    }
    let e = engine(vec![src]);
    let s = screen(&e, "FILINGS", &[]);
    match &s.status {
        ScreenStatus::NotAvailable { reason } => assert!(reason.contains("needs a contact email"), "{reason}"),
        other => panic!("{other:?}"),
    }
    // No source at all.
    let none = engine(vec![Stand { id: "stand-none", caps: Some(caps(&[Capability::Quotes])), ..Stand::default() }]);
    let s = screen(&none, "FILINGS", &[]);
    assert!(matches!(s.status, ScreenStatus::NotAvailable { ref reason } if reason.contains("no configured source provides SEC filings")), "{:?}", s.status);
}

// ---------------------------------------------------------------------------
// TODAY
// ---------------------------------------------------------------------------

fn quotes_source() -> Stand {
    let mut quotes = HashMap::new();
    quotes.insert("AAPL", (110.0, 100.0));
    quotes.insert("SPY", (500.0, 495.0));
    Stand { id: "stand-quotes", caps: Some(caps(&[Capability::Quotes])), quotes, ..Stand::default() }
}

fn add_position(e: &Engine, sym: &str, qty: f64, price: f64, date: &str) {
    let store = &e.stores().app;
    let pid = match store.portfolios().unwrap().first() {
        Some(p) => p.id,
        None => store.create_portfolio("Test", "USD").unwrap(),
    };
    store
        .add_transaction(&Transaction {
            id: 0,
            portfolio_id: pid,
            security: Some(format!("{sym} US Equity")),
            kind: meridian_types::TransactionKind::Buy,
            trade_date: date.into(),
            settle_date: None,
            quantity: qty,
            price: Some(price),
            amount: None,
            fees: 0.0,
            currency: None,
            note: None,
            source: None,
            fingerprint: None,
        })
        .unwrap();
}

#[test]
fn today_sections_degrade_on_their_own() {
    let e = engine(vec![quotes_source()]);
    add_position(&e, "AAPL", 10.0, 90.0, "2026-01-05");
    add_position(&e, "MSFT", 2.0, 300.0, "2026-02-02");
    let s = screen(&e, "TODAY", &[]);
    assert!(matches!(s.status, ScreenStatus::Ok), "{:?}", s.status);
    let n = notices(&s);
    for needle in [
        "Value and today's change leave out MSFT: no quote returned by stand-quotes",
        "10-year Treasury yield: NOT AVAILABLE",
        "Markets: NOT AVAILABLE — QQQ, BTCUSD",
        "Coming up — Earnings: NOT AVAILABLE",
        "Coming up — Dividends: NOT AVAILABLE",
        "Coming up — Macro: NOT AVAILABLE",
        "New filings: NOT AVAILABLE — no configured source provides SEC filings",
        "News on your holdings: NOT AVAILABLE — no configured source provides company news",
    ] {
        assert!(n.iter().any(|(l, t)| *l == NoticeLevel::Warning && t.contains(needle)), "missing {needle:?} in {n:?}");
    }
    // AAPL: 10 × 110 = 1,100; today 10 × (110 − 100) = +100 (+10%).
    let fields = s
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Fields { title: Some(t), fields, .. } if t == "Portfolio" => Some(fields.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(fields[0].value, Some(1_100.0));
    assert_eq!(fields[1].value, Some(100.0));
    assert!((fields[2].value.unwrap() - 10.0).abs() < 1e-9);
    let markets = table(&s, "Markets").unwrap();
    assert_eq!(texts(markets, 0), ["S&P 500 (SPY)"]);
    let holdings = table(&s, "Holdings").unwrap();
    assert_eq!(texts(holdings, 0), ["AAPL", "MSFT"]);
    assert!(holdings.columns.iter().any(|c| c.live.is_some()));
    assert!(table(&s, "Coming up").is_none() && table(&s, "News").is_none());
}

#[test]
fn today_without_a_portfolio_prompts_import_and_shows_the_watchlist() {
    let e = engine(vec![quotes_source()]);
    let s = screen(&e, "TODAY", &[]);
    assert_eq!(s.menu[0].action.function, "IMPORT");
    assert!(has_notice(&s, NoticeLevel::Info, "No portfolio yet. Import your portfolio"), "{:?}", notices(&s));
    let t = table(&s, "Watchlist — Default").expect("watchlist instead of holdings");
    assert!(t.columns.iter().all(|c| c.title != "Value"));
    assert!(s.blocks.iter().all(|b| !matches!(b, Block::Fields { title: Some(t), .. } if t == "Portfolio")));
}

#[test]
fn today_loads_its_sections_concurrently() {
    let delay = Duration::from_millis(400);
    let slow = Stand {
        id: "stand-slow",
        caps: Some(caps(&[Capability::DividendCalendar, Capability::EconomicCalendar, Capability::Filings, Capability::News])),
        filings: filings_source().filings,
        delay,
        ..Stand::default()
    };
    let e = engine(vec![quotes_source(), slow]);
    add_position(&e, "AAPL", 10.0, 90.0, "2026-01-05");
    let t0 = Instant::now();
    let s = screen(&e, "TODAY", &[]);
    let took = t0.elapsed();
    assert!(matches!(s.status, ScreenStatus::Ok));
    // Calendar (two sources), ~11 filings fetches and news each wait 400 ms;
    // one after another that is several seconds.
    assert!(took < Duration::from_millis(1_500), "TODAY took {took:?}");
    assert!(table(&s, "New filings").is_some(), "{:?}", notices(&s));
}

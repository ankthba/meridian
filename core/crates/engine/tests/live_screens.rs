//! LIVE-mode screen behaviour with stand-in providers (no network): WEI's
//! ETF proxies, DVD merging complementary dividend sources, and FA's split
//! notice. The fakes carry made-up ids and test values only.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::NaiveDate;
use meridian_engine::screen::{Block, NoticeLevel, Screen, ScreenStatus};
use meridian_engine::screens::ScreenRequest;
use meridian_engine::{DataMode, Engine, EngineConfig, NullEvents};
use meridian_provider::{
    AiPolicy, CachePolicy, Capabilities, Capability, CapabilityEntry, FundamentalsRequest, Provider, ProviderError,
    ProviderResult,
};
use meridian_types::{
    AssetClass, DataDelay, Dividend, DividendKind, Dividends, FeedSource, Fundamentals, PeriodDividend, PeriodType,
    Provenance, ProviderId, Quote, ReportedSplit, SecurityKey, Statement, StatementKind, StatementLine,
};

/// 2026-10-05 16:00 UTC.
const CLOCK: i64 = 1_791_216_000_000_000_000;

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

fn caps(list: &[Capability], source: FeedSource) -> Capabilities {
    Capabilities {
        entries: list
            .iter()
            .map(|c| CapabilityEntry {
                capability: *c,
                asset_classes: vec![AssetClass::Equity, AssetClass::Etf],
                delay: DataDelay::EndOfDay,
                source: source.clone(),
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

/// A stand-in source: quotes (always empty), dividends and fundamentals as
/// configured.
struct Fake {
    id: &'static str,
    caps: Capabilities,
    dividends: Option<ProviderResult<Dividends>>,
    fundamentals: Option<Fundamentals>,
}

#[async_trait]
impl Provider for Fake {
    fn id(&self) -> ProviderId {
        ProviderId::new(self.id)
    }
    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }
    async fn quotes(&self, _keys: &[SecurityKey]) -> ProviderResult<Vec<Quote>> {
        Ok(Vec::new())
    }
    async fn dividends(&self, key: &SecurityKey) -> ProviderResult<Dividends> {
        match &self.dividends {
            Some(Ok(d)) => Ok(Dividends { key: key.clone(), ..d.clone() }),
            Some(Err(e)) => Err(e.clone()),
            None => Err(ProviderError::Unsupported { capability: Capability::Dividends }),
        }
    }
    async fn fundamentals(&self, req: &FundamentalsRequest) -> ProviderResult<Fundamentals> {
        self.fundamentals.clone().map(|f| Fundamentals { key: req.key.clone(), ..f }).ok_or(ProviderError::NotFound("none".into()))
    }
}

fn engine(providers: Vec<Fake>) -> Arc<Engine> {
    let providers: Vec<Arc<dyn Provider>> = providers.into_iter().map(|p| Arc::new(p) as Arc<dyn Provider>).collect();
    Engine::new(&EngineConfig::test(DataMode::Live, CLOCK), providers, Arc::new(NullEvents)).expect("engine")
}

fn screen(e: &Arc<Engine>, f: &str, sec: Option<&str>, args: &[(&str, &str)]) -> Screen {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let req = ScreenRequest {
        function: f.into(),
        security: sec.map(|s| s.parse().unwrap()),
        args: args.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect(),
    };
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

fn table_titles(s: &Screen) -> Vec<String> {
    s.blocks
        .iter()
        .filter_map(|b| match b {
            Block::Table(t) => Some(t.title.clone().unwrap_or_default()),
            _ => None,
        })
        .collect()
}

fn quotes_only() -> Fake {
    Fake { id: "fake-quotes", caps: caps(&[Capability::Quotes], FeedSource::SingleVenue("IEX".into())), dividends: None, fundamentals: None }
}

// --- WEI ----------------------------------------------------------------------

#[test]
fn wei_shows_labelled_etf_proxies_when_no_source_has_index_levels() {
    let e = engine(vec![quotes_only()]);
    let s = screen(&e, "WEI", None, &[]);
    assert!(matches!(s.status, ScreenStatus::Ok), "{:?}", s.status);
    assert_eq!(
        notices(&s),
        vec![(
            NoticeLevel::Info,
            "Index levels aren't available from free sources; rows show ETF proxies, whose % change approximates the index's."
                .to_owned()
        )]
    );
    assert_eq!(s.sources.len(), 1);
    assert_eq!(s.sources[0].provider, "fake-quotes");
    let Some(Block::Table(t)) = s.blocks.iter().find(|b| matches!(b, Block::Table(_))) else { panic!("table") };
    let titles: Vec<&str> = t.columns.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(&titles[..3], &["Index", "Proxy", "ETF Last"]);
    let row = |index: &str| {
        t.rows.iter().find(|r| r.cells[0].text.as_deref() == Some(index)).unwrap_or_else(|| panic!("row {index}"))
    };
    let spx = row("S&P 500");
    assert_eq!(spx.security.as_deref(), Some("SPY US Equity"));
    assert!(spx.cells[1].text.as_deref().unwrap().starts_with("SPY"));
    assert_eq!(spx.action.as_ref().unwrap().function, "GP");
    assert_eq!(row("Nikkei 225").security.as_deref(), Some("EWJ US Equity"));
    assert_eq!(row("Nasdaq Composite").security.as_deref(), Some("ONEQ US Equity"));
    // Every index row (not region headers) is bound to an ETF.
    let index_rows: Vec<_> = t.rows.iter().filter(|r| !r.emphasis).collect();
    assert_eq!(index_rows.len(), 24);
    assert!(index_rows.iter().all(|r| r.security.as_deref().is_some_and(|k| k.ends_with(" US Equity"))));
    e.shutdown();
}

#[test]
fn wei_without_index_or_etf_quotes_is_not_available() {
    let e = engine(vec![Fake { id: "fake-filings", caps: caps(&[Capability::Dividends], FeedSource::Official), dividends: None, fundamentals: None }]);
    let s = screen(&e, "WEI", None, &[]);
    let ScreenStatus::NotAvailable { reason } = &s.status else { panic!("{:?}", s.status) };
    assert!(reason.contains("ETFs used as proxies"), "{reason}");
    e.shutdown();
}

// --- DVD ------------------------------------------------------------------------

fn cash(ex: NaiveDate, amount: f64, currency: &str) -> Dividend {
    Dividend {
        declared_date: None,
        ex_date: ex,
        record_date: Some(ex),
        pay_date: Some(ex + chrono::Duration::days(14)),
        amount,
        currency: currency.into(),
        frequency: None,
        kind: DividendKind::Regular,
    }
}

fn events_source(result: ProviderResult<Vec<Dividend>>) -> Fake {
    Fake {
        id: "fake-events",
        caps: caps(&[Capability::Quotes, Capability::Dividends], FeedSource::Aggregated),
        dividends: Some(result.map(|dividends| Dividends {
            key: SecurityKey::equity("X"),
            dividends,
            per_period: vec![],
            reported_splits: vec![],
            provenance: prov("fake-events"),
        })),
        fundamentals: None,
    }
}

fn period(period_type: PeriodType, fy: i32, fp: &str, end: NaiveDate, declared: f64) -> PeriodDividend {
    PeriodDividend {
        period_type,
        fiscal_year: fy,
        fiscal_period: fp.into(),
        period_end: end,
        declared_per_share: Some(declared),
        paid_per_share: None,
        currency: "USD".into(),
    }
}

fn filings_source(result: ProviderResult<()>, splits: Vec<ReportedSplit>) -> Fake {
    Fake {
        id: "fake-filings",
        caps: caps(&[Capability::Dividends, Capability::Fundamentals], FeedSource::Official),
        dividends: Some(result.map(|()| Dividends {
            key: SecurityKey::equity("X"),
            dividends: vec![],
            per_period: vec![
                period(PeriodType::Annual, 2025, "FY", d(2025, 12, 31), 0.50),
                period(PeriodType::Annual, 2024, "FY", d(2024, 12, 31), 0.44),
                period(PeriodType::Quarterly, 2026, "Q2", d(2026, 6, 30), 0.13),
                period(PeriodType::Quarterly, 2026, "Q1", d(2026, 3, 31), 0.13),
            ],
            reported_splits: splits,
            provenance: Provenance { source: FeedSource::Official, ..prov("fake-filings") },
        })),
        fundamentals: None,
    }
}

fn split_2025() -> ReportedSplit {
    ReportedSplit { ratio: 2.0, period_start: Some(d(2025, 6, 2)), period_end: d(2025, 6, 2), filed: d(2025, 7, 31) }
}

#[test]
fn dvd_merges_event_and_per_period_sources() {
    let events = vec![cash(d(2026, 9, 12), 0.13, "USD"), cash(d(2026, 6, 12), 0.13, "USD"), cash(d(2025, 12, 12), 0.13, "")];
    let e = engine(vec![events_source(Ok(events)), filings_source(Ok(()), vec![split_2025()])]);
    let s = screen(&e, "DVD", Some("TSTX US Equity"), &[]);
    assert!(matches!(s.status, ScreenStatus::Ok), "{:?}", s.status);
    let badges: Vec<&str> = s.sources.iter().map(|b| b.provider.as_str()).collect();
    assert_eq!(badges, vec!["fake-events", "fake-filings"]);
    let titles = table_titles(&s);
    assert_eq!(titles.len(), 4, "{titles:?}");
    assert!(titles[1].starts_with("Dividends per Share by Fiscal Year (USD"));
    assert!(titles[2].starts_with("Dividends per Share by Fiscal Quarter"));
    assert!(titles[3].starts_with("Stock Splits Reported in Filings"));
    // The split falls inside the per-period window.
    let n = notices(&s);
    assert_eq!(n.len(), 1, "{n:?}");
    assert_eq!(n[0].0, NoticeLevel::Warning);
    assert!(n[0].1.ends_with("Per-share values are as reported; periods before the split are not adjusted."), "{}", n[0].1);
    // One TTM event has no stated currency: no total across currencies, no yield.
    let Some(Block::Fields { fields, .. }) = s.blocks.first() else { panic!("fields") };
    let ttm = fields.iter().find(|f| f.label == "Div (TTM)").unwrap();
    assert_eq!(ttm.text.as_deref(), Some("n/a — mixed currencies"));
    assert!(!fields.iter().any(|f| f.label == "Yield (TTM) %"));
    e.shutdown();
}

#[test]
fn dvd_marks_the_missing_part_when_one_source_fails() {
    let unauthorized = ProviderError::Unauthorized("Alpaca API key not set — add it in Settings".into());
    let e = engine(vec![events_source(Err(unauthorized)), filings_source(Ok(()), vec![])]);
    let s = screen(&e, "DVD", Some("TSTX US Equity"), &[]);
    assert!(matches!(s.status, ScreenStatus::Ok), "{:?}", s.status);
    assert_eq!(
        notices(&s),
        vec![(
            NoticeLevel::Warning,
            "Ex-dates, record and pay dates: NOT AVAILABLE — fake-events: Alpaca API key not set — add it in Settings".to_owned()
        )]
    );
    // Summary from the latest fiscal year instead of TTM events.
    let Some(Block::Fields { fields, .. }) = s.blocks.first() else { panic!("fields") };
    assert_eq!(fields[1].label, "DPS Declared");
    assert_eq!(fields[1].value, Some(0.5));
    assert_eq!(table_titles(&s).len(), 2);
    e.shutdown();

    // The other way round: events shown, per-period part NOT AVAILABLE.
    let no_contact = ProviderError::Unauthorized("Set your contact name and email for SEC EDGAR in Settings".into());
    let e = engine(vec![events_source(Ok(vec![cash(d(2026, 9, 12), 0.13, "USD")])), filings_source(Err(no_contact), vec![])]);
    let s = screen(&e, "DVD", Some("TSTX US Equity"), &[]);
    let n = notices(&s);
    assert_eq!(n.len(), 1, "{n:?}");
    assert!(n[0].1.starts_with("Dividends per share by fiscal period: NOT AVAILABLE — fake-filings: Set your contact"), "{}", n[0].1);
    let Some(Block::Fields { fields, .. }) = s.blocks.first() else { panic!("fields") };
    let ttm = fields.iter().find(|f| f.label == "Div (TTM)").unwrap();
    assert_eq!(ttm.value, Some(0.13));
    e.shutdown();
}

#[test]
fn dvd_non_payer_says_none_rather_than_not_available() {
    let not_found = ProviderError::NotFound("TSTX reports no dividends per share in its XBRL financial data".into());
    let e = engine(vec![events_source(Ok(vec![])), filings_source(Err(not_found), vec![])]);
    let s = screen(&e, "DVD", Some("TSTX US Equity"), &[]);
    assert!(matches!(s.status, ScreenStatus::Ok), "{:?}", s.status);
    let n = notices(&s);
    assert_eq!(n.len(), 2, "{n:?}");
    assert!(n.iter().all(|(l, _)| *l == NoticeLevel::Info));
    assert!(n[0].1.starts_with("No dividend or split events"));
    assert!(n[1].1.contains("none reported — fake-filings: Not found: TSTX reports no dividends per share"), "{}", n[1].1);
    e.shutdown();
}

#[test]
fn dvd_is_not_available_when_every_source_fails() {
    let e = engine(vec![
        events_source(Err(ProviderError::Unauthorized("Alpaca API key not set — add it in Settings".into()))),
        filings_source(Err(ProviderError::Unauthorized("Set your contact name and email for SEC EDGAR in Settings".into())), vec![]),
    ]);
    let s = screen(&e, "DVD", Some("TSTX US Equity"), &[]);
    let ScreenStatus::NotAvailable { reason } = &s.status else { panic!("{:?}", s.status) };
    assert!(reason.contains("fake-events: Alpaca API key not set") && reason.contains("fake-filings: Set your contact"), "{reason}");
    e.shutdown();
}

// --- FA -----------------------------------------------------------------------

fn income(fy: i32, end: NaiveDate, eps: f64) -> Statement {
    Statement {
        kind: StatementKind::Income,
        period_type: PeriodType::Annual,
        fiscal_year: fy,
        fiscal_period: "FY".into(),
        period_end: end,
        currency: "USD".into(),
        lines: vec![StatementLine { code: "eps_diluted".into(), label: "EPS (Diluted)".into(), value: Some(eps), depth: 1, source_tag: None }],
    }
}

fn fa_source(splits: Vec<ReportedSplit>) -> Fake {
    Fake {
        id: "fake-filings",
        caps: caps(&[Capability::Fundamentals], FeedSource::Official),
        dividends: None,
        fundamentals: Some(Fundamentals {
            key: SecurityKey::equity("X"),
            statements: vec![income(2025, d(2025, 12, 31), 2.4), income(2024, d(2024, 12, 31), 2.0), income(2023, d(2023, 12, 31), 3.6)],
            reported_splits: splits,
            provenance: prov("fake-filings"),
        }),
    }
}

const SPLIT_TEXT: &str = "Per-share values are as reported; periods before the split are not adjusted.";

#[test]
fn fa_flags_a_split_inside_the_displayed_window() {
    let e = engine(vec![fa_source(vec![split_2025()])]);
    let is = screen(&e, "FA", Some("TSTX US Equity"), &[]);
    let n = notices(&is);
    assert_eq!(n.len(), 1, "{n:?}");
    assert_eq!(n[0].0, NoticeLevel::Warning);
    assert!(n[0].1.contains("conversion ratio 2 for 06/02/2025") && n[0].1.ends_with(SPLIT_TEXT), "{}", n[0].1);
    let ratios = screen(&e, "FA", Some("TSTX US Equity"), &[("stmt", "RATIOS")]);
    assert!(notices(&ratios).iter().any(|(_, t)| t.ends_with(SPLIT_TEXT)));
    // No per-share lines on the balance sheet: no notice.
    let bs = screen(&e, "FA", Some("TSTX US Equity"), &[("stmt", "BS")]);
    assert!(notices(&bs).iter().all(|(_, t)| !t.ends_with(SPLIT_TEXT)));
    e.shutdown();

    // A split before the oldest displayed period doesn't affect comparability.
    let old = ReportedSplit { ratio: 7.0, period_start: None, period_end: d(2014, 6, 9), filed: d(2014, 7, 23) };
    let e = engine(vec![fa_source(vec![old])]);
    let is = screen(&e, "FA", Some("TSTX US Equity"), &[]);
    assert!(notices(&is).is_empty());
    e.shutdown();
}

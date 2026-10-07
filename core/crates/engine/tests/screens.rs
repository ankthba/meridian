//! Snapshot tests for every function screen: each screen is built from the
//! mock provider with a frozen clock, serialized, and compared with the
//! committed baseline in `tests/snapshots/`. Set `UPDATE_SNAPSHOTS=1` to
//! re-record after an intentional change and review the diff.

use std::sync::Arc;

use meridian_engine::screens::ScreenRequest;
use meridian_engine::screen::{Screen, ScreenStatus};
use meridian_engine::{DataMode, Engine, EngineConfig, NullEvents};
use meridian_provider::Provider;
use meridian_provider_mock::{MockConfig, MockProvider};
use meridian_types::{FixedClock, SecurityKey};

/// 2026-10-05 16:00 UTC (12:00 New York, market open).
const CLOCK: i64 = 1_791_216_000_000_000_000;

fn engine() -> Arc<Engine> {
    let mock: Arc<dyn Provider> = Arc::new(MockProvider::new(MockConfig {
        seed: 42,
        clock: Arc::new(FixedClock(CLOCK)),
        extra_symbols: 0,
        updates_per_symbol_per_sec: 0.0,
    }));
    Engine::new(&EngineConfig::test(DataMode::Mock, CLOCK), vec![mock], Arc::new(NullEvents)).expect("engine")
}

fn req(f: &str, sec: Option<&str>, args: &[(&str, &str)]) -> ScreenRequest {
    ScreenRequest {
        function: f.into(),
        security: sec.map(|s| s.parse::<SecurityKey>().expect("key")),
        args: args.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect(),
    }
}

fn cases() -> Vec<(&'static str, ScreenRequest)> {
    let aapl = Some("AAPL US Equity");
    vec![
        ("des_aapl", req("DES", aapl, &[])),
        ("menu_aapl", req("MENU", aapl, &[])),
        ("hp_aapl", req("HP", aapl, &[])),
        ("hp_aapl_weekly", req("HP", aapl, &[("period", "Weekly")])),
        ("gp_aapl", req("GP", aapl, &[])),
        ("gip_aapl", req("GIP", aapl, &[])),
        ("n", req("N", None, &[])),
        ("top", req("TOP", None, &[])),
        ("cn_aapl", req("CN", aapl, &[])),
        ("cf_aapl", req("CF", aapl, &[])),
        ("fa_msft_is", req("FA", Some("MSFT US Equity"), &[])),
        ("fa_msft_bs_q", req("FA", Some("MSFT US Equity"), &[("stmt", "BS"), ("per", "Quarterly")])),
        ("fa_msft_ratios", req("FA", Some("MSFT US Equity"), &[("stmt", "RATIOS")])),
        ("ee_nvda", req("EE", Some("NVDA US Equity"), &[])),
        ("ern_nvda", req("ERN", Some("NVDA US Equity"), &[])),
        ("anr_aapl", req("ANR", aapl, &[])),
        ("hds_aapl", req("HDS", aapl, &[])),
        ("dvd_ko", req("DVD", Some("KO US Equity"), &[])),
        ("omon_aapl", req("OMON", aapl, &[])),
        ("ovdv_spy", req("OVDV", Some("SPY US Equity"), &[])),
        ("ovme_aapl", req("OVME", aapl, &[])),
        ("ovme_aapl_condor_binomial", req("OVME", aapl, &[("strategy", "Iron Condor"), ("model", "Binomial")])),
        ("eqs", req("EQS", None, &[("min_cap_b", "100")])),
        ("rv_msft", req("RV", Some("MSFT US Equity"), &[])),
        ("corr", req("CORR", None, &[])),
        ("btst_spy", req("BTST", Some("SPY US Equity"), &[])),
        ("port", req("PORT", None, &[])),
        ("alrt", req("ALRT", None, &[])),
        ("wei", req("WEI", None, &[])),
        ("eco", req("ECO", None, &[])),
        ("eco_cpi", req("ECO", None, &[("series", "CPIAUCSL")])),
        ("eco_curve", req("ECO", None, &[("view", "curve")])),
        ("fxc", req("FXC", None, &[])),
        ("cryp", req("CRYP", None, &[])),
        ("w", req("W", None, &[])),
        ("most", req("MOST", None, &[])),
        ("secf_apple", req("SECF", None, &[("q", "apple")])),
        ("help", req("HELP", None, &[])),
        ("help_gp", req("HELP", None, &[("topic", "GP")])),
        ("today", req("TODAY", None, &[])),
        ("calendar", req("CALENDAR", None, &[])),
        ("calendar_month_all", req("CALENDAR", None, &[("range", "month"), ("scope", "all"), ("importance", "all")])),
        ("filings", req("FILINGS", None, &[])),
    ]
}

/// Stable JSON for comparison (floats rounded to remove last-bit noise).
fn normalize(s: &Screen) -> String {
    fn round(v: serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Number(n) if n.is_f64() => {
                let x = n.as_f64().unwrap_or(0.0);
                serde_json::json!((x * 1e6).round() / 1e6)
            }
            serde_json::Value::Array(a) => serde_json::Value::Array(a.into_iter().map(round).collect()),
            serde_json::Value::Object(o) => serde_json::Value::Object(o.into_iter().map(|(k, v)| (k, round(v))).collect()),
            other => other,
        }
    }
    serde_json::to_string_pretty(&round(serde_json::to_value(s).expect("json"))).expect("pretty")
}

/// Compares `screen` with `tests/snapshots/<name>.json` (writing it when
/// missing or `UPDATE_SNAPSHOTS` is set). Returns false on a difference and
/// leaves `<name>.new.json` to review.
fn matches_snapshot(name: &str, screen: &Screen) -> bool {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    let got = normalize(screen);
    let path = dir.join(format!("{name}.json"));
    if std::env::var("UPDATE_SNAPSHOTS").is_ok() || !path.exists() {
        std::fs::write(&path, &got).expect("write snapshot");
        return true;
    }
    let want = std::fs::read_to_string(&path).expect("read snapshot");
    if want == got {
        return true;
    }
    std::fs::write(dir.join(format!("{name}.new.json")), &got).expect("write new");
    false
}

#[test]
fn every_function_screen_matches_snapshot() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("rt");
    let engine = engine();
    rt.block_on(engine.refresh_universe());
    let mut failures = Vec::new();
    for (name, r) in cases() {
        let screen = rt.block_on(engine.screen(r.clone()));
        assert!(
            matches!(screen.status, ScreenStatus::Ok),
            "{name}: expected Ok in MOCK mode, got {:?}",
            screen.status
        );
        assert!(!screen.blocks.is_empty(), "{name}: no blocks");
        if !matches_snapshot(name, &screen) {
            failures.push(name);
        }
    }
    engine.shutdown();
    assert!(failures.is_empty(), "snapshots differ (see *.new.json): {failures:?}");
}

fn tables(s: &Screen) -> Vec<&meridian_engine::screen::Table> {
    s.blocks
        .iter()
        .filter_map(|b| match b {
            meridian_engine::screen::Block::Table(t) => Some(t),
            _ => None,
        })
        .collect()
}

fn table<'a>(s: &'a Screen, prefix: &str) -> &'a meridian_engine::screen::Table {
    tables(s)
        .into_iter()
        .find(|t| t.title.as_deref().is_some_and(|x| x.starts_with(prefix)))
        .unwrap_or_else(|| panic!("no table titled {prefix}…"))
}

/// TODAY with a portfolio (MOCK data), and the FILINGS read state: opening
/// a filing in CF marks it read; "Mark all read" clears the inbox.
#[test]
fn today_with_holdings_and_filing_read_state() {
    use meridian_engine::screen::Field;
    use meridian_store::Transaction;

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("rt");
    let engine = engine();
    rt.block_on(engine.refresh_universe());
    let store = &engine.stores().app;
    let pid = store.create_portfolio("Brokerage", "USD").expect("portfolio");
    let tx = |sec: &str, date: &str, qty: f64, price: f64| Transaction {
        id: 0,
        portfolio_id: pid,
        security: sec.into(),
        trade_date: date.into(),
        quantity: qty,
        price,
        fees: 0.0,
        note: None,
    };
    for t in [
        tx("AAPL US Equity", "2025-03-03", 20.0, 180.0),
        tx("MSFT US Equity", "2025-06-02", 8.0, 410.0),
        tx("KO US Equity", "2026-10-05", 50.0, 79.25),
        tx("BTCUSD Curncy", "2024-01-08", 0.05, 45_000.0),
    ] {
        store.add_transaction(&t).expect("transaction");
    }

    let today = rt.block_on(engine.screen(req("TODAY", None, &[])));
    assert!(matches!(today.status, ScreenStatus::Ok), "{:?}", today.status);
    let mut failures = Vec::new();
    if !matches_snapshot("today_holdings", &today) {
        failures.push("today_holdings");
    }
    // The summary adds up the holdings table.
    let holdings = table(&today, "Holdings");
    assert_eq!(holdings.rows.len(), 4);
    let value_col = holdings.columns.iter().position(|c| c.title == "Value").expect("value column");
    let sum: f64 = holdings.rows.iter().filter_map(|r| r.cells[value_col].value).sum();
    let summary: Vec<&Field> = today
        .blocks
        .iter()
        .find_map(|b| match b {
            meridian_engine::screen::Block::Fields { title: Some(t), fields, .. } if t == "Portfolio" => Some(fields.iter().collect()),
            _ => None,
        })
        .expect("portfolio fields");
    assert!((summary[0].value.unwrap() - sum).abs() < 1e-6, "value {:?} vs rows {sum}", summary[0].value);
    assert!(today.menu.iter().all(|m| m.action.function != "IMPORT"), "no import prompt with a portfolio");

    // FILINGS: open the newest unread filing, then mark all read.
    let inbox = rt.block_on(engine.screen(req("FILINGS", None, &[])));
    let t = table(&inbox, "Last 30 days");
    let first = t.rows.first().expect("a filing").clone();
    let unread_before = t.rows.iter().filter(|r| r.cells[0].text.is_some()).count();
    assert!(unread_before > 0);
    let open = first.action.expect("row opens CF");
    assert_eq!(open.function, "CF");
    let args: Vec<(&str, &str)> = open.args.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let cf = rt.block_on(engine.screen(req("CF", open.security.as_deref(), &args)));
    assert!(matches!(cf.status, ScreenStatus::Ok), "{:?}", cf.status);
    let inbox = rt.block_on(engine.screen(req("FILINGS", None, &[])));
    let after = table(&inbox, "Last 30 days").rows.iter().filter(|r| r.cells[0].text.is_some()).count();
    assert_eq!(after, unread_before - 1);
    let mark = inbox.menu.iter().find(|m| m.label == "Mark all read").expect("mark all read").action.clone();
    let args: Vec<(&str, &str)> = mark.args.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let cleared = rt.block_on(engine.screen(req("FILINGS", None, &args)));
    assert!(table(&cleared, "Last 30 days").rows.iter().all(|r| r.cells[0].text.is_none()));
    assert!(cleared.menu.iter().all(|m| m.label != "Mark all read"));
    let unread_only = rt.block_on(engine.screen(req("FILINGS", None, &[("show", "Unread")])));
    assert!(tables(&unread_only).is_empty());
    engine.shutdown();
    assert!(failures.is_empty(), "snapshots differ (see *.new.json): {failures:?}");
}

#[test]
fn a_listed_story_opens_and_an_unknown_one_says_so() {
    use meridian_engine::screen::Block;
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("rt");
    let engine = engine();
    let top = rt.block_on(engine.screen(req("TOP", None, &[])));
    let action = top
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Table(t) => t.rows.first().and_then(|r| r.action.clone()),
            _ => None,
        })
        .expect("a story row");
    let args: Vec<(&str, &str)> = action.args.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let story = rt.block_on(engine.screen(req(&action.function, None, &args)));
    assert!(matches!(story.status, ScreenStatus::Ok), "{:?}", story.status);
    let gone = rt.block_on(engine.screen(req("TOP", None, &[("story", "no-such-story")])));
    assert!(matches!(gone.status, ScreenStatus::NotAvailable { .. }), "{:?}", gone.status);
    engine.shutdown();
}

#[test]
fn unknown_function_is_not_available() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("rt");
    let engine = engine();
    let s = rt.block_on(engine.screen(req("ZZZ", None, &[])));
    assert!(matches!(s.status, ScreenStatus::NotAvailable { .. }));
    engine.shutdown();
}

#[test]
fn mock_and_live_providers_never_mix() {
    // A real provider id in MOCK mode must be rejected at construction.
    struct Fake(meridian_provider::Capabilities);
    #[async_trait::async_trait]
    impl Provider for Fake {
        fn id(&self) -> meridian_types::ProviderId {
            meridian_types::ProviderId::new("coinbase")
        }
        fn capabilities(&self) -> &meridian_provider::Capabilities {
            &self.0
        }
    }
    let caps = meridian_provider::Capabilities {
        entries: vec![],
        rate_limit: None,
        max_stream_symbols: None,
        cache_policy: meridian_provider::CachePolicy::Unrestricted,
        attribution: None,
        display_allowed: true,
        ai_policy: meridian_provider::AiPolicy::Allowed,
        requires_credentials: false,
        terms_note: String::new(),
        docs_url: String::new(),
    };
    let r = Engine::new(&EngineConfig::test(DataMode::Mock, CLOCK), vec![Arc::new(Fake(caps))], Arc::new(NullEvents));
    assert!(r.is_err());
}

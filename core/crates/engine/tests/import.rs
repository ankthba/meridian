//! Broker CSV import end to end: preview, commit with de-duplication, and
//! the PORT values that result. Uses the hand-built fixtures of
//! `meridian-import` and the mock provider with a frozen clock.

use std::sync::Arc;

use meridian_engine::import::{ImportTarget, security_for};
use meridian_engine::screen::{Block, Screen, ScreenStatus};
use meridian_engine::screens::ScreenRequest;
use meridian_engine::{DataMode, Engine, EngineConfig, EngineError, NullEvents};
use meridian_import::{ColumnMapping, Format, TransactionKind};
use meridian_provider::Provider;
use meridian_provider_mock::{MockConfig, MockProvider};
use meridian_types::FixedClock;

/// 2026-10-05 16:00 UTC.
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

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../import/tests/fixtures").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn port(engine: &Arc<Engine>, rt: &tokio::runtime::Runtime, pid: i64) -> Screen {
    let req = ScreenRequest { function: "PORT".into(), security: None, args: vec![("portfolio".into(), pid.to_string())] };
    let s = rt.block_on(engine.screen(req));
    assert!(matches!(s.status, ScreenStatus::Ok), "{:?}", s.status);
    s
}

fn field(s: &Screen, label: &str) -> f64 {
    s.blocks
        .iter()
        .find_map(|b| match b {
            Block::Fields { fields, .. } => fields.iter().find(|f| f.label == label).and_then(|f| f.value),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no field {label}"))
}

fn notices(s: &Screen) -> Vec<String> {
    s.blocks
        .iter()
        .filter_map(|b| match b {
            Block::Notice { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Positions table: security → (qty, avg cost).
fn positions(s: &Screen) -> Vec<(String, f64, Option<f64>)> {
    s.blocks
        .iter()
        .find_map(|b| match b {
            Block::Table(t) if t.title.as_deref() == Some("Positions") => Some(
                t.rows.iter().map(|r| (r.cells[0].text.clone().unwrap_or_default(), r.cells[1].value.unwrap_or(f64::NAN), r.cells[2].value)).collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn robinhood_import_feeds_port_and_skips_duplicates() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("rt");
    let engine = engine();
    let csv = fixture("robinhood_activity.csv");

    let preview = engine.preview_import(&csv, None).expect("preview");
    let parsed = preview.parsed.as_ref().expect("recognized");
    assert_eq!(parsed.format, Format::Robinhood);
    assert_eq!(parsed.transactions.len(), 12);
    assert_eq!(preview.headers.len(), 9);

    let first = engine.commit_import(&csv, None, &ImportTarget::New("Robinhood".into())).expect("commit");
    assert_eq!((first.imported, first.duplicates, first.warnings.len()), (12, 0, 2));
    assert_eq!(first.portfolio_name, "Robinhood");
    let again = engine.commit_import(&csv, None, &ImportTarget::Existing(first.portfolio_id)).expect("again");
    assert_eq!((again.imported, again.duplicates), (0, 12));

    let s = port(&engine, &rt, first.portfolio_id);
    // AAPL: 10 @ 180 + 0.013 reinvested for 2.50, then 5 sold for 999.97.
    let avg = 1802.5 / 10.013;
    let aapl_cost = 1802.5 - 5.0 * avg;
    assert!(near(field(&s, "Cost basis"), aapl_cost + 2000.0 + 150.0));
    assert!(near(field(&s, "Realized P&L"), 999.97 - 5.0 * avg));
    assert!(near(field(&s, "Dividends"), 2.5));
    assert!(near(field(&s, "Interest"), 1.23));
    assert!(near(field(&s, "Fees and taxes"), 5.0));
    assert!(near(field(&s, "Net deposits"), 4500.0));
    let pos = positions(&s);
    let get = |k: &str| pos.iter().find(|p| p.0 == k).cloned().unwrap_or_else(|| panic!("{k} in {pos:?}"));
    // NVDA 4 shares split +36: 40 shares, cost unchanged.
    assert_eq!(get("NVDA US Equity").1, 40.0);
    assert!(near(get("NVDA US Equity").2.unwrap(), 50.0));
    // ABCD reverse split 1:20: 100 → 5 shares at the same total cost.
    assert_eq!(get("ABCD US Equity").1, 5.0);
    assert!(near(get("ABCD US Equity").2.unwrap(), 30.0));
    assert!(near(get("AAPL US Equity").1, 5.013));
    // ABCD has no mock quote: its value is left out and said so.
    assert!(notices(&s).iter().any(|n| n.starts_with("No current price for ABCD US Equity")), "{:?}", notices(&s));
    // Total return = unrealized (priced, cost known) + realized + income - fees.
    let unrealized = field(&s, "Unrealized P&L");
    assert!(near(field(&s, "Total return"), unrealized + field(&s, "Realized P&L") + 2.5 + 1.23 - 5.0));

    // A later, overlapping report adds only its new rows.
    let later = engine.commit_import(&fixture("robinhood_activity_later.csv"), None, &ImportTarget::Existing(first.portfolio_id)).expect("later");
    assert_eq!((later.imported, later.duplicates), (2, 2));
    let s = port(&engine, &rt, first.portfolio_id);
    assert!(near(field(&s, "Net deposits"), 4750.0));
    engine.shutdown();
}

#[test]
fn schwab_import_flags_transfers_without_cost() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("rt");
    let engine = engine();
    let out = engine.commit_import(&fixture("schwab_2024.csv"), None, &ImportTarget::New("Schwab".into())).expect("commit");
    assert_eq!(out.format, Format::Schwab);
    assert_eq!((out.imported, out.warnings.len()), (12, 2));
    let s = port(&engine, &rt, out.portfolio_id);
    // AVGO: 1 @ 1500, +9 split, 5 sold for 877.48: realized 877.48 - 750.
    assert!(near(field(&s, "Realized P&L"), 127.48));
    assert!(near(field(&s, "Dividends"), 1.8));
    assert!(near(field(&s, "Interest"), 0.87));
    assert!(near(field(&s, "Fees and taxes"), 0.33));
    let pos = positions(&s);
    let vti = pos.iter().find(|p| p.0 == "VTI US Equity").expect("VTI");
    assert_eq!((vti.1, vti.2), (10.0, None));
    assert!(notices(&s).iter().any(|n| n.starts_with("Cost basis unknown for VTI US Equity")), "{:?}", notices(&s));
    // VTI's unknown cost stays out of the cost basis total.
    let avgo = 750.0;
    let abcd = 150.0;
    let bnd = 721.8;
    assert!(near(field(&s, "Cost basis"), avgo + abcd + bnd));
    engine.shutdown();
}

#[test]
fn positions_snapshot_into_new_and_existing_portfolios() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("rt");
    let engine = engine();
    let csv = fixture("positions_fidelity.csv");
    let out = engine.commit_import(&csv, None, &ImportTarget::New("Fidelity holdings".into())).expect("commit");
    assert_eq!((out.format, out.imported), (Format::Positions, 3));
    let s = port(&engine, &rt, out.portfolio_id);
    let pos = positions(&s);
    let brk = pos.iter().find(|p| p.0 == "BRK/B US Equity").expect("share class mapped to BRK/B");
    assert!(near(brk.2.unwrap(), 400.0));
    assert!(near(field(&s, "Cost basis"), 1800.0 + 1200.0));
    // The same snapshot again: all duplicates, nothing added, no warning.
    let again = engine.commit_import(&csv, None, &ImportTarget::Existing(out.portfolio_id)).expect("again");
    assert_eq!((again.imported, again.duplicates), (0, 3));
    assert!(!again.warnings.iter().any(|w| w.message.contains("may be counted twice")));
    // A snapshot added to a portfolio that has history says it may double count.
    let history = engine.commit_import(&fixture("schwab_2020.csv"), None, &ImportTarget::New("History".into())).expect("history");
    let mixed = engine.commit_import(&csv, None, &ImportTarget::Existing(history.portfolio_id)).expect("mixed");
    assert_eq!(mixed.imported, 3);
    assert!(mixed.warnings.iter().any(|w| w.message.starts_with("History already had 3 transactions") && w.message.contains("may be counted twice")));
    engine.shutdown();
}

#[test]
fn unrecognized_files_need_a_mapping() {
    let engine = engine();
    let csv = fixture("generic_mapped.csv");
    let preview = engine.preview_import(&csv, None).expect("preview");
    assert!(preview.parsed.is_none());
    assert_eq!(preview.header_line, 1);
    assert_eq!(preview.suggested_mapping.symbol, Some(2));
    assert!(preview.warnings[0].message.starts_with("Format not recognized"));
    assert!(matches!(engine.commit_import(&csv, None, &ImportTarget::New("X".into())), Err(EngineError::InvalidInput(_))));
    let mapping = ColumnMapping { day_first: true, description: Some(6), ..preview.suggested_mapping.clone() };
    let mapped = engine.preview_import(&csv, Some(&mapping)).expect("mapped preview");
    let p = mapped.parsed.expect("parsed");
    assert_eq!(p.format, Format::Mapped);
    assert_eq!(p.counts(), vec![(TransactionKind::Buy, 1), (TransactionKind::Sell, 1), (TransactionKind::Dividend, 1), (TransactionKind::Deposit, 1)]);
    let out = engine.commit_import(&csv, Some(&mapping), &ImportTarget::New("Mapped".into())).expect("commit");
    assert_eq!(out.imported, 4);
    engine.shutdown();
}

#[test]
fn targets_are_validated() {
    let engine = engine();
    let csv = fixture("schwab_2020.csv");
    let out = engine.commit_import(&csv, None, &ImportTarget::New("Taxable".into())).expect("commit");
    assert!(matches!(engine.commit_import(&csv, None, &ImportTarget::New("taxable".into())), Err(EngineError::InvalidInput(m)) if m.contains("already exists")));
    assert!(matches!(engine.commit_import(&csv, None, &ImportTarget::New("  ".into())), Err(EngineError::InvalidInput(_))));
    assert!(matches!(engine.commit_import(&csv, None, &ImportTarget::Existing(out.portfolio_id + 99)), Err(EngineError::InvalidInput(_))));
    assert!(matches!(engine.preview_import("", None), Err(EngineError::InvalidInput(_))));
    assert_eq!(security_for("brk.b").as_deref(), Some("BRK/B US Equity"));
    engine.shutdown();
}

#[test]
fn port_offers_import_and_explains_an_empty_portfolio() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("rt");
    let engine = engine();
    let s = rt.block_on(engine.screen(ScreenRequest::new("PORT", None)));
    let item = s.menu.iter().find(|m| m.label == "Import from broker…").expect("menu item");
    assert_eq!(item.action.function, "IMPORT");
    assert!(item.action.args.iter().any(|(k, _)| k == "portfolio"));
    assert!(notices(&s).iter().any(|n| n.contains("Import from broker…")));
    let help = rt.block_on(engine.screen(ScreenRequest::new("IMPORT", None)));
    assert!(matches!(help.status, ScreenStatus::Ok));
    engine.shutdown();
}

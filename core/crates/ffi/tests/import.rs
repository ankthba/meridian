//! The import FFI against an in-memory MOCK core: preview caps, commit and
//! de-duplication, the mapping path, and argument errors.

use std::fmt::Write as _;
use std::sync::Arc;

use meridian_ffi::{Core, CoreConfigFfi, CoreError, CoreEventFfi, CoreEvents, DataModeFfi, ImportKindFfi, ImportMappingFfi, SecretSource};

#[derive(Debug)]
struct NoSecrets;
impl SecretSource for NoSecrets {
    fn secret(&self, _provider: String, _field: String) -> Option<String> {
        None
    }
}

#[derive(Debug)]
struct Quiet;
impl CoreEvents for Quiet {
    fn on_event(&self, _event: CoreEventFfi) {}
}

fn core() -> Arc<Core> {
    let config = CoreConfigFfi {
        mode: DataModeFfi::Mock,
        data_dir: std::env::temp_dir().join("meridian-import-test").to_string_lossy().into_owned(),
        in_memory: true,
        fixed_clock_ns: Some(1_791_216_000_000_000_000),
        mock_seed: 42,
        mock_extra_symbols: 0,
        mock_update_rate: 0.0,
    };
    Core::new(config, Arc::new(NoSecrets), Arc::new(Quiet)).expect("core")
}

/// A Robinhood-layout file with `buys` distinct buys and `unknown` rows
/// with an unknown code (hand-built, invented values).
fn robinhood(buys: usize, unknown: usize) -> String {
    let mut s = String::from("\"Activity Date\",\"Process Date\",\"Settle Date\",\"Instrument\",\"Description\",\"Trans Code\",\"Quantity\",\"Price\",\"Amount\"\n");
    for i in 0..buys {
        let px = 100 + i;
        let _ = writeln!(s, "\"1/5/2026\",\"1/5/2026\",\"1/6/2026\",\"AAPL\",\"Apple\nCUSIP: 037833100\",\"Buy\",\"1\",\"${px}.00\",\"(${px}.00)\"");
    }
    for _ in 0..unknown {
        s.push_str("\"1/2/2026\",\"1/2/2026\",\"1/2/2026\",\"\",\"?\",\"ZZZZ\",\"\",\"\",\"$1.00\"\n");
    }
    s
}

#[tokio::test(flavor = "multi_thread")]
async fn preview_is_capped_and_commit_skips_duplicates() {
    let core = core();
    let csv = robinhood(120, 250);
    let p = core.preview_import(csv.clone(), None).await.expect("preview");
    assert!(p.recognized);
    assert_eq!(p.format.as_deref(), Some("robinhood"));
    assert_eq!(p.headers.len(), 9);
    assert_eq!((p.rows.len(), p.total_rows), (50, 120));
    assert_eq!((p.warnings.len(), p.warning_count), (200, 250));
    assert_eq!(p.counts.len(), 1);
    assert_eq!((p.counts[0].kind, p.counts[0].count), (ImportKindFfi::Buy, 120));
    assert_eq!(p.rows[0].security.as_deref(), Some("AAPL US Equity"));
    assert_eq!(p.rows[0].trade_date, "2026-01-05");
    assert_eq!(p.first_date.as_deref(), Some("2026-01-05"));

    let r = core.commit_import(csv.clone(), None, None, Some("Robinhood".into())).await.expect("commit");
    assert_eq!((r.imported, r.duplicates, r.warning_count), (120, 0, 250));
    assert_eq!(r.format, "robinhood");
    let again = core.commit_import(csv, None, Some(r.portfolio_id), None).await.expect("again");
    assert_eq!((again.imported, again.duplicates), (0, 120));
    core.shutdown().expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn unrecognized_files_go_through_a_mapping() {
    let core = core();
    let csv = "When,What,Ticker,Qty,Px\n2026-01-02,Buy,MSFT,2,400\n".to_owned();
    let p = core.preview_import(csv.clone(), None).await.expect("preview");
    assert!(!p.recognized);
    assert_eq!(p.format, None);
    assert_eq!(p.header_line, 1);
    assert_eq!(p.suggested_mapping.symbol, Some(2));
    assert!(matches!(core.commit_import(csv.clone(), None, None, Some("X".into())).await, Err(CoreError::InvalidInput { .. })));
    let mapping = ImportMappingFfi {
        header_line: 1,
        trade_date: Some(0),
        settle_date: None,
        symbol: Some(2),
        action: Some(1),
        quantity: Some(3),
        price: Some(4),
        amount: None,
        fees: None,
        currency: None,
        description: None,
        cost_basis: None,
        average_cost: None,
        default_currency: "USD".into(),
        day_first: false,
    };
    let m = core.preview_import(csv.clone(), Some(mapping.clone())).await.expect("mapped");
    assert_eq!((m.format.as_deref(), m.total_rows), (Some("mapped"), 1));
    let r = core.commit_import(csv, Some(mapping), None, Some("Mapped".into())).await.expect("commit");
    assert_eq!(r.imported, 1);
    core.shutdown().expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn exactly_one_target() {
    let core = core();
    let csv = robinhood(1, 0);
    assert!(matches!(core.commit_import(csv.clone(), None, None, None).await, Err(CoreError::InvalidInput { .. })));
    assert!(matches!(core.commit_import(csv.clone(), None, Some(1), Some("Y".into())).await, Err(CoreError::InvalidInput { .. })));
    assert!(matches!(core.commit_import(csv, None, Some(999), None).await, Err(CoreError::InvalidInput { .. })));
    assert!(matches!(core.preview_import(String::new(), None).await, Err(CoreError::InvalidInput { .. })));
    core.shutdown().expect("shutdown");
}

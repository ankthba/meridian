//! Format tests over the hand-built fixtures in `tests/fixtures` (see
//! `tests/fixtures/SOURCES.md`). Expected line numbers were cross-checked
//! with Python's `csv` module.

use chrono::NaiveDate;
use meridian_import::{ColumnMapping, Format, ImportError, ImportOptions, ImportedTx, ParsedImport, TransactionKind as K, detect, parse};

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn opts() -> ImportOptions {
    ImportOptions { as_of: d(2026, 10, 7) }
}

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

fn read(name: &str) -> ParsedImport {
    parse(&fixture(name), None, &opts()).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn count(p: &ParsedImport, k: K) -> usize {
    p.transactions.iter().filter(|t| t.kind == k).count()
}

fn find<'a>(p: &'a ParsedImport, k: K, sym: &str) -> Vec<&'a ImportedTx> {
    p.transactions.iter().filter(|t| t.kind == k && t.symbol.as_deref() == Some(sym)).collect()
}

fn warning_lines(p: &ParsedImport) -> Vec<u32> {
    p.warnings.iter().map(|w| w.line).collect()
}

fn close(a: Option<f64>, b: f64) -> bool {
    a.is_some_and(|a| (a - b).abs() < 1e-9)
}

#[test]
fn robinhood_activity_report() {
    let p = read("robinhood_activity.csv");
    assert_eq!(p.format, Format::Robinhood);
    assert_eq!(p.header_line, 1);
    assert_eq!(p.headers.len(), 9);
    // Unknown code (line 2) and the option (line 8) are warnings; the
    // disclaimer footer is not.
    assert_eq!(warning_lines(&p), vec![2, 8], "{:#?}", p.warnings);
    assert!(p.warnings[0].message.contains("unknown Trans Code 'XYZQ'"));
    assert!(p.warnings[1].message.contains("options are not supported"));
    assert_eq!(p.counts(), vec![(K::Buy, 3), (K::Sell, 1), (K::Dividend, 1), (K::ReinvestedDividend, 1), (K::Interest, 1), (K::Fee, 1), (K::Split, 2), (K::Deposit, 1), (K::Withdrawal, 1)]);
    // Chronological: the deposit (last line) comes first.
    assert_eq!(p.transactions[0].kind, K::Deposit);
    assert_eq!(p.transactions[0].trade_date, d(2026, 1, 2));
    assert!(close(p.transactions[0].amount, 5000.0));
    // Same-day buys keep their real order (file order reversed).
    let jan5: Vec<&str> = p.transactions.iter().filter(|t| t.trade_date == d(2026, 1, 5)).filter_map(|t| t.symbol.as_deref()).collect();
    assert_eq!(jan5, ["AAPL", "NVDA", "ABCD"]);
    let buy = find(&p, K::Buy, "AAPL")[0];
    assert!(close(buy.quantity, 10.0) && close(buy.price, 180.0) && close(buy.amount, -1800.0));
    assert_eq!(buy.settle_date, Some(d(2026, 1, 6)));
    assert_eq!(buy.description, "Apple CUSIP: 037833100");
    assert_eq!(buy.line, 23);
    let drip = find(&p, K::ReinvestedDividend, "AAPL")[0];
    assert!(close(drip.quantity, 0.013) && close(drip.amount, -2.5));
    let sell = find(&p, K::Sell, "AAPL")[0];
    assert!(close(sell.quantity, -5.0) && close(sell.amount, 999.97));
    // SPL: the additional shares.
    assert!(close(find(&p, K::Split, "NVDA")[0].quantity, 36.0));
    // SPR: "100S" leaves, "5" arrives; netted into one row.
    let spr = find(&p, K::Split, "ABCD");
    assert_eq!(spr.len(), 1);
    assert!(close(spr[0].quantity, -95.0));
    assert!(close(p.transactions.iter().find(|t| t.kind == K::Fee).and_then(|t| t.amount), -5.0));
    assert!(p.transactions.iter().all(|t| t.currency == "USD"));
}

#[test]
fn robinhood_reimport_fingerprints_match_on_overlap() {
    let a = read("robinhood_activity.csv");
    let b = read("robinhood_activity_later.csv");
    let fa: std::collections::HashSet<&str> = a.transactions.iter().map(|t| t.fingerprint.as_str()).collect();
    let shared = b.transactions.iter().filter(|t| fa.contains(t.fingerprint.as_str())).count();
    // The overlap is the withdrawal and the reverse split (the option and
    // the unknown code are warnings in both files); the two new rows differ.
    assert_eq!(shared, 2);
    assert_eq!(b.transactions.len(), 4);
    // Parsing twice gives identical fingerprints.
    assert_eq!(a.transactions.iter().map(|t| &t.fingerprint).collect::<Vec<_>>(), read("robinhood_activity.csv").transactions.iter().map(|t| &t.fingerprint).collect::<Vec<_>>());
    let unique: std::collections::HashSet<&str> = a.transactions.iter().map(|t| t.fingerprint.as_str()).collect();
    assert_eq!(unique.len(), a.transactions.len());
}

#[test]
fn fidelity_history_2025_layout() {
    let p = read("fidelity_history_2025.csv");
    assert_eq!(p.format, Format::Fidelity);
    assert_eq!(p.header_line, 3);
    // Option (6), unknown action (5); the core SPAXX reinvestment (10) is a
    // note. The disclaimer block after the data is not reported.
    let lines = warning_lines(&p);
    assert_eq!(lines, vec![5, 6, 10], "{:#?}", p.warnings);
    assert!(p.warnings[2].message.contains("core money-market"));
    assert_eq!(p.counts(), vec![(K::Buy, 3), (K::Sell, 1), (K::Dividend, 2), (K::ReinvestedDividend, 1), (K::Fee, 1), (K::Split, 1), (K::Deposit, 1), (K::Withdrawal, 1)]);
    let sell = find(&p, K::Sell, "VOO")[0];
    assert!(close(sell.quantity, -4.0) && close(sell.price, 550.0) && close(sell.amount, 2199.99) && close(sell.fees, 0.01));
    assert_eq!(sell.settle_date, Some(d(2025, 7, 16)));
    // The split row's Amount is a market value, not cash.
    let split = find(&p, K::Split, "ABCD")[0];
    assert!(close(split.quantity, 20.0) && split.amount.is_none());
    // Income on the core fund is kept.
    assert!(close(find(&p, K::Dividend, "SPAXX")[0].amount, 8.2));
    assert!(close(find(&p, K::Fee, "VXUS")[0].amount, -0.45));
}

#[test]
fn fidelity_all_accounts_2026_layout() {
    let p = read("fidelity_all_accounts_2026q1.csv");
    assert_eq!(p.format, Format::Fidelity);
    assert!(p.warnings.is_empty(), "{:#?}", p.warnings);
    // Price before Quantity is read by name.
    let schd = find(&p, K::Buy, "SCHD");
    assert_eq!(schd.len(), 3);
    assert!(schd.iter().all(|t| close(t.quantity, 10.0) && close(t.price, 27.5)));
    // Two identical rows in one account and one in another: all distinct
    // (the account is not hashed; the occurrence number tells them apart).
    let fps: std::collections::HashSet<&str> = schd.iter().map(|t| t.fingerprint.as_str()).collect();
    assert_eq!(fps.len(), 3);
    assert_eq!(schd.iter().filter(|t| t.account.as_deref() == Some("Z00000001")).count(), 2);
    // Reverse split legs (one under the old CUSIP) pair onto the ticker.
    let rs = find(&p, K::Split, "ABCD");
    assert_eq!(rs.len(), 1);
    assert!(close(rs[0].quantity, -95.0));
    // ACAT shares arrive without a cost basis.
    let acat = find(&p, K::TransferIn, "VTI")[0];
    assert!(close(acat.quantity, 15.0) && acat.price.is_none() && acat.cost_basis.is_none());
    assert_eq!((count(&p, K::Deposit), count(&p, K::Withdrawal), count(&p, K::Other)), (1, 1, 1));
}

#[test]
fn fidelity_single_account_file_overlaps_the_all_accounts_file() {
    // One account's history downloaded on its own has no account column;
    // the all-accounts download names the account. The same rows must
    // fingerprint the same, or importing both doubles them.
    let single = read("fidelity_history_z00000001_2026q1.csv");
    let all = read("fidelity_all_accounts_2026q1.csv");
    assert_eq!(single.format, Format::Fidelity);
    assert!(single.warnings.is_empty(), "{:#?}", single.warnings);
    assert_eq!(single.transactions.len(), 9);
    assert!(single.transactions.iter().all(|t| t.account.is_none()));
    // The account stays on the rows for display.
    assert!(all.transactions.iter().all(|t| t.account.is_some()));
    let in_all: std::collections::HashSet<&str> = all.transactions.iter().map(|t| t.fingerprint.as_str()).collect();
    let (shared, new): (Vec<&ImportedTx>, Vec<&ImportedTx>) = single.transactions.iter().partition(|t| in_all.contains(t.fingerprint.as_str()));
    // Everything but the December deposit, which is older than the
    // all-accounts file: both SCHD buys, the netted reverse split, the
    // journal pair, the ACAT, the ABCD buy and the cash in lieu.
    assert_eq!(shared.len(), 8, "{new:#?}");
    assert_eq!(new.len(), 1);
    assert_eq!((new[0].kind, new[0].trade_date), (K::Deposit, d(2025, 12, 15)));
}

#[test]
fn fidelity_short_sales_are_warnings_not_a_long_position() {
    let text = "Run Date,Action,Symbol,Description,Type,Quantity,Price ($),Commission ($),Fees ($),Accrued Interest ($),Amount ($),Settlement Date\n\
                03/10/2026,YOU BOUGHT SHORT COVER TESLA INC (TSLA) (Short),TSLA,TESLA INC,Short,10,200,,,,-2000.00,03/11/2026\n\
                03/02/2026,YOU SOLD SHORT SALE TESLA INC (TSLA) (Margin),TSLA,TESLA INC,Short,-10,250,,,,2500.00,03/03/2026\n\
                03/02/2026,YOU BOUGHT APPLE INC (AAPL) (Cash),AAPL,APPLE INC,Cash,1,200,,,,-200.00,03/03/2026\n";
    let p = parse(text, None, &opts()).unwrap();
    assert_eq!(p.format, Format::Fidelity);
    assert_eq!(p.counts(), vec![(K::Buy, 1)]);
    assert!(find(&p, K::Buy, "TSLA").is_empty() && find(&p, K::Sell, "TSLA").is_empty());
    assert_eq!(warning_lines(&p), vec![2, 3]);
    assert!(p.warnings.iter().all(|w| w.message.contains("short sales are not supported")), "{:#?}", p.warnings);
}

#[test]
fn fidelity_july_2026_layout() {
    let p = read("fidelity_history_2026q3.csv");
    assert_eq!(p.format, Format::Fidelity);
    assert_eq!(p.header_line, 1);
    assert!(p.warnings.is_empty(), "{:#?}", p.warnings);
    assert_eq!(p.transactions.len(), 3);
    let buy = find(&p, K::Buy, "IVV")[0];
    assert_eq!(buy.trade_date, d(2026, 7, 8));
    assert_eq!(buy.settle_date, Some(d(2026, 7, 9)));
    // The "as of" date inside the action is the event date.
    assert_eq!(find(&p, K::Dividend, "IVV")[0].trade_date, d(2026, 7, 15));
}

#[test]
fn schwab_2020_banner_trailing_commas_and_footer() {
    let p = read("schwab_2020.csv");
    assert_eq!(p.format, Format::Schwab);
    assert_eq!(p.header_line, 2);
    assert_eq!(p.headers.len(), 8);
    assert!(p.warnings.is_empty(), "{:#?}", p.warnings);
    assert_eq!(p.transactions.len(), 3);
    // "12/15/2020 as of 12/11/2020": the event date.
    assert_eq!(p.transactions.iter().find(|t| t.kind == K::Interest).map(|t| t.trade_date), Some(d(2020, 12, 11)));
    assert!(close(find(&p, K::Buy, "SCHB")[0].amount, -1094.40));
    let total: f64 = p.transactions.iter().filter_map(|t| t.amount).sum();
    assert!((total - 906.02).abs() < 1e-9, "matches the file's Transactions Total");
}

#[test]
fn schwab_2024_actions() {
    let p = read("schwab_2024.csv");
    assert_eq!(p.format, Format::Schwab);
    assert_eq!(warning_lines(&p), vec![12, 16], "{:#?}", p.warnings);
    // AVGO: buy 1, split +9 as of 07/12, sell 5 (unsigned in the file).
    let split = find(&p, K::Split, "AVGO")[0];
    assert_eq!(split.trade_date, d(2024, 7, 12));
    assert!(close(split.quantity, 9.0) && split.price.is_none());
    let sell = find(&p, K::Sell, "AVGO")[0];
    assert!(close(sell.quantity, -5.0) && close(sell.fees, 0.02) && close(sell.amount, 877.48));
    // Reverse split with the old leg under its CUSIP nets to -95 on ABCD.
    let rs = find(&p, K::Split, "ABCD");
    assert_eq!(rs.len(), 1);
    assert!(close(rs[0].quantity, -95.0));
    // Reinvestment pair: income row plus share purchase.
    assert!(close(find(&p, K::Dividend, "BND")[0].amount, 1.8));
    assert!(close(find(&p, K::ReinvestedDividend, "BND")[0].quantity, 0.0249));
    // Journaled shares: transfer in, valuation price dropped.
    let j = find(&p, K::TransferIn, "VTI")[0];
    assert!(close(j.quantity, 10.0) && j.price.is_none());
    assert!(close(p.transactions.iter().find(|t| t.kind == K::Deposit).and_then(|t| t.amount), 5000.0));
    assert_eq!((count(&p, K::Fee), count(&p, K::Interest)), (1, 1));
}

#[test]
fn vanguard_download() {
    let p = read("vanguard_ofx.csv");
    assert_eq!(p.format, Format::Vanguard);
    assert_eq!(p.header_line, 7);
    let msgs: Vec<(u32, &str)> = p.warnings.iter().map(|w| (w.line, w.message.as_str())).collect();
    assert_eq!(msgs.len(), 5, "{msgs:#?}");
    assert!(msgs[0].0 == 2 && msgs[0].1.contains("holdings summary (lines 2–3)"));
    assert!(msgs[1].0 == 9 && msgs[1].1.starts_with("2 settlement-fund sweep rows skipped") && msgs[1].1.ends_with("lines 9, 16"));
    assert!(msgs[2].0 == 15 && msgs[2].1.starts_with("1 settlement-fund reinvestments skipped"));
    assert!(msgs[3].0 == 21 && msgs[3].1.contains("options are not supported"));
    assert!(msgs[4].0 == 23 && msgs[4].1.contains("unknown Transaction Type 'Mystery'"));
    // Rows are grouped by holding in the file; output is by date.
    assert!(p.transactions.windows(2).all(|w| w[0].trade_date <= w[1].trade_date));
    assert_eq!(p.transactions[0].kind, K::Deposit);
    let sell = find(&p, K::Sell, "VTI")[0];
    assert!(close(sell.quantity, -4.0) && close(sell.amount, 1039.98) && close(sell.fees, 0.02) && close(sell.price, 260.0));
    // Share Price on income rows is ignored.
    assert!(find(&p, K::Dividend, "VTI")[0].price.is_none());
    let rs = find(&p, K::Split, "ABCD");
    assert_eq!(rs.len(), 1);
    assert!(close(rs[0].quantity, -95.0));
    assert_eq!(p.counts(), vec![(K::Buy, 2), (K::Sell, 1), (K::Dividend, 2), (K::ReinvestedDividend, 1), (K::Split, 1), (K::Deposit, 1), (K::Withdrawal, 1), (K::Other, 1)]);
    assert!(p.transactions.iter().all(|t| t.account.as_deref() == Some("12345678")));
}

#[test]
fn fidelity_positions_snapshot() {
    let p = read("positions_fidelity.csv");
    assert_eq!(p.format, Format::Positions);
    assert!(p.snapshot);
    assert_eq!(p.transactions.len(), 3);
    assert!(p.transactions.iter().all(|t| t.kind == K::TransferIn && t.trade_date == d(2026, 10, 7)));
    let aapl = find(&p, K::TransferIn, "AAPL")[0];
    assert!(close(aapl.quantity, 10.0) && close(aapl.cost_basis, 1800.0));
    assert!(find(&p, K::TransferIn, "VTI")[0].cost_basis.is_none(), "-- is a missing cost basis");
    assert!(close(find(&p, K::TransferIn, "BRK.B")[0].cost_basis, 1200.0));
    let msgs: Vec<&str> = p.warnings.iter().map(|w| w.message.as_str()).collect();
    assert_eq!(msgs.len(), 3, "{msgs:#?}");
    assert!(msgs[0].contains("SPAXX** is the account's core cash position"));
    assert!(msgs[1].contains("CUSIP"));
    assert!(msgs[2].contains("pending activity"));
}

#[test]
fn schwab_positions_snapshot() {
    let p = read("positions_schwab.csv");
    assert_eq!(p.format, Format::Positions);
    assert_eq!(p.header_line, 3);
    let schb = find(&p, K::TransferIn, "SCHB")[0];
    assert!(close(schb.quantity, 1200.0) && close(schb.cost_basis, 20400.0));
    assert!(find(&p, K::TransferIn, "MSFT")[0].cost_basis.is_none(), "Incomplete cost basis");
    assert_eq!(warning_lines(&p), vec![6, 7, 8]);
    assert!(p.warnings[0].message.contains("option"));
    assert!(p.warnings[1].message.contains("summary row"));
}

#[test]
fn snapshot_fingerprints_ignore_the_import_day() {
    let a = parse(&fixture("positions_fidelity.csv"), None, &opts()).unwrap();
    let b = parse(&fixture("positions_fidelity.csv"), None, &ImportOptions { as_of: d(2027, 1, 1) }).unwrap();
    let fa: Vec<&str> = a.transactions.iter().map(|t| t.fingerprint.as_str()).collect();
    let fb: Vec<&str> = b.transactions.iter().map(|t| t.fingerprint.as_str()).collect();
    assert_eq!(fa, fb);
}

fn generic_mapping() -> ColumnMapping {
    ColumnMapping {
        header_line: 1,
        trade_date: Some(0),
        action: Some(1),
        symbol: Some(2),
        quantity: Some(3),
        price: Some(4),
        amount: Some(5),
        description: Some(6),
        default_currency: "USD".into(),
        day_first: true,
        ..Default::default()
    }
}

#[test]
fn generic_mapping_path() {
    let text = fixture("generic_mapped.csv");
    // Not recognized without a mapping: the error carries the header.
    match parse(&text, None, &opts()) {
        Err(ImportError::Unrecognized { header_line, headers }) => {
            assert_eq!(header_line, 1);
            assert_eq!(headers, ["Date", "Type", "Ticker", "Shares", "Price", "Total", "Notes"]);
            let s = meridian_import::suggest_mapping(header_line, &headers);
            assert_eq!((s.trade_date, s.action, s.symbol, s.quantity, s.price, s.amount), (Some(0), Some(1), Some(2), Some(3), Some(4), Some(5)));
        }
        other => panic!("expected Unrecognized, got {other:?}"),
    }
    let p = parse(&text, Some(&generic_mapping()), &opts()).unwrap();
    assert_eq!(p.format, Format::Mapped);
    assert!(!p.snapshot);
    assert_eq!(p.counts(), vec![(K::Buy, 1), (K::Sell, 1), (K::Dividend, 1), (K::Deposit, 1)]);
    let buy = find(&p, K::Buy, "AAPL")[0];
    // DD/MM dates; a positive total on a buy becomes cash out.
    assert_eq!(buy.trade_date, d(2026, 1, 15));
    assert!(close(buy.amount, -1800.0) && close(buy.quantity, 10.0));
    assert!(close(find(&p, K::Sell, "AAPL")[0].quantity, -4.0));
    assert_eq!(warning_lines(&p), vec![6, 7, 8], "{:#?}", p.warnings);
    assert!(p.warnings[0].message.contains("unrecognized action 'Journal'"));
    assert!(p.warnings[1].message.contains("without a quantity"));
    assert!(p.warnings[2].message.contains("unreadable date"));
}

#[test]
fn generic_mapping_without_action_and_as_snapshot() {
    let text = "Ticker,When,Qty,Px\nAAPL,2026-01-02,5,100\nAAPL,2026-02-02,-2,110\n";
    let m = ColumnMapping { header_line: 1, symbol: Some(0), trade_date: Some(1), quantity: Some(2), price: Some(3), ..Default::default() };
    let p = parse(text, Some(&m), &opts()).unwrap();
    assert_eq!(p.counts(), vec![(K::Buy, 1), (K::Sell, 1)]);
    let snap = "Ticker,Qty,Avg\nAAPL,5,100\nMSFT,2,\n";
    let m = ColumnMapping { header_line: 1, symbol: Some(0), quantity: Some(1), average_cost: Some(2), ..Default::default() };
    let p = parse(snap, Some(&m), &opts()).unwrap();
    assert!(p.snapshot);
    assert!(close(find(&p, K::TransferIn, "AAPL")[0].cost_basis, 500.0));
    assert!(find(&p, K::TransferIn, "MSFT")[0].cost_basis.is_none());
}

#[test]
fn bad_mappings_are_errors() {
    let text = fixture("generic_mapped.csv");
    let past = ColumnMapping { amount: Some(40), ..generic_mapping() };
    assert!(matches!(parse(&text, Some(&past), &opts()), Err(ImportError::BadMapping(m)) if m.contains("past the last header column")));
    let no_line = ColumnMapping { header_line: 99, ..generic_mapping() };
    assert!(matches!(parse(&text, Some(&no_line), &opts()), Err(ImportError::BadMapping(_))));
    let snapshot_without_qty = ColumnMapping { header_line: 1, symbol: Some(2), ..Default::default() };
    assert!(matches!(parse(&text, Some(&snapshot_without_qty), &opts()), Err(ImportError::BadMapping(_))));
}

#[test]
fn detection_and_empty_files() {
    assert_eq!(detect(&fixture("vanguard_ofx.csv")).format, Some(Format::Vanguard));
    assert_eq!(detect(&fixture("positions_schwab.csv")).format, Some(Format::Positions));
    let unknown = detect("\n\nWhen,What,Value\n1,2,3\n");
    assert_eq!((unknown.format, unknown.header_line, unknown.headers.len()), (None, 3, 3));
    // A history with symbol and quantity columns but an unknown date or
    // type column is not mistaken for holdings.
    assert_eq!(detect("When,What,Ticker,Qty,Px\n2026-01-02,Buy,MSFT,2,400\n").format, None);
    assert_eq!(detect("Symbol,Quantity,Price,Transaction\nMSFT,2,400,Buy\n").format, None);
    // Merrill's positions header has a close-of-business date and is a
    // snapshot.
    let merrill = "\"COB Date\",\"Security #\",\"Symbol\",\"CUSIP #\",\"Security Description\",\"Account Nickname\",\"Account Registration\",\"Account #\",\"Quantity\",\"Price ($)\",\"Value ($)\"\n";
    assert_eq!(detect(merrill).format, Some(Format::Positions));
    assert_eq!(parse("\n \n", None, &opts()), Err(ImportError::Empty));
    // A recognized header with no rows says so.
    let p = parse("\"Date\",\"Action\",\"Symbol\",\"Description\",\"Quantity\",\"Price\",\"Fees & Comm\",\"Amount\"\n", None, &opts()).unwrap();
    assert!(p.transactions.is_empty() && p.warnings.len() == 1 && p.warnings[0].message.contains("no transactions"));
}

#[test]
fn histories_with_unspaced_headers_are_not_read_as_positions() {
    // E*TRADE-style headers have no space in the date and type columns.
    // Read as holdings, Bought 10 / Sold 10 / Bought 5 would be 15 shares.
    let etrade = "TransactionDate,TransactionType,SecurityType,Symbol,Quantity,Amount,Price,Commission,Description\n\
                  10/01/26,Bought,EQ,AAPL,10,-1800.00,180.00,0.00,APPLE INC\n\
                  10/02/26,Sold,EQ,AAPL,-10,1900.00,190.00,0.00,APPLE INC\n\
                  10/03/26,Bought,EQ,AAPL,5,-950.00,190.00,0.00,APPLE INC\n";
    assert_eq!(detect(etrade).format, None);
    let headers = match parse(etrade, None, &opts()) {
        Err(ImportError::Unrecognized { header_line: 1, headers }) => headers,
        other => panic!("expected Unrecognized, got {other:?}"),
    };
    // The mapping suggestion finds the unspaced columns, and the file reads
    // as three trades.
    let m = meridian_import::suggest_mapping(1, &headers);
    assert_eq!((m.trade_date, m.action, m.symbol, m.quantity, m.amount, m.price, m.fees), (Some(0), Some(1), Some(3), Some(4), Some(5), Some(6), Some(7)));
    let p = parse(etrade, Some(&m), &opts()).unwrap();
    assert!(!p.snapshot);
    assert_eq!(p.counts(), vec![(K::Buy, 2), (K::Sell, 1)]);
    let net: f64 = p.transactions.iter().filter_map(|t| t.quantity).sum();
    assert!((net - 5.0).abs() < 1e-9);
    // IBKR-style: one `Date/Time` column.
    let ibkr = "Symbol,Date/Time,Quantity,T. Price,Proceeds,Comm/Fee\nAAPL,\"2026-10-01, 09:31:00\",10,180,-1800,-1\nAAPL,\"2026-10-02, 10:00:00\",-10,190,1900,-1\n";
    assert_eq!(detect(ibkr).format, None);
    for header in ["Symbol,Quantity,TradeTime", "Symbol,Quantity,Activity Type", "Symbol,Quantity,Buy/Sell", "Symbol,Quantity,Txn Type"] {
        assert_eq!(detect(&format!("{header}\nAAPL,1,x\n")).format, None, "{header}");
    }
    // Positions headers with `Type`, `Last Updated` or `Fractional` columns
    // are still snapshots.
    for header in ["Symbol,Quantity,Type", "Symbol,Quantity,Last Updated", "Symbol,Fractional Shares,Quantity"] {
        assert_eq!(detect(&format!("{header}\nAAPL,1,x\n")).format, Some(Format::Positions), "{header}");
    }
}

#[test]
fn same_day_rows_keep_their_real_order() {
    // Robinhood lists newest first: a one-day file's sell (bought earlier
    // that day) comes first and must end up after the buy.
    let header = "\"Activity Date\",\"Process Date\",\"Settle Date\",\"Instrument\",\"Description\",\"Trans Code\",\"Quantity\",\"Price\",\"Amount\"\n";
    let text = format!(
        "{header}\"3/2/2026\",\"3/2/2026\",\"3/3/2026\",\"AAPL\",\"Apple\",\"Sell\",\"1\",\"$11.00\",\"$11.00\"\n\"3/2/2026\",\"3/2/2026\",\"3/3/2026\",\"AAPL\",\"Apple\",\"Buy\",\"1\",\"$10.00\",\"($10.00)\"\n"
    );
    let p = parse(&text, None, &opts()).unwrap();
    assert_eq!(p.transactions.iter().map(|t| t.kind).collect::<Vec<_>>(), [K::Buy, K::Sell]);
    // A mapped (oldest-first) file keeps its order.
    let m = ColumnMapping { header_line: 1, trade_date: Some(0), symbol: Some(1), quantity: Some(2), price: Some(3), ..Default::default() };
    let p = parse("Date,Ticker,Qty,Px\n2026-03-02,AAPL,1,10\n2026-03-02,AAPL,-1,11\n", Some(&m), &opts()).unwrap();
    assert_eq!(p.transactions.iter().map(|t| t.kind).collect::<Vec<_>>(), [K::Buy, K::Sell]);
}

#[test]
fn as_of_rows_do_not_flip_the_file_order() {
    // A short newest-first Schwab export whose top row is dated "as of" an
    // older day. Judged by trade dates it looked oldest first, so it was not
    // reversed and the 10/07 sell sorted before the 10/07 buy.
    let schwab = "\"Date\",\"Action\",\"Symbol\",\"Description\",\"Quantity\",\"Price\",\"Fees & Comm\",\"Amount\"\n\
                  \"10/08/2026 as of 09/30/2026\",\"Credit Interest\",\"\",\"SCHWAB1 INT 09/01-09/30\",\"\",\"\",\"\",\"$0.40\"\n\
                  \"10/07/2026\",\"Sell\",\"AAPL\",\"APPLE INC\",\"10\",\"$200.00\",\"\",\"$2000.00\"\n\
                  \"10/07/2026\",\"Buy\",\"AAPL\",\"APPLE INC\",\"10\",\"$190.00\",\"\",\"-$1900.00\"\n";
    let p = parse(schwab, None, &opts()).unwrap();
    assert_eq!(p.transactions.iter().map(|t| t.kind).collect::<Vec<_>>(), [K::Interest, K::Buy, K::Sell]);
    assert_eq!((p.transactions[0].trade_date, p.transactions[0].listed_date), (d(2026, 9, 30), d(2026, 10, 8)));
    // The same with Fidelity's "as of" inside the Action.
    let fidelity = "Run Date,Action,Symbol,Description,Type,Quantity,Price ($),Commission ($),Fees ($),Accrued Interest ($),Amount ($),Settlement Date\n\
                    10/08/2026,DIVIDEND RECEIVED as of 09/30/2026 APPLE INC (AAPL) (Cash),AAPL,APPLE INC,Cash,0,,,,,2.60,\n\
                    10/07/2026,YOU SOLD APPLE INC (AAPL) (Cash),AAPL,APPLE INC,Cash,-10,200,,,,2000.00,10/08/2026\n\
                    10/07/2026,YOU BOUGHT APPLE INC (AAPL) (Cash),AAPL,APPLE INC,Cash,10,190,,,,-1900.00,10/08/2026\n";
    let p = parse(fidelity, None, &opts()).unwrap();
    assert_eq!(p.transactions.iter().map(|t| t.kind).collect::<Vec<_>>(), [K::Dividend, K::Buy, K::Sell]);
    // A file someone re-sorted oldest first is still read in its order.
    let resorted = "\"Date\",\"Action\",\"Symbol\",\"Description\",\"Quantity\",\"Price\",\"Fees & Comm\",\"Amount\"\n\
                    \"10/06/2026\",\"Buy\",\"AAPL\",\"APPLE INC\",\"10\",\"$190.00\",\"\",\"-$1900.00\"\n\
                    \"10/07/2026\",\"Buy\",\"MSFT\",\"MICROSOFT CORP\",\"1\",\"$400.00\",\"\",\"-$400.00\"\n\
                    \"10/07/2026\",\"Sell\",\"MSFT\",\"MICROSOFT CORP\",\"1\",\"$410.00\",\"\",\"$410.00\"\n";
    let p = parse(resorted, None, &opts()).unwrap();
    assert_eq!(p.transactions.iter().map(|t| t.kind).collect::<Vec<_>>(), [K::Buy, K::Buy, K::Sell]);
}

/// `schwab_2024.csv` with its two reverse-split rows (lines 5 and 6) swapped.
fn schwab_2024_legs_swapped() -> String {
    let text = fixture("schwab_2024.csv");
    let mut lines: Vec<&str> = text.lines().collect();
    assert!(lines[4].contains("\"Reverse Split\",\"ABCD\"") && lines[5].contains("\"Reverse Split\",\"000000AA3\""));
    lines.swap(4, 5);
    lines.join("\n") + "\n"
}

#[test]
fn netted_reverse_split_fingerprint_ignores_leg_order() {
    let a = read("schwab_2024.csv");
    let b = parse(&schwab_2024_legs_swapped(), None, &opts()).unwrap();
    let (ra, rb) = (find(&a, K::Split, "ABCD"), find(&b, K::Split, "ABCD"));
    assert_eq!((ra.len(), rb.len()), (1, 1));
    assert!(close(ra[0].quantity, -95.0) && close(rb[0].quantity, -95.0));
    assert_eq!(ra[0].fingerprint, rb[0].fingerprint);
    // Every other row agrees too, so re-importing either file adds nothing.
    let fa: std::collections::HashSet<&str> = a.transactions.iter().map(|t| t.fingerprint.as_str()).collect();
    assert!(b.transactions.iter().all(|t| fa.contains(t.fingerprint.as_str())));
    // A split that is not netted keeps its own fingerprint shape.
    assert!(find(&a, K::Split, "AVGO")[0].fingerprint.ends_with("-0"));
}

#[test]
fn crlf_and_quoted_newlines_count_lines() {
    let text = fixture("robinhood_activity.csv").replace('\n', "\r\n");
    let p = parse(&text, None, &opts()).unwrap();
    assert_eq!(warning_lines(&p), vec![2, 8]);
    assert_eq!(find(&p, K::Buy, "AAPL")[0].line, 23);
}

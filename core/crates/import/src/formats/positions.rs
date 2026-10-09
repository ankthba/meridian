//! Holdings snapshots: one row per position with a symbol, a quantity and,
//! optionally, the total cost basis or the average cost per share.
//!
//! Matches the positions downloads of Fidelity, Charles Schwab (single and
//! all-accounts), E*TRADE and Merrill, the holdings section of a Vanguard
//! download without transactions, and any CSV with `Symbol` and `Quantity`
//! (or `Shares`) columns and no date column. Column names and special rows
//! are sourced in the README.
//!
//! Each holding becomes a `TransferIn` dated on the import day (the caller
//! passes it in), carrying the stated cost basis. A position without a cost
//! basis is still imported; PORT then shows its cost and P&L as unknown.

use super::{Columns, is_cusip, is_option_symbol, norm, num_in, opt_text};
use crate::csv::Record;
use crate::value::{self, DateOrder, Decimal};
use crate::{Format, ImportOptions, Output, Row, TransactionKind};

const SYMBOL: &[&str] = &["symbol", "ticker"];
const QUANTITY: &[&str] = &["quantity", "qty (quantity)", "qty", "shares"];
const COST_BASIS: &[&str] = &["cost basis total", "cost basis", "total cost basis", "total cost"];
const AVERAGE_COST: &[&str] = &["average cost basis", "cost basis per share", "cost/share", "price paid $", "average cost", "avg cost", "cost per share"];
const DESCRIPTION: &[&str] = &["description", "security description", "investment name", "name"];
const ACCOUNT: &[&str] = &["account number", "account name/number", "account #", "account"];
const ASSET_TYPE: &[&str] = &["security type", "asset type"];

/// Summary rows in the symbol column (Schwab, E*TRADE, Fidelity).
const SUMMARY: &[&str] = &[
    "cash & cash investments",
    "account total",
    "positions total",
    "futures cash",
    "futures positions market value",
    "pending activity",
    "total",
    "cash",
];

/// Footer lines that are not data (Fidelity, E*TRADE).
const FOOTERS: &[&str] = &["the data and information in this spreadsheet", "brokerage services are provided", "date downloaded", "date exported", "generated at"];

/// Words that, before `time`, make a column a transaction timestamp
/// (`Trade Time`, `ExecTime`).
const TIMED: &[&str] = &["trade", "exec", "transaction", "activity", "settle", "order", "fill", "post", "process", "run"];

/// Lower-cased with everything but letters and digits removed, so
/// `TransactionDate`, `Transaction Date` and `Date/Time` compare alike.
pub(crate) fn squash(s: &str) -> String {
    s.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

/// Whether a header name belongs to a transaction history: a date column
/// (`Trade Date`, `TransactionDate`, `Date/Time`), a transaction time, or a
/// transaction type column (`Action`, `TransactionType`, `Activity`,
/// `Trans Code`). `Type` alone is not one: Fidelity's positions file uses it
/// for the account type. Merrill's positions file has a close-of-business
/// date (`COB Date`) and stays a snapshot.
fn is_history_column(name: &str) -> bool {
    let n = squash(name);
    if n.is_empty() || n == "cobdate" {
        return false;
    }
    let date = n.contains("date") && !n.contains("update");
    let time = n == "time" || n == "when" || (n.ends_with("time") && TIMED.iter().any(|w| n.starts_with(w)));
    let kind = n.contains("transaction")
        || n.contains("activity")
        || n.contains("txn")
        || n.starts_with("action")
        || (n.ends_with("action") && !n.ends_with("fraction"))
        || matches!(n.as_str(), "transcode" | "transtype" | "trancode" | "trantype" | "buysell");
    date || time || kind
}

/// A symbol and a quantity column, and nothing that looks like a
/// transaction date, time or type, so an unknown history goes to the
/// mapping UI instead of being read as holdings.
pub(crate) fn matches(c: &Columns) -> bool {
    c.find(SYMBOL).is_some() && c.find(QUANTITY).is_some() && !c.any(is_history_column)
}

/// Which columns hold a snapshot's values.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SnapshotCols {
    pub(crate) symbol: Option<usize>,
    pub(crate) quantity: Option<usize>,
    pub(crate) cost_basis: Option<usize>,
    pub(crate) average_cost: Option<usize>,
    pub(crate) description: Option<usize>,
    pub(crate) account: Option<usize>,
    pub(crate) asset_type: Option<usize>,
    /// Decimal separator (a mapped file's `decimal_comma`).
    pub(crate) decimal: Decimal,
}

impl SnapshotCols {
    fn from_header(header: &Record) -> Self {
        let c = Columns::new(header);
        Self {
            symbol: c.find(SYMBOL),
            quantity: c.find(QUANTITY),
            cost_basis: c.find(COST_BASIS),
            average_cost: c.find(AVERAGE_COST),
            description: c.find(DESCRIPTION),
            account: c.find(ACCOUNT),
            asset_type: c.find(ASSET_TYPE),
            decimal: Decimal::Point,
        }
    }
}

pub(crate) fn is_footer(rec: &Record) -> bool {
    let first = norm(rec.fields.iter().find(|f| !f.trim().is_empty()).map_or("", String::as_str));
    FOOTERS.iter().any(|p| first.starts_with(p))
}

pub(crate) fn parse(recs: &[Record], header: usize, opts: ImportOptions, out: &mut Output) {
    let mut cols = SnapshotCols::from_header(&recs[header]);
    // Schwab's all-accounts export repeats an account-name row and the
    // header for each account.
    let mut account: Option<String> = None;
    let rest = &recs[header + 1..];
    for (i, rec) in rest.iter().enumerate() {
        if rec.is_blank() || is_footer(rec) {
            continue;
        }
        if matches(&Columns::new(rec)) {
            cols = SnapshotCols::from_header(rec);
            continue;
        }
        if rec.filled() == 1 {
            let next_is_header = rest[i + 1..].iter().find(|r| !r.is_blank()).is_some_and(|r| matches(&Columns::new(r)));
            if next_is_header {
                account = Some(value::text(rec.fields.iter().find(|f| !f.trim().is_empty()).map_or("", String::as_str)));
                continue;
            }
        }
        snapshot_row_with_account(rec, Format::Positions, cols, opts, "USD", account.clone(), out);
    }
}

/// Reads one holding; unreadable or unsupported rows become warnings.
pub(crate) fn snapshot_row(rec: &Record, family: Format, cols: SnapshotCols, opts: ImportOptions, currency: &str, out: &mut Output) {
    snapshot_row_with_account(rec, family, cols, opts, currency, None, out);
}

fn snapshot_row_with_account(rec: &Record, family: Format, cols: SnapshotCols, opts: ImportOptions, currency: &str, account: Option<String>, out: &mut Output) {
    let raw_symbol = cols.symbol.map_or("", |i| rec.get(i));
    if rec.fields.iter().any(|f| norm(f) == "pending activity") {
        out.warn(rec, "Not imported: pending activity is not a holding");
        return;
    }
    if SUMMARY.contains(&norm(raw_symbol).as_str()) {
        out.warn(rec, format!("Not imported: '{}' is a summary row, not a holding", value::text(raw_symbol)));
        return;
    }
    // Fidelity marks the core (cash sweep) position with `**`.
    if raw_symbol.ends_with("**") {
        out.warn(rec, format!("Not imported: {raw_symbol} is the account's core cash position, not a holding"));
        return;
    }
    let Some(symbol) = value::symbol(raw_symbol) else {
        out.warn(rec, "Not imported: no symbol (cash, option or note row)");
        return;
    };
    let asset_type = cols.asset_type.map(|i| norm(rec.get(i))).unwrap_or_default();
    if asset_type.contains("option") || is_option_symbol(&symbol) {
        out.warn(rec, format!("Not imported: {symbol} looks like an option; options are not supported"));
        return;
    }
    if value::date(&symbol, DateOrder::MonthFirst).is_ok() {
        out.warn(rec, "Not imported: tax-lot detail row (the position row above it is imported)");
        return;
    }
    if is_cusip(&symbol) {
        out.warn(rec, format!("Not imported: {symbol} is a CUSIP (bond or CD); fixed income is not supported"));
        return;
    }
    let qty = match num_in(rec, cols.quantity, "quantity", cols.decimal) {
        Ok(Some(q)) => q,
        Ok(None) => {
            out.warn(rec, format!("Not imported: {symbol} has no quantity (cash or money-market balance)"));
            return;
        }
        Err(e) => {
            out.warn(rec, format!("Not imported: unreadable {}", e.describe()));
            return;
        }
    };
    if qty <= 0.0 {
        out.warn(rec, format!("Not imported: {symbol} quantity {qty} (only long positions are supported)"));
        return;
    }
    // Schwab writes "Incomplete" when it lacks a lot's basis.
    let cost_cell = |i: Option<usize>| i.filter(|i| !rec.get(*i).eq_ignore_ascii_case("incomplete"));
    let cost = match (
        num_in(rec, cost_cell(cols.cost_basis), "cost basis", cols.decimal),
        num_in(rec, cost_cell(cols.average_cost), "average cost", cols.decimal),
    ) {
        (Ok(Some(total)), _) => Some(total.abs()),
        (Ok(None), Ok(Some(avg))) => Some(avg.abs() * qty),
        (Ok(None), Ok(None)) => None,
        (Err(e), _) | (_, Err(e)) => {
            out.warn(rec, format!("Not imported: unreadable {}", e.describe()));
            return;
        }
    };
    let mut r = Row::new(TransactionKind::TransferIn, opts.as_of);
    r.symbol = Some(symbol);
    r.quantity = Some(qty);
    r.cost_basis = cost;
    currency.clone_into(&mut r.currency);
    r.description = opt_text(rec, cols.description).unwrap_or_default();
    r.account = opt_text(rec, cols.account).or(account);
    r.action = "Position".into();
    r.snapshot = true;
    out.push(family, rec, r);
}

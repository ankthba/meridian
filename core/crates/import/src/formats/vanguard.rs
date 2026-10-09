//! Vanguard brokerage transaction download (`OfxDownload.csv`, "A
//! spreadsheet-compatible CSV file"). Sources and confidence: README.
//!
//! The file has a holdings section first (`Account Number,Investment
//! Name,Symbol,Shares,Share Price,Total Value,`), then the transactions
//! section read here, then optional retirement-plan sections. Dates are
//! ISO (`YYYY-MM-DD`) in current files and `MM/DD/YYYY` in older ones;
//! numbers are plain signed decimals; every row ends with a comma.
//! `Net Amount` is the signed cash effect with commissions already taken
//! out. Rows are grouped by account and holding, not by date.

use super::{Columns, check_shares, norm, num, opt_text};
use crate::csv::Record;
use crate::value::{self, DateOrder};
use crate::{Format, Output, Row, TransactionKind};

pub(crate) fn matches(c: &Columns) -> bool {
    c.has_all(&["trade date", "transaction type", "transaction description", "symbol", "shares", "net amount"])
}

enum Kind {
    Is(TransactionKind),
    /// Shares in/out when shares move, else cash in/out by the sign of
    /// Net Amount.
    Transfer,
    /// Cash in or out by the sign of Net Amount.
    Cash,
    /// Moves between cash and the settlement fund; no portfolio effect.
    Sweep,
    Unsupported(&'static str),
}

fn kind(t: &str) -> Option<Kind> {
    use TransactionKind as K;
    Some(match norm(t).as_str() {
        "buy" => Kind::Is(K::Buy),
        "sell" => Kind::Is(K::Sell),
        "dividend" | "capital gain (lt)" | "capital gain (st)" => Kind::Is(K::Dividend),
        "reinvestment" | "reinvestment (lt gain)" | "reinvestment (st gain)" => Kind::Is(K::ReinvestedDividend),
        "interest" | "interest charge" => Kind::Is(K::Interest),
        "fee" | "withholding" => Kind::Is(K::Fee),
        "stock split" => Kind::Is(K::Split),
        "corp action (spinoff)" => Kind::Is(K::TransferIn),
        "corp action (cash in lieu)" => Kind::Is(K::CashInLieu),
        "funds received" | "contribution" | "rollover (incoming)" => Kind::Is(K::Deposit),
        "withdrawal" | "distribution" => Kind::Is(K::Withdrawal),
        "transfer (incoming)" | "transfer (outgoing)" | "transfer" | "conversion (incoming)" | "conversion (outgoing)" => Kind::Transfer,
        "wire in" | "wire out" => Kind::Cash,
        "sweep in" | "sweep out" => Kind::Sweep,
        "buy to open" | "sell to close" | "sell to open" | "buy to close" | "expired" | "exercised" | "assigned" | "corp action (sec exchange)" => {
            Kind::Unsupported("options are not supported")
        }
        "corp action (merger)" | "corp action (exchange)" => Kind::Unsupported("mergers and exchanges are not supported; adjust the holding by hand"),
        "corp action (redemption)" => Kind::Unsupported("bond and CD redemptions are not supported"),
        "sell short" | "buy to cover" => Kind::Unsupported("short sales are not supported"),
        _ => return None,
    })
}

/// A row that starts another section of the download.
fn is_section_header(rec: &Record) -> bool {
    matches!(norm(rec.get(0)).as_str(), "account number" | "plan number" | "fund account number")
}

struct Cols {
    date: usize,
    kind: usize,
    settle: Option<usize>,
    desc: Option<usize>,
    name: Option<usize>,
    symbol: Option<usize>,
    shares: Option<usize>,
    price: Option<usize>,
    fees: Option<usize>,
    net: Option<usize>,
    account: Option<usize>,
}

enum Outcome {
    Imported,
    Sweep,
    Settlement,
}

pub(crate) fn parse(recs: &[Record], header: usize, out: &mut Output) {
    let c = Columns::new(&recs[header]);
    let (Some(date), Some(kind_i)) = (c.find(&["trade date"]), c.find(&["transaction type"])) else { return };
    let cols = Cols {
        date,
        kind: kind_i,
        settle: c.find(&["settlement date"]),
        desc: c.find(&["transaction description"]),
        name: c.find(&["investment name"]),
        symbol: c.find(&["symbol"]),
        shares: c.find(&["shares"]),
        price: c.find(&["share price"]),
        fees: c.find(&["commissions and fees", "commission fees"]),
        net: c.find(&["net amount"]),
        account: c.find(&["account number"]),
    };
    // The holdings summary above the transactions is not imported.
    let held: Vec<u32> = recs[..header]
        .iter()
        .skip_while(|r| !is_section_header(r))
        .filter(|r| !r.is_blank() && !is_section_header(r))
        .map(|r| r.line)
        .collect();
    if let (Some(first), Some(last)) = (held.first(), held.last()) {
        out.note(
            *first,
            format!(
                "The holdings summary (lines {first}–{last}) is not imported: PORT rebuilds holdings from the transactions, which cover only the downloaded date range"
            ),
        );
    }
    let mut sweeps: Vec<u32> = Vec::new();
    let mut settlement: Vec<u32> = Vec::new();
    for (i, rec) in recs.iter().enumerate().skip(header + 1) {
        if rec.is_blank() {
            continue;
        }
        if is_section_header(rec) {
            let rows = recs[i + 1..].iter().filter(|r| !r.is_blank() && !is_section_header(r)).count();
            out.note(rec.line, format!("A retirement-plan section starts here ({rows} rows); only brokerage accounts are imported"));
            break;
        }
        match row(rec, &cols, out) {
            Ok(Outcome::Imported) => {}
            Ok(Outcome::Sweep) => sweeps.push(rec.line),
            Ok(Outcome::Settlement) => settlement.push(rec.line),
            Err(msg) => out.warn(rec, msg),
        }
    }
    if let Some(first) = sweeps.first() {
        out.note(
            *first,
            format!(
                "{} settlement-fund sweep rows skipped (cash moving into and out of the settlement fund; no effect on holdings), lines {}",
                sweeps.len(),
                list(&sweeps)
            ),
        );
    }
    if let Some(first) = settlement.first() {
        out.note(
            *first,
            format!(
                "{} settlement-fund reinvestments skipped (no shares; their dividend rows are imported as income), lines {}",
                settlement.len(),
                list(&settlement)
            ),
        );
    }
}

fn list(lines: &[u32]) -> String {
    let mut s: Vec<String> = lines.iter().take(20).map(ToString::to_string).collect();
    if lines.len() > 20 {
        s.push("…".into());
    }
    s.join(", ")
}

fn row(rec: &Record, c: &Cols, out: &mut Output) -> Result<Outcome, String> {
    use TransactionKind as K;
    let bad = |e: value::BadValue| format!("Not imported: unreadable {}", e.describe());
    let trade_date = value::date(rec.get(c.date), DateOrder::MonthFirst).map_err(bad)?;
    let settle_date = match c.settle.map(|i| rec.get(i)).filter(|s| !s.is_empty()) {
        Some(s) => Some(value::date(s, DateOrder::MonthFirst).map_err(bad)?),
        None => None,
    };
    let type_text = value::text(rec.get(c.kind));
    let Some(k) = kind(&type_text) else {
        return Err(format!("Not imported: unknown Transaction Type '{type_text}'"));
    };
    let shares = num(rec, c.shares, "shares").map_err(bad)?.filter(|q| *q != 0.0);
    let net = num(rec, c.net, "net amount").map_err(bad)?;
    let kind = match k {
        // The settlement fund's reinvestment rows carry no shares.
        Kind::Is(K::ReinvestedDividend) if shares.is_none() => return Ok(Outcome::Settlement),
        Kind::Is(k) => k,
        Kind::Transfer => match (shares, net) {
            (Some(q), _) => {
                if q > 0.0 {
                    K::TransferIn
                } else {
                    K::TransferOut
                }
            }
            (None, Some(n)) if n > 0.0 => K::Deposit,
            (None, Some(n)) if n < 0.0 => K::Withdrawal,
            _ => return Err(format!("Not imported: {type_text} with no shares or amount")),
        },
        Kind::Cash => match net {
            Some(n) if n < 0.0 => K::Withdrawal,
            Some(_) => K::Deposit,
            None => return Err(format!("Not imported: {type_text} without a net amount")),
        },
        Kind::Sweep => return Ok(Outcome::Sweep),
        Kind::Unsupported(why) => return Err(format!("Not imported: {type_text} — {why}")),
    };
    let mut r = Row::new(kind, trade_date);
    r.settle_date = settle_date;
    r.symbol = c.symbol.and_then(|i| value::symbol(rec.get(i)));
    r.amount = net;
    r.fees = num(rec, c.fees, "commission").map_err(bad)?.map(f64::abs).filter(|f| *f != 0.0);
    // Share Price is only meaningful on trades (0 or 1 elsewhere).
    if matches!(kind, K::Buy | K::Sell | K::ReinvestedDividend) {
        r.price = num(rec, c.price, "share price").map_err(bad)?.map(f64::abs);
    }
    r.quantity = match kind {
        K::Buy | K::ReinvestedDividend | K::TransferIn => shares.map(f64::abs),
        K::Sell | K::TransferOut => shares.map(|q| -q.abs()),
        K::Split => shares,
        _ => None,
    };
    let desc = opt_text(rec, c.desc).unwrap_or_default();
    let name = opt_text(rec, c.name).unwrap_or_default();
    r.description = match (desc.is_empty(), name.is_empty()) {
        (false, false) => format!("{desc} — {name}"),
        (false, true) => desc,
        _ => name,
    };
    r.action.clone_from(&type_text);
    r.account = opt_text(rec, c.account);
    if kind.moves_shares() {
        check_shares(&r, &type_text)?;
    } else if r.amount.is_none() {
        return Err(format!("Not imported: {type_text} without a net amount"));
    }
    out.push(Format::Vanguard, rec, r);
    Ok(Outcome::Imported)
}

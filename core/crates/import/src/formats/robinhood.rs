//! Robinhood account activity report (Account → Reports and statements →
//! Account activity report). Sources and confidence: README.
//!
//! Every cell is quoted; dates are `M/D/YYYY`; money is `$1,234.56` with
//! negatives in parentheses; descriptions span lines (`Name`, `CUSIP: …`,
//! and an optional third line such as `Dividend Reinvestment`). A quantity
//! may end in `S`, which marks shares leaving (the old leg of a reverse
//! split). The file ends with a disclaimer row. Rows are newest first.

use super::{Columns, check_shares, norm};
use crate::csv::Record;
use crate::value::{self, DateOrder};
use crate::{Format, Output, Row, TransactionKind};

const DISCLAIMER: &str = "the data provided is for informational purposes only";

pub(crate) fn matches(c: &Columns) -> bool {
    c.has_all(&["activity date", "instrument", "trans code", "quantity", "amount"])
}

/// How a Trans Code is imported.
enum Code {
    Kind(TransactionKind),
    /// Cash in or out by the sign of Amount.
    Cash,
    /// Shares in (transfer) when a quantity is present, else cash.
    Acat { incoming: bool },
    Unsupported(&'static str),
}

fn code(c: &str) -> Option<Code> {
    use TransactionKind as K;
    Some(match c.to_ascii_uppercase().as_str() {
        "BUY" => Code::Kind(K::Buy),
        // Buy cancellation: shares returned for a refund.
        "SELL" | "BCXL" => Code::Kind(K::Sell),
        "SPL" | "SPR" => Code::Kind(K::Split),
        // Spinoff shares and shares received without cash (cost basis
        // not in the file).
        "SOFF" | "REC" => Code::Kind(K::TransferIn),
        "CDIV" | "SCAP" | "MDIV" => Code::Kind(K::Dividend),
        "INT" | "SLIP" | "MTCH" | "GDBP" | "MINT" => Code::Kind(K::Interest),
        "GOLD" | "GMPC" | "DTAX" | "DFEE" | "AFEE" => Code::Kind(K::Fee),
        "MISC" => Code::Kind(K::Other),
        "ACH" | "RTP" | "DCF" | "WIRE" | "ITRF" | "CFIR" | "XENT" | "XENT_RWR" | "XENT_RWM" | "XENT_CC" | "FUTSWP" => Code::Cash,
        "ACATI" => Code::Acat { incoming: true },
        "ACATO" => Code::Acat { incoming: false },
        "BTO" | "STC" | "STO" | "BTC" | "OEXP" | "OASGN" | "OEXCS" | "OCC" | "OCA" => Code::Unsupported("options are not supported"),
        "SXCH" | "MRGS" | "MRGC" | "MRGR" | "CONV" => Code::Unsupported("share exchanges and mergers are not supported; adjust the holding by hand"),
        _ => return None,
    })
}

pub(crate) fn parse(recs: &[Record], header: usize, out: &mut Output) {
    let c = Columns::new(&recs[header]);
    let col = |n: &str| c.find(&[n]);
    let (Some(date_i), Some(code_i)) = (col("activity date"), col("trans code")) else { return };
    let settle_i = col("settle date");
    let sym_i = col("instrument");
    let desc_i = col("description");
    let qty_i = col("quantity");
    let px_i = col("price");
    let amt_i = col("amount");
    let acct_i = col("account type");
    for rec in &recs[header + 1..] {
        if rec.is_blank() {
            continue;
        }
        if rec.fields.iter().any(|f| norm(f).starts_with(DISCLAIMER)) && rec.get(date_i).is_empty() && rec.get(code_i).is_empty() {
            continue;
        }
        if let Err(msg) = row(rec, date_i, code_i, settle_i, sym_i, desc_i, qty_i, px_i, amt_i, acct_i, out) {
            out.warn(rec, msg);
        }
    }
}

fn row(
    rec: &Record,
    date_i: usize,
    code_i: usize,
    settle_i: Option<usize>,
    sym_i: Option<usize>,
    desc_i: Option<usize>,
    qty_i: Option<usize>,
    px_i: Option<usize>,
    amt_i: Option<usize>,
    acct_i: Option<usize>,
    out: &mut Output,
) -> Result<(), String> {
    use TransactionKind as K;
    let bad = |e: value::BadValue| format!("Not imported: unreadable {}", e.describe());
    let trade_date = value::date(rec.get(date_i), DateOrder::MonthFirst).map_err(bad)?;
    let settle_date = match settle_i.map(|i| rec.get(i)).filter(|s| !s.is_empty()) {
        Some(s) => Some(value::date(s, DateOrder::MonthFirst).map_err(bad)?),
        None => None,
    };
    let trans_code = rec.get(code_i).to_owned();
    if trans_code.is_empty() {
        return Err("Not imported: no Trans Code".into());
    }
    let Some(code) = code(&trans_code) else {
        return Err(format!("Not imported: unknown Trans Code '{trans_code}'"));
    };
    // A trailing `S` marks shares leaving.
    let qty_cell = qty_i.map_or("", |i| rec.get(i));
    let (qty_text, leaving) = match qty_cell.strip_suffix(['S', 's']) {
        Some(q) => (q, true),
        None => (qty_cell, false),
    };
    let quantity = value::number(qty_text, "quantity").map_err(bad)?.map(|q| if leaving { -q.abs() } else { q });
    let price = super::num(rec, px_i, "price").map_err(bad)?;
    let amount = super::num(rec, amt_i, "amount").map_err(bad)?;
    let description = desc_i.map(|i| value::text(rec.get(i))).unwrap_or_default();
    let symbol = sym_i.and_then(|i| value::symbol(rec.get(i)));
    let kind = match code {
        Code::Kind(K::Buy) if description.to_ascii_lowercase().contains("dividend reinvestment") => K::ReinvestedDividend,
        Code::Kind(k) => k,
        Code::Cash => match amount {
            Some(a) if a < 0.0 && description.to_ascii_lowercase().contains("fee") => K::Fee,
            Some(a) if a < 0.0 => K::Withdrawal,
            Some(_) => K::Deposit,
            None => return Err(format!("Not imported: {trans_code} without an amount")),
        },
        Code::Acat { incoming } => match (quantity.filter(|q| *q != 0.0), amount) {
            (Some(_), _) => {
                if incoming { K::TransferIn } else { K::TransferOut }
            }
            (None, Some(a)) => {
                if a < 0.0 { K::Withdrawal } else { K::Deposit }
            }
            (None, None) => return Err(format!("Not imported: {trans_code} without a quantity or amount")),
        },
        Code::Unsupported(why) => return Err(format!("Not imported: {trans_code} — {why}")),
    };
    let mut r = Row::new(kind, trade_date);
    r.settle_date = settle_date;
    r.symbol = symbol;
    r.price = price.map(f64::abs);
    r.amount = amount;
    r.description = description;
    r.action.clone_from(&trans_code);
    r.account = super::opt_text(rec, acct_i);
    r.quantity = match kind {
        K::Buy | K::ReinvestedDividend | K::TransferIn => quantity.map(f64::abs),
        K::Sell | K::TransferOut => quantity.map(|q| -q.abs()),
        K::Split => quantity,
        _ => None,
    };
    if kind.moves_shares() {
        check_shares(&r, &trans_code)?;
    } else if r.amount.is_none() {
        return Err(format!("Not imported: {trans_code} without an amount"));
    }
    out.push(Format::Robinhood, rec, r);
    Ok(())
}

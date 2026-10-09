//! Fidelity account history (Accounts & Trade → Portfolio → Activity &
//! Orders → Download as CSV; `History_for_Account_<acct>.csv` or
//! `Accounts_History.csv`). Sources and confidence: README.
//!
//! Fidelity has changed this file many times: columns come and go
//! (`Account`, `Account Number`, `Cash Balance`, exchange-currency columns),
//! names carry a ` ($)` suffix or not, `Security Description`/`Security
//! Type` became `Description`/`Type`, and Price and Quantity swapped places
//! in 2026. Columns are therefore looked up by name. Dates are `MM/DD/YYYY`,
//! or `MM-DD-YYYY` since July 2026. Numbers are plain signed decimals.
//! A row is data only when its first cell is a date; the file ends with a
//! disclaimer block. The `Action` text names the transaction (`YOU BOUGHT
//! …`, `DIVIDEND RECEIVED …`), sometimes with an `as of` event date.

use super::{Columns, check_shares, is_ticker, norm, num, opt_text, pair_reverse_splits};
use crate::csv::Record;
use crate::value::{self, DateOrder};
use crate::{Format, Output, Row, TransactionKind};

/// First words of the footer block (2019–2026 files).
const FOOTERS: &[&str] = &["the data and information in this spreadsheet", "brokerage services are provided", "date downloaded", "date exported"];

/// Core (cash sweep) money market funds: buying and redeeming them is cash
/// moving, not an investment.
const CORE: &[&str] = &["SPAXX", "FDRXX", "FZFXX", "FCASH", "CORE"];

pub(crate) fn matches(c: &Columns) -> bool {
    c.has_all(&["run date", "action", "symbol"]) && c.find(&["amount ($)", "amount"]).is_some() && c.find(&["quantity"]).is_some()
}

enum Kind {
    Is(TransactionKind),
    /// Shares in/out by the sign of Quantity, else cash by Amount.
    Transfer,
    /// Cash in/out by the sign of Amount.
    Cash,
    /// Cash moving into or out of the core position.
    Sweep,
    Unsupported(&'static str),
}

/// Classifies the Action text (whitespace-collapsed, case-insensitive).
/// Order matters: taxes before dividends and interest, option and
/// cancellation forms before plain trades.
fn kind(action: &str) -> Option<Kind> {
    use TransactionKind as K;
    let a = norm(action);
    let starts = |p: &str| a.starts_with(p);
    let has = |p: &str| a.contains(p);
    // Workplace-plan rows inside all-accounts files (title case, no symbol).
    if matches!(
        a.as_str(),
        "contributions" | "dividend" | "transfer" | "exchange in" | "exchange out" | "investment gain/loss" | "revenue credit" | "recordkeeping fee"
    ) {
        return Some(Kind::Unsupported("workplace plan (401k) rows are not supported"));
    }
    Some(if has("opening transaction") || has("closing transaction") || starts("expired") || starts("assigned") || starts("exercised") {
        Kind::Unsupported("options are not supported")
    } else if starts("buy cancel") || starts("sell cancel") || has(" cxl ") {
        Kind::Unsupported("cancelled trades are not supported; delete the original trade in PORT")
    } else if starts("you sold short sale") || starts("you bought short cover") {
        // Before the plain trades: "YOU SOLD SHORT SALE …" read as a sell
        // and "YOU BOUGHT SHORT COVER …" as a buy left a phantom long.
        Kind::Unsupported("short sales are not supported")
    } else if starts("you bought") {
        Kind::Is(K::Buy)
    } else if starts("you sold") {
        Kind::Is(K::Sell)
    } else if starts("foreign tax paid")
        || starts("adj foreign tax")
        || starts("non-resident tax")
        || starts("fed tax w/h")
        || starts("fee charged")
        || starts("advisor fee")
        || starts("adjust fee")
    {
        Kind::Is(K::Fee)
    } else if starts("dividend received") || starts("dividend adjustment") || starts("short-term cap gain") || starts("long-term cap gain") || starts("return of capital") {
        Kind::Is(K::Dividend)
    } else if starts("reinvestment") {
        Kind::Is(K::ReinvestedDividend)
    } else if starts("interest earned") || starts("muni exempt int") || starts("interest fully paid") || starts("interest") {
        Kind::Is(K::Interest)
    } else if starts("distribution spinoff") {
        Kind::Is(K::TransferIn)
    } else if starts("distribution") || starts("reverse split") {
        Kind::Is(K::Split)
    } else if starts("in lieu of") {
        Kind::Is(K::Other)
    } else if starts("merger") || starts("name changed") {
        Kind::Unsupported("mergers and name changes are not supported; adjust the holding by hand")
    } else if starts("redemption payout") {
        Kind::Unsupported("bond and T-bill redemptions are not supported")
    } else if starts("redemption from core") || starts("purchase into core") || starts("exchanged to") {
        Kind::Sweep
    } else if has("acat") || starts("rollover shares") || starts("transferred from") || starts("transferred to") {
        Kind::Transfer
    } else if starts("electronic funds transfer")
        || starts("wire transfer")
        || starts("direct deposit")
        || starts("direct debit")
        || starts("check received")
        || starts("check paid")
        || starts("bill payment")
        || starts("debit card")
        || starts("debit crd")
        || starts("cash advance")
        || starts("journaled")
        || starts("cash contribution")
        || starts("contribution")
        || starts("partic contr")
        || starts("co contr")
        || starts("rollover cash")
        || starts("normal distr")
        || starts("early dist")
    {
        Kind::Cash
    } else {
        return None;
    })
}

/// The `as of` date inside an Action: `as of 02/15/2019`, `as of
/// Jan-30-2026`, `as of 2026-07-15`, `AS OF 01-16-24`.
fn as_of(action: &str) -> Option<chrono::NaiveDate> {
    let lower = action.to_ascii_lowercase();
    let i = lower.find("as of ")?;
    let token = action[i + "as of ".len()..].split_whitespace().next()?;
    value::date(token, DateOrder::MonthFirst).ok().or_else(|| chrono::NaiveDate::parse_from_str(token, "%b-%d-%Y").ok())
}

pub(crate) fn parse(recs: &[Record], header: usize, out: &mut Output) {
    let c = Columns::new(&recs[header]);
    let Some(date_i) = c.find(&["run date"]) else { return };
    let first_tx = out.txs.len();
    let mut sweeps: Vec<u32> = Vec::new();
    for rec in &recs[header + 1..] {
        if rec.is_blank() {
            continue;
        }
        let first = norm(rec.fields.iter().find(|f| !f.trim().is_empty()).map_or("", String::as_str));
        if FOOTERS.iter().any(|p| first.starts_with(p)) {
            break;
        }
        if value::date(rec.get(date_i), DateOrder::MonthFirst).is_err() {
            out.warn(rec, "Not imported: not a transaction row (no run date)");
            continue;
        }
        match row(rec, &c, date_i, out) {
            Ok(true) => {}
            Ok(false) => sweeps.push(rec.line),
            Err(msg) => out.warn(rec, msg),
        }
    }
    pair_reverse_splits(out, first_tx, |t| norm(&t.action).starts_with("reverse split"));
    if let Some(first) = sweeps.first() {
        let lines: Vec<String> = sweeps.iter().take(20).map(ToString::to_string).collect();
        out.note(
            *first,
            format!(
                "{} core money-market rows skipped (cash moving into and out of the core position; no effect on holdings), lines {}{}",
                sweeps.len(),
                lines.join(", "),
                if sweeps.len() > 20 { ", …" } else { "" }
            ),
        );
    }
}

/// Imports one row. `Ok(false)` means a core-position sweep was skipped.
fn row(rec: &Record, c: &Columns, date_i: usize, out: &mut Output) -> Result<bool, String> {
    use TransactionKind as K;
    let bad = |e: value::BadValue| format!("Not imported: unreadable {}", e.describe());
    let run_date = value::date(rec.get(date_i), DateOrder::MonthFirst).map_err(bad)?;
    let action = opt_text(rec, c.find(&["action"])).unwrap_or_default();
    let Some(k) = kind(&action) else {
        return Err(format!("Not imported: unrecognized Action '{action}'"));
    };
    let symbol = c.find(&["symbol"]).and_then(|i| value::symbol(rec.get(i)));
    let quantity = num(rec, c.find(&["quantity"]), "quantity").map_err(bad)?.filter(|q| *q != 0.0);
    let price = num(rec, c.find(&["price ($)", "price"]), "price").map_err(bad)?;
    let amount = num(rec, c.find(&["amount ($)", "amount"]), "amount").map_err(bad)?;
    let commission = num(rec, c.find(&["commission ($)", "commission"]), "commission").map_err(bad)?;
    let fees = num(rec, c.find(&["fees ($)", "fees"]), "fees").map_err(bad)?;
    let core = symbol.as_deref().is_some_and(|s| CORE.contains(&s));
    let kind = match k {
        Kind::Sweep => return Ok(false),
        Kind::Is(K::Buy | K::Sell | K::ReinvestedDividend) if core => return Ok(false),
        // An RSU vest posts as a buy for $0; its cost basis is the value at
        // vest, which the row's price gives when present.
        Kind::Is(K::Buy) if norm(&action).contains("rsu") && amount.is_none_or(|a| a == 0.0) => K::TransferIn,
        Kind::Is(k) => k,
        Kind::Transfer => match (quantity, amount) {
            (Some(q), _) if symbol.is_some() => {
                if q > 0.0 {
                    K::TransferIn
                } else {
                    K::TransferOut
                }
            }
            (_, Some(a)) if a < 0.0 => K::Withdrawal,
            (_, Some(_)) => K::Deposit,
            _ => return Err(format!("Not imported: {action} without a quantity or amount")),
        },
        Kind::Cash => match amount {
            Some(a) if a < 0.0 => K::Withdrawal,
            Some(_) => K::Deposit,
            None => return Err(format!("Not imported: {action} without an amount")),
        },
        Kind::Unsupported(why) => return Err(format!("Not imported: {action} — {why}")),
    };
    let mut r = Row::new(kind, as_of(&action).unwrap_or(run_date));
    r.listed_date = run_date;
    r.settle_date = c.find(&["settlement date"]).and_then(|i| value::date(rec.get(i), DateOrder::MonthFirst).ok());
    r.symbol = symbol;
    r.currency = opt_text(rec, c.find(&["currency"])).map_or_else(|| "USD".to_owned(), |s| s.to_ascii_uppercase());
    r.description = opt_text(rec, c.find(&["description", "security description"])).unwrap_or_default();
    r.action.clone_from(&action);
    r.account = opt_text(rec, c.find(&["account number"])).or_else(|| opt_text(rec, c.find(&["account"])));
    let total_fees = commission.unwrap_or(0.0).abs() + fees.unwrap_or(0.0).abs();
    r.fees = (total_fees > 0.0).then_some(total_fees);
    // Rows that only move shares (Type "Shares") carry a market value in
    // Amount, not cash.
    r.amount = if matches!(kind, K::Split | K::TransferIn | K::TransferOut) { None } else { amount };
    r.price = match kind {
        K::Buy | K::Sell | K::ReinvestedDividend | K::TransferIn => price.map(f64::abs).filter(|p| *p > 0.0),
        _ => None,
    };
    // A transfer's price is a valuation, except for an RSU vest.
    if matches!(kind, K::TransferIn) && !norm(&action).contains("rsu") {
        r.price = None;
    }
    r.quantity = match kind {
        K::Buy | K::ReinvestedDividend | K::TransferIn => quantity.map(f64::abs),
        K::Sell | K::TransferOut => quantity.map(|q| -q.abs()),
        K::Split => quantity,
        _ => None,
    };
    if kind.moves_shares() {
        if kind == K::Split && norm(&action).starts_with("reverse split") {
            if r.quantity.is_none() {
                return Err(format!("Not imported: {action} without a quantity"));
            }
        } else {
            check_shares(&r, &action)?;
        }
        if matches!(kind, K::Buy | K::Sell | K::ReinvestedDividend) && r.amount.is_none() && r.price.is_none() {
            return Err(format!("Not imported: {action} without a price or amount"));
        }
    } else if r.amount.is_none() {
        return Err(format!("Not imported: {action} without an amount"));
    }
    if r.symbol.as_deref().is_some_and(|s| !is_ticker(s)) && !kind.moves_shares() {
        // Income on a bond or option (CUSIP or option symbol): keep the
        // cash, drop the symbol so it doesn't open a holding.
        r.symbol = None;
    }
    out.push(Format::Fidelity, rec, r);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_of_forms() {
        let d = |y, m, dd| chrono::NaiveDate::from_ymd_opt(y, m, dd).unwrap();
        assert_eq!(as_of("DIVIDEND RECEIVED as of 02/15/2019 X"), Some(d(2019, 2, 15)));
        assert_eq!(as_of("DIVIDEND RECEIVED as of Jan-30-2026 X"), Some(d(2026, 1, 30)));
        assert_eq!(as_of("REINVESTMENT as of 2026-07-15 X"), Some(d(2026, 7, 15)));
        assert_eq!(as_of("YOU BOUGHT RSU1234 AS OF 01-16-24 X"), Some(d(2024, 1, 16)));
        assert_eq!(as_of("YOU BOUGHT APPLE INC (AAPL) (Cash)"), None);
    }

    #[test]
    fn action_table_order() {
        use TransactionKind as K;
        let k = |a: &str| match kind(a) {
            Some(Kind::Is(k)) => format!("{k:?}"),
            Some(Kind::Transfer) => "Transfer".into(),
            Some(Kind::Cash) => "Cash".into(),
            Some(Kind::Sweep) => "Sweep".into(),
            Some(Kind::Unsupported(_)) => "Unsupported".into(),
            None => "None".into(),
        };
        assert_eq!(k("YOU BOUGHT APPLE INC (AAPL) (Cash)"), format!("{:?}", K::Buy));
        assert_eq!(k("YOU BOUGHT OPENING TRANSACTION CALL (XLE) ..."), "Unsupported");
        assert_eq!(k("YOU SOLD SHORT SALE TESLA INC (TSLA) (Margin)"), "Unsupported");
        assert_eq!(k("YOU BOUGHT SHORT COVER TESLA INC (TSLA) (Short)"), "Unsupported");
        assert_eq!(k("YOU SOLD ISHARES SHORT TREASURY BOND ETF (SHV) (Cash)"), format!("{:?}", K::Sell));
        assert_eq!(k("NON-RESIDENT TAX DIVIDEND RECEIVED X"), format!("{:?}", K::Fee));
        assert_eq!(k("DIVIDEND RECEIVED X"), format!("{:?}", K::Dividend));
        assert_eq!(k("DISTRIBUTION SPINOFF FROM:(ABC ) XYZ"), format!("{:?}", K::TransferIn));
        assert_eq!(k("DISTRIBUTION NVIDIA CORP (NVDA) (Cash)"), format!("{:?}", K::Split));
        assert_eq!(k("TRANSFER OF ASSETS ACAT RECEIVE"), "Transfer");
        assert_eq!(k("Electronic Funds Transfer Received (Cash)"), "Cash");
        assert_eq!(k("Contributions"), "Unsupported");
        assert_eq!(k("SOMETHING NEW"), "None");
    }
}

//! Charles Schwab brokerage transaction history (Accounts → History →
//! Transactions → Export → CSV). Sources and confidence: README.
//!
//! Three layouts exist and all are read: up to ~2021 a banner line
//! (`"Transactions  for account … as of …"`), a header with a trailing comma
//! and a `Transactions Total` footer; ~2022–mid 2023 the same without the
//! trailing comma; since late 2023 just the header and rows. Dates are
//! `MM/DD/YYYY` or `MM/DD/YYYY as of MM/DD/YYYY`, where the second date is
//! when the event happened (used as the trade date). Money is `-$1234.56`;
//! trade quantities are unsigned, share movements signed. `Fees & Comm` is
//! already included in `Amount`. Rows are newest first.

use super::{Columns, check_shares, norm, num, opt_text, pair_reverse_splits};
use crate::csv::Record;
use crate::value::{self, DateOrder};
use crate::{Format, Output, Row, TransactionKind};

pub(crate) fn matches(c: &Columns) -> bool {
    c.has_all(&["date", "action", "symbol", "description", "quantity", "price", "fees & comm", "amount"])
}

enum Kind {
    Is(TransactionKind),
    /// Cash in or out by the sign of Amount.
    Cash,
    /// Shares in or out by the sign of Quantity (else cash by Amount).
    Shares,
    Unsupported(&'static str),
}

fn kind(action: &str) -> Option<Kind> {
    use TransactionKind as K;
    Some(match norm(action).as_str() {
        "buy" => Kind::Is(K::Buy),
        "sell" => Kind::Is(K::Sell),
        "reinvest shares" => Kind::Is(K::ReinvestedDividend),
        // The cash leg of a reinvestment, and plain distributions.
        "reinvest dividend" | "qual div reinvest" | "qual div reinvest adj" | "non-qualified div" | "short term cap gain reinvest" | "long term cap gain reinvest"
        | "pr yr div reinvest" | "cash dividend" | "qualified dividend" | "special dividend" | "special qual div" | "special non qual div" | "pr yr cash div"
        | "pr yr special div" | "div adjustment" | "long term cap gain" | "short term cap gain" => Kind::Is(K::Dividend),
        "return of capital" => Kind::Is(K::ReturnOfCapital),
        "bank interest" | "credit interest" | "bond interest" | "margin interest" | "interest adj" | "promotional award" => Kind::Is(K::Interest),
        "nra tax adj" | "nra withholding" | "nra withhold" | "pr yr nra tax" | "foreign tax paid" | "foreign tax reclaim" | "foreign tax reclaim adj"
        | "irs withhold adj" | "adr mgmt fee" | "service fee" | "advisor fee" => Kind::Is(K::Fee),
        "cash in lieu" => Kind::Is(K::CashInLieu),
        "misc cash entry" | "adjustment" => Kind::Is(K::Other),
        // A stock dividend adds shares without changing cost, like a split.
        "stock split" | "reverse split" | "stock div dist" => Kind::Is(K::Split),
        "spin-off" | "stock plan activity" => Kind::Is(K::TransferIn),
        "moneylink transfer" | "moneylink deposit" | "moneylink adj" | "wire sent" | "wire received" | "wire funds received" | "wire funds" | "wire funds adj"
        | "funds received" | "funds paid" | "bank transfer" | "journal" | "internal transfer" | "futures mm sweep" | "visa purchase" => Kind::Cash,
        "journaled shares" | "security transfer" => Kind::Shares,
        "buy to open" | "sell to open" | "buy to close" | "sell to close" | "expired" | "assigned" | "exchange or exercise" | "options frwd split"
        | "options frwd split adj" => Kind::Unsupported("options are not supported"),
        "sell short" | "buy to cover" => Kind::Unsupported("short sales are not supported"),
        "cancel buy" | "cancel sell" => Kind::Unsupported("cancelled trades are not supported; delete the original trade in PORT"),
        "stock merger" | "name change" | "conversion" | "cash merger" | "cash merger adj" => {
            Kind::Unsupported("mergers and name changes are not supported; adjust the holding by hand")
        }
        "full redemption" | "full redemption adj" => Kind::Unsupported("bond and CD redemptions are not supported"),
        _ => return None,
    })
}

/// `MM/DD/YYYY` or `MM/DD/YYYY as of MM/DD/YYYY`: the event date.
fn event_date(cell: &str) -> Result<chrono::NaiveDate, value::BadValue> {
    let lower = cell.to_ascii_lowercase();
    let s = match lower.find(" as of ") {
        Some(i) => &cell[i + " as of ".len()..],
        None => cell,
    };
    value::date(s, DateOrder::MonthFirst)
}

pub(crate) fn parse(recs: &[Record], header: usize, out: &mut Output) {
    let c = Columns::new(&recs[header]);
    let (Some(date_i), Some(action_i)) = (c.find(&["date"]), c.find(&["action"])) else { return };
    let first_tx = out.txs.len();
    for rec in &recs[header + 1..] {
        if rec.is_blank() || norm(rec.get(0)) == "transactions total" {
            continue;
        }
        if let Err(msg) = row(rec, &c, date_i, action_i, out) {
            out.warn(rec, msg);
        }
    }
    pair_reverse_splits(out, first_tx, |t| norm(&t.action) == "reverse split");
}

fn row(rec: &Record, c: &Columns, date_i: usize, action_i: usize, out: &mut Output) -> Result<(), String> {
    use TransactionKind as K;
    let bad = |e: value::BadValue| format!("Not imported: unreadable {}", e.describe());
    let trade_date = event_date(rec.get(date_i)).map_err(bad)?;
    // The first date of `MM/DD/YYYY as of MM/DD/YYYY` (the text after it
    // is ignored) is the posting date the file is ordered by.
    let listed_date = value::date(rec.get(date_i), DateOrder::MonthFirst).map_err(bad)?;
    let action = value::text(rec.get(action_i));
    let Some(k) = kind(&action) else {
        return Err(format!("Not imported: unknown Action '{action}'"));
    };
    let quantity = num(rec, c.find(&["quantity"]), "quantity").map_err(bad)?.filter(|q| *q != 0.0);
    let price = num(rec, c.find(&["price"]), "price").map_err(bad)?;
    let amount = num(rec, c.find(&["amount"]), "amount").map_err(bad)?;
    let fees = num(rec, c.find(&["fees & comm"]), "fees").map_err(bad)?;
    let kind = match k {
        Kind::Is(k) => k,
        Kind::Cash => match amount {
            Some(a) if a < 0.0 => K::Withdrawal,
            Some(_) => K::Deposit,
            None => return Err(format!("Not imported: {action} without an amount")),
        },
        Kind::Shares => match (quantity, amount) {
            (Some(q), _) => {
                if q > 0.0 {
                    K::TransferIn
                } else {
                    K::TransferOut
                }
            }
            (None, Some(a)) if a < 0.0 => K::Withdrawal,
            (None, Some(_)) => K::Deposit,
            (None, None) => return Err(format!("Not imported: {action} without a quantity or amount")),
        },
        Kind::Unsupported(why) => return Err(format!("Not imported: {action} — {why}")),
    };
    let mut r = Row::new(kind, trade_date);
    r.listed_date = listed_date;
    r.symbol = c.find(&["symbol"]).and_then(|i| value::symbol(rec.get(i)));
    r.amount = amount;
    r.fees = fees.map(f64::abs).filter(|f| *f != 0.0);
    r.description = opt_text(rec, c.find(&["description"])).unwrap_or_default();
    r.action.clone_from(&action);
    r.price = match kind {
        K::Buy | K::Sell | K::ReinvestedDividend => price.map(f64::abs),
        // Shares vested from an employer plan: Schwab's price, when given,
        // is the fair market value at vest, which is their cost basis.
        K::TransferIn if norm(&action) == "stock plan activity" => price.map(f64::abs),
        // A journal's or split's price is a valuation, not a cost.
        _ => None,
    };
    r.quantity = match kind {
        K::Buy | K::ReinvestedDividend | K::TransferIn => quantity.map(f64::abs),
        K::Sell | K::TransferOut => quantity.map(|q| -q.abs()),
        K::Split => quantity,
        _ => None,
    };
    if kind.moves_shares() {
        // A reverse split's old leg may carry the old CUSIP; it is paired
        // with the new symbol after the file is read.
        if kind == K::Split && norm(&action) == "reverse split" {
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
    out.push(Format::Schwab, rec, r);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_of_dates_use_the_event_date() {
        let d = |y, m, dd| chrono::NaiveDate::from_ymd_opt(y, m, dd).unwrap();
        assert_eq!(event_date("07/15/2024 as of 07/12/2024").unwrap(), d(2024, 7, 12));
        assert_eq!(event_date("07/15/2024").unwrap(), d(2024, 7, 15));
        // The listed (posting) date is the first one.
        assert_eq!(value::date("07/15/2024 as of 07/12/2024", DateOrder::MonthFirst).unwrap(), d(2024, 7, 15));
    }
}

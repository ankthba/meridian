//! Generic CSV through a user-chosen column mapping.
//!
//! With a date column, each row is a transaction whose kind comes from the
//! mapped action column by the keyword table in [`classify`] (shown to the
//! user in the preview before anything is saved). Without an action column,
//! a positive quantity is a buy and a negative one a sell. Without a date
//! column, the file is a positions snapshot. Numbers use a decimal point
//! unless the mapping says `decimal_comma`.

use super::positions::squash;
use super::{Columns, norm, num_in, opt_text, positions, signed};
use crate::csv::Record;
use crate::value::{self, DateOrder, Decimal};
use crate::{ColumnMapping, Format, ImportError, ImportOptions, Output, Row, TransactionKind};

/// Keyword classification of an action/type cell, checked in this order
/// (first match wins). `has_quantity` separates a reinvestment purchase
/// (shares) from the reinvested dividend's cash row, and transfers of
/// shares from transfers of cash.
#[must_use]
pub(crate) fn classify(action: &str, quantity: Option<f64>, amount: Option<f64>) -> Option<TransactionKind> {
    use TransactionKind as K;
    let a = norm(action);
    let words: Vec<&str> = a.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |w: &str| words.contains(&w);
    let starts = |p: &str| words.iter().any(|w| w.starts_with(p));
    let shares = quantity.is_some_and(|q| q != 0.0);
    Some(if starts("split") {
        K::Split
    } else if starts("reinvest") {
        if shares { K::ReinvestedDividend } else { K::Dividend }
    } else if has("fee") || has("fees") || has("commission") || has("tax") || has("taxes") {
        K::Fee
    } else if starts("dividend") || has("div") || has("distribution") || (has("cap") && has("gain")) || (has("capital") && starts("gain")) {
        K::Dividend
    } else if has("interest") {
        K::Interest
    } else if has("buy") || has("bought") || has("purchase") {
        K::Buy
    } else if has("sell") || has("sold") || has("sale") || has("redemption") {
        K::Sell
    } else if starts("transfer") {
        match (quantity.filter(|q| *q != 0.0), amount) {
            (Some(q), _) => {
                if q > 0.0 { K::TransferIn } else { K::TransferOut }
            }
            (None, Some(m)) if m > 0.0 => K::Deposit,
            (None, Some(m)) if m < 0.0 => K::Withdrawal,
            _ => return None,
        }
    } else if starts("deposit") || has("contribution") {
        K::Deposit
    } else if starts("withdraw") || has("disbursement") {
        K::Withdrawal
    } else {
        return None;
    })
}

fn synonyms(field: &str) -> &'static [&'static str] {
    match field {
        "trade_date" => &["trade date", "date", "activity date", "run date", "transaction date", "date/time"],
        "settle_date" => &["settle date", "settlement date"],
        "symbol" => &["symbol", "ticker", "instrument"],
        "action" => &["action", "transaction type", "type", "trans code", "activity"],
        "quantity" => &["quantity", "qty", "shares", "qty (quantity)", "units"],
        "price" => &["price", "price ($)", "share price", "unit price"],
        "amount" => &["amount", "amount ($)", "net amount", "total"],
        "fees" => &["fees", "fee", "fees & comm", "commission", "commission ($)", "commission fees"],
        "currency" => &["currency"],
        "description" => &["description", "security description", "transaction description", "name"],
        "cost_basis" => &["cost basis", "cost basis total", "total cost"],
        "average_cost" => &["average cost", "average cost basis", "avg cost", "cost per share"],
        _ => &[],
    }
}

/// A starting mapping from the header names; `recs` (read with
/// `delimiter`) is the file, used to propose `decimal_comma`.
pub(crate) fn suggest(recs: &[Record], delimiter: char, header_line: u32, headers: &[String]) -> ColumnMapping {
    // Compared without spaces and punctuation, so `TransactionDate` finds
    // "transaction date".
    let names: Vec<String> = headers.iter().map(|h| squash(h)).collect();
    let find = |field: &str| synonyms(field).iter().find_map(|s| names.iter().position(|n| *n == squash(s)));
    let mut m = ColumnMapping {
        header_line,
        trade_date: find("trade_date"),
        settle_date: find("settle_date"),
        symbol: find("symbol"),
        action: find("action"),
        quantity: find("quantity"),
        price: find("price"),
        amount: find("amount"),
        fees: find("fees"),
        currency: find("currency"),
        description: find("description"),
        cost_basis: find("cost_basis"),
        average_cost: find("average_cost"),
        default_currency: "USD".into(),
        day_first: false,
        decimal_comma: false,
    };
    // A comma-delimited file can't hold an unquoted decimal comma; in a
    // `;`- or tab-delimited one, `180,50` is one value.
    m.decimal_comma = delimiter != ',' && decimal_comma_likely(recs, &m);
    m
}

/// Whether the mapped number columns' cells mostly end in `,` and one or
/// two digits (`180,50`, `1.805,00`) rather than `.` and one or two digits.
/// When no number column was recognized (headers in another language),
/// every cell that is not a date votes.
fn decimal_comma_likely(recs: &[Record], m: &ColumnMapping) -> bool {
    let mapped: Vec<usize> = [m.amount, m.price, m.fees, m.cost_basis, m.average_cost, m.quantity].into_iter().flatten().collect();
    let start = recs.iter().position(|r| r.line == m.header_line).map_or(0, |i| i + 1);
    let (mut comma, mut point) = (0_usize, 0_usize);
    for rec in recs.iter().skip(start) {
        let all: Vec<usize>;
        let cols = if mapped.is_empty() {
            all = (0..rec.fields.len()).collect();
            &all
        } else {
            &mapped
        };
        for &c in cols {
            let cell = rec.get(c);
            if value::date(cell, DateOrder::DayFirst).is_ok() {
                continue;
            }
            if value::looks_decimal_comma(cell) {
                comma += 1;
            } else if value::looks_decimal_point(cell) {
                point += 1;
            }
        }
    }
    comma > point
}

fn check(m: &ColumnMapping, width: usize) -> Result<(), ImportError> {
    let cols = [
        ("date", m.trade_date),
        ("settle date", m.settle_date),
        ("symbol", m.symbol),
        ("action", m.action),
        ("quantity", m.quantity),
        ("price", m.price),
        ("amount", m.amount),
        ("fees", m.fees),
        ("currency", m.currency),
        ("description", m.description),
        ("cost basis", m.cost_basis),
        ("average cost", m.average_cost),
    ];
    for (name, c) in cols {
        if let Some(c) = c
            && c >= width
        {
            return Err(ImportError::BadMapping(format!("the {name} column ({}) is past the last header column ({width})", c + 1)));
        }
    }
    if m.trade_date.is_some() {
        if m.action.is_none() && m.quantity.is_none() {
            return Err(ImportError::BadMapping("map an action/type column or a quantity column".into()));
        }
    } else if m.symbol.is_none() || m.quantity.is_none() {
        return Err(ImportError::BadMapping("without a date column the file is read as positions: map the symbol and quantity columns".into()));
    }
    Ok(())
}

pub(crate) fn parse(recs: &[Record], header: usize, m: &ColumnMapping, opts: ImportOptions, out: &mut Output) -> Result<(), ImportError> {
    let width = Columns::new(&recs[header]).width().max(recs[header].fields.len());
    check(m, width)?;
    let order = if m.day_first { DateOrder::DayFirst } else { DateOrder::MonthFirst };
    let decimal = if m.decimal_comma { Decimal::Comma } else { Decimal::Point };
    let default_ccy = if m.default_currency.trim().is_empty() { "USD".to_owned() } else { m.default_currency.trim().to_ascii_uppercase() };
    for rec in &recs[header + 1..] {
        if rec.is_blank() {
            continue;
        }
        if m.trade_date.is_none() {
            let cols = positions::SnapshotCols {
                symbol: m.symbol,
                quantity: m.quantity,
                cost_basis: m.cost_basis,
                average_cost: m.average_cost,
                description: m.description,
                decimal,
                ..Default::default()
            };
            positions::snapshot_row(rec, Format::Mapped, cols, opts, &default_ccy, out);
            continue;
        }
        if let Err(e) = row(rec, m, order, decimal, &default_ccy, out) {
            out.warn(rec, e);
        }
    }
    Ok(())
}

fn row(rec: &Record, m: &ColumnMapping, order: DateOrder, decimal: Decimal, default_ccy: &str, out: &mut Output) -> Result<(), String> {
    let date_cell = m.trade_date.map_or("", |i| rec.get(i));
    let trade_date = value::date(date_cell, order).map_err(|e| format!("Not imported: unreadable {}", e.describe()))?;
    let settle_date = match m.settle_date.map(|i| rec.get(i)).filter(|s| !s.is_empty()) {
        Some(s) => Some(value::date(s, order).map_err(|e| format!("Not imported: unreadable settle {}", e.describe()))?),
        None => None,
    };
    let bad = |e: value::BadValue| format!("Not imported: unreadable {}", e.describe());
    let num = |i: Option<usize>, what: &'static str| num_in(rec, i, what, decimal).map_err(bad);
    let quantity = num(m.quantity, "quantity")?;
    let price = num(m.price, "price")?;
    let amount = num(m.amount, "amount")?;
    let fees = num(m.fees, "fees")?;
    let cost_basis = num(m.cost_basis, "cost basis")?;
    let action = opt_text(rec, m.action).unwrap_or_default();
    let symbol = m.symbol.and_then(|i| value::symbol(rec.get(i)));
    let kind = if m.action.is_some() {
        classify(&action, quantity, amount).ok_or_else(|| format!("Not imported: unrecognized action '{action}'"))?
    } else {
        match quantity {
            Some(q) if q > 0.0 => TransactionKind::Buy,
            Some(q) if q < 0.0 => TransactionKind::Sell,
            _ => return Err("Not imported: no action column and no quantity".into()),
        }
    };
    use TransactionKind as K;
    let mut r = Row::new(kind, trade_date);
    r.settle_date = settle_date;
    r.symbol = symbol;
    r.price = price.map(f64::abs);
    r.fees = fees.map(f64::abs);
    r.cost_basis = cost_basis.map(f64::abs);
    r.currency = opt_text(rec, m.currency).map_or_else(|| default_ccy.to_owned(), |c| c.to_ascii_uppercase());
    r.description = opt_text(rec, m.description).unwrap_or_default();
    r.action = action;
    match kind {
        K::Buy | K::ReinvestedDividend => {
            r.quantity = signed(quantity, true);
            r.amount = signed(amount, false);
        }
        K::Sell => {
            r.quantity = signed(quantity, false);
            r.amount = signed(amount, true);
        }
        K::TransferIn => r.quantity = signed(quantity, true),
        K::TransferOut => r.quantity = signed(quantity, false),
        K::Split => r.quantity = quantity,
        K::Fee | K::Withdrawal => r.amount = signed(amount, false),
        K::Deposit => r.amount = signed(amount, true),
        K::Dividend | K::Interest | K::Other => r.amount = amount,
    }
    if kind.moves_shares() && r.symbol.is_none() {
        return Err(format!("Not imported: {} without a symbol", kind.label().to_lowercase()));
    }
    if kind.moves_shares() && r.quantity.is_none_or(|q| q == 0.0) {
        return Err(format!("Not imported: {} without a quantity", kind.label().to_lowercase()));
    }
    if matches!(kind, K::Buy | K::Sell | K::ReinvestedDividend) && r.price.is_none() && r.amount.is_none() {
        return Err(format!("Not imported: {} without a price or amount", kind.label().to_lowercase()));
    }
    if !kind.moves_shares() && r.amount.is_none() {
        return Err(format!("Not imported: {} without an amount", kind.label().to_lowercase()));
    }
    out.push(Format::Mapped, rec, r);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_table() {
        use TransactionKind as K;
        let c = |a: &str, q: Option<f64>, m: Option<f64>| classify(a, q, m);
        assert_eq!(c("Buy", None, None), Some(K::Buy));
        assert_eq!(c("YOU BOUGHT", None, None), Some(K::Buy));
        assert_eq!(c("Sell", None, None), Some(K::Sell));
        assert_eq!(c("Cash Dividend", None, None), Some(K::Dividend));
        assert_eq!(c("Reinvest Shares", Some(1.2), None), Some(K::ReinvestedDividend));
        assert_eq!(c("Qual Div Reinvest", None, Some(5.0)), Some(K::Dividend));
        assert_eq!(c("Long Term Cap Gain", None, Some(5.0)), Some(K::Dividend));
        assert_eq!(c("Foreign Tax Paid", None, Some(-1.0)), Some(K::Fee));
        assert_eq!(c("ADR Mgmt Fee", None, Some(-1.0)), Some(K::Fee));
        assert_eq!(c("Bank Interest", None, Some(1.0)), Some(K::Interest));
        assert_eq!(c("Stock Split", Some(10.0), None), Some(K::Split));
        assert_eq!(c("Transfer", Some(10.0), None), Some(K::TransferIn));
        assert_eq!(c("Transfer", Some(-10.0), None), Some(K::TransferOut));
        assert_eq!(c("Transfer", None, Some(100.0)), Some(K::Deposit));
        assert_eq!(c("Transfer", None, Some(-100.0)), Some(K::Withdrawal));
        assert_eq!(c("Transfer", None, None), None);
        assert_eq!(c("Deposit", None, Some(1.0)), Some(K::Deposit));
        assert_eq!(c("Withdrawal", None, Some(-1.0)), Some(K::Withdrawal));
        assert_eq!(c("Journal", None, Some(1.0)), None);
        assert_eq!(c("", None, None), None);
    }

    #[test]
    fn suggestion_uses_header_names() {
        let h: Vec<String> = ["Date", "Type", "Ticker", "Shares", "Price", "Total", "Notes"].iter().map(|s| (*s).to_string()).collect();
        let m = suggest(&[], ',', 3, &h);
        assert_eq!(m.header_line, 3);
        assert_eq!((m.trade_date, m.action, m.symbol, m.quantity, m.price, m.amount), (Some(0), Some(1), Some(2), Some(3), Some(4), Some(5)));
        assert_eq!((m.fees, m.description, m.cost_basis), (None, None, None));
    }
}

//! Per-broker readers. Each one knows its header signature, its code or
//! action table, and its preamble/footer quirks.

pub(crate) mod fidelity;
pub(crate) mod mapped;
pub(crate) mod positions;
pub(crate) mod robinhood;
pub(crate) mod schwab;
pub(crate) mod vanguard;

use crate::csv::Record;
use crate::value::{self, BadValue, Decimal};
use crate::{Format, ImportOptions, Output};

/// Lower-cased, whitespace-collapsed header name.
pub(crate) fn norm(s: &str) -> String {
    value::text(s).to_lowercase()
}

/// Header-name lookup for one header row.
#[derive(Debug, Clone)]
pub(crate) struct Columns {
    names: Vec<String>,
}

impl Columns {
    pub(crate) fn new(header: &Record) -> Self {
        Self { names: header.fields.iter().map(|f| norm(f)).collect() }
    }

    /// Index of the first column whose name is any of `names`.
    pub(crate) fn find(&self, names: &[&str]) -> Option<usize> {
        names.iter().find_map(|n| self.names.iter().position(|h| h == n))
    }

    /// Whether every one of `names` is present.
    pub(crate) fn has_all(&self, names: &[&str]) -> bool {
        names.iter().all(|n| self.names.iter().any(|h| h == n))
    }

    /// Whether any header name satisfies `f`.
    pub(crate) fn any(&self, f: impl Fn(&str) -> bool) -> bool {
        self.names.iter().any(|n| f(n))
    }

    /// Number of non-empty header names.
    pub(crate) fn width(&self) -> usize {
        self.names.iter().filter(|n| !n.is_empty()).count()
    }
}

pub(crate) fn header_matches(f: Format, r: &Record) -> bool {
    let c = Columns::new(r);
    match f {
        Format::Robinhood => robinhood::matches(&c),
        Format::Fidelity => fidelity::matches(&c),
        Format::Schwab => schwab::matches(&c),
        Format::Vanguard => vanguard::matches(&c),
        Format::Positions => positions::matches(&c),
        Format::Mapped => false,
    }
}

pub(crate) fn parse(f: Format, recs: &[Record], header: usize, opts: ImportOptions, out: &mut Output) {
    match f {
        Format::Robinhood => robinhood::parse(recs, header, out),
        Format::Fidelity => fidelity::parse(recs, header, out),
        Format::Schwab => schwab::parse(recs, header, out),
        Format::Vanguard => vanguard::parse(recs, header, out),
        Format::Positions => positions::parse(recs, header, opts, out),
        Format::Mapped => {}
    }
}

/// Reads optional numeric cell `i` (absent column = `None`).
pub(crate) fn num(rec: &Record, i: Option<usize>, what: &'static str) -> Result<Option<f64>, BadValue> {
    num_in(rec, i, what, Decimal::Point)
}

/// [`num`] with the file's decimal separator (mapped files).
pub(crate) fn num_in(rec: &Record, i: Option<usize>, what: &'static str, decimal: Decimal) -> Result<Option<f64>, BadValue> {
    match i {
        Some(i) => value::number_in(rec.get(i), what, decimal),
        None => Ok(None),
    }
}

/// Reads optional text cell `i`, collapsed; empty becomes `None`.
pub(crate) fn opt_text(rec: &Record, i: Option<usize>) -> Option<String> {
    i.map(|i| value::text(rec.get(i))).filter(|s| !s.is_empty())
}

/// Nine alphanumerics with at least three digits: the shape of a CUSIP
/// (bonds, CDs, Treasuries, or the old leg of a corporate action).
/// Tickers are at most five letters plus a share class.
pub(crate) fn is_cusip(s: &str) -> bool {
    s.len() == 9 && s.bytes().all(|b| b.is_ascii_alphanumeric()) && s.bytes().filter(u8::is_ascii_digit).count() >= 3
}

/// Option symbols: Schwab's `SPY 03/31/2020 284.00 P`, OCC-like
/// `TSLA  230317P00180000`, Fidelity's `-XLE260731C55`.
pub(crate) fn is_option_symbol(s: &str) -> bool {
    s.contains(' ') || s.starts_with('-')
}

/// A stock or fund ticker (not an option or a CUSIP).
pub(crate) fn is_ticker(s: &str) -> bool {
    !s.is_empty() && !is_option_symbol(s) && !is_cusip(s)
}

/// Checks a share-moving row: it needs a symbol that is a stock or fund
/// ticker (not an option or a CUSIP) and a non-zero quantity. `what` names
/// the row's action in the message.
pub(crate) fn check_shares(r: &crate::Row, what: &str) -> Result<(), String> {
    let Some(sym) = r.symbol.as_deref() else {
        return Err(format!("Not imported: {what} without a symbol"));
    };
    if is_option_symbol(sym) {
        return Err(format!("Not imported: {what} {sym} looks like an option; options are not supported"));
    }
    if is_cusip(sym) {
        return Err(format!("Not imported: {what} {sym} is a CUSIP (bond, CD or Treasury); fixed income is not supported"));
    }
    if r.quantity.is_none_or(|q| q == 0.0) {
        return Err(format!("Not imported: {what} without a quantity"));
    }
    Ok(())
}

/// Reverse splits arrive as two rows on one date (new shares +, old shares
/// −) and a leg's symbol is often a CUSIP. Rows from `start` on that
/// `is_reverse` selects are grouped by account and date; when a group has
/// exactly one ticker, every row takes it so the rows net into one change
/// (see `net_splits`). Rows still without a ticker become warnings.
pub(crate) fn pair_reverse_splits(out: &mut Output, start: usize, is_reverse: impl Fn(&crate::ImportedTx) -> bool) {
    use std::collections::BTreeMap;
    let idx: Vec<usize> = (start..out.txs.len()).filter(|&i| is_reverse(&out.txs[i])).collect();
    let mut groups: BTreeMap<(Option<String>, chrono::NaiveDate), Vec<usize>> = BTreeMap::new();
    for &i in &idx {
        groups.entry((out.txs[i].account.clone(), out.txs[i].trade_date)).or_default().push(i);
    }
    for rows in groups.values() {
        let mut tickers: Vec<String> = rows.iter().filter_map(|&i| out.txs[i].symbol.clone()).filter(|s| is_ticker(s)).collect();
        tickers.sort();
        tickers.dedup();
        if let [ticker] = tickers.as_slice() {
            for &i in rows {
                out.txs[i].symbol = Some(ticker.clone());
            }
        }
    }
    for &i in idx.iter().rev() {
        if !out.txs[i].symbol.as_deref().is_some_and(is_ticker) {
            let t = out.txs.remove(i);
            out.warnings.push(crate::ImportWarning {
                line: t.line,
                message: format!(
                    "Not imported: reverse split leg {} has no ticker to pair with; adjust the holding by hand",
                    t.symbol.as_deref().unwrap_or("(no symbol)")
                ),
                text: t.description,
            });
        }
    }
}

/// Magnitude helper: brokers differ on whether a sell's quantity is
/// negative, so kinds with a known direction take the absolute value and
/// apply their own sign.
pub(crate) fn signed(v: Option<f64>, positive: bool) -> Option<f64> {
    v.map(|x| if positive { x.abs() } else { -x.abs() })
}

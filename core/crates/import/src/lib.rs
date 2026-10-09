//! Broker CSV import for portfolios.
//!
//! [`parse`] detects the export format from its header row (Robinhood,
//! Fidelity, Charles Schwab, Vanguard, or a positions snapshot) or applies a
//! user-chosen [`ColumnMapping`], and returns normalized transactions plus
//! warnings. Rows that can't be read or aren't supported become warnings
//! with their line number; nothing is dropped silently and nothing is
//! guessed. Sources for every format are listed in this crate's README.
//!
//! The crate is pure: no I/O and no clock. The caller supplies the date a
//! positions snapshot is recorded on.

pub mod csv;
pub mod fingerprint;
mod formats;
pub mod value;

use chrono::NaiveDate;
pub use meridian_types::TransactionKind;

use crate::csv::Record;
use crate::fingerprint::{Fingerprinter, Parts};

/// A recognized export format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// Robinhood account activity report.
    Robinhood,
    /// Fidelity account history download.
    Fidelity,
    /// Charles Schwab brokerage transaction history export.
    Schwab,
    /// Vanguard brokerage transaction download.
    Vanguard,
    /// Holdings snapshot: symbol, quantity, optional cost basis.
    Positions,
    /// Any CSV read through a user-chosen [`ColumnMapping`].
    Mapped,
}

impl Format {
    /// Stable identifier (stored with imported rows).
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Format::Robinhood => "robinhood",
            Format::Fidelity => "fidelity",
            Format::Schwab => "schwab",
            Format::Vanguard => "vanguard",
            Format::Positions => "positions",
            Format::Mapped => "mapped",
        }
    }

    /// Display name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Format::Robinhood => "Robinhood account activity",
            Format::Fidelity => "Fidelity account history",
            Format::Schwab => "Charles Schwab transactions",
            Format::Vanguard => "Vanguard transactions",
            Format::Positions => "Positions snapshot",
            Format::Mapped => "Mapped columns",
        }
    }

    /// The formats detected automatically, in detection priority order.
    pub const DETECTED: [Format; 5] = [Format::Robinhood, Format::Fidelity, Format::Schwab, Format::Vanguard, Format::Positions];
}

/// One normalized transaction.
///
/// Signs: `quantity` is the change in shares (buy +, sell −, split ±),
/// `amount` is the cash flow (+ into the account, − out of it), `fees` is
/// a positive cost. See [`TransactionKind`] for how each kind is treated.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedTx {
    /// 1-based line the row starts on.
    pub line: u32,
    pub trade_date: NaiveDate,
    pub settle_date: Option<NaiveDate>,
    /// Ticker as the broker wrote it, upper-cased.
    pub symbol: Option<String>,
    pub kind: TransactionKind,
    pub quantity: Option<f64>,
    pub price: Option<f64>,
    pub amount: Option<f64>,
    pub fees: Option<f64>,
    /// Total cost basis stated by the source (positions snapshots).
    pub cost_basis: Option<f64>,
    /// ISO 4217 code.
    pub currency: String,
    /// The broker's description, whitespace collapsed.
    pub description: String,
    /// The broker's action or code text (`Buy`, `CDIV`, `YOU BOUGHT …`).
    pub action: String,
    /// Account name or number when the export has one. Shown to the user;
    /// not part of the fingerprint (see `fingerprint.rs`).
    pub account: Option<String>,
    /// Stable identifier for de-duplication on re-import.
    pub fingerprint: String,
}

/// A row that was not imported, or a note about the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportWarning {
    /// 1-based line (0 for the file as a whole).
    pub line: u32,
    pub message: String,
    /// The row's text, truncated.
    pub text: String,
}

/// Result of reading a file.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedImport {
    pub format: Format,
    /// 1-based line of the header row.
    pub header_line: u32,
    pub headers: Vec<String>,
    pub transactions: Vec<ImportedTx>,
    pub warnings: Vec<ImportWarning>,
    /// The file is a holdings snapshot (every row a `TransferIn` dated on
    /// the import day), not a transaction history.
    pub snapshot: bool,
}

impl ParsedImport {
    /// Number of transactions of each kind, in [`TransactionKind::ALL`]
    /// order, omitting zero counts.
    #[must_use]
    pub fn counts(&self) -> Vec<(TransactionKind, usize)> {
        TransactionKind::ALL
            .into_iter()
            .map(|k| (k, self.transactions.iter().filter(|t| t.kind == k).count()))
            .filter(|(_, n)| *n > 0)
            .collect()
    }
}

/// How a user mapped an unrecognized file's columns. Column numbers are
/// 0-based positions in the header row.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ColumnMapping {
    /// 1-based line of the header row; data starts after it.
    pub header_line: u32,
    /// Without a date column the file is read as a positions snapshot.
    pub trade_date: Option<usize>,
    pub settle_date: Option<usize>,
    pub symbol: Option<usize>,
    /// Column describing the transaction type (`Buy`, `Dividend` …).
    pub action: Option<usize>,
    pub quantity: Option<usize>,
    pub price: Option<usize>,
    pub amount: Option<usize>,
    pub fees: Option<usize>,
    pub currency: Option<usize>,
    pub description: Option<usize>,
    /// Total cost basis (positions snapshots and transfers in).
    pub cost_basis: Option<usize>,
    /// Average cost per share (positions snapshots).
    pub average_cost: Option<usize>,
    /// Currency when there is no currency column. Empty means USD.
    pub default_currency: String,
    /// Dates are `DD/MM/YYYY` rather than `MM/DD/YYYY`.
    pub day_first: bool,
}

/// Inputs the caller supplies (the crate has no clock).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportOptions {
    /// Date recorded for positions-snapshot holdings (normally today).
    pub as_of: NaiveDate,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImportError {
    #[error("the file is empty")]
    Empty,
    #[error("the file's format was not recognized; map its columns to import it")]
    Unrecognized {
        /// Best guess at the header row (first row with several cells).
        header_line: u32,
        headers: Vec<String>,
    },
    #[error("{0}")]
    BadMapping(String),
}

/// Result of format detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub format: Option<Format>,
    /// 1-based line of the header row (the best guess when unrecognized).
    pub header_line: u32,
    pub headers: Vec<String>,
}

/// Reads the CSV text into records with the sniffed delimiter.
fn records(text: &str) -> Vec<Record> {
    csv::read(text, csv::sniff_delimiter(text))
}

fn headers_of(r: &Record) -> Vec<String> {
    let mut h: Vec<String> = r.fields.iter().map(|f| value::text(f)).collect();
    // A trailing comma adds an empty last column; drop it.
    while h.last().is_some_and(String::is_empty) {
        h.pop();
    }
    h
}

/// First record with at least three non-empty cells, else the first
/// non-blank one.
fn guess_header(recs: &[Record]) -> Option<usize> {
    recs.iter().position(|r| r.filled() >= 3).or_else(|| recs.iter().position(|r| !r.is_blank()))
}

fn detect_in(recs: &[Record]) -> Option<(Format, usize)> {
    for f in Format::DETECTED {
        if let Some(i) = recs.iter().position(|r| formats::header_matches(f, r)) {
            return Some((f, i));
        }
    }
    None
}

/// Detects the format from the header row.
#[must_use]
pub fn detect(text: &str) -> Detection {
    let recs = records(text);
    let (format, i) = match detect_in(&recs) {
        Some((f, i)) => (Some(f), Some(i)),
        None => (None, guess_header(&recs)),
    };
    Detection { format, header_line: i.map_or(0, |i| recs[i].line), headers: i.map(|i| headers_of(&recs[i])).unwrap_or_default() }
}

/// Parses an export. With `mapping`, the file is read through it whatever
/// its format; without, the format is detected from the header row.
pub fn parse(text: &str, mapping: Option<&ColumnMapping>, opts: &ImportOptions) -> Result<ParsedImport, ImportError> {
    let recs = records(text);
    if recs.iter().all(Record::is_blank) {
        return Err(ImportError::Empty);
    }
    let mut out = Output::default();
    let (format, header_idx) = if let Some(m) = mapping {
        let idx = recs
            .iter()
            .position(|r| r.line == m.header_line)
            .ok_or_else(|| ImportError::BadMapping(format!("there is no row starting on line {}", m.header_line)))?;
        formats::mapped::parse(&recs, idx, m, *opts, &mut out)?;
        (Format::Mapped, idx)
    } else {
        let Some((f, idx)) = detect_in(&recs) else {
            let d = guess_header(&recs);
            return Err(ImportError::Unrecognized {
                header_line: d.map_or(0, |i| recs[i].line),
                headers: d.map(|i| headers_of(&recs[i])).unwrap_or_default(),
            });
        };
        formats::parse(f, &recs, idx, *opts, &mut out);
        (f, idx)
    };
    chronological(&mut out.txs, format);
    net_splits(&mut out.txs);
    out.warnings.sort_by_key(|w| w.line);
    if out.txs.is_empty() && out.warnings.is_empty() {
        out.warnings.push(ImportWarning { line: 0, message: "The file has a header row but no transactions".into(), text: String::new() });
    }
    Ok(ParsedImport {
        format,
        header_line: recs[header_idx].line,
        headers: headers_of(&recs[header_idx]),
        transactions: out.txs,
        warnings: out.warnings,
        snapshot: format == Format::Positions || mapping.is_some_and(|m| m.trade_date.is_none()),
    })
}

/// Puts rows in trade-date order. Robinhood, Fidelity and Schwab list
/// newest first, so their files are reversed before the stable sort (unless
/// the file was re-sorted oldest first); that keeps same-day rows in the
/// order they happened. Other files keep their own same-day order.
fn chronological(txs: &mut [ImportedTx], format: Format) {
    let newest_first = matches!(format, Format::Robinhood | Format::Fidelity | Format::Schwab);
    if newest_first
        && let (Some(f), Some(l)) = (txs.first(), txs.last())
        && f.trade_date >= l.trade_date
    {
        txs.reverse();
    }
    txs.sort_by_key(|t| t.trade_date);
}

/// Merges split rows for the same account, symbol and date into one net
/// change, so a reverse split's "remove old shares" and "add new shares"
/// legs never pass through an empty position.
fn net_splits(txs: &mut Vec<ImportedTx>) {
    let mut out: Vec<ImportedTx> = Vec::with_capacity(txs.len());
    for t in txs.drain(..) {
        if t.kind == TransactionKind::Split
            && let Some(prev) = out
                .iter_mut()
                .rev()
                .take_while(|p| p.trade_date == t.trade_date)
                .find(|p| p.kind == TransactionKind::Split && p.symbol == t.symbol && p.account == t.account)
        {
            prev.quantity = Some(prev.quantity.unwrap_or(0.0) + t.quantity.unwrap_or(0.0));
            if !t.description.is_empty() && t.description != prev.description {
                prev.description = format!("{} / {}", prev.description, t.description);
            }
            continue;
        }
        out.push(t);
    }
    *txs = out;
}

/// A starting mapping for the column-mapping UI, chosen by header names.
/// The user confirms or changes it; nothing is imported from a guess.
#[must_use]
pub fn suggest_mapping(header_line: u32, headers: &[String]) -> ColumnMapping {
    formats::mapped::suggest(header_line, headers)
}

/// Accumulates parsed rows and warnings.
#[derive(Debug, Default)]
pub(crate) struct Output {
    pub(crate) txs: Vec<ImportedTx>,
    pub(crate) warnings: Vec<ImportWarning>,
    fp: Fingerprinter,
}

/// A row ready to be fingerprinted and stored.
#[derive(Debug, Clone)]
pub(crate) struct Row {
    pub(crate) kind: TransactionKind,
    pub(crate) trade_date: NaiveDate,
    pub(crate) settle_date: Option<NaiveDate>,
    pub(crate) symbol: Option<String>,
    pub(crate) quantity: Option<f64>,
    pub(crate) price: Option<f64>,
    pub(crate) amount: Option<f64>,
    pub(crate) fees: Option<f64>,
    pub(crate) cost_basis: Option<f64>,
    pub(crate) currency: String,
    pub(crate) description: String,
    pub(crate) action: String,
    pub(crate) account: Option<String>,
    /// Holdings snapshot row: the date is left out of the fingerprint so
    /// importing the same snapshot again on another day is a duplicate.
    pub(crate) snapshot: bool,
}

impl Row {
    pub(crate) fn new(kind: TransactionKind, trade_date: NaiveDate) -> Self {
        Self {
            kind,
            trade_date,
            settle_date: None,
            symbol: None,
            quantity: None,
            price: None,
            amount: None,
            fees: None,
            cost_basis: None,
            currency: "USD".into(),
            description: String::new(),
            action: String::new(),
            account: None,
            snapshot: false,
        }
    }
}

impl Output {
    pub(crate) fn warn(&mut self, rec: &Record, message: impl Into<String>) {
        self.warnings.push(ImportWarning { line: rec.line, message: message.into(), text: rec.excerpt() });
    }

    pub(crate) fn note(&mut self, line: u32, message: impl Into<String>) {
        self.warnings.push(ImportWarning { line, message: message.into(), text: String::new() });
    }

    /// Fingerprints `row` under `family` and keeps it.
    pub(crate) fn push(&mut self, family: Format, rec: &Record, row: Row) {
        let date = if row.snapshot { String::new() } else { row.trade_date.format("%Y-%m-%d").to_string() };
        let fingerprint = self.fp.next(&Parts {
            family: family.id(),
            date: &date,
            action: &row.action,
            symbol: row.symbol.as_deref().unwrap_or(""),
            quantity: row.quantity,
            price: row.price,
            amount: row.amount,
            extra: row.cost_basis,
        });
        self.txs.push(ImportedTx {
            line: rec.line,
            trade_date: row.trade_date,
            settle_date: row.settle_date,
            symbol: row.symbol,
            kind: row.kind,
            quantity: row.quantity,
            price: row.price,
            amount: row.amount,
            fees: row.fees,
            cost_basis: row.cost_basis,
            currency: row.currency,
            description: row.description,
            action: row.action,
            account: row.account,
            fingerprint,
        });
    }
}

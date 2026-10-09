//! Broker CSV import into portfolios: preview (parse only) and commit
//! (parse, convert to store rows, insert with de-duplication).

use meridian_import::{ColumnMapping, Format, ImportError, ImportOptions, ImportWarning, ImportedTx, ParsedImport};
use meridian_store::{PortfolioTarget, Transaction};
use meridian_types::{SecurityKey, TransactionKind, nanos_to_date};

use crate::core::Engine;
use crate::error::{EngineError, EngineResult};

/// What a file would import, without saving anything.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportPreview {
    /// `None` when the format was not recognized and no mapping was given:
    /// the app should offer the column-mapping UI.
    pub parsed: Option<ParsedImport>,
    pub header_line: u32,
    pub headers: Vec<String>,
    /// Starting point for the column-mapping UI.
    pub suggested_mapping: ColumnMapping,
    /// Present when `parsed` is `None`.
    pub warnings: Vec<ImportWarning>,
}

/// Where to import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportTarget {
    Existing(i64),
    New(String),
}

/// What a commit did.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportOutcome {
    pub portfolio_id: i64,
    pub portfolio_name: String,
    pub format: Format,
    pub imported: usize,
    pub duplicates: usize,
    pub warnings: Vec<ImportWarning>,
}

/// Security key for a broker ticker. Share classes written `BRK.B` or
/// `BRK B` become the terminal's `BRK/B`.
#[must_use]
pub fn security_for(symbol: &str) -> Option<String> {
    let s: String = symbol.trim().to_ascii_uppercase().chars().map(|c| if c == '.' || c == ' ' { '/' } else { c }).collect();
    if s.is_empty() || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '/' || c == '-') {
        return None;
    }
    Some(SecurityKey::equity(&s).to_string())
}

/// Store row for an imported transaction. A transfer in carries its cost
/// basis as the per-share price: a stated total cost basis wins, because a
/// mapped file's price column on a transfer is usually the market value on
/// the transfer day; the row's price is used only without one (an RSU or
/// stock-plan vest, whose price is the value at vest).
#[must_use]
pub fn to_store(t: &ImportedTx, format: Format) -> Transaction {
    let quantity = t.quantity.unwrap_or(0.0);
    let price = match t.kind {
        TransactionKind::TransferIn => t.cost_basis.filter(|_| quantity.abs() > 0.0).map(|c| c / quantity.abs()).or(t.price),
        _ => t.price,
    };
    let note = match (t.description.is_empty(), t.action.is_empty()) {
        (false, _) => Some(t.description.clone()),
        (true, false) => Some(t.action.clone()),
        (true, true) => None,
    };
    Transaction {
        id: 0,
        portfolio_id: 0,
        security: t.symbol.as_deref().and_then(security_for),
        kind: t.kind,
        trade_date: t.trade_date.format("%Y-%m-%d").to_string(),
        settle_date: t.settle_date.map(|d| d.format("%Y-%m-%d").to_string()),
        quantity,
        price,
        amount: t.amount,
        fees: t.fees.unwrap_or(0.0),
        currency: Some(t.currency.clone()),
        note,
        source: Some(format.id().to_owned()),
        fingerprint: Some(t.fingerprint.clone()),
    }
}

impl Engine {
    fn import_options(&self) -> ImportOptions {
        ImportOptions { as_of: nanos_to_date(self.now()) }
    }

    /// Parses `csv` (detected, or through `mapping`) without saving.
    pub fn preview_import(&self, csv: &str, mapping: Option<&ColumnMapping>) -> EngineResult<ImportPreview> {
        match meridian_import::parse(csv, mapping, &self.import_options()) {
            Ok(p) => Ok(ImportPreview {
                header_line: p.header_line,
                headers: p.headers.clone(),
                suggested_mapping: mapping.cloned().unwrap_or_else(|| meridian_import::suggest_mapping(csv, p.header_line, &p.headers)),
                parsed: Some(p),
                warnings: Vec::new(),
            }),
            Err(ImportError::Unrecognized { header_line, headers }) => Ok(ImportPreview {
                parsed: None,
                suggested_mapping: meridian_import::suggest_mapping(csv, header_line, &headers),
                warnings: vec![ImportWarning {
                    line: header_line,
                    message: "Format not recognized (supported: Robinhood, Fidelity, Charles Schwab, Vanguard, positions snapshots). Map the columns to import it.".into(),
                    text: headers.join(","),
                }],
                header_line,
                headers,
            }),
            Err(e) => Err(EngineError::InvalidInput(e.to_string())),
        }
    }

    /// Imports `csv` into an existing or new portfolio. Rows already
    /// imported into that portfolio (same fingerprint) are skipped.
    pub fn commit_import(&self, csv: &str, mapping: Option<&ColumnMapping>, target: &ImportTarget) -> EngineResult<ImportOutcome> {
        let parsed = meridian_import::parse(csv, mapping, &self.import_options()).map_err(|e| EngineError::InvalidInput(e.to_string()))?;
        let store = &self.stores().app;
        let rows: Vec<Transaction> = parsed.transactions.iter().map(|t| to_store(t, parsed.format)).collect();
        let mut warnings = parsed.warnings.clone();
        let mut existing = 0;
        let (store_target, name) = match target {
            ImportTarget::Existing(id) => {
                let p = store.portfolio(*id)?.ok_or_else(|| EngineError::InvalidInput(format!("there is no portfolio {id}")))?;
                existing = store.transaction_count(*id, None)?;
                let foreign = rows.iter().filter(|r| r.currency.as_deref().is_some_and(|c| !c.eq_ignore_ascii_case(&p.base_currency))).count();
                if foreign > 0 {
                    warnings.push(ImportWarning {
                        line: 0,
                        message: format!("{foreign} rows are not in {}; PORT lists them but leaves them out of totals (no FX conversion)", p.base_currency),
                        text: String::new(),
                    });
                }
                (PortfolioTarget::Existing(*id), p.name)
            }
            ImportTarget::New(name) => {
                let name = name.trim();
                if name.is_empty() {
                    return Err(EngineError::InvalidInput("name the new portfolio".into()));
                }
                if store.portfolios()?.iter().any(|p| p.name.eq_ignore_ascii_case(name)) {
                    return Err(EngineError::InvalidInput(format!("a portfolio named {name} already exists")));
                }
                (PortfolioTarget::New { name, base_currency: "" }, name.to_owned())
            }
        };
        // A new portfolio takes the file's currency when it has only one.
        let base = {
            let mut c: Vec<&str> = rows.iter().filter_map(|r| r.currency.as_deref()).collect();
            c.sort_unstable();
            c.dedup();
            if c.len() == 1 { c[0].to_owned() } else { "USD".to_owned() }
        };
        let store_target = match store_target {
            PortfolioTarget::New { name, .. } => PortfolioTarget::New { name, base_currency: &base },
            t @ PortfolioTarget::Existing(_) => t,
        };
        let counts = store.import_transactions(store_target, &rows)?;
        if parsed.snapshot && existing > 0 && counts.inserted > 0 {
            warnings.push(ImportWarning {
                line: 0,
                message: format!(
                    "{name} already had {existing} transactions; the snapshot's holdings were added to them and may be counted twice. Import a snapshot into a new portfolio to replace holdings."
                ),
                text: String::new(),
            });
        }
        Ok(ImportOutcome {
            portfolio_id: counts.portfolio_id,
            portfolio_name: name,
            format: parsed.format,
            imported: counts.inserted,
            duplicates: counts.duplicates,
            warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_classes_and_bad_symbols() {
        assert_eq!(security_for("aapl").as_deref(), Some("AAPL US Equity"));
        assert_eq!(security_for("BRK.B").as_deref(), Some("BRK/B US Equity"));
        assert_eq!(security_for("BRK B").as_deref(), Some("BRK/B US Equity"));
        assert_eq!(security_for(""), None);
        assert_eq!(security_for("A$B"), None);
    }

    #[test]
    fn stated_cost_basis_wins_over_a_transfers_price() {
        // A mapped transfer in with a market-value price column and a cost
        // basis column: 10 shares worth 50 that cost 300 in total.
        let csv = "Date,Type,Symbol,Qty,Price,Cost Basis\n2026-03-02,Transfer in,AAPL,10,50.00,300.00\n2026-03-02,Transfer in,MSFT,2,400.00,\n";
        let m = ColumnMapping { header_line: 1, trade_date: Some(0), action: Some(1), symbol: Some(2), quantity: Some(3), price: Some(4), cost_basis: Some(5), ..Default::default() };
        let opts = ImportOptions { as_of: chrono::NaiveDate::from_ymd_opt(2026, 10, 7).expect("date") };
        let p = meridian_import::parse(csv, Some(&m), &opts).expect("parse");
        let rows: Vec<Transaction> = p.transactions.iter().map(|t| to_store(t, p.format)).collect();
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].kind, rows[0].quantity, rows[0].price), (TransactionKind::TransferIn, 10.0, Some(30.0)));
        // Without a cost basis the row's price is all there is.
        assert_eq!((rows[1].quantity, rows[1].price), (2.0, Some(400.0)));
        let l = crate::portfolio::ledger(&rows, "USD");
        assert!((l.holdings[0].cost - 300.0).abs() < 1e-9);
    }
}

//! Portfolio transaction kinds shared by the importer, the store and the
//! PORT screen.

use serde::{Deserialize, Serialize};

/// What a portfolio transaction does. Quantities are signed share deltas
/// and amounts are signed cash flows (positive = cash into the account),
/// so each kind only says how the row is treated:
///
/// | Kind | Shares | Cash | P&L treatment |
/// |---|---|---|---|
/// | `Buy` | + | − | adds to cost basis |
/// | `Sell` | − | + | realizes P&L against average cost |
/// | `Dividend` | | + | income (negative = withholding reversed) |
/// | `ReinvestedDividend` | + | − | shares bought with a dividend; adds to cost basis (the dividend itself is its own `Dividend` row) |
/// | `Interest` | | ± | income |
/// | `Fee` | | − | expense (fees, taxes withheld) |
/// | `Split` | ± | | changes shares, not cost |
/// | `TransferIn` | + | | adds shares at the stated cost basis, if any |
/// | `TransferOut` | − | | removes shares at average cost, no P&L |
/// | `Deposit` / `Withdrawal` | | ± | external cash flow, not P&L |
/// | `Other` | | | listed, but not part of holdings or P&L |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TransactionKind {
    Buy,
    Sell,
    Dividend,
    ReinvestedDividend,
    Interest,
    Fee,
    Split,
    TransferIn,
    TransferOut,
    Deposit,
    Withdrawal,
    Other,
}

impl TransactionKind {
    pub const ALL: [TransactionKind; 12] = [
        TransactionKind::Buy,
        TransactionKind::Sell,
        TransactionKind::Dividend,
        TransactionKind::ReinvestedDividend,
        TransactionKind::Interest,
        TransactionKind::Fee,
        TransactionKind::Split,
        TransactionKind::TransferIn,
        TransactionKind::TransferOut,
        TransactionKind::Deposit,
        TransactionKind::Withdrawal,
        TransactionKind::Other,
    ];

    /// Stable identifier used in storage. Never change an existing code.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            TransactionKind::Buy => "buy",
            TransactionKind::Sell => "sell",
            TransactionKind::Dividend => "dividend",
            TransactionKind::ReinvestedDividend => "reinvest",
            TransactionKind::Interest => "interest",
            TransactionKind::Fee => "fee",
            TransactionKind::Split => "split",
            TransactionKind::TransferIn => "transfer_in",
            TransactionKind::TransferOut => "transfer_out",
            TransactionKind::Deposit => "deposit",
            TransactionKind::Withdrawal => "withdrawal",
            TransactionKind::Other => "other",
        }
    }

    #[must_use]
    pub fn from_code(code: &str) -> Option<TransactionKind> {
        TransactionKind::ALL.into_iter().find(|k| k.code() == code)
    }

    /// Sentence-case label for screens.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            TransactionKind::Buy => "Buy",
            TransactionKind::Sell => "Sell",
            TransactionKind::Dividend => "Dividend",
            TransactionKind::ReinvestedDividend => "Reinvested dividend",
            TransactionKind::Interest => "Interest",
            TransactionKind::Fee => "Fee",
            TransactionKind::Split => "Split",
            TransactionKind::TransferIn => "Transfer in",
            TransactionKind::TransferOut => "Transfer out",
            TransactionKind::Deposit => "Deposit",
            TransactionKind::Withdrawal => "Withdrawal",
            TransactionKind::Other => "Other",
        }
    }

    /// Whether the kind changes a share position.
    #[must_use]
    pub fn moves_shares(self) -> bool {
        matches!(
            self,
            TransactionKind::Buy
                | TransactionKind::Sell
                | TransactionKind::ReinvestedDividend
                | TransactionKind::Split
                | TransactionKind::TransferIn
                | TransactionKind::TransferOut
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip_and_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for k in TransactionKind::ALL {
            assert_eq!(TransactionKind::from_code(k.code()), Some(k));
            assert!(seen.insert(k.code()), "duplicate code {}", k.code());
        }
        assert_eq!(TransactionKind::from_code("nope"), None);
    }
}

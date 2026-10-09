//! Portfolio accounting from a transaction list: holdings at average cost,
//! realized P&L, income, fees and external cash flows. Pure; PORT adds
//! prices and risk on top.
//!
//! Rules by kind (see `TransactionKind`):
//! - Buys and reinvested dividends add shares and cost (the cash paid,
//!   which includes commissions; or quantity × price + fees).
//! - Sells realize proceeds − shares × average cost; shares sold beyond
//!   what the history holds are flagged, never turned into a short.
//! - Splits change shares, not cost. Transfers in add shares at the stated
//!   cost basis; without one the position's cost is unknown and flagged.
//!   Transfers out remove shares at average cost without P&L.
//! - Dividends and interest are income; fees (and taxes withheld) are
//!   expenses; deposits and withdrawals are external flows, not P&L.
//! - A return of capital is not income: it lowers the position's cost
//!   basis, and any part beyond the basis is a realized gain. On shares
//!   whose cost is unknown, or that the history doesn't hold, it can't be
//!   applied and is flagged.
//! - Cash in lieu of a fractional share is realized proceeds of that
//!   fraction. Exports don't say how large the fraction was, so its cost
//!   stays in the remaining shares' basis: total return is exact, and the
//!   realized/unrealized split is off by the fraction's cost (cents to a
//!   few dollars). On a position the history doesn't hold (or whose cost is
//!   unknown) the realized result is marked unknown.
//! - Rows in another currency than the portfolio's are left out and
//!   counted, since no FX conversion is applied.

use std::collections::BTreeMap;

use meridian_store::Transaction;
use meridian_types::TransactionKind;

const EPS: f64 = 1e-9;

/// One security's position and history-derived results.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Holding {
    pub security: String,
    pub quantity: f64,
    /// Cost basis of the shares held (meaningful when `cost_known`).
    pub cost: f64,
    /// False when some held shares arrived without a cost basis.
    pub cost_known: bool,
    pub realized: f64,
    /// False when a sell's cost or proceeds were unknown.
    pub realized_known: bool,
    /// Dividends and interest attributed to this security.
    pub income: f64,
    /// Fee rows attributed to this security.
    pub fees: f64,
    /// Data problems found while replaying the history, in order.
    pub issues: Vec<String>,
}

impl Holding {
    fn new(security: &str) -> Self {
        Self { security: security.to_owned(), cost_known: true, realized_known: true, ..Default::default() }
    }

    /// Average cost per share, when known.
    #[must_use]
    pub fn average_cost(&self) -> Option<f64> {
        (self.cost_known && self.quantity > EPS).then(|| self.cost / self.quantity)
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.quantity > EPS
    }
}

/// Totals over a portfolio's history.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Ledger {
    /// Every security the history touches, in first-seen order.
    pub holdings: Vec<Holding>,
    pub dividends: f64,
    pub interest: f64,
    pub fees: f64,
    pub deposits: f64,
    pub withdrawals: f64,
    /// Rows of kind Other (listed, not counted).
    pub other: usize,
    /// Rows left out because of their currency, by currency.
    pub other_currency: BTreeMap<String, usize>,
}

impl Ledger {
    pub fn open(&self) -> impl Iterator<Item = &Holding> {
        self.holdings.iter().filter(|h| h.is_open())
    }

    /// Realized P&L over holdings whose realized result is known.
    #[must_use]
    pub fn realized(&self) -> f64 {
        self.holdings.iter().filter(|h| h.realized_known).map(|h| h.realized).sum()
    }
}

/// Cash flow of a row: the stated amount, else −(quantity × price) − fees.
#[must_use]
pub fn cash_flow(t: &Transaction) -> Option<f64> {
    t.amount.or_else(|| t.price.map(|p| -(t.quantity * p) - t.fees))
}

/// Replays `txs` (in trade-date order) for a portfolio in `base_currency`.
#[must_use]
pub fn ledger(txs: &[Transaction], base_currency: &str) -> Ledger {
    use TransactionKind as K;
    let mut l = Ledger::default();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for t in txs {
        if let Some(c) = t.currency.as_deref()
            && !c.eq_ignore_ascii_case(base_currency)
        {
            *l.other_currency.entry(c.to_ascii_uppercase()).or_default() += 1;
            continue;
        }
        let cash = cash_flow(t);
        let holding = t.security.as_deref().filter(|s| !s.is_empty()).map(|s| {
            *index.entry(s.to_owned()).or_insert_with(|| {
                l.holdings.push(Holding::new(s));
                l.holdings.len() - 1
            })
        });
        let q = t.quantity.abs();
        let date = &t.trade_date;
        match (t.kind, holding) {
            (K::Buy | K::ReinvestedDividend, Some(i)) => {
                let h = &mut l.holdings[i];
                h.quantity += q;
                if let Some(c) = cash {
                    h.cost += c.abs();
                } else {
                    h.cost_known = false;
                    h.issues.push(format!("{date}: bought {q} shares with no price or amount"));
                }
            }
            (K::Sell, Some(i)) => {
                let h = &mut l.holdings[i];
                if h.quantity <= EPS {
                    h.realized_known = false;
                    h.issues.push(format!("{date}: sold {q} shares the imported history doesn't hold"));
                    continue;
                }
                let sold = q.min(h.quantity);
                let avg = h.cost / h.quantity;
                match cash {
                    Some(c) if h.cost_known && q > 0.0 => h.realized += c.abs() * (sold / q) - sold * avg,
                    _ => h.realized_known = false,
                }
                if q - sold > EPS {
                    h.realized_known = false;
                    h.issues.push(format!("{date}: sold {q} shares but only {sold} were held"));
                }
                h.cost -= sold * avg;
                h.quantity -= sold;
            }
            (K::Split, Some(i)) => {
                let h = &mut l.holdings[i];
                if h.quantity <= EPS {
                    h.issues.push(format!("{date}: split of {} shares on a position the history doesn't hold", t.quantity));
                    continue;
                }
                h.quantity = (h.quantity + t.quantity).max(0.0);
            }
            (K::TransferIn, Some(i)) => {
                let h = &mut l.holdings[i];
                h.quantity += q;
                if let Some(p) = t.price {
                    h.cost += q * p;
                } else {
                    h.cost_known = false;
                    h.issues.push(format!("{date}: {q} shares transferred in without a cost basis"));
                }
            }
            (K::TransferOut, Some(i)) => {
                let h = &mut l.holdings[i];
                if h.quantity <= EPS {
                    h.issues.push(format!("{date}: transferred out {q} shares the history doesn't hold"));
                    continue;
                }
                let moved = q.min(h.quantity);
                let avg = h.cost / h.quantity;
                if q - moved > EPS {
                    h.issues.push(format!("{date}: transferred out {q} shares but only {moved} were held"));
                }
                h.cost -= moved * avg;
                h.quantity -= moved;
            }
            (K::Dividend, h) => {
                let c = cash.unwrap_or(0.0);
                l.dividends += c;
                if let Some(i) = h {
                    l.holdings[i].income += c;
                }
            }
            (K::Interest, h) => {
                let c = cash.unwrap_or(0.0);
                l.interest += c;
                if let Some(i) = h {
                    l.holdings[i].income += c;
                }
            }
            (K::Fee, h) => {
                let c = -cash.unwrap_or(0.0);
                l.fees += c;
                if let Some(i) = h {
                    l.holdings[i].fees += c;
                }
            }
            (K::ReturnOfCapital, Some(i)) => {
                let c = cash.unwrap_or(0.0);
                let h = &mut l.holdings[i];
                if h.quantity <= EPS {
                    h.issues.push(format!("{date}: return of capital of {c} on a position the history doesn't hold (not applied)"));
                    continue;
                }
                if !h.cost_known {
                    h.issues.push(format!("{date}: return of capital of {c} on shares with an unknown cost basis (not applied)"));
                    continue;
                }
                h.cost -= c;
                // Beyond the basis, a return of capital is a gain.
                if h.cost < 0.0 {
                    h.realized -= h.cost;
                    h.cost = 0.0;
                }
            }
            (K::CashInLieu, Some(i)) => {
                let c = cash.unwrap_or(0.0);
                let h = &mut l.holdings[i];
                h.realized += c;
                if h.quantity <= EPS {
                    h.realized_known = false;
                    h.issues.push(format!("{date}: cash in lieu of {c} on a position the history doesn't hold"));
                } else if !h.cost_known {
                    h.realized_known = false;
                }
            }
            (K::Deposit, _) => l.deposits += cash.unwrap_or(0.0).abs(),
            (K::Withdrawal, _) => l.withdrawals += cash.unwrap_or(0.0).abs(),
            // Share-moving and cost-basis kinds without a security can't be
            // applied (the importer refuses the former); they count as Other.
            (K::Other, _)
            | (K::Buy | K::Sell | K::ReinvestedDividend | K::ReturnOfCapital | K::Split | K::CashInLieu | K::TransferIn | K::TransferOut, None) => l.other += 1,
        }
        // A closed position starts fresh.
        if let Some(i) = holding {
            let h = &mut l.holdings[i];
            if h.quantity.abs() <= EPS {
                h.quantity = 0.0;
                h.cost = 0.0;
                h.cost_known = true;
            }
        }
    }
    l
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx(kind: TransactionKind, sec: Option<&str>, qty: f64, price: Option<f64>, amount: Option<f64>) -> Transaction {
        Transaction {
            id: 0,
            portfolio_id: 1,
            security: sec.map(Into::into),
            kind,
            trade_date: "2026-01-02".into(),
            settle_date: None,
            quantity: qty,
            price,
            amount,
            fees: 0.0,
            currency: None,
            note: None,
            source: None,
            fingerprint: None,
        }
    }

    const A: Option<&str> = Some("AAPL US Equity");

    #[test]
    fn average_cost_realized_and_manual_trades() {
        use TransactionKind as K;
        let mut buy = Transaction::trade(1, "AAPL US Equity", "2026-01-02", 10.0, 100.0, 1.0);
        let l = ledger(&[buy.clone()], "USD");
        assert_eq!(l.holdings[0].cost, 1001.0);
        buy.quantity = 10.0;
        let txs = vec![
            buy,
            tx(K::Buy, A, 10.0, Some(120.0), Some(-1200.0)),
            // Sell 5 at 130 net 649: avg cost (1001 + 1200) / 20 = 110.05.
            tx(K::Sell, A, -5.0, Some(130.0), Some(649.0)),
        ];
        let l = ledger(&txs, "USD");
        let h = &l.holdings[0];
        assert_eq!(h.quantity, 15.0);
        assert!((h.realized - (649.0 - 5.0 * 110.05)).abs() < 1e-9);
        assert!((h.average_cost().unwrap() - 110.05).abs() < 1e-9);
        assert!(h.issues.is_empty());
    }

    #[test]
    fn split_keeps_cost_and_scales_average() {
        use TransactionKind as K;
        let l = ledger(&[tx(K::Buy, A, 10.0, Some(300.0), Some(-3000.0)), tx(K::Split, A, 30.0, None, None)], "USD");
        let h = &l.holdings[0];
        assert_eq!(h.quantity, 40.0);
        assert_eq!(h.cost, 3000.0);
        assert_eq!(h.average_cost(), Some(75.0));
    }

    #[test]
    fn income_fees_and_flows() {
        use TransactionKind as K;
        let txs = vec![
            tx(K::Deposit, None, 0.0, None, Some(5000.0)),
            tx(K::Buy, A, 10.0, Some(100.0), Some(-1000.0)),
            tx(K::Dividend, A, 0.0, None, Some(2.4)),
            tx(K::ReinvestedDividend, A, 0.02, Some(120.0), Some(-2.4)),
            tx(K::Fee, A, 0.0, None, Some(-0.3)),
            tx(K::Interest, None, 0.0, None, Some(1.1)),
            tx(K::Withdrawal, None, 0.0, None, Some(-500.0)),
            tx(K::Other, None, 0.0, None, Some(7.0)),
        ];
        let l = ledger(&txs, "USD");
        let h = &l.holdings[0];
        assert!((h.quantity - 10.02).abs() < 1e-12);
        assert!((h.cost - 1002.4).abs() < 1e-9);
        assert!((h.income - 2.4).abs() < 1e-12);
        assert!((h.fees - 0.3).abs() < 1e-12);
        assert!((l.dividends - 2.4).abs() < 1e-12 && (l.interest - 1.1).abs() < 1e-12 && (l.fees - 0.3).abs() < 1e-12);
        assert_eq!((l.deposits, l.withdrawals, l.other), (5000.0, 500.0, 1));
    }

    #[test]
    fn return_of_capital_lowers_cost_and_is_not_income() {
        use TransactionKind as K;
        let l = ledger(&[tx(K::Buy, A, 10.0, Some(100.0), Some(-1000.0)), tx(K::ReturnOfCapital, A, 0.0, None, Some(50.0))], "USD");
        let h = &l.holdings[0];
        assert_eq!((h.quantity, h.cost, h.realized, h.income), (10.0, 950.0, 0.0, 0.0));
        assert_eq!(h.average_cost(), Some(95.0));
        assert_eq!((l.dividends, l.other), (0.0, 0));
        // Beyond the basis the excess is a realized gain; the basis stops at 0.
        let l = ledger(&[tx(K::Buy, A, 1.0, Some(10.0), Some(-10.0)), tx(K::ReturnOfCapital, A, 0.0, None, Some(15.0))], "USD");
        let h = &l.holdings[0];
        assert_eq!((h.cost, h.realized), (0.0, 5.0));
        assert!(h.realized_known);
        // Unknown cost or no position: not applied, said so.
        let l = ledger(&[tx(K::TransferIn, A, 10.0, None, None), tx(K::ReturnOfCapital, A, 0.0, None, Some(5.0))], "USD");
        assert!(l.holdings[0].issues.iter().any(|i| i.contains("unknown cost basis")));
        let l = ledger(&[tx(K::ReturnOfCapital, A, 0.0, None, Some(5.0))], "USD");
        assert!(l.holdings[0].issues[0].contains("doesn't hold"));
        assert_eq!(l.realized(), 0.0);
        // Without a security it is listed, not counted.
        assert_eq!(ledger(&[tx(K::ReturnOfCapital, None, 0.0, None, Some(5.0))], "USD").other, 1);
    }

    #[test]
    fn cash_in_lieu_is_realized_proceeds() {
        use TransactionKind as K;
        // 101 shares at 1.00, a 1-for-20 reverse split pays 0.05 share in cash.
        let txs = vec![
            tx(K::Buy, A, 101.0, Some(1.0), Some(-101.0)),
            tx(K::Split, A, -96.0, None, None),
            tx(K::CashInLieu, A, 0.0, None, Some(0.75)),
        ];
        let l = ledger(&txs, "USD");
        let h = &l.holdings[0];
        // The fraction's cost stays in the basis of the 5 shares.
        assert_eq!((h.quantity, h.cost), (5.0, 101.0));
        assert!((h.realized - 0.75).abs() < 1e-12 && h.realized_known);
        assert!((l.realized() - 0.75).abs() < 1e-12);
        assert_eq!((l.dividends, l.other), (0.0, 0));
        // On a position the history doesn't hold, the realized result is unknown.
        let l = ledger(&[tx(K::CashInLieu, A, 0.0, None, Some(0.75))], "USD");
        assert!(!l.holdings[0].realized_known && l.holdings[0].issues[0].contains("cash in lieu"));
        assert_eq!(l.realized(), 0.0);
    }

    #[test]
    fn transfers_with_and_without_cost_basis() {
        use TransactionKind as K;
        let l = ledger(&[tx(K::TransferIn, A, 10.0, Some(50.0), None)], "USD");
        assert_eq!(l.holdings[0].average_cost(), Some(50.0));
        let l = ledger(&[tx(K::TransferIn, A, 10.0, None, None), tx(K::Sell, A, -4.0, Some(60.0), Some(240.0))], "USD");
        let h = &l.holdings[0];
        assert!(!h.cost_known && !h.realized_known);
        assert_eq!(h.average_cost(), None);
        assert_eq!(h.quantity, 6.0);
        assert_eq!(h.issues.len(), 1);
        let l = ledger(&[tx(K::Buy, A, 10.0, Some(10.0), None), tx(K::TransferOut, A, -4.0, None, None)], "USD");
        let h = &l.holdings[0];
        assert_eq!((h.quantity, h.cost, h.realized), (6.0, 60.0, 0.0));
    }

    #[test]
    fn missing_history_is_flagged_not_shorted() {
        use TransactionKind as K;
        let l = ledger(&[tx(K::Sell, A, -5.0, Some(10.0), Some(50.0)), tx(K::Split, A, 5.0, None, None)], "USD");
        let h = &l.holdings[0];
        assert_eq!(h.quantity, 0.0);
        assert!(!h.realized_known);
        assert_eq!(h.issues.len(), 2);
        let l = ledger(&[tx(K::Buy, A, 2.0, Some(10.0), Some(-20.0)), tx(K::Sell, A, -5.0, Some(10.0), Some(50.0))], "USD");
        let h = &l.holdings[0];
        assert_eq!(h.quantity, 0.0);
        assert!(!h.realized_known && h.issues[0].contains("only 2"));
        assert_eq!(l.realized(), 0.0);
    }

    #[test]
    fn closing_resets_unknown_cost_and_other_currencies_are_left_out() {
        use TransactionKind as K;
        let mut eur = tx(K::Buy, A, 1.0, Some(10.0), Some(-10.0));
        eur.currency = Some("eur".into());
        let txs = vec![
            tx(K::TransferIn, A, 2.0, None, None),
            tx(K::TransferOut, A, -2.0, None, None),
            tx(K::Buy, A, 1.0, Some(10.0), Some(-10.0)),
            eur,
        ];
        let l = ledger(&txs, "USD");
        assert_eq!(l.holdings[0].average_cost(), Some(10.0));
        assert_eq!(l.other_currency.get("EUR"), Some(&1));
    }
}

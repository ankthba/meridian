//! Single-instrument, close-to-close signal backtester.
//!
//! # Execution model (no look-ahead)
//!
//! - A strategy produces a target position in `{−1, 0, +1}` (fraction of
//!   equity, short only if `allow_short`) from closes `0..=t` only.
//! - The target computed on bar `t`'s close is **executed at bar `t+1`'s
//!   close**, so a signal can never trade on the price that produced it.
//! - Orders are sized at `target × equity / close`; the fill is the close
//!   moved against the trade by `slippage_bps`, and `commission_bps` is
//!   charged on the filled notional. Shares are held between rebalances (no
//!   daily re-leveraging); equity is marked at each close.
//! - If equity reaches zero or below, the position is liquidated and the
//!   strategy stops trading.
//!
//! All ratio outputs are **fractions** (`0.05` = 5 %), including the fields
//! whose names end in `_pct`. Timestamps are Unix nanoseconds (UTC).

use serde::{Deserialize, Serialize};

use crate::error::{AnalyticsError, Result};
use crate::indicators;
use crate::returns;

/// Trading rule.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Strategy {
    /// Always long.
    BuyAndHold,
    /// Long while `SMA(fast) > SMA(slow)`; short (or flat) while below.
    SmaCross { fast: usize, slow: usize },
    /// Wilder RSI mean reversion: go long when RSI < `lower`, exit the long
    /// when RSI > `upper`; with shorting, go short above `upper` and cover
    /// below `lower`.
    RsiReversion { period: usize, lower: f64, upper: f64 },
    /// Donchian channel breakout: long when the close exceeds the highest
    /// close of the previous `lookback` bars; exit (or go short) when it falls
    /// below the lowest.
    Breakout { lookback: usize },
    /// Long while the MACD line is above its signal line; short (or flat) below.
    MacdCross { fast: usize, slow: usize, signal: usize },
}

/// Backtest parameters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BacktestConfig {
    /// Starting equity in currency units.
    pub initial_capital: f64,
    /// Commission in basis points of traded notional.
    pub commission_bps: f64,
    /// Slippage in basis points, applied against each fill.
    pub slippage_bps: f64,
    /// Whether bearish signals open short positions (otherwise they go flat).
    pub allow_short: bool,
    /// Bars per year for annualizing volatility and Sharpe (252 daily equities).
    pub periods_per_year: f64,
}

impl Default for BacktestConfig {
    fn default() -> Self {
        Self { initial_capital: 100_000.0, commission_bps: 0.0, slippage_bps: 0.0, allow_short: false, periods_per_year: 252.0 }
    }
}

/// Side of a trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Direction {
    Long,
    Short,
}

/// One round trip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TradeRecord {
    /// Bar index of the entry fill.
    pub entry_index: usize,
    /// Bar index of the exit fill (last bar for a still-open trade).
    pub exit_index: usize,
    /// Entry fill price including slippage.
    pub entry_price: f64,
    /// Exit fill price including slippage (last close for an open trade).
    pub exit_price: f64,
    pub direction: Direction,
    /// Units of the instrument traded (positive).
    pub quantity: f64,
    /// Net P&L in currency after commissions and slippage.
    pub pnl: f64,
    /// `pnl` as a fraction of the entry notional.
    pub return_pct: f64,
    /// True if the trade was still open on the last bar (marked to the close).
    pub is_open: bool,
}

/// Backtest output. Series have one entry per input bar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BacktestResult {
    /// Equity marked at each close, in currency.
    pub equity_curve: Vec<f64>,
    /// Target position held after bar `t`'s execution (−1, 0, +1).
    pub position: Vec<f64>,
    pub trades: Vec<TradeRecord>,
    /// Final / initial equity − 1.
    pub total_return: f64,
    /// Annualized growth over the calendar span of `ts` (365.25-day years).
    pub cagr: f64,
    /// Annualized volatility of bar-to-bar equity returns.
    pub annualized_vol: f64,
    /// Annualized Sharpe ratio of equity returns with a zero risk-free rate.
    pub sharpe: f64,
    /// Maximum drawdown of the equity curve (positive fraction).
    pub max_drawdown: f64,
    /// Fraction of closed trades with positive P&L (`NaN` if none closed).
    pub win_rate: f64,
    /// Number of trades, including one still open at the end.
    pub num_trades: usize,
    /// Fraction of bars with a non-zero position.
    pub exposure_pct: f64,
    /// Buy-and-hold return of the instrument, first to last close, no costs.
    pub benchmark_return: f64,
}

const NANOS_PER_YEAR: f64 = 365.25 * 86_400.0 * 1e9;

fn bad(msg: impl Into<String>) -> AnalyticsError {
    AnalyticsError::InvalidInput(msg.into())
}

fn validate_strategy(s: &Strategy) -> Result<()> {
    match *s {
        Strategy::BuyAndHold => Ok(()),
        Strategy::SmaCross { fast, slow } => {
            if fast == 0 || fast >= slow {
                return Err(bad("SmaCross needs 0 < fast < slow"));
            }
            Ok(())
        }
        Strategy::RsiReversion { period, lower, upper } => {
            if period == 0 || !(0.0..=100.0).contains(&lower) || !(0.0..=100.0).contains(&upper) || lower >= upper {
                return Err(bad("RsiReversion needs period > 0 and 0 ≤ lower < upper ≤ 100"));
            }
            Ok(())
        }
        Strategy::Breakout { lookback } => {
            if lookback == 0 {
                return Err(bad("Breakout needs lookback > 0"));
            }
            Ok(())
        }
        Strategy::MacdCross { fast, slow, signal } => {
            if fast == 0 || fast >= slow || signal == 0 {
                return Err(bad("MacdCross needs 0 < fast < slow and signal > 0"));
            }
            Ok(())
        }
    }
}

/// Target position for each bar, computed from `close[..=t]` only.
fn targets(close: &[f64], strategy: &Strategy, allow_short: bool) -> Vec<f64> {
    let bear = if allow_short { -1.0 } else { 0.0 };
    let n = close.len();
    // Above/below a reference line, holding the previous state on a tie.
    let cross = |a: &[f64], b: &[f64]| {
        let mut state = 0.0;
        (0..n)
            .map(|t| {
                if a[t].is_nan() || b[t].is_nan() {
                    state = 0.0;
                } else if a[t] > b[t] {
                    state = 1.0;
                } else if a[t] < b[t] {
                    state = bear;
                }
                state
            })
            .collect::<Vec<f64>>()
    };
    match *strategy {
        Strategy::BuyAndHold => vec![1.0; n],
        Strategy::SmaCross { fast, slow } => cross(&indicators::sma(close, fast), &indicators::sma(close, slow)),
        Strategy::MacdCross { fast, slow, signal } => {
            let m = indicators::macd(close, fast, slow, signal);
            cross(&m.macd, &m.signal)
        }
        Strategy::RsiReversion { period, lower, upper } => {
            let r = indicators::rsi(close, period);
            let mut state = 0.0;
            r.iter()
                .map(|&v| {
                    if v.is_nan() {
                        return state;
                    }
                    if v < lower {
                        state = 1.0;
                    } else if v > upper {
                        state = bear;
                    }
                    state
                })
                .collect()
        }
        Strategy::Breakout { lookback } => {
            let hh = indicators::rolling_max(close, lookback);
            let ll = indicators::rolling_min(close, lookback);
            let mut state = 0.0;
            (0..n)
                .map(|t| {
                    // Channel of the previous `lookback` closes (excludes bar t).
                    if t == 0 || hh[t - 1].is_nan() {
                        return state;
                    }
                    if close[t] > hh[t - 1] {
                        state = 1.0;
                    } else if close[t] < ll[t - 1] {
                        state = bear;
                    }
                    state
                })
                .collect()
        }
    }
}

struct OpenTrade {
    entry_index: usize,
    entry_price: f64,
    direction: Direction,
    quantity: f64,
    entry_commission: f64,
}

fn close_trade(open: OpenTrade, exit_index: usize, exit_price: f64, exit_commission: f64, is_open: bool) -> TradeRecord {
    let sign = match open.direction {
        Direction::Long => 1.0,
        Direction::Short => -1.0,
    };
    let pnl = sign * (exit_price - open.entry_price) * open.quantity - open.entry_commission - exit_commission;
    TradeRecord {
        entry_index: open.entry_index,
        exit_index,
        entry_price: open.entry_price,
        exit_price,
        direction: open.direction,
        quantity: open.quantity,
        pnl,
        return_pct: pnl / (open.entry_price * open.quantity),
        is_open,
    }
}

/// Runs `strategy` over the bar closes `close` with timestamps `ts` (Unix
/// nanos, strictly increasing). See the module docs for the execution model.
///
/// Errors: mismatched lengths, fewer than 2 bars, non-positive/non-finite
/// closes, non-increasing timestamps, invalid strategy parameters or config.
pub fn run_backtest(ts: &[i64], close: &[f64], strategy: &Strategy, config: &BacktestConfig) -> Result<BacktestResult> {
    if ts.len() != close.len() {
        return Err(AnalyticsError::LengthMismatch { expected: close.len(), got: ts.len() });
    }
    let n = close.len();
    if n < 2 {
        return Err(AnalyticsError::InsufficientData { needed: 2, got: n });
    }
    if close.iter().any(|c| !(c.is_finite() && *c > 0.0)) {
        return Err(bad("closes must be finite and positive"));
    }
    if ts.windows(2).any(|w| w[1] <= w[0]) {
        return Err(bad("timestamps must be strictly increasing"));
    }
    if !(config.initial_capital.is_finite() && config.initial_capital > 0.0) {
        return Err(bad("initial_capital must be positive"));
    }
    if !(config.commission_bps >= 0.0 && config.slippage_bps >= 0.0 && config.periods_per_year > 0.0) {
        return Err(bad("costs must be non-negative and periods_per_year positive"));
    }
    validate_strategy(strategy)?;

    let target = targets(close, strategy, config.allow_short);
    let slip = config.slippage_bps / 1e4;
    let comm = config.commission_bps / 1e4;

    let mut equity = vec![config.initial_capital; n];
    let mut position = vec![0.0; n];
    let mut trades = Vec::new();
    let mut cash = config.initial_capital;
    let mut shares = 0.0_f64;
    let mut pos = 0.0_f64;
    let mut open: Option<OpenTrade> = None;
    let mut bust = false;

    for t in 1..n {
        let px = close[t];
        let mut want = if bust { 0.0 } else { target[t - 1] };
        if !bust && cash + shares * px <= 0.0 {
            bust = true;
            want = 0.0;
        }
        if want != pos {
            let eq_pre = cash + shares * px;
            let new_shares = if want == 0.0 { 0.0 } else { want * eq_pre / px };
            let delta = new_shares - shares;
            let fill = px * (1.0 + slip * delta.signum());
            cash -= delta * fill + delta.abs() * fill * comm;
            if let Some(o) = open.take() {
                let exit_comm = o.quantity * fill * comm;
                trades.push(close_trade(o, t, fill, exit_comm, false));
            }
            if want != 0.0 {
                let quantity = new_shares.abs();
                open = Some(OpenTrade {
                    entry_index: t,
                    entry_price: fill,
                    direction: if want > 0.0 { Direction::Long } else { Direction::Short },
                    quantity,
                    entry_commission: quantity * fill * comm,
                });
            }
            shares = new_shares;
            pos = want;
        }
        equity[t] = cash + shares * px;
        position[t] = pos;
    }
    if let Some(o) = open.take() {
        trades.push(close_trade(o, n - 1, close[n - 1], 0.0, true));
    }

    let initial = config.initial_capital;
    let last = equity[n - 1];
    let span_years = (ts[n - 1] - ts[0]) as f64 / NANOS_PER_YEAR;
    let eq_returns = returns::simple_returns(&equity);
    let closed: Vec<&TradeRecord> = trades.iter().filter(|t| !t.is_open).collect();
    let win_rate = if closed.is_empty() {
        f64::NAN
    } else {
        closed.iter().filter(|t| t.pnl > 0.0).count() as f64 / closed.len() as f64
    };
    Ok(BacktestResult {
        total_return: last / initial - 1.0,
        cagr: returns::cagr(initial, last, span_years),
        annualized_vol: returns::annualized_volatility(&eq_returns, config.periods_per_year),
        sharpe: returns::sharpe(&eq_returns, 0.0, config.periods_per_year),
        max_drawdown: returns::max_drawdown(&equity).max_drawdown,
        win_rate,
        num_trades: trades.len(),
        exposure_pct: position.iter().filter(|p| **p != 0.0).count() as f64 / n as f64,
        benchmark_return: close[n - 1] / close[0] - 1.0,
        equity_curve: equity,
        position,
        trades,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400_000_000_000;

    fn days(n: usize) -> Vec<i64> {
        (0..n as i64).map(|i| 1_700_000_000_000_000_000 + i * DAY).collect()
    }

    fn wave(n: usize) -> Vec<f64> {
        (0..n).map(|i| 100.0 + 10.0 * (i as f64 * 0.15).sin() + i as f64 * 0.05 + 3.0 * (i as f64 * 0.9).cos()).collect()
    }

    fn cfg() -> BacktestConfig {
        BacktestConfig { initial_capital: 1000.0, ..BacktestConfig::default() }
    }

    fn all_strategies() -> Vec<Strategy> {
        vec![
            Strategy::BuyAndHold,
            Strategy::SmaCross { fast: 5, slow: 20 },
            Strategy::RsiReversion { period: 14, lower: 30.0, upper: 70.0 },
            Strategy::Breakout { lookback: 10 },
            Strategy::MacdCross { fast: 12, slow: 26, signal: 9 },
        ]
    }

    #[test]
    fn buy_and_hold_with_costs_by_hand() {
        let close = [100.0, 100.0, 110.0, 90.0];
        let config = BacktestConfig { commission_bps: 10.0, slippage_bps: 10.0, ..cfg() };
        let r = run_backtest(&days(4), &close, &Strategy::BuyAndHold, &config).unwrap();
        // Signal at bar 0, filled at bar 1: 10 shares at 100.1, commission 1.001.
        let cash = 1000.0 - 1001.0 - 1.001;
        assert_eq!(r.equity_curve[0], 1000.0);
        assert!((r.equity_curve[1] - (cash + 1000.0)).abs() < 1e-9);
        assert!((r.equity_curve[2] - (cash + 1100.0)).abs() < 1e-9);
        assert!((r.equity_curve[3] - (cash + 900.0)).abs() < 1e-9);
        assert_eq!(r.position, vec![0.0, 1.0, 1.0, 1.0]);
        assert_eq!(r.trades.len(), 1);
        let t = &r.trades[0];
        assert!(t.is_open && t.entry_index == 1 && (t.entry_price - 100.1).abs() < 1e-12);
        assert!((t.pnl - ((90.0 - 100.1) * 10.0 - 1.001)).abs() < 1e-9);
        assert!((r.benchmark_return + 0.1).abs() < 1e-12);
        assert!(r.win_rate.is_nan());
        assert!((r.exposure_pct - 0.75).abs() < 1e-12);
        assert!((r.max_drawdown - (1.0 - r.equity_curve[3] / r.equity_curve[2])).abs() < 1e-12);
    }

    /// Changing prices after bar k must not change any position up to k+1
    /// (the bar where a signal from k executes) or any equity up to k.
    #[test]
    fn no_look_ahead() {
        let n = 200;
        let base = wave(n);
        for strategy in all_strategies() {
            for allow_short in [false, true] {
                let config = BacktestConfig { allow_short, commission_bps: 5.0, slippage_bps: 2.0, ..cfg() };
                let a = run_backtest(&days(n), &base, &strategy, &config).unwrap();
                for k in [30, 80, 150] {
                    let mut future = base.clone();
                    for (j, v) in future.iter_mut().enumerate().skip(k + 1) {
                        *v = 50.0 + 40.0 * ((j * 7919 % 101) as f64 / 101.0);
                    }
                    let b = run_backtest(&days(n), &future, &strategy, &config).unwrap();
                    assert_eq!(a.position[..=k + 1], b.position[..=k + 1], "{strategy:?} k={k}");
                    assert_eq!(a.equity_curve[..=k], b.equity_curve[..=k], "{strategy:?} k={k}");
                }
            }
        }
    }

    #[test]
    fn signal_trades_next_bar_not_the_jump() {
        // Flat, then a jump at bar 25 that flips the SMA cross. The jump bar's
        // return must not be earned: entry is at bar 26's close.
        let mut close = vec![100.0; 40];
        for c in close.iter_mut().skip(25) {
            *c = 120.0;
        }
        let r = run_backtest(&days(40), &close, &Strategy::SmaCross { fast: 2, slow: 5 }, &cfg()).unwrap();
        assert_eq!(r.position[25], 0.0);
        assert_eq!(r.position[26], 1.0);
        assert!(r.equity_curve.iter().all(|e| (e - 1000.0).abs() < 1e-9), "{:?}", r.equity_curve);
    }

    #[test]
    fn sma_cross_trades_and_shorts() {
        let n = 300;
        let close = wave(n);
        let long_only = run_backtest(&days(n), &close, &Strategy::SmaCross { fast: 5, slow: 20 }, &cfg()).unwrap();
        assert!(long_only.position.iter().all(|p| *p == 0.0 || *p == 1.0));
        assert!(long_only.num_trades > 3);
        assert!(long_only.trades.iter().all(|t| t.direction == Direction::Long));
        let with_short = run_backtest(
            &days(n),
            &close,
            &Strategy::SmaCross { fast: 5, slow: 20 },
            &BacktestConfig { allow_short: true, ..cfg() },
        )
        .unwrap();
        assert!(with_short.position.contains(&-1.0));
        assert!(with_short.trades.iter().any(|t| t.direction == Direction::Short));
        // Trades chain: each exit bar is the next entry bar when reversing.
        for w in with_short.trades.windows(2) {
            assert!(w[0].exit_index <= w[1].entry_index);
        }
        // Without costs, the sum of trade P&L equals the equity change.
        let total: f64 = with_short.trades.iter().map(|t| t.pnl).sum();
        assert!((total - (with_short.equity_curve[n - 1] - 1000.0)).abs() < 1e-6);
    }

    #[test]
    fn rsi_and_breakout_behave() {
        // Steady decline then steady rise.
        let mut close: Vec<f64> = (0..30).map(|i| 100.0 - f64::from(i)).collect();
        close.extend((0..30).map(|i| 71.0 + 2.0 * f64::from(i)));
        let n = close.len();
        let r = run_backtest(&days(n), &close, &Strategy::RsiReversion { period: 5, lower: 30.0, upper: 70.0 }, &cfg()).unwrap();
        // Oversold during the decline → long; overbought in the rally → exit.
        let first_long = r.position.iter().position(|p| *p == 1.0).unwrap();
        assert!(first_long < 30);
        assert_eq!(r.position[n - 1], 0.0);
        assert_eq!(r.trades.len(), 1);
        assert!(!r.trades[0].is_open);

        let b = run_backtest(&days(n), &close, &Strategy::Breakout { lookback: 5 }, &cfg()).unwrap();
        // Breaks above the 5-bar high a few bars into the rally.
        assert!(b.position[..30].iter().all(|p| *p == 0.0));
        assert_eq!(b.position[n - 1], 1.0);
        assert!(b.total_return > 0.0);
    }

    #[test]
    fn metrics_consistent() {
        let n = 400;
        let close = wave(n);
        let r = run_backtest(&days(n), &close, &Strategy::MacdCross { fast: 12, slow: 26, signal: 9 }, &cfg()).unwrap();
        assert_eq!(r.equity_curve.len(), n);
        assert_eq!(r.position.len(), n);
        assert!((r.total_return - (r.equity_curve[n - 1] / 1000.0 - 1.0)).abs() < 1e-12);
        let years = (n - 1) as f64 / 365.25;
        assert!((r.cagr - ((1.0 + r.total_return).powf(1.0 / years) - 1.0)).abs() < 1e-9);
        assert!(r.max_drawdown >= 0.0 && r.max_drawdown < 1.0);
        assert!((0.0..=1.0).contains(&r.win_rate));
        assert!((r.benchmark_return - (close[n - 1] / close[0] - 1.0)).abs() < 1e-12);
    }

    #[test]
    fn validation() {
        let c = [1.0, 2.0, 3.0];
        let ts = days(3);
        assert!(matches!(run_backtest(&ts[..2], &c, &Strategy::BuyAndHold, &cfg()), Err(AnalyticsError::LengthMismatch { .. })));
        assert!(matches!(run_backtest(&ts[..1], &c[..1], &Strategy::BuyAndHold, &cfg()), Err(AnalyticsError::InsufficientData { .. })));
        assert!(run_backtest(&ts, &[1.0, -2.0, 3.0], &Strategy::BuyAndHold, &cfg()).is_err());
        assert!(run_backtest(&[3, 2, 1], &c, &Strategy::BuyAndHold, &cfg()).is_err());
        assert!(run_backtest(&ts, &c, &Strategy::SmaCross { fast: 5, slow: 5 }, &cfg()).is_err());
        assert!(run_backtest(&ts, &c, &Strategy::RsiReversion { period: 14, lower: 70.0, upper: 30.0 }, &cfg()).is_err());
        assert!(run_backtest(&ts, &c, &Strategy::Breakout { lookback: 0 }, &cfg()).is_err());
        assert!(run_backtest(&ts, &c, &Strategy::BuyAndHold, &BacktestConfig { initial_capital: 0.0, ..cfg() }).is_err());
    }

    #[test]
    fn short_squeeze_busts_and_stops() {
        // Short from bar 1, price explodes: equity goes negative → liquidated.
        let mut close = vec![100.0, 99.0, 98.0, 97.0];
        close.extend([300.0, 400.0, 500.0]);
        let config = BacktestConfig { allow_short: true, ..cfg() };
        let r = run_backtest(&days(close.len()), &close, &Strategy::SmaCross { fast: 1, slow: 2 }, &config).unwrap();
        assert!(r.position.contains(&-1.0));
        assert_eq!(*r.position.last().unwrap(), 0.0);
        let e = r.equity_curve[r.equity_curve.len() - 1];
        assert_eq!(e, r.equity_curve[r.equity_curve.len() - 2]);
    }
}

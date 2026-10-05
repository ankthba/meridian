//! Pure analytics for Meridian: technical indicators, return and risk
//! statistics, regression, option pricing (Black-Scholes-Merton and CRR
//! binomial), implied volatility, volatility surfaces, option strategy
//! payoffs, a bar-level backtester, chart decimation and FX cross rates.
//!
//! # Rules
//!
//! - **No I/O, no clocks, no global RNG.** Randomised methods take an explicit
//!   `seed` and use a ChaCha stream cipher RNG, so results are reproducible
//!   bit-for-bit on every platform and with or without the `parallel` feature.
//! - **Functions over slices.** Series outputs have the same length as their
//!   input, with `f64::NAN` at warm-up positions (and wherever the inputs make
//!   the value undefined). Anything that is not computable is `NaN`, an
//!   `Option::None`, or an [`AnalyticsError`] — never a made-up number.
//! - **Units.** Prices are plain `f64` in quote currency (the fixed-point
//!   `i64` prices of `meridian-types` are converted by the caller). Rates,
//!   yields, volatilities and returns are decimal fractions (`0.05` = 5 %),
//!   continuously compounded where noted. Option time is in years (ACT/365
//!   is the caller's choice). Every function documents its own units.
//!
//! # Modules
//!
//! | Module | Contents |
//! |---|---|
//! | [`indicators`] | SMA, EMA, WMA, Bollinger, RSI, MACD, ATR, stochastic, VWAP, OBV |
//! | [`returns`] | simple/log/cumulative returns, volatility, drawdown, Sharpe, Sortino, CAGR |
//! | [`stats`] | moments, covariance/correlation (matrices), OLS, beta, percentiles |
//! | [`risk`] | historical / parametric / Monte Carlo VaR, CVaR, portfolio volatility |
//! | [`options`] | normal distribution, BSM price and Greeks, CRR binomial, implied vol |
//! | [`surface`] | implied-volatility surface with total-variance interpolation |
//! | [`strategies`] | multi-leg option strategies: payoff, mark-to-model, breakevens, Greeks |
//! | [`backtest`] | signal backtester with next-bar execution and costs |
//! | [`decimate`] | min/max LOD pyramid and OHLC bucketing for charts |
//! | [`fx`] | cross-rate matrix from USD rates |
//! | [`special`] | `erf`/`erfc`, normal pdf/cdf/quantile (also re-exported from [`options`]) |
//!
//! # Example
//!
//! ```
//! use meridian_analytics::options::{
//!     BsInputs, ExerciseStyle, OptionRight, PricingModel, black_scholes_price, implied_volatility,
//! };
//!
//! // Hull's textbook example: S=42, K=40, r=10 %, σ=20 %, six months.
//! let call = BsInputs {
//!     spot: 42.0, strike: 40.0, rate: 0.10, dividend_yield: 0.0,
//!     vol: 0.20, time_years: 0.5, right: OptionRight::Call,
//! };
//! let price = black_scholes_price(&call);
//! assert!((price - 4.76).abs() < 0.005);
//! let iv = implied_volatility(price, &call, ExerciseStyle::European, PricingModel::BlackScholes);
//! assert!((iv.unwrap() - 0.20).abs() < 1e-9);
//! ```

pub mod backtest;
pub mod decimate;
pub mod error;
pub mod fx;
pub mod indicators;
mod linalg;
pub mod options;
pub mod returns;
pub mod risk;
pub mod special;
pub mod stats;
pub mod strategies;
pub mod surface;

pub use error::{AnalyticsError, Result};

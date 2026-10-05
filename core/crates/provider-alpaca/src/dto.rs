//! Private wire types for the Alpaca Market Data REST API.
//!
//! Field names follow the OpenAPI definitions at docs.alpaca.markets
//! (checked 2026-10-05). Everything is optional unless the normalizer can't
//! work without it, so a missing field becomes `None` downstream instead of
//! failing the whole response. Unknown fields are ignored.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// `stock_trade` (snapshot `latestTrade`).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct StockTrade {
    pub t: Option<DateTime<Utc>>,
    pub p: Option<f64>,
    pub s: Option<f64>,
}

/// `stock_quote` (snapshot `latestQuote`). Sizes are in shares since
/// 2025-11-03 (round lots before).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct StockQuote {
    pub t: Option<DateTime<Utc>>,
    pub bp: Option<f64>,
    pub bs: Option<f64>,
    pub ap: Option<f64>,
    #[serde(rename = "as")]
    pub ask_size: Option<f64>,
}

/// `stock_bar` in a bars response. `vw` and `n` exist but `BarSeries` has
/// no column for them.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Bar {
    pub t: DateTime<Utc>,
    pub o: f64,
    pub h: f64,
    pub l: f64,
    pub c: f64,
    pub v: f64,
}

/// Same shape as [`Bar`] but lenient, for snapshot sub-objects where a
/// partially populated bar shouldn't fail the whole quote.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SnapshotBar {
    pub t: Option<DateTime<Utc>>,
    pub o: Option<f64>,
    pub h: Option<f64>,
    pub l: Option<f64>,
    pub c: Option<f64>,
    pub v: Option<f64>,
    pub vw: Option<f64>,
}

/// `stock_snapshot`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StockSnapshot {
    pub latest_trade: Option<StockTrade>,
    pub latest_quote: Option<StockQuote>,
    pub minute_bar: Option<SnapshotBar>,
    pub daily_bar: Option<SnapshotBar>,
    pub prev_daily_bar: Option<SnapshotBar>,
}

/// `GET /v2/stocks/snapshots`: an object keyed by symbol.
pub(crate) type StockSnapshotsResp = HashMap<String, Option<StockSnapshot>>;

/// `GET /v2/stocks/{symbol}/bars`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct StockBarsResp {
    /// The API has been seen to send `null` instead of `[]` when empty.
    pub bars: Option<Vec<Bar>>,
    pub next_page_token: Option<String>,
}

/// `option_trade`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct OptionTrade {
    pub t: Option<DateTime<Utc>>,
    pub p: Option<f64>,
}

/// `option_quote`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct OptionQuote {
    pub t: Option<DateTime<Utc>>,
    pub bp: Option<f64>,
    pub bs: Option<f64>,
    pub ap: Option<f64>,
    #[serde(rename = "as")]
    pub ask_size: Option<f64>,
}

/// `option_greeks` (Alpaca's Black-Scholes values).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct OptionGreeks {
    pub delta: Option<f64>,
    pub gamma: Option<f64>,
    pub rho: Option<f64>,
    pub theta: Option<f64>,
    pub vega: Option<f64>,
}

/// `option_snapshot`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OptionSnapshot {
    pub latest_quote: Option<OptionQuote>,
    pub latest_trade: Option<OptionTrade>,
    pub greeks: Option<OptionGreeks>,
    pub implied_volatility: Option<f64>,
    pub daily_bar: Option<SnapshotBar>,
}

/// `GET /v1beta1/options/snapshots/{underlying_symbol}`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct OptionChainResp {
    pub snapshots: Option<HashMap<String, Option<OptionSnapshot>>>,
    pub next_page_token: Option<String>,
}

/// `news` article.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct NewsArticle {
    pub id: i64,
    pub headline: Option<String>,
    pub summary: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
    pub url: Option<String>,
    #[serde(default)]
    pub symbols: Vec<String>,
    pub source: Option<String>,
}

/// `GET /v1beta1/news`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct NewsResp {
    pub news: Option<Vec<NewsArticle>>,
    pub next_page_token: Option<String>,
}

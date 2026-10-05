//! OMON, OVDV, OVME — option monitor, volatility surface, option valuation.

use std::sync::Arc;

use chrono::NaiveDate;
use meridian_analytics::options::{BsInputs, PricingModel, black_scholes_greeks, implied_volatility, price};
use meridian_analytics::strategies::{self, Leg, LegKind};
use meridian_analytics::surface::{VolPoint, VolSurface};
use meridian_analytics::returns;
use meridian_provider::ChainRequest;
use meridian_types::{
    BarInterval, ExerciseStyle, GreeksSource, MarketSector, NANOS_PER_DAY, OptionChain, OptionContract, OptionRight, SecurityKey,
    nanos_to_date,
};

use super::{ScreenRequest, error_screen, require_security, stale_notice};
use crate::cache::{Fetched, ttl};
use crate::core::Engine;
use crate::error::{EngineError, EngineResult};
use crate::screen::{
    Action, Block, Cell, Column, Field, Format, HeatMap, Input, InputKind, NoticeLevel, Row, Screen, Style, Table, XyChart, XySeries,
};

/// Rates and dividends used to price options, with where they came from.
#[derive(Debug, Clone)]
pub struct PricingParams {
    pub spot: f64,
    pub rate: f64,
    pub rate_source: String,
    pub div_yield: f64,
    pub div_source: String,
}

const ASSUMED_RATE: f64 = 0.04;

impl Engine {
    pub async fn option_chain(&self, key: &SecurityKey) -> EngineResult<Fetched<OptionChain>> {
        let router = self.router().clone();
        let req = ChainRequest { underlying: key.clone(), expiry: None };
        self.cached("chain", &key.to_string(), ttl::CHAIN, async move { router.option_chain(req).await }).await
    }

    /// Spot, a risk-free rate (UST 3M when available), and a dividend yield.
    pub async fn pricing_params(&self, key: &SecurityKey, chain_spot: Option<f64>) -> EngineResult<PricingParams> {
        let (quote, curve, divs) = tokio::join!(self.quote_row(key), self.yield_curve("UST", None), async {
            if key.sector == MarketSector::Equity { self.router().dividends(key).await.ok() } else { None }
        });
        let spot = quote.map(|q| q.last).filter(|x| x.is_finite() && *x > 0.0).or(chain_spot).ok_or_else(|| EngineError::NotAvailable {
            what: "underlying price".into(),
            reason: "no quote for the underlying".into(),
        })?;
        let (rate, rate_source) = match curve.ok().and_then(|c| c.value.points.iter().find(|p| p.tenor == "3 Mo").and_then(|p| p.yield_pct)) {
            Some(y) => ((1.0 + y / 100.0).ln(), format!("UST 3M {y:.2}%")),
            None => (ASSUMED_RATE, format!("assumed {:.2}% (no rate source)", ASSUMED_RATE * 100.0)),
        };
        let today = nanos_to_date(self.now());
        let (div_yield, div_source) = match divs {
            Some(d) => {
                let ttm: f64 = d
                    .value
                    .dividends
                    .iter()
                    .filter(|x| x.ex_date > today - chrono::Duration::days(365) && !matches!(x.kind, meridian_types::DividendKind::Split))
                    .map(|x| x.amount)
                    .sum();
                (ttm / spot, format!("TTM {:.2}%", ttm / spot * 100.0))
            }
            None => (0.0, "none".into()),
        };
        Ok(PricingParams { spot, rate, rate_source, div_yield, div_source })
    }
}

fn years_to(expiry: NaiveDate, today: NaiveDate) -> f64 {
    // Expire at the close: count the expiry day.
    ((expiry - today).num_days() as f64 + 0.7).max(1.0 / 365.0) / 365.0
}

fn style_for(key: &SecurityKey) -> ExerciseStyle {
    if key.sector == MarketSector::Index { ExerciseStyle::European } else { ExerciseStyle::American }
}

/// IV and Greeks per contract: vendor values when present, else computed
/// (Black-Scholes IV from the mid, then Black-Scholes Greeks).
#[derive(Debug, Clone, Copy, Default)]
struct Computed {
    iv: Option<f64>,
    delta: Option<f64>,
    gamma: Option<f64>,
    theta: Option<f64>,
    vega: Option<f64>,
    vendor: bool,
}

fn compute(c: &OptionContract, p: &PricingParams, today: NaiveDate) -> Computed {
    if let Some(g) = &c.greeks
        && g.iv.is_some()
    {
        return Computed {
            iv: g.iv,
            delta: g.delta,
            gamma: g.gamma,
            theta: g.theta,
            vega: g.vega,
            vendor: matches!(g.source, GreeksSource::Vendor),
        };
    }
    let Some(mid) = c.mid().filter(|m| *m > 0.0) else { return Computed::default() };
    let inputs = BsInputs {
        spot: p.spot,
        strike: c.strike,
        rate: p.rate,
        dividend_yield: p.div_yield,
        vol: 0.3,
        time_years: years_to(c.expiry, today),
        right: c.right,
    };
    let Some(iv) = implied_volatility(mid, &inputs, ExerciseStyle::European, PricingModel::BlackScholes) else {
        return Computed::default();
    };
    let g = black_scholes_greeks(&inputs.with_vol(iv));
    Computed { iv: Some(iv), delta: Some(g.delta), gamma: Some(g.gamma), theta: Some(g.theta), vega: Some(g.vega), vendor: false }
}

fn expiry_label(e: NaiveDate, today: NaiveDate) -> String {
    format!("{} ({}d)", e.format("%m/%d/%y"), (e - today).num_days())
}

fn parse_expiry(label: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(label.split_whitespace().next()?, "%m/%d/%y").ok()
}

async fn hv30(engine: &Engine, key: &SecurityKey) -> Option<f64> {
    let now = engine.now();
    let bars = engine.bars(key, BarInterval::Day, Some(now - 60 * NANOS_PER_DAY), now + NANOS_PER_DAY).await.ok()?;
    let c = &bars.value.close;
    if c.len() < 22 {
        return None;
    }
    let r = returns::log_returns(&c[c.len() - 22..]);
    Some(returns::annualized_volatility(&r, 252.0) * 100.0)
}

// --- OMON -----------------------------------------------------------------

pub(crate) async fn omon(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    const T: &str = "Option Monitor";
    let key = match require_security("OMON", T, &req) {
        Ok(k) => k,
        Err(s) => return *s,
    };
    let ks = key.to_string();
    let chain = match engine.option_chain(&key).await {
        Ok(c) => c,
        Err(e) => return error_screen("OMON", T, Some(&key), &e),
    };
    let params = match engine.pricing_params(&key, chain.value.underlying_price).await {
        Ok(p) => p,
        Err(e) => return error_screen("OMON", T, Some(&key), &e),
    };
    let today = nanos_to_date(engine.now());
    let expiries = chain.value.expiries();
    let Some(&first_exp) = expiries.first() else {
        return Screen::not_available("OMON", T, Some(ks), "no listed expiries");
    };
    let expiry = req.arg("expiry").and_then(parse_expiry).filter(|e| expiries.contains(e)).unwrap_or(first_exp);
    let n_strikes: usize = req.arg("strikes").and_then(|x| x.parse().ok()).unwrap_or(20);

    let mut s = Screen::new("OMON", format!("{ks} — {T}"), Some(ks.clone()));
    s.source(&chain.value.provenance);
    stale_notice(&mut s, &chain);
    s.menu_item("Vol Surface", Action::new("OVDV", Some(&ks)), false);
    s.menu_item("Option Valuation", Action::new("OVME", Some(&ks)), false);
    s.push(Block::Inputs {
        title: None,
        inputs: vec![
            Input {
                id: "expiry".into(),
                label: "Expiry".into(),
                value: expiry_label(expiry, today),
                kind: InputKind::Choice,
                options: expiries.iter().map(|e| expiry_label(*e, today)).collect(),
            },
            Input {
                id: "strikes".into(),
                label: "Strikes".into(),
                value: n_strikes.to_string(),
                kind: InputKind::Choice,
                options: vec!["10".into(), "20".into(), "40".into(), "200".into()],
            },
        ],
    });

    let contracts: Vec<&OptionContract> = chain.value.contracts.iter().filter(|c| c.expiry == expiry).collect();
    let mut strikes = chain.value.strikes(expiry);
    let atm_idx = strikes.iter().enumerate().min_by(|a, b| (a.1 - params.spot).abs().total_cmp(&(b.1 - params.spot).abs())).map_or(0, |(i, _)| i);
    let lo = atm_idx.saturating_sub(n_strikes / 2);
    let hi = (lo + n_strikes).min(strikes.len());
    strikes = strikes[lo..hi].to_vec();

    let atm_strike = strikes.iter().copied().min_by(|a, b| (a - params.spot).abs().total_cmp(&(b - params.spot).abs()));
    let mut any_vendor = false;
    let mut call_oi = 0.0;
    let mut put_oi = 0.0;
    let mut atm_iv = None;
    let mut rows = Vec::with_capacity(strikes.len());
    for k in &strikes {
        let call = contracts.iter().find(|c| c.strike == *k && c.right == OptionRight::Call);
        let put = contracts.iter().find(|c| c.strike == *k && c.right == OptionRight::Put);
        let cc = call.map(|c| compute(c, &params, today)).unwrap_or_default();
        let pc = put.map(|c| compute(c, &params, today)).unwrap_or_default();
        any_vendor |= cc.vendor || pc.vendor;
        call_oi += call.and_then(|c| c.open_interest).unwrap_or(0.0);
        put_oi += put.and_then(|c| c.open_interest).unwrap_or(0.0);
        if Some(*k) == atm_strike {
            atm_iv = cc.iv.or(pc.iv);
        }
        let call_itm = *k < params.spot;
        let side = |c: Option<&&OptionContract>, x: Computed, itm: bool| -> Vec<Cell> {
            let st = if itm { Style::Emphasis } else { Style::Normal };
            vec![
                Cell::num(c.and_then(|c| c.bid)).styled(st),
                Cell::num(c.and_then(|c| c.ask)).styled(st),
                Cell::num(c.and_then(|c| c.last)).styled(st),
                Cell::num(x.iv.map(|v| v * 100.0)).styled(st),
                Cell::num(x.delta).styled(st),
                Cell::num(x.gamma).styled(st),
                Cell::num(x.theta).styled(st),
                Cell::num(x.vega).styled(st),
                Cell::num(c.and_then(|c| c.volume)).styled(st),
                Cell::num(c.and_then(|c| c.open_interest)).styled(st),
            ]
        };
        let mut cells = side(call, cc, call_itm);
        let strike_cell = Cell::num(Some(*k)).styled(if Some(*k) == atm_strike { Style::Warning } else { Style::Emphasis });
        cells.push(strike_cell);
        cells.extend(side(put, pc, !call_itm));
        let mut row = Row::new(cells).action(
            Action::new("OVME", Some(&ks)).arg("k1", format!("{k}")).arg("expiry", expiry.format("%m/%d/%Y").to_string()),
        );
        if Some(*k) == atm_strike {
            row = row.emphasis();
        }
        rows.push(row);
    }

    let (hv, quote) = tokio::join!(hv30(&engine, &key), engine.quote_row(&key));
    let dec = engine.price_decimals(&key);
    s.push(Block::Fields {
        title: None,
        columns: 4,
        fields: vec![
            Field::num("Underlying", Some(params.spot), Format::Price { decimals: dec }).styled(Style::Emphasis),
            Field::num("% Chg", quote.map(|q| q.pct_change).filter(|x| x.is_finite()), Format::ChangePercent { decimals: 2 }),
            Field::num("ATM IV %", atm_iv.map(|v| v * 100.0), Format::Number { decimals: 2 }),
            Field::num("30D HV %", hv, Format::Number { decimals: 2 }),
            Field::num("Days to Exp", Some((expiry - today).num_days() as f64), Format::Integer),
            Field::num("Put/Call OI", (call_oi > 0.0).then(|| put_oi / call_oi), Format::Number { decimals: 2 }),
            Field::text("Rate", &params.rate_source),
            Field::text("Div Yield", &params.div_source),
        ],
    });
    s.push(Block::Notice {
        level: NoticeLevel::Info,
        text: if any_vendor {
            "IV and Greeks from the data provider; blanks are computed (Black-Scholes from mid).".into()
        } else {
            "IV and Greeks computed by Meridian (Black-Scholes from bid/ask mid). Theta per day, vega per vol point.".into()
        },
    });
    let half = |side: &str| -> Vec<Column> {
        vec![
            Column::num("Bid", Format::Number { decimals: 2 }, 6),
            Column::num("Ask", Format::Number { decimals: 2 }, 6),
            Column::num("Last", Format::Number { decimals: 2 }, 6),
            Column::num(&format!("{side}IV"), Format::Number { decimals: 1 }, 5),
            Column::num("Dlt", Format::Number { decimals: 2 }, 5),
            Column::num("Gam", Format::Number { decimals: 3 }, 5),
            Column::num("Tht", Format::Number { decimals: 2 }, 5),
            Column::num("Vga", Format::Number { decimals: 2 }, 4),
            Column::num("Volm", Format::Large { decimals: 0 }, 5),
            Column::num("OI", Format::Large { decimals: 0 }, 5),
        ]
    };
    let mut columns = half("C");
    columns.push(Column::num("Strike", Format::Number { decimals: 2 }, 7));
    columns.extend(half("P"));
    s.push(Block::Table(Table { title: Some(format!("Calls | Strike | Puts — {}", expiry_label(expiry, today))), columns, rows, page_size: Some(40), numbered: false }));
    s
}

// --- OVDV -----------------------------------------------------------------

pub(crate) async fn ovdv(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    const T: &str = "Volatility Surface";
    let key = match require_security("OVDV", T, &req) {
        Ok(k) => k,
        Err(s) => return *s,
    };
    let ks = key.to_string();
    let chain = match engine.option_chain(&key).await {
        Ok(c) => c,
        Err(e) => return error_screen("OVDV", T, Some(&key), &e),
    };
    let params = match engine.pricing_params(&key, chain.value.underlying_price).await {
        Ok(p) => p,
        Err(e) => return error_screen("OVDV", T, Some(&key), &e),
    };
    let today = nanos_to_date(engine.now());
    // Out-of-the-money contracts carry the cleanest IVs.
    let points: Vec<VolPoint> = chain
        .value
        .contracts
        .iter()
        .filter(|c| (c.right == OptionRight::Put && c.strike < params.spot) || (c.right == OptionRight::Call && c.strike >= params.spot))
        .filter_map(|c| compute(c, &params, today).iv.map(|iv| VolPoint { expiry_years: years_to(c.expiry, today), strike: c.strike, iv }))
        .collect();
    let surface = match VolSurface::from_points(&points, params.spot) {
        Ok(v) => v,
        Err(e) => return Screen::not_available("OVDV", T, Some(ks), format!("cannot build surface: {e}")),
    };
    let mut s = Screen::new("OVDV", format!("{ks} — {T}"), Some(ks.clone()));
    s.source(&chain.value.provenance);
    stale_notice(&mut s, &chain);
    s.menu_item("Option Monitor", Action::new("OMON", Some(&ks)), false);
    s.menu_item("Option Valuation", Action::new("OVME", Some(&ks)), false);

    let moneyness: Vec<f64> = (0..=8).map(|i| 0.80 + 0.05 * f64::from(i)).collect();
    let expiries: Vec<NaiveDate> = chain.value.expiries();
    let mut values = Vec::new();
    let mut row_labels = Vec::new();
    for e in &expiries {
        let t = years_to(*e, today);
        row_labels.push(expiry_label(*e, today));
        for m in &moneyness {
            values.push(surface.iv_at(t, params.spot * m).map_or(f64::NAN, |v| v * 100.0));
        }
    }
    s.push(Block::Heat(HeatMap {
        title: "Implied Vol % by Expiry × Moneyness (strike / spot)".into(),
        row_labels: row_labels.clone(),
        col_labels: moneyness.iter().map(|m| format!("{:.0}%", m * 100.0)).collect(),
        values: values.clone(),
        format: Format::Number { decimals: 1 },
        diverging: false,
    }));
    let sel = req.arg("expiry").and_then(parse_expiry).filter(|e| expiries.contains(e)).or_else(|| expiries.iter().copied().find(|e| (*e - today).num_days() >= 25)).or(expiries.first().copied());
    if let Some(e) = sel {
        let smile = surface.smile(years_to(e, today));
        s.push(Block::Xy(XyChart {
            title: format!("Smile — {}", expiry_label(e, today)),
            x_label: "Strike".into(),
            y_label: "IV %".into(),
            series: vec![XySeries { name: "IV".into(), x: smile.iter().map(|p| p.0).collect(), y: smile.iter().map(|p| p.1 * 100.0).collect(), style: Style::Emphasis, bars: false }],
            x_marker: Some(params.spot),
            height_rows: 9,
        }));
    }
    let ts = surface.term_structure(params.spot);
    s.push(Block::Xy(XyChart {
        title: "ATM Term Structure".into(),
        x_label: "Years".into(),
        y_label: "IV %".into(),
        series: vec![XySeries { name: "ATM IV".into(), x: ts.iter().map(|p| p.0).collect(), y: ts.iter().map(|p| p.1 * 100.0).collect(), style: Style::Up, bars: false }],
        x_marker: None,
        height_rows: 7,
    }));
    s.push(Block::Inputs {
        title: None,
        inputs: vec![Input {
            id: "expiry".into(),
            label: "Smile Expiry".into(),
            value: sel.map(|e| expiry_label(e, today)).unwrap_or_default(),
            kind: InputKind::Choice,
            options: row_labels,
        }],
    });
    s
}

// --- OVME -----------------------------------------------------------------

const STRATEGIES: &[&str] = &[
    "Long Call",
    "Long Put",
    "Covered Call",
    "Bull Call Spread",
    "Bear Put Spread",
    "Straddle",
    "Strangle",
    "Iron Condor",
    "Butterfly",
];

fn round_strike(x: f64) -> f64 {
    let step = if x < 25.0 {
        0.5
    } else if x < 200.0 {
        1.0
    } else if x < 1000.0 {
        5.0
    } else {
        25.0
    };
    (x / step).round() * step
}

pub(crate) async fn ovme(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    const T: &str = "Option Valuation";
    let key = match require_security("OVME", T, &req) {
        Ok(k) => k,
        Err(s) => return *s,
    };
    let ks = key.to_string();
    let params = match engine.pricing_params(&key, None).await {
        Ok(p) => p,
        Err(e) => return error_screen("OVME", T, Some(&key), &e),
    };
    let today = nanos_to_date(engine.now());
    let num = |id: &str| req.arg(id).and_then(|x| x.trim().trim_end_matches('%').parse::<f64>().ok());
    let spot = num("spot").unwrap_or(params.spot);
    let strategy = req.arg("strategy").filter(|s| STRATEGIES.contains(s)).unwrap_or("Long Call").to_owned();
    let expiry = req
        .arg("expiry")
        .and_then(super::parse_date)
        .unwrap_or_else(|| today + chrono::Duration::days(30));
    let t = years_to(expiry, today);
    let hv = hv30(&engine, &key).await;
    let vol = num("vol").map_or_else(|| hv.map_or(0.25, |h| h / 100.0), |v| v / 100.0);
    let rate = num("rate").map_or(params.rate, |r| r / 100.0);
    let q = num("div").map_or(params.div_yield, |d| d / 100.0);
    let model = match req.arg("model") {
        Some("Binomial") => PricingModel::Binomial { steps: 300 },
        _ => PricingModel::BlackScholes,
    };
    let style = if model == PricingModel::BlackScholes { ExerciseStyle::European } else { style_for(&key) };
    let k1 = num("k1").unwrap_or_else(|| round_strike(spot));
    let k2 = num("k2").unwrap_or_else(|| round_strike(spot * 1.05));
    let k3 = num("k3").unwrap_or_else(|| round_strike(spot * 0.95));
    let k4 = num("k4").unwrap_or_else(|| round_strike(spot * 1.10));
    let qty = num("qty").unwrap_or(1.0);

    let p = |right: OptionRight, k: f64| {
        price(&BsInputs { spot, strike: k, rate, dividend_yield: q, vol, time_years: t, right }, style, model)
    };
    let (c, pu) = (OptionRight::Call, OptionRight::Put);
    let legs: Vec<Leg> = match strategy.as_str() {
        "Long Put" => vec![Leg::put(k1, 1.0, p(pu, k1), t, vol)],
        "Covered Call" => strategies::covered_call(spot, k2, p(c, k2), t, vol),
        "Bull Call Spread" => strategies::vertical_spread(c, k1, p(c, k1), k2, p(c, k2), t, vol),
        "Bear Put Spread" => strategies::vertical_spread(pu, k1, p(pu, k1), k3, p(pu, k3), t, vol),
        "Straddle" => strategies::straddle(k1, p(c, k1), p(pu, k1), t, vol),
        "Strangle" => strategies::strangle(k3, p(pu, k3), k2, p(c, k2), t, vol),
        "Iron Condor" => {
            let ks4 = [round_strike(spot * 0.90), k3, k2, k4];
            strategies::iron_condor(ks4, [p(pu, ks4[0]), p(pu, ks4[1]), p(c, ks4[2]), p(c, ks4[3])], t, vol)
        }
        "Butterfly" => strategies::butterfly(c, [k3, k1, k2], [p(c, k3), p(c, k1), p(c, k2)], t, vol),
        _ => strategies::long_call(k1, p(c, k1), t, vol),
    };
    let legs = strategies::scale_legs(&legs, qty);

    let mut s = Screen::new("OVME", format!("{ks} — {T}"), Some(ks.clone()));
    if let Some(pr) = engine.quote_provenance(&key) {
        s.source(&pr);
    }
    s.menu_item("Option Monitor", Action::new("OMON", Some(&ks)), false);
    s.menu_item("Vol Surface", Action::new("OVDV", Some(&ks)), false);
    s.push(Block::Inputs {
        title: Some("Inputs".into()),
        inputs: vec![
            Input { id: "strategy".into(), label: "Strategy".into(), value: strategy.clone(), kind: InputKind::Choice, options: STRATEGIES.iter().map(|x| (*x).into()).collect() },
            Input { id: "spot".into(), label: "Underlying".into(), value: format!("{spot:.2}"), kind: InputKind::Number, options: vec![] },
            Input { id: "expiry".into(), label: "Expiry".into(), value: expiry.format("%m/%d/%Y").to_string(), kind: InputKind::Date, options: vec![] },
            Input { id: "k1".into(), label: "Strike 1".into(), value: format!("{k1}"), kind: InputKind::Number, options: vec![] },
            Input { id: "k2".into(), label: "Strike 2 (upper)".into(), value: format!("{k2}"), kind: InputKind::Number, options: vec![] },
            Input { id: "k3".into(), label: "Strike 3 (lower)".into(), value: format!("{k3}"), kind: InputKind::Number, options: vec![] },
            Input { id: "k4".into(), label: "Strike 4 (wing)".into(), value: format!("{k4}"), kind: InputKind::Number, options: vec![] },
            Input { id: "vol".into(), label: "Vol %".into(), value: format!("{:.2}", vol * 100.0), kind: InputKind::Number, options: vec![] },
            Input { id: "rate".into(), label: "Rate %".into(), value: format!("{:.3}", rate * 100.0), kind: InputKind::Number, options: vec![] },
            Input { id: "div".into(), label: "Div Yld %".into(), value: format!("{:.3}", q * 100.0), kind: InputKind::Number, options: vec![] },
            Input { id: "qty".into(), label: "Quantity".into(), value: format!("{qty}"), kind: InputKind::Number, options: vec![] },
            Input { id: "model".into(), label: "Model".into(), value: if model == PricingModel::BlackScholes { "Black-Scholes".into() } else { "Binomial".into() }, kind: InputKind::Choice, options: vec!["Black-Scholes".into(), "Binomial".into()] },
        ],
    });
    s.push(Block::Notice {
        level: NoticeLevel::Info,
        text: format!(
            "Vol default: {}; rate default: {}; div default: {}. Prices per share; ×100 per contract.",
            hv.map_or("25% (no history)".into(), |h| format!("30D realized {h:.2}%")),
            params.rate_source,
            params.div_source
        ),
    });

    // Leg table with per-leg value and Greeks.
    let mut rows = Vec::new();
    let mut total = meridian_analytics::options::GreekValues::ZERO;
    let mut net_premium = 0.0;
    for leg in &legs {
        let (kind, right) = match leg.kind {
            LegKind::Call => ("Call", Some(OptionRight::Call)),
            LegKind::Put => ("Put", Some(OptionRight::Put)),
            LegKind::Stock => ("Stock", None),
        };
        let g = right.map_or(
            meridian_analytics::options::GreekValues { delta: 1.0, ..meridian_analytics::options::GreekValues::ZERO },
            |r| meridian_analytics::options::greeks(&BsInputs { spot, strike: leg.strike, rate, dividend_yield: q, vol, time_years: t, right: r }, style, model),
        )
        .scaled(leg.quantity);
        total += g;
        net_premium += leg.premium * leg.quantity;
        rows.push(Row::new(vec![
            Cell::text(if leg.quantity >= 0.0 { "Long" } else { "Short" }).styled(if leg.quantity >= 0.0 { Style::Up } else { Style::Down }),
            Cell::text(kind),
            Cell::num((right.is_some()).then_some(leg.strike)),
            Cell::num(Some(leg.quantity)),
            Cell::num(Some(leg.premium)),
            Cell::num(Some(g.delta)),
            Cell::num(Some(g.gamma)),
            Cell::num(Some(g.theta)),
            Cell::num(Some(g.vega)),
        ]));
    }
    rows.push(
        Row::new(vec![
            Cell::text("Total").styled(Style::Emphasis),
            Cell::empty(),
            Cell::empty(),
            Cell::empty(),
            Cell::num(Some(net_premium)).styled(Style::Emphasis),
            Cell::num(Some(total.delta)).styled(Style::Emphasis),
            Cell::num(Some(total.gamma)).styled(Style::Emphasis),
            Cell::num(Some(total.theta)).styled(Style::Emphasis),
            Cell::num(Some(total.vega)).styled(Style::Emphasis),
        ])
        .emphasis(),
    );
    s.push(Block::Table(Table {
        title: Some(format!("{strategy} — {} model", if model == PricingModel::BlackScholes { "Black-Scholes" } else { "Binomial (CRR, 300 steps)" })),
        columns: vec![
            Column::text("Side", 6),
            Column::text("Type", 6),
            Column::num("Strike", Format::Number { decimals: 2 }, 9),
            Column::num("Qty", Format::Number { decimals: 2 }, 6),
            Column::num("Price", Format::Number { decimals: 4 }, 9),
            Column::num("Delta", Format::Number { decimals: 3 }, 8),
            Column::num("Gamma", Format::Number { decimals: 4 }, 8),
            Column::num("Theta", Format::Number { decimals: 4 }, 8),
            Column::num("Vega", Format::Number { decimals: 4 }, 8),
        ],
        rows,
        page_size: None,
        numbered: false,
    }));

    let lo = spot * 0.6;
    let hi = spot * 1.4;
    let spots: Vec<f64> = (0..=120).map(|i| lo + (hi - lo) * f64::from(i) / 120.0).collect();
    let expiry_pnl = strategies::payoff_curve(&legs, &spots);
    let today_pnl = strategies::theoretical_curve(&legs, &spots, rate, q, 0.0);
    let (max_profit, max_loss) = strategies::max_profit_loss(&legs, lo, hi);
    let bes = strategies::breakevens(&legs, lo, hi);
    s.push(Block::Fields {
        title: Some("Payoff".into()),
        columns: 3,
        fields: vec![
            Field::num("Net Premium", Some(net_premium), Format::Number { decimals: 4 }),
            Field::opt_text("Max Profit", Some(max_profit.map_or("Unlimited".into(), |v| format!("{v:.4}")))),
            Field::opt_text("Max Loss", Some(max_loss.map_or("Unlimited".into(), |v| format!("{v:.4}")))),
            Field::text("Breakevens", if bes.is_empty() { "—".into() } else { bes.iter().map(|b| format!("{b:.2}")).collect::<Vec<_>>().join(", ") }),
            Field::num("Days", Some((expiry - today).num_days() as f64), Format::Integer),
            Field::num("Position Δ ($/1%)", Some(total.delta * spot * 0.01 * 100.0), Format::Number { decimals: 2 }),
        ],
    });
    s.push(Block::Xy(XyChart {
        title: "P&L per share vs underlying".into(),
        x_label: "Underlying".into(),
        y_label: "P&L".into(),
        series: vec![
            XySeries { name: "At expiry".into(), x: spots.clone(), y: expiry_pnl, style: Style::Emphasis, bars: false },
            XySeries { name: "Today (model)".into(), x: spots, y: today_pnl, style: Style::Warning, bars: false },
        ],
        x_marker: Some(spot),
        height_rows: 12,
    }));

    // Scenario grid: spot moves × time elapsed.
    let moves = [-0.20, -0.10, -0.05, 0.0, 0.05, 0.10, 0.20];
    let fractions = [0.0, 0.25, 0.5, 0.75];
    let mut columns = vec![Column::text("Spot Move", 10)];
    for f in fractions {
        columns.push(Column::num(&format!("T+{:.0}d", t * 365.0 * f), Format::Change { decimals: 3 }, 10));
    }
    columns.push(Column::num("Expiry", Format::Change { decimals: 3 }, 10));
    let scen_rows = moves
        .iter()
        .map(|m| {
            let sp = spot * (1.0 + m);
            let mut cells = vec![Cell::text(format!("{:+.0}% ({sp:.2})", m * 100.0))];
            for f in fractions {
                cells.push(Cell::signed(strategies::theoretical_curve(&legs, &[sp], rate, q, t * f).first().copied()));
            }
            cells.push(Cell::signed(Some(strategies::payoff_at_expiry(&legs, sp))));
            Row::new(cells)
        })
        .collect();
    s.push(Block::Table(Table { title: Some("Scenario P&L per share".into()), columns, rows: scen_rows, page_size: None, numbered: false }));
    s
}

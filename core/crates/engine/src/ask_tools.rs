//! ASK integration: the read-only tools the model may call, the AI service
//! used for filing summaries, and per-provider AI policy enforcement.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use meridian_analytics::{options as opt, returns, stats};
use meridian_ask::{
    AnthropicClient, AskConfig, AskContext, AskObserver, AskOutcome, AskSession, CancelFlag, Message, SourceRef, ToolAudit,
    ToolDefinition, ToolExecutor, ToolOutcome,
};
use meridian_provider::{AiPolicy, NewsQuery, NewsScope};
use meridian_types::{BarInterval, OptionRight, PeriodType, Provenance, SecurityKey, StatementKind, nanos_to_date};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex as AsyncMutex;

use crate::core::{AiService, Engine};
use crate::error::{EngineError, EngineResult};
use crate::events::EngineEvent;
use crate::screens::parse_range;

fn schema(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

fn def(name: &str, description: &str, props: Value, required: &[&str]) -> ToolDefinition {
    ToolDefinition { name: name.into(), description: description.into(), input_schema: schema(props, required) }
}

fn s(desc: &str) -> Value {
    json!({ "type": "string", "description": desc })
}

/// Tools exposed to the model. Every tool is read-only except
/// `show_function`, which only changes what a panel displays.
pub struct EngineTools {
    engine: Arc<Engine>,
}

impl EngineTools {
    #[must_use]
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }

    fn ai_allowed(&self, p: &Provenance) -> bool {
        if p.synthetic {
            return true;
        }
        self.engine
            .router()
            .capabilities()
            .iter()
            .find(|(id, _)| *id == p.provider)
            .is_some_and(|(_, c)| c.ai_policy == AiPolicy::Allowed)
    }

    fn guard(&self, p: &Provenance) -> Result<(), ToolOutcome> {
        if self.ai_allowed(p) {
            Ok(())
        } else {
            Err(ToolOutcome::error(format!(
                "Data from provider '{}' is excluded from ASK: its terms do not clearly allow sending data to a third-party AI service.",
                p.provider
            )))
        }
    }

    fn key(security: &str) -> Result<SecurityKey, ToolOutcome> {
        security.parse().map_err(|_| ToolOutcome::error(format!("not a security key: {security}. Use e.g. 'AAPL US Equity'.")))
    }
}

fn audit(tool: &str, input: &Value, sources: &[&Provenance], rows: Option<u64>) -> ToolAudit {
    ToolAudit {
        tool: tool.into(),
        input_json: input.to_string(),
        sources: sources.iter().map(|p| SourceRef::from(*p)).collect(),
        rows,
        ..ToolAudit::default()
    }
}

fn fin(x: f64) -> Value {
    if x.is_finite() { json!(x) } else { Value::Null }
}

#[derive(Deserialize)]
struct SecIn {
    security: String,
}

#[derive(Deserialize)]
struct BarsIn {
    security: String,
    interval: String,
    range: String,
}

#[derive(Deserialize)]
struct FundIn {
    security: String,
    period: String,
    statement: String,
}

#[derive(Deserialize)]
struct NewsIn {
    security: String,
    query: String,
}

#[derive(Deserialize)]
struct FilingsIn {
    security: String,
    form: String,
}

#[derive(Deserialize)]
struct FilingSectionIn {
    security: String,
    accession: String,
    section: String,
}

#[derive(Deserialize)]
struct SeriesIn {
    id: String,
    last_n: u32,
}

#[derive(Deserialize)]
struct SqlIn {
    sql: String,
}

#[derive(Deserialize)]
struct ComputeIn {
    op: String,
    x: Vec<f64>,
    y: Vec<f64>,
    a: f64,
    b: f64,
}

#[derive(Deserialize)]
struct ShowIn {
    function: String,
    security: String,
    range: String,
}

#[async_trait]
impl ToolExecutor for EngineTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![
            def("get_quote", "Current quote for one security (last, bid/ask, change, volume). Call for any question about the current or latest price.", json!({"security": s("Security key, e.g. 'AAPL US Equity', 'EURUSD Curncy', 'SPX Index'")}), &["security"]),
            def("get_bars", "Historical OHLCV bars. Use for price history, returns over a period, highs/lows. Returns at most 400 points (evenly sampled) plus exact first/last/high/low.", json!({"security": s("Security key"), "interval": s("1d, 1w, 1h, 30m, 5m or 1m"), "range": s("1D, 5D, 1M, 3M, 6M, YTD, 1Y, 2Y, 5Y, 10Y or MAX")}), &["security", "interval", "range"]),
            def("get_fundamentals", "Reported financial statements (values in currency units; EPS per share). Use for revenue, margins, earnings, balance sheet, cash flow.", json!({"security": s("Security key"), "period": s("annual or quarterly"), "statement": s("income, balance or cashflow")}), &["security", "period", "statement"]),
            def("get_estimates", "Consensus estimates (EPS, revenue, EBITDA) by period.", json!({"security": s("Security key")}), &["security"]),
            def("get_earnings", "Earnings history: reported EPS/revenue vs estimates and surprise %.", json!({"security": s("Security key")}), &["security"]),
            def("get_recommendations", "Analyst rating counts and price targets.", json!({"security": s("Security key")}), &["security"]),
            def("get_holders", "Largest institutional, fund and insider holders.", json!({"security": s("Security key")}), &["security"]),
            def("get_dividends", "Dividend and split history.", json!({"security": s("Security key")}), &["security"]),
            def("search_news", "Recent news headlines. Pass security '' for market news; query '' for no text filter.", json!({"security": s("Security key or ''"), "query": s("Text filter or ''")}), &["security", "query"]),
            def("search_filings", "SEC filings list for a company. form '' for all forms (e.g. '10-K', '10-Q', '8-K').", json!({"security": s("Security key"), "form": s("Form type or ''")}), &["security", "form"]),
            def("get_filing_section", "Text of one section of a filing (first 8,000 characters). section matches a heading substring, e.g. 'Risk Factors'.", json!({"security": s("Security key"), "accession": s("Accession number from search_filings"), "section": s("Heading substring")}), &["security", "accession", "section"]),
            def("get_option_summary", "Option chain summary for the nearest monthly expiry: spot, ATM IV, put/call open interest, and contracts near the money.", json!({"security": s("Underlying security key")}), &["security"]),
            def("get_economic_series", "Economic time series (FRED-style IDs such as CPIAUCSL, UNRATE, PAYEMS, FEDFUNDS, DGS10, GDPC1). Returns the last N observations.", json!({"id": s("Series ID"), "last_n": {"type": "integer", "description": "Observations to return (1-240)"}}), &["id", "last_n"]),
            def("get_yield_curve", "Latest US Treasury par yield curve.", json!({}), &[]),
            def("sql_query", "Read-only SQL (DuckDB dialect) over locally cached data: v_bars(provider, security, interval, ts, open, high, low, close, volume), v_fundamentals(provider, security, statement, period_type, fiscal_year, fiscal_period, period_end, currency, code, label, value), v_news(provider, source, headline, summary, url, published_at, tickers, topics), v_filings(provider, accession, cik, company, form, filed, period_of_report), v_econ(provider, series_id, date, value). Only SELECT; max 200 rows. MOCK mode caches nothing, so prefer the other tools.", json!({"sql": s("A single SELECT statement")}), &["sql"]),
            def("compute", "Arithmetic and statistics on numbers you already retrieved. ALWAYS use this for any derived figure (percent changes, ratios, growth, averages). ops: ratio (a/b), difference (a-b), pct_change ((b/a-1)*100), sum (x), mean (x), stdev (x, sample), cagr (a=start, b=end, y=[years]) in %, correlation (x,y), beta (x=asset returns, y=market returns), annualized_vol (x=daily simple returns) in %, max_drawdown (x=prices) in %, bs_call/bs_put (x=[spot, strike, years, vol, rate]). Pass [] or 0 for unused inputs.", json!({"op": {"type": "string", "enum": ["ratio","difference","pct_change","sum","mean","stdev","cagr","correlation","beta","annualized_vol","max_drawdown","bs_call","bs_put"]}, "x": {"type": "array", "items": {"type": "number"}}, "y": {"type": "array", "items": {"type": "number"}}, "a": {"type": "number"}, "b": {"type": "number"}}), &["op", "x", "y", "a", "b"]),
            def("get_watchlists", "The user's watchlists and their securities.", json!({}), &[]),
            def("show_function", "Display a terminal function in the user's next panel, e.g. GP chart, FA financials, OMON options, HP prices. Use after answering when a chart or table would help.", json!({"function": s("Mnemonic: GP, GIP, HP, DES, FA, EE, ERN, ANR, HDS, DVD, CN, CF, OMON, OVDV, CORR, ECO, WEI, FXC, CRYP"), "security": s("Security key or '' for functions without one"), "range": s("Range for GP/GIP like 1Y, or ''")}), &["function", "security", "range"]),
        ]
    }

    #[allow(clippy::too_many_lines)]
    async fn execute(&self, name: &str, input: Value) -> ToolOutcome {
        let started = Instant::now();
        let mut out = self.run(name, &input).await.unwrap_or_else(|e| e);
        out.audit.tool = name.into();
        if out.audit.input_json.is_empty() {
            out.audit.input_json = input.to_string();
        }
        out.audit.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        out
    }
}

macro_rules! parse_in {
    ($ty:ty, $input:expr) => {
        serde_json::from_value::<$ty>($input.clone()).map_err(|e| ToolOutcome::error(format!("invalid input: {e}")))?
    };
}

impl EngineTools {
    #[allow(clippy::too_many_lines)]
    async fn run(&self, name: &str, input: &Value) -> Result<ToolOutcome, ToolOutcome> {
        let e = &self.engine;
        let err = |x: EngineError| ToolOutcome::error(x.user_message());
        match name {
            "get_quote" => {
                let i = parse_in!(SecIn, input);
                let key = Self::key(&i.security)?;
                let prov = e.quote_provenance(&key).ok_or_else(|| ToolOutcome::error("no quote source for this security"))?;
                self.guard(&prov)?;
                let r = e.quote_row(&key).await.ok_or_else(|| ToolOutcome::error("no quote available"))?;
                let v = json!({
                    "security": key.to_string(), "last": fin(r.last), "bid": fin(r.bid), "ask": fin(r.ask),
                    "open": fin(r.open), "high": fin(r.high), "low": fin(r.low), "prev_close": fin(r.prev_close),
                    "net_change": fin(r.net_change), "pct_change": fin(r.pct_change), "volume": fin(r.volume),
                    "as_of": meridian_types::nanos_to_datetime(r.ts_event).to_rfc3339(),
                    "source": prov.provider.to_string(), "delay": prov.delay.badge(), "synthetic": prov.synthetic,
                });
                Ok(ToolOutcome::json(&v, audit(name, input, &[&prov], Some(1))))
            }
            "get_bars" => {
                let i = parse_in!(BarsIn, input);
                let key = Self::key(&i.security)?;
                let iv = BarInterval::from_code(&i.interval).ok_or_else(|| ToolOutcome::error("bad interval"))?;
                let (from, to) = parse_range(&i.range, e.now());
                let b = e.bars(&key, iv, from, to).await.map_err(err)?;
                self.guard(&b.value.provenance)?;
                let s = &b.value;
                if s.is_empty() {
                    return Err(ToolOutcome::error("no bars in range"));
                }
                let n = s.len();
                let step = n.div_ceil(400).max(1);
                let pts: Vec<Value> = (0..n)
                    .filter(|k| k % step == 0 || *k == n - 1)
                    .map(|k| json!([nanos_to_date(s.ts[k]).to_string(), s.close[k]]))
                    .collect();
                let (hi_i, hi) = s.high.iter().enumerate().fold((0, f64::MIN), |a, (k, v)| if *v > a.1 { (k, *v) } else { a });
                let (lo_i, lo) = s.low.iter().enumerate().fold((0, f64::MAX), |a, (k, v)| if *v < a.1 { (k, *v) } else { a });
                let v = json!({
                    "security": key.to_string(), "interval": iv.code(), "bars": n,
                    "first": {"date": nanos_to_date(s.ts[0]).to_string(), "close": s.close[0]},
                    "last": {"date": nanos_to_date(s.ts[n - 1]).to_string(), "close": s.close[n - 1]},
                    "high": {"date": nanos_to_date(s.ts[hi_i]).to_string(), "value": hi},
                    "low": {"date": nanos_to_date(s.ts[lo_i]).to_string(), "value": lo},
                    "sampled_closes": pts, "sampling_step": step,
                    "source": s.provenance.provider.to_string(), "synthetic": s.provenance.synthetic, "stale_cache": b.stale,
                });
                Ok(ToolOutcome::json(&v, audit(name, input, &[&s.provenance], Some(n as u64))))
            }
            "get_fundamentals" => {
                let i = parse_in!(FundIn, input);
                let key = Self::key(&i.security)?;
                let pt = if i.period.starts_with('q') { PeriodType::Quarterly } else { PeriodType::Annual };
                let kind = match i.statement.as_str() {
                    "balance" => StatementKind::Balance,
                    "cashflow" | "cash_flow" => StatementKind::CashFlow,
                    _ => StatementKind::Income,
                };
                let f = e.fundamentals(&key, pt, if pt == PeriodType::Annual { 10 } else { 12 }).await.map_err(err)?;
                self.guard(&f.value.provenance)?;
                let periods: Vec<Value> = f
                    .value
                    .statements
                    .iter()
                    .filter(|st| st.kind == kind)
                    .map(|st| {
                        let lines: serde_json::Map<String, Value> = st.lines.iter().map(|l| (l.code.clone(), l.value.map_or(Value::Null, |v| json!(v)))).collect();
                        json!({"fiscal_year": st.fiscal_year, "fiscal_period": st.fiscal_period, "period_end": st.period_end.to_string(), "currency": st.currency, "values": lines})
                    })
                    .collect();
                let rows = periods.len() as u64;
                let v = json!({"security": key.to_string(), "statement": i.statement, "period": i.period, "periods": periods, "source": f.value.provenance.provider.to_string(), "synthetic": f.value.provenance.synthetic});
                Ok(ToolOutcome::json(&v, audit(name, input, &[&f.value.provenance], Some(rows))))
            }
            "get_estimates" | "get_earnings" | "get_recommendations" | "get_holders" | "get_dividends" => {
                let i = parse_in!(SecIn, input);
                let key = Self::key(&i.security)?;
                let r = e.router();
                let (val, prov) = match name {
                    "get_estimates" => r.estimates(&key).await.map(|x| (serde_json::to_value(&x.value.estimates), x.value.provenance)),
                    "get_earnings" => r.earnings(&key).await.map(|x| (serde_json::to_value(&x.value.records), x.value.provenance)),
                    "get_recommendations" => r.recommendations(&key).await.map(|x| {
                        let mut v = x.value.clone();
                        v.ratings.truncate(15);
                        (serde_json::to_value(&v), x.value.provenance)
                    }),
                    "get_holders" => r.holders(&key).await.map(|x| {
                        let mut h = x.value.holders.clone();
                        h.sort_by(|a, b| b.shares.total_cmp(&a.shares));
                        h.truncate(25);
                        (serde_json::to_value(&h), x.value.provenance)
                    }),
                    _ => r.dividends(&key).await.map(|x| {
                        let mut d = x.value.dividends.clone();
                        d.sort_by(|a, b| b.ex_date.cmp(&a.ex_date));
                        d.truncate(40);
                        (serde_json::to_value(&d), x.value.provenance)
                    }),
                }
                .map_err(|x| err(x.into()))?;
                self.guard(&prov)?;
                let data = val.map_err(|x| ToolOutcome::error(x.to_string()))?;
                let rows = data.as_array().map_or(1, Vec::len) as u64;
                let v = json!({"security": key.to_string(), "data": data, "source": prov.provider.to_string(), "synthetic": prov.synthetic});
                Ok(ToolOutcome::json(&v, audit(name, input, &[&prov], Some(rows))))
            }
            "search_news" => {
                let i = parse_in!(NewsIn, input);
                let keys: Vec<SecurityKey> = if i.security.trim().is_empty() { vec![] } else { vec![Self::key(&i.security)?] };
                let q = NewsQuery {
                    scope: if keys.is_empty() { NewsScope::Market } else { NewsScope::Company },
                    keys,
                    text: (!i.query.trim().is_empty()).then(|| i.query.clone()),
                    from: Some(e.now() - 14 * meridian_types::NANOS_PER_DAY),
                    to: None,
                    limit: 30,
                };
                let items = e.news_items(q).await.map_err(err)?;
                let allowed: Vec<_> = items.iter().filter(|n| self.ai_allowed(&n.provenance)).collect();
                let provs: Vec<&Provenance> = allowed.iter().map(|n| &n.provenance).collect();
                let list: Vec<Value> = allowed
                    .iter()
                    .map(|n| json!({"published": meridian_types::nanos_to_datetime(n.published_at).to_rfc3339(), "source": n.source, "headline": n.headline, "summary": n.summary, "tickers": n.tickers, "synthetic": n.provenance.synthetic}))
                    .collect();
                let excluded = items.len() - allowed.len();
                let v = json!({"stories": list, "excluded_by_terms": excluded});
                Ok(ToolOutcome::json(&v, audit(name, input, &provs.into_iter().take(3).collect::<Vec<_>>(), Some(allowed.len() as u64))))
            }
            "search_filings" => {
                let i = parse_in!(FilingsIn, input);
                let key = Self::key(&i.security)?;
                let forms: Vec<&str> = if i.form.trim().is_empty() { vec![] } else { vec![i.form.as_str()] };
                let f = e.filings_list(&key, &forms, 40).await.map_err(err)?;
                if let Some(first) = f.value.first() {
                    self.guard(&first.provenance)?;
                }
                let list: Vec<Value> = f
                    .value
                    .iter()
                    .map(|x| json!({"accession": x.accession, "form": x.form, "filed": x.filed.to_string(), "period": x.period_of_report.map(|d| d.to_string()), "description": x.description}))
                    .collect();
                let provs: Vec<&Provenance> = f.value.first().map(|x| &x.provenance).into_iter().collect();
                Ok(ToolOutcome::json(&json!({"security": key.to_string(), "filings": list}), audit(name, input, &provs, Some(f.value.len() as u64))))
            }
            "get_filing_section" => {
                let i = parse_in!(FilingSectionIn, input);
                let key = Self::key(&i.security)?;
                let list = e.filings_list(&key, &[], 400).await.map_err(err)?;
                let filing = list.value.iter().find(|f| f.accession == i.accession).ok_or_else(|| ToolOutcome::error("accession not found; call search_filings first"))?;
                self.guard(&filing.provenance)?;
                let doc = e.filing_document(filing).await.map_err(err)?;
                let needle = i.section.to_lowercase();
                let sec = doc.value.sections.iter().find(|s| s.title.to_lowercase().contains(&needle));
                let v = match sec {
                    Some(s) => json!({"title": s.title, "text": s.text.chars().take(8000).collect::<String>(), "truncated": s.text.chars().count() > 8000}),
                    None => json!({"error": "section not found", "available_sections": doc.value.sections.iter().map(|s| s.title.clone()).collect::<Vec<_>>()}),
                };
                Ok(ToolOutcome::json(&v, audit(name, input, &[&filing.provenance], Some(1))))
            }
            "get_option_summary" => {
                let i = parse_in!(SecIn, input);
                let key = Self::key(&i.security)?;
                let chain = e.option_chain(&key).await.map_err(err)?;
                self.guard(&chain.value.provenance)?;
                let p = e.pricing_params(&key, chain.value.underlying_price).await.map_err(err)?;
                let today = nanos_to_date(e.now());
                let exp = chain.value.expiries().into_iter().find(|d| (*d - today).num_days() >= 20).ok_or_else(|| ToolOutcome::error("no expiry ≥ 20 days"))?;
                let t = ((exp - today).num_days() as f64 + 0.7) / 365.0;
                let mut near: Vec<_> = chain.value.contracts.iter().filter(|c| c.expiry == exp).collect();
                near.sort_by(|a, b| (a.strike - p.spot).abs().total_cmp(&(b.strike - p.spot).abs()));
                let iv_of = |c: &meridian_types::OptionContract| -> Option<f64> {
                    c.greeks.as_ref().and_then(|g| g.iv).or_else(|| {
                        let mid = c.mid()?;
                        opt::implied_volatility(mid, &opt::BsInputs { spot: p.spot, strike: c.strike, rate: p.rate, dividend_yield: p.div_yield, vol: 0.3, time_years: t, right: c.right }, meridian_types::ExerciseStyle::European, opt::PricingModel::BlackScholes)
                    })
                };
                let atm = near.iter().take(2).filter_map(|c| iv_of(c)).collect::<Vec<_>>();
                let call_oi: f64 = chain.value.contracts.iter().filter(|c| c.expiry == exp && c.right == OptionRight::Call).filter_map(|c| c.open_interest).sum();
                let put_oi: f64 = chain.value.contracts.iter().filter(|c| c.expiry == exp && c.right == OptionRight::Put).filter_map(|c| c.open_interest).sum();
                let rows: Vec<Value> = near
                    .iter()
                    .take(10)
                    .map(|c| json!({"strike": c.strike, "right": if c.right == OptionRight::Call {"call"} else {"put"}, "bid": c.bid, "ask": c.ask, "iv": iv_of(c), "open_interest": c.open_interest}))
                    .collect();
                let v = json!({
                    "security": key.to_string(), "spot": p.spot, "expiry": exp.to_string(), "days": (exp - today).num_days(),
                    "atm_iv": if atm.is_empty() { Value::Null } else { json!(atm.iter().sum::<f64>() / atm.len() as f64) },
                    "put_call_oi_ratio": if call_oi > 0.0 { json!(put_oi / call_oi) } else { Value::Null },
                    "rate_used": p.rate_source, "contracts_near_money": rows,
                });
                Ok(ToolOutcome::json(&v, audit(name, input, &[&chain.value.provenance], Some(near.len() as u64))))
            }
            "get_economic_series" => {
                let i = parse_in!(SeriesIn, input);
                let f = e.economic_series(&i.id).await.map_err(err)?;
                self.guard(&f.value.provenance)?;
                let valid: Vec<_> = f.value.observations.iter().filter(|o| o.value.is_some()).collect();
                let n = (i.last_n.clamp(1, 240) as usize).min(valid.len());
                let obs: Vec<Value> = valid[valid.len() - n..].iter().map(|o| json!([o.date.to_string(), o.value])).collect();
                let v = json!({"id": f.value.id, "title": f.value.title, "units": f.value.units, "frequency": f.value.frequency, "observations": obs, "synthetic": f.value.provenance.synthetic});
                Ok(ToolOutcome::json(&v, audit(name, input, &[&f.value.provenance], Some(obs.len() as u64))))
            }
            "get_yield_curve" => {
                let c = e.yield_curve("UST", None).await.map_err(err)?;
                self.guard(&c.value.provenance)?;
                let pts: Vec<Value> = c.value.points.iter().map(|p| json!({"tenor": p.tenor, "yield_pct": p.yield_pct})).collect();
                Ok(ToolOutcome::json(&json!({"date": c.value.date.to_string(), "points": pts}), audit(name, input, &[&c.value.provenance], Some(pts.len() as u64))))
            }
            "sql_query" => {
                let i = parse_in!(SqlIn, input);
                let store = e.stores().clone();
                let sql = i.sql.clone();
                let r = tokio::task::spawn_blocking(move || store.market.query_ask(&sql, 200, Duration::from_secs(5)))
                    .await
                    .map_err(|x| ToolOutcome::error(x.to_string()))?
                    .map_err(|x| ToolOutcome::error(x.to_string()))?;
                // Rows from providers whose terms forbid AI use must not leave.
                let caps = e.router().capabilities();
                let blocked: Vec<String> = caps.iter().filter(|(_, c)| c.ai_policy != AiPolicy::Allowed).map(|(id, _)| id.to_string()).collect();
                let pcol = r.columns.iter().position(|c| c == "provider");
                if !blocked.is_empty() && pcol.is_none() {
                    return Err(ToolOutcome::error("include the provider column so rows from providers excluded by their terms can be filtered"));
                }
                let rows: Vec<&Vec<Value>> = r.rows.iter().filter(|row| pcol.is_none_or(|i| !blocked.iter().any(|b| row.get(i).and_then(Value::as_str) == Some(b.as_str())))).collect();
                let v = json!({"columns": r.columns, "rows": rows, "truncated": r.truncated});
                let mut a = audit(name, input, &[], Some(rows.len() as u64));
                a.sql = Some(i.sql);
                Ok(ToolOutcome::json(&v, a))
            }
            "compute" => {
                let i = parse_in!(ComputeIn, input);
                let r = compute(&i).map_err(ToolOutcome::error)?;
                Ok(ToolOutcome::json(&json!({"op": i.op, "result": fin(r)}), audit(name, input, &[], None)))
            }
            "get_watchlists" => {
                let w = e.stores().app.watchlists().map_err(|x| ToolOutcome::error(x.to_string()))?;
                let v: Vec<Value> = w.iter().map(|l| json!({"name": l.name, "securities": l.securities})).collect();
                Ok(ToolOutcome::json(&json!({"watchlists": v}), audit(name, input, &[], Some(v.len() as u64))))
            }
            "show_function" => {
                let i = parse_in!(ShowIn, input);
                let sec = (!i.security.trim().is_empty()).then(|| i.security.clone());
                if let Some(s) = &sec {
                    Self::key(s)?;
                }
                let mut args = Vec::new();
                if !i.range.trim().is_empty() {
                    args.push(("range".to_string(), i.range.clone()));
                }
                e.emit(EngineEvent::Show { function: i.function.to_ascii_uppercase(), security: sec, args });
                Ok(ToolOutcome::json(&json!({"shown": true}), audit(name, input, &[], None)))
            }
            other => Err(ToolOutcome::error(format!("unknown tool {other}"))),
        }
    }
}

/// Deterministic arithmetic for derived figures.
fn compute(i: &ComputeIn) -> Result<f64, String> {
    let need = |v: &[f64], n: usize| if v.len() >= n { Ok(()) } else { Err(format!("{} needs at least {n} values", i.op)) };
    Ok(match i.op.as_str() {
        "ratio" => {
            if i.b == 0.0 {
                return Err("division by zero".into());
            }
            i.a / i.b
        }
        "difference" => i.a - i.b,
        "pct_change" => {
            if i.a == 0.0 {
                return Err("pct_change from zero".into());
            }
            (i.b / i.a - 1.0) * 100.0
        }
        "sum" => i.x.iter().sum(),
        "mean" => {
            need(&i.x, 1)?;
            stats::mean(&i.x)
        }
        "stdev" => {
            need(&i.x, 2)?;
            stats::std_dev(&i.x)
        }
        "cagr" => {
            let years = i.y.first().copied().ok_or("cagr needs y=[years]")?;
            returns::cagr(i.a, i.b, years) * 100.0
        }
        "correlation" => {
            need(&i.x, 3)?;
            stats::correlation(&i.x, &i.y)
        }
        "beta" => {
            need(&i.x, 3)?;
            stats::beta(&i.x, &i.y)
        }
        "annualized_vol" => {
            need(&i.x, 2)?;
            returns::annualized_volatility(&i.x, 252.0) * 100.0
        }
        "max_drawdown" => {
            need(&i.x, 2)?;
            returns::max_drawdown(&i.x).max_drawdown * 100.0
        }
        "bs_call" | "bs_put" => {
            need(&i.x, 5)?;
            opt::black_scholes_price(&opt::BsInputs {
                spot: i.x[0],
                strike: i.x[1],
                time_years: i.x[2],
                vol: i.x[3],
                rate: i.x[4],
                dividend_yield: 0.0,
                right: if i.op == "bs_call" { OptionRight::Call } else { OptionRight::Put },
            })
        }
        other => return Err(format!("unknown op {other}")),
    })
}

/// ASK sessions held by the engine.
pub struct AskService {
    engine: Arc<Engine>,
    client: Arc<AnthropicClient>,
    sessions: AsyncMutex<HashMap<String, Arc<AsyncMutex<AskSession>>>>,
    cancels: parking_lot::Mutex<HashMap<String, CancelFlag>>,
}

impl AskService {
    #[must_use]
    pub fn new(engine: Arc<Engine>, client: Arc<AnthropicClient>) -> Self {
        Self { engine, client, sessions: AsyncMutex::new(HashMap::new()), cancels: parking_lot::Mutex::new(HashMap::new()) }
    }

    /// Answers `question` in session `session_id` (created on first use).
    pub async fn ask(&self, session_id: &str, question: &str, security: Option<SecurityKey>, observer: Arc<dyn AskObserver>) -> EngineResult<AskOutcome> {
        let session = {
            let mut map = self.sessions.lock().await;
            map.entry(session_id.to_owned())
                .or_insert_with(|| {
                    let tools: Arc<dyn ToolExecutor> = Arc::new(EngineTools::new(self.engine.clone()));
                    Arc::new(AsyncMutex::new(
                        AskSession::new(self.client.clone(), tools, AskConfig::default(), session_id).with_clock(self.engine.clock().clone()),
                    ))
                })
                .clone()
        };
        let cancel = CancelFlag::new();
        self.cancels.lock().insert(session_id.to_owned(), cancel.clone());
        let ctx = AskContext { panel_security: security, date: nanos_to_date(self.engine.now()) };
        let mut s = session.lock().await;
        let r = s.ask(question, &ctx, observer, cancel).await;
        self.cancels.lock().remove(session_id);
        let now = self.engine.now();
        let _ = self.engine.stores().app.create_ask_session(session_id, &question.chars().take(60).collect::<String>(), now);
        if let Some(turn) = s.transcript().turns.last()
            && let Ok(j) = serde_json::to_string(turn)
        {
            let _ = self.engine.stores().app.append_ask_turn(session_id, &j, now);
        }
        r.map_err(|e| EngineError::Internal(e.to_string()))
    }

    pub fn cancel(&self, session_id: &str) {
        if let Some(c) = self.cancels.lock().get(session_id) {
            c.cancel();
        }
    }
}

/// Filing summaries through the same client (single request, no tools).
pub struct ClaudeAi {
    client: Arc<AnthropicClient>,
}

impl ClaudeAi {
    #[must_use]
    pub fn new(client: Arc<AnthropicClient>) -> Self {
        Self { client }
    }
}

const SUMMARY_SYSTEM: &str = "You summarize SEC filings for an analyst using a terminal. Write a concise summary (at most 250 words) with short headed sections: Overview, Key Changes or Events, Financial Highlights, Risks. Use only facts and numbers that appear verbatim in the provided text; if a section has nothing relevant, write 'Not discussed in this filing.' Never estimate or invent figures. Plain text, no markdown tables.";

#[async_trait]
impl AiService for ClaudeAi {
    async fn summarize_filing(&self, filing: &meridian_types::Filing, doc: &meridian_types::FilingDocument) -> EngineResult<String> {
        let mut text = doc.full_text();
        if text.chars().count() > 150_000 {
            text = text.chars().take(150_000).collect();
        }
        let opts = meridian_ask::RequestOptions { effort: meridian_ask::Effort::Medium, ..meridian_ask::RequestOptions::default() };
        let msg = Message::user(vec![json!({"type": "text", "text": format!("{} {} filed {} (accession {}).\n\n<filing>\n{text}\n</filing>", filing.company, filing.form, filing.filed, filing.accession)})]);
        let body = meridian_ask::request::build_body(&opts, SUMMARY_SYSTEM, &[], &[msg]);
        let betas = opts.betas();
        let cancel = CancelFlag::new();
        let mut sink = |_: meridian_ask::message::StreamUpdate<'_>| {};
        let reply = self.client.stream_message(&body, &betas, &cancel, &mut sink).await.map_err(|e| EngineError::Internal(e.to_string()))?;
        if let Some(cat) = reply.refusal_category() {
            return Err(EngineError::Internal(format!("the model declined ({cat})")));
        }
        Ok(format!("{}\n\n— Summary generated by {} from the filing text. Verify figures against the document.", reply.text(), reply.served_model()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ci(op: &str, x: Vec<f64>, y: Vec<f64>, a: f64, b: f64) -> ComputeIn {
        ComputeIn { op: op.into(), x, y, a, b }
    }

    #[test]
    fn compute_ops() {
        assert_eq!(compute(&ci("pct_change", vec![], vec![], 100.0, 110.0)).unwrap(), 10.000000000000009);
        assert_eq!(compute(&ci("ratio", vec![], vec![], 1.0, 4.0)).unwrap(), 0.25);
        assert!(compute(&ci("ratio", vec![], vec![], 1.0, 0.0)).is_err());
        assert!((compute(&ci("cagr", vec![], vec![2.0], 100.0, 121.0)).unwrap() - 10.0).abs() < 1e-9);
        assert_eq!(compute(&ci("sum", vec![1.0, 2.0, 3.0], vec![], 0.0, 0.0)).unwrap(), 6.0);
        assert!(compute(&ci("bogus", vec![], vec![], 0.0, 0.0)).is_err());
    }

    #[test]
    fn tool_schemas_are_strict() {
        // Every schema must be closed and require all its properties.
        let tools = vec![
            def("x", "d", json!({"a": s("a")}), &["a"]),
        ];
        for t in tools {
            assert_eq!(t.input_schema["additionalProperties"], json!(false));
        }
    }
}

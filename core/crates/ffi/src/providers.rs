//! Composition root: builds the provider set for the data mode. MOCK mode
//! registers only the mock provider; LIVE mode registers only real
//! providers. They never mix. Order matters: the router tries providers in
//! this order for each capability.

use std::sync::Arc;

use meridian_ask::AnthropicClient;
use meridian_engine::{AiService, DataMode, EngineConfig};
use meridian_provider::Provider;
use meridian_types::{Clock, FixedClock, SystemClock};
use secrecy::SecretString;

use crate::core::{CoreConfigFfi, SecretSource, read_provider_settings};

pub(crate) struct Built {
    pub providers: Vec<Arc<dyn Provider>>,
    pub ai: Option<Arc<dyn AiService>>,
    pub anthropic: Option<Arc<AnthropicClient>>,
}

fn secret(secrets: &dyn SecretSource, provider: &str, field: &str) -> Option<SecretString> {
    secrets.secret(provider.into(), field.into()).filter(|s| !s.trim().is_empty()).map(|s| SecretString::from(s.trim().to_owned()))
}

pub(crate) fn build(config: &CoreConfigFfi, econf: &EngineConfig, secrets: &dyn SecretSource) -> Built {
    let clock: Arc<dyn Clock> = match econf.fixed_clock {
        Some(t) => Arc::new(FixedClock(t)),
        None => Arc::new(SystemClock),
    };
    let providers: Vec<Arc<dyn Provider>> = match econf.mode {
        DataMode::Mock => vec![Arc::new(meridian_provider_mock::MockProvider::new(meridian_provider_mock::MockConfig {
            seed: config.mock_seed,
            clock,
            extra_symbols: config.mock_extra_symbols as usize,
            // 0 disables ticks (deterministic snapshots); negative = default.
            updates_per_symbol_per_sec: if config.mock_update_rate >= 0.0 { config.mock_update_rate } else { 2.0 },
        }))],
        DataMode::Live => live(econf, secrets),
    };
    let anthropic = anthropic(secrets).map(Arc::new);
    let ai: Option<Arc<dyn AiService>> = anthropic.clone().map(|c| Arc::new(meridian_engine::ask_tools::ClaudeAi::new(c)) as Arc<dyn AiService>);
    Built { providers, ai, anthropic }
}

/// Live providers in routing order: the router tries them in this order for
/// each capability.
pub(crate) const LIVE_ORDER: &[&str] = &["alpaca", "coinbase", "kraken", "frankfurter", "treasury", "fred", "edgar", "finnhub", "rss"];

fn live(econf: &EngineConfig, secrets: &dyn SecretSource) -> Vec<Arc<dyn Provider>> {
    LIVE_ORDER
        .iter()
        .filter_map(|name| match single(name, econf, secrets)? {
            Ok(p) => Some(p),
            Err(e) => {
                tracing::warn!(provider = name, error = %e, "provider unavailable");
                None
            }
        })
        .collect()
}

fn wrap<P: Provider>(r: meridian_provider::ProviderResult<P>) -> Result<Arc<dyn Provider>, String> {
    r.map(|p| Arc::new(p) as Arc<dyn Provider>).map_err(|e| e.to_string())
}

/// One live provider built from the current credentials and settings, or
/// `None` for an unknown name.
pub(crate) fn single(name: &str, econf: &EngineConfig, secrets: &dyn SecretSource) -> Option<Result<Arc<dyn Provider>, String>> {
    Some(match name {
        "alpaca" => {
            let feed = read_provider_settings(econf, "alpaca")
                .and_then(|v| v.get("feed").and_then(|f| f.as_str()).map(str::to_owned))
                .unwrap_or_else(|| "iex".into());
            wrap(meridian_provider_alpaca::AlpacaProvider::new(meridian_provider_alpaca::AlpacaConfig {
                key_id: secret(secrets, "alpaca", "key_id"),
                secret_key: secret(secrets, "alpaca", "secret_key"),
                feed: if feed == "sip" { meridian_provider_alpaca::AlpacaFeed::Sip } else { meridian_provider_alpaca::AlpacaFeed::Iex },
            }))
        }
        "coinbase" => wrap(meridian_provider_coinbase::CoinbaseProvider::new()),
        "kraken" => wrap(meridian_provider_kraken::KrakenProvider::new()),
        "frankfurter" => wrap(meridian_provider_frankfurter::FrankfurterProvider::new()),
        "treasury" => wrap(meridian_provider_treasury::TreasuryProvider::new()),
        "fred" => wrap(meridian_provider_fred::FredProvider::new(meridian_provider_fred::FredConfig { api_key: secret(secrets, "fred", "api_key") })),
        "edgar" => {
            let contact = read_provider_settings(econf, "edgar")
                .and_then(|v| v.get("contact").and_then(|f| f.as_str()).map(str::to_owned))
                .unwrap_or_default();
            wrap(meridian_provider_edgar::EdgarProvider::new(meridian_provider_edgar::EdgarConfig { contact }))
        }
        "finnhub" => wrap(meridian_provider_finnhub::FinnhubProvider::new(meridian_provider_finnhub::FinnhubConfig { api_key: secret(secrets, "finnhub", "api_key") })),
        "rss" => wrap(meridian_provider_rss::RssProvider::new(meridian_provider_rss::RssConfig::default())),
        _ => return None,
    })
}

/// The Anthropic client from the stored key, if one is set.
pub(crate) fn anthropic(secrets: &dyn SecretSource) -> Option<AnthropicClient> {
    secret(secrets, "anthropic", "api_key").and_then(|k| AnthropicClient::new(k).ok())
}

fn price(v: Option<f64>) -> String {
    v.map_or_else(|| "no price yet".to_owned(), |x| format!("{x:.2}"))
}

/// One cheap real request that shows `p` works with the current settings,
/// described in a sentence for Settings.
pub(crate) async fn probe(name: &str, p: Arc<dyn Provider>) -> Result<String, meridian_provider::ProviderError> {
    use meridian_provider::{CurveRequest, NewsQuery, NewsScope, SeriesRequest};
    use meridian_types::SecurityKey;
    let first_quote = |key: SecurityKey, label: &'static str| {
        let p = p.clone();
        async move {
            let q = p.quotes(std::slice::from_ref(&key)).await?;
            let last = q.first().and_then(|q| q.last.or(q.bid));
            Ok::<String, meridian_provider::ProviderError>(format!("Connected · {label} {}", price(last)))
        }
    };
    match name {
        "alpaca" => first_quote(SecurityKey::equity("AAPL"), "AAPL").await,
        "coinbase" | "kraken" => first_quote(SecurityKey::currency("BTCUSD"), "BTC/USD").await,
        "frankfurter" => first_quote(SecurityKey::currency("EURUSD"), "EUR/USD").await,
        "treasury" => {
            let c = p.yield_curve(&CurveRequest { name: "UST".into(), date: None }).await?;
            let ten = c.points.iter().find(|pt| pt.tenor.eq_ignore_ascii_case("10Y")).and_then(|pt| pt.yield_pct);
            Ok(format!("Connected · par curve for {}, 10Y {}%", c.date, price(ten)))
        }
        "fred" => {
            let s = p.economic_series(&SeriesRequest { id: "UNRATE".into(), from: None, to: None }).await?;
            let last = s.observations.iter().rev().find(|o| o.value.is_some());
            Ok(match last {
                Some(o) => format!("Connected · unemployment rate {}% ({})", price(o.value), o.date),
                None => "Connected · series returned no observations".to_owned(),
            })
        }
        "edgar" => {
            let i = p.instrument(&SecurityKey::equity("AAPL")).await?;
            Ok(format!("Connected · AAPL is {}", i.name))
        }
        "finnhub" => {
            let r = p.recommendations(&SecurityKey::equity("AAPL")).await?;
            let n = r.strong_buy + r.buy + r.hold + r.sell + r.strong_sell;
            Ok(format!("Connected · {n} analyst ratings on AAPL"))
        }
        "rss" => {
            let page = p
                .news(&NewsQuery { scope: NewsScope::PressReleases, keys: vec![], text: None, from: None, to: None, limit: 20 })
                .await?;
            Ok(format!("Connected · {} recent press releases", page.items.len()))
        }
        _ => Err(meridian_provider::ProviderError::NotFound(format!("unknown data source {name}"))),
    }
}

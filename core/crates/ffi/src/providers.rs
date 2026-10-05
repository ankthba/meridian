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
use crate::error::CoreResult;

pub(crate) struct Built {
    pub providers: Vec<Arc<dyn Provider>>,
    pub ai: Option<Arc<dyn AiService>>,
    pub anthropic: Option<Arc<AnthropicClient>>,
}

fn secret(secrets: &dyn SecretSource, provider: &str, field: &str) -> Option<SecretString> {
    secrets.secret(provider.into(), field.into()).filter(|s| !s.trim().is_empty()).map(|s| SecretString::from(s.trim().to_owned()))
}

pub(crate) fn build(config: &CoreConfigFfi, econf: &EngineConfig, secrets: &dyn SecretSource) -> CoreResult<Built> {
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
    let anthropic = secret(secrets, "anthropic", "api_key").and_then(|k| AnthropicClient::new(k).ok()).map(Arc::new);
    let ai: Option<Arc<dyn AiService>> = anthropic.clone().map(|c| Arc::new(meridian_engine::ask_tools::ClaudeAi::new(c)) as Arc<dyn AiService>);
    Ok(Built { providers, ai, anthropic })
}

fn push<P: Provider>(out: &mut Vec<Arc<dyn Provider>>, r: meridian_provider::ProviderResult<P>, name: &str) {
    match r {
        Ok(p) => out.push(Arc::new(p)),
        Err(e) => tracing::warn!(provider = name, error = %e, "provider unavailable"),
    }
}

fn live(econf: &EngineConfig, secrets: &dyn SecretSource) -> Vec<Arc<dyn Provider>> {
    let mut out: Vec<Arc<dyn Provider>> = Vec::new();
    let alpaca_feed = read_provider_settings(econf, "alpaca")
        .and_then(|v| v.get("feed").and_then(|f| f.as_str()).map(str::to_owned))
        .unwrap_or_else(|| "iex".into());
    push(
        &mut out,
        meridian_provider_alpaca::AlpacaProvider::new(meridian_provider_alpaca::AlpacaConfig {
            key_id: secret(secrets, "alpaca", "key_id"),
            secret_key: secret(secrets, "alpaca", "secret_key"),
            feed: if alpaca_feed == "sip" { meridian_provider_alpaca::AlpacaFeed::Sip } else { meridian_provider_alpaca::AlpacaFeed::Iex },
        }),
        "alpaca",
    );
    push(&mut out, meridian_provider_coinbase::CoinbaseProvider::new(), "coinbase");
    push(&mut out, meridian_provider_kraken::KrakenProvider::new(), "kraken");
    push(&mut out, meridian_provider_frankfurter::FrankfurterProvider::new(), "frankfurter");
    push(&mut out, meridian_provider_treasury::TreasuryProvider::new(), "treasury");
    push(&mut out, meridian_provider_fred::FredProvider::new(meridian_provider_fred::FredConfig { api_key: secret(secrets, "fred", "api_key") }), "fred");
    let contact = read_provider_settings(econf, "edgar")
        .and_then(|v| v.get("contact").and_then(|f| f.as_str()).map(str::to_owned))
        .unwrap_or_default();
    push(&mut out, meridian_provider_edgar::EdgarProvider::new(meridian_provider_edgar::EdgarConfig { contact }), "edgar");
    push(
        &mut out,
        meridian_provider_finnhub::FinnhubProvider::new(meridian_provider_finnhub::FinnhubConfig { api_key: secret(secrets, "finnhub", "api_key") }),
        "finnhub",
    );
    push(&mut out, meridian_provider_rss::RssProvider::new(meridian_provider_rss::RssConfig::default()), "rss");
    out
}

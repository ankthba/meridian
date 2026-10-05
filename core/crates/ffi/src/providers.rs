//! Composition root: builds the provider set for the data mode. MOCK mode
//! registers only the mock provider; LIVE mode registers only real
//! providers. They never mix.

use std::sync::Arc;

use meridian_engine::{AiService, DataMode, EngineConfig};
use meridian_provider::Provider;
use meridian_types::{Clock, FixedClock, SystemClock};

use crate::core::{CoreConfigFfi, SecretSource};
use crate::error::CoreResult;

pub(crate) struct Built {
    pub providers: Vec<Arc<dyn Provider>>,
    pub ai: Option<Arc<dyn AiService>>,
}

pub(crate) fn build(config: &CoreConfigFfi, econf: &EngineConfig, secrets: &dyn SecretSource) -> CoreResult<Built> {
    let clock: Arc<dyn Clock> = match econf.fixed_clock {
        Some(t) => Arc::new(FixedClock(t)),
        None => Arc::new(SystemClock),
    };
    let providers: Vec<Arc<dyn Provider>> = match econf.mode {
        DataMode::Mock => mock(config, clock),
        DataMode::Live => live(econf, secrets),
    };
    Ok(Built { providers, ai: None })
}

fn live(_econf: &EngineConfig, _secrets: &dyn SecretSource) -> Vec<Arc<dyn Provider>> {
    Vec::new()
}

// MOCK-WIRING: replaced when the mock provider crate lands.
fn mock(_config: &CoreConfigFfi, _clock: Arc<dyn Clock>) -> Vec<Arc<dyn Provider>> {
    Vec::new()
}

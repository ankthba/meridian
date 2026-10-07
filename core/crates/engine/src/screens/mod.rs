//! Function screens. Each module builds a [`Screen`] for one or more
//! mnemonics; [`Engine::screen`] dispatches.

mod alerts;
mod analysis;
pub mod chart;
mod common;
mod company;
mod compare;
mod des;
mod dividends;
mod filings;
mod help;
mod hp;
mod macro_eco;
mod monitors;
mod news;
mod options;
mod secf;

use std::sync::Arc;

use meridian_types::SecurityKey;
use serde::{Deserialize, Serialize};

pub use common::*;

use crate::core::Engine;
use crate::screen::Screen;

/// Request for a function screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenRequest {
    pub function: String,
    pub security: Option<SecurityKey>,
    pub args: Vec<(String, String)>,
}

impl ScreenRequest {
    #[must_use]
    pub fn new(function: &str, security: Option<SecurityKey>) -> Self {
        Self { function: function.to_ascii_uppercase(), security, args: Vec::new() }
    }

    #[must_use]
    pub fn arg(&self, k: &str) -> Option<&str> {
        self.args.iter().rev().find(|(key, _)| key.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str())
    }

    #[must_use]
    pub fn with_arg(mut self, k: &str, v: &str) -> Self {
        self.args.push((k.into(), v.into()));
        self
    }
}

/// Signature every screen builder shares.
pub(crate) type Builder = fn(Arc<Engine>, ScreenRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = Screen> + Send>>;

macro_rules! builder {
    ($f:path) => {{
        fn b(e: Arc<Engine>, r: ScreenRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = Screen> + Send>> {
            Box::pin(async move { $f(e, r).await })
        }
        b as Builder
    }};
}

pub(crate) fn builder_for(function: &str) -> Option<Builder> {
    Some(match function {
        "DES" => builder!(des::des),
        "HP" => builder!(hp::hp),
        "N" | "TOP" | "CN" => builder!(news::news),
        "CF" => builder!(filings::cf),
        "FA" => builder!(company::fa),
        "EE" => builder!(company::ee),
        "ERN" => builder!(company::ern),
        "ANR" => builder!(company::anr),
        "HDS" => builder!(company::hds),
        "DVD" => builder!(dividends::dvd),
        "ECO" => builder!(macro_eco::eco),
        "W" => builder!(monitors::worksheet),
        "WEI" => builder!(monitors::wei),
        "CRYP" => builder!(monitors::cryp),
        "FXC" => builder!(monitors::fxc),
        "MOST" => builder!(monitors::most),
        "ALRT" => builder!(alerts::alrt),
        "SECF" => builder!(secf::secf),
        "HELP" => builder!(help::help),
        "MENU" => builder!(secf::security_menu),
        _ => return extra_builder_for(function),
    })
}

impl Engine {
    /// Builds the screen for `req`. Never fails: errors and missing data
    /// become NOT AVAILABLE / error screens.
    pub async fn screen(self: &Arc<Self>, req: ScreenRequest) -> Screen {
        let function = req.function.to_ascii_uppercase();
        let security = req.security.as_ref().map(ToString::to_string);
        let Some(build) = builder_for(&function) else {
            return Screen::not_available(&function, &function, security, "this function is not implemented in Meridian");
        };
        let args = req.args.clone();
        let mut screen = build(self.clone(), req).await;
        screen.args = args;
        let mode_badge_needed = screen.sources.is_empty() && self.mode() == crate::config::DataMode::Mock;
        if mode_badge_needed {
            screen.sources.push(crate::screen::SourceBadge {
                provider: "mock".into(),
                delay: "MOCK".into(),
                source: "MOCK".into(),
                synthetic: true,
                as_of: self.now(),
                attribution: None,
            });
        }
        screen
    }
}

/// Hook run after the instrument universe loads (autocomplete index).
pub(crate) fn suggest_hook(engine: &Engine) {
    secf::rebuild(engine);
}

/// Screens backed by the analytics crate (charts, options, analytics).
pub(crate) fn analytics_builder_for(function: &str) -> Option<Builder> {
    Some(match function {
        "GP" | "GIP" => builder!(chart::gp),
        "OMON" => builder!(options::omon),
        "OVDV" => builder!(options::ovdv),
        "OVME" => builder!(options::ovme),
        "EQS" => builder!(analysis::eqs),
        "RV" => builder!(analysis::rv),
        "CORR" => builder!(analysis::corr),
        "PORT" => builder!(analysis::port),
        "BTST" => builder!(analysis::btst),
        "COMPARE" => builder!(compare::compare),
        _ => return None,
    })
}

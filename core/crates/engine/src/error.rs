use meridian_provider::{Capability, ProviderError};
use meridian_store::StoreError;
use thiserror::Error;

pub type EngineResult<T> = Result<T, EngineError>;

#[derive(Debug, Clone, Error, PartialEq)]
pub enum EngineError {
    #[error("{what} is not available: {reason}")]
    NotAvailable { what: String, reason: String },
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("cancelled")]
    Cancelled,
    #[error("internal error: {0}")]
    Internal(String),
}

impl EngineError {
    /// Turns "no provider offers this" into the user-facing NOT AVAILABLE
    /// reason, leaving other errors as they are.
    #[must_use]
    pub fn not_available(capability: Capability, mode_hint: &str) -> Self {
        EngineError::NotAvailable { what: capability.label().into(), reason: mode_hint.into() }
    }

    /// User-facing explanation for a NOT AVAILABLE / error screen.
    #[must_use]
    pub fn user_message(&self) -> String {
        match self {
            EngineError::NotAvailable { what, reason } => format!("{} not available — {reason}", capitalize(what)),
            EngineError::Provider(ProviderError::Unsupported { capability }) => {
                format!("{} not available — no configured data source provides it", capitalize(capability.label()))
            }
            EngineError::Provider(ProviderError::NotEntitled { capability, plan }) => {
                format!("{} requires the {plan} plan", capitalize(capability.label()))
            }
            EngineError::Provider(ProviderError::Unauthorized(m)) => m.clone(),
            EngineError::Provider(ProviderError::RateLimited { .. }) => "Rate limited by the data provider; try again shortly".into(),
            EngineError::Provider(ProviderError::NotFound(m)) | EngineError::NotFound(m) => format!("Not found: {m}"),
            EngineError::Provider(ProviderError::Network(m)) => format!("Network error: {m}"),
            other => other.to_string(),
        }
    }

    /// Whether this should render as NOT AVAILABLE rather than an error.
    #[must_use]
    pub fn is_unavailable(&self) -> bool {
        matches!(
            self,
            EngineError::NotAvailable { .. }
                | EngineError::Provider(
                    ProviderError::Unsupported { .. } | ProviderError::NotEntitled { .. } | ProviderError::Unauthorized(_)
                )
        )
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

use std::time::Duration;

use thiserror::Error;

use crate::capability::Capability;

pub type ProviderResult<T> = Result<T, ProviderError>;

#[derive(Debug, Clone, Error, PartialEq)]
pub enum ProviderError {
    /// The provider doesn't offer this at all.
    #[error("{capability:?} is not offered by this provider")]
    Unsupported { capability: Capability },
    /// Offered, but not on the configured plan.
    #[error("{capability:?} requires plan {plan}")]
    NotEntitled { capability: Capability, plan: String },
    #[error("rate limited")]
    RateLimited { retry_after: Option<Duration> },
    /// Missing or rejected credentials, or required configuration (e.g. the
    /// SEC User-Agent contact) not set.
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("HTTP {status}: {body_snippet}")]
    Http { status: u16, body_snippet: String },
    /// Vendor payload didn't match the documented format. A bug or a vendor
    /// change, never a user error.
    #[error("could not parse {context}")]
    Parse { context: String },
    #[error("upstream error: {0}")]
    Upstream(String),
    #[error("cancelled")]
    Cancelled,
}

impl ProviderError {
    /// Whether retrying the same request later could succeed.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            ProviderError::RateLimited { .. } | ProviderError::Network(_) => true,
            ProviderError::Http { status, .. } => *status >= 500 || *status == 429 || *status == 408,
            _ => false,
        }
    }

    #[must_use]
    pub fn parse(context: impl Into<String>) -> Self {
        ProviderError::Parse { context: context.into() }
    }

    /// Maps an HTTP status to the closest error kind.
    #[must_use]
    pub fn from_status(status: u16, body: &str, retry_after: Option<Duration>) -> Self {
        let snippet: String = body.chars().take(300).collect();
        match status {
            401 | 403 => ProviderError::Unauthorized(format!("HTTP {status}: {snippet}")),
            404 => ProviderError::NotFound(snippet),
            429 => ProviderError::RateLimited { retry_after },
            _ => ProviderError::Http { status, body_snippet: snippet },
        }
    }
}

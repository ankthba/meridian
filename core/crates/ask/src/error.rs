use std::time::Duration;

use thiserror::Error;

/// Everything that can stop an ASK question from producing a verified answer.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AskError {
    /// No Anthropic API key is configured (or it is empty).
    #[error("no Anthropic API key is configured")]
    MissingApiKey,

    /// Non-retryable HTTP failure from the Messages API, or a retryable one
    /// that exhausted its retries.
    #[error("Messages API returned HTTP {status}: {message}")]
    Http { status: u16, message: String },

    /// HTTP 429 after retries. `retry_after` comes from the `retry-after`
    /// header when present.
    #[error("rate limited by the Messages API")]
    RateLimited { retry_after: Option<Duration> },

    /// HTTP 529 / `overloaded_error` after retries.
    #[error("the Messages API is overloaded")]
    Overloaded,

    /// Transport failure, malformed SSE, an `error` event mid-stream, or a
    /// stream that ended without `message_stop`.
    #[error("stream error: {0}")]
    Stream(String),

    /// The model (or a safety classifier) declined. `category` is
    /// `stop_details.category` when the API supplied one.
    #[error("the model declined to answer (category: {})", category.as_deref().unwrap_or("unspecified"))]
    Refusal { category: Option<String> },

    /// The caller cancelled the question.
    #[error("cancelled")]
    Cancelled,

    /// The response hit `max_tokens` (or the context window) before it was
    /// complete. Tools from a truncated turn are never run.
    #[error("the response was truncated before it finished")]
    Truncated,

    /// The tool loop ran for the configured maximum number of requests
    /// without reaching a final answer.
    #[error("the tool loop hit its iteration limit")]
    IterationLimit,
}

impl AskError {
    /// Short machine-readable code for logs and the audit transcript.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            AskError::MissingApiKey => "missing_api_key",
            AskError::Http { .. } => "http",
            AskError::RateLimited { .. } => "rate_limited",
            AskError::Overloaded => "overloaded",
            AskError::Stream(_) => "stream",
            AskError::Refusal { .. } => "refusal",
            AskError::Cancelled => "cancelled",
            AskError::Truncated => "truncated",
            AskError::IterationLimit => "iteration_limit",
        }
    }
}

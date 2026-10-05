//! Raw-HTTP client for `POST /v1/messages` with SSE streaming.
//!
//! There is no official Rust SDK, so this follows the documented wire
//! format directly: headers `x-api-key`, `anthropic-version: 2023-06-01`,
//! `content-type: application/json`, and `anthropic-beta` when a beta
//! feature is in use.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;

use crate::cancel::CancelFlag;
use crate::error::AskError;
use crate::message::{AssistantMessage, EventError, MessageAccumulator, StreamUpdate};
use crate::request::ANTHROPIC_VERSION;
use crate::sse::SseParser;
use crate::transport::{HttpRequest, HttpResponse, ReqwestTransport, Transport, TransportError};

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// Retries for failures that happen before any content has streamed:
/// HTTP 429 / 529 / 5xx, connection errors, and an `error` event that
/// arrives ahead of `message_start`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub base_delay: Duration,
    /// Upper bound on any single wait. A `retry-after` longer than this is
    /// returned to the caller instead of being slept on.
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self { max_retries: 2, base_delay: Duration::from_millis(500), max_delay: Duration::from_secs(20) }
    }
}

impl RetryPolicy {
    fn backoff(&self, attempt: u32) -> Duration {
        self.base_delay.saturating_mul(1u32 << attempt.min(16)).min(self.max_delay)
    }
}

pub struct AnthropicClient {
    http: Arc<dyn Transport>,
    api_key: SecretString,
    base_url: String,
    retry: RetryPolicy,
}

impl std::fmt::Debug for AnthropicClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicClient")
            .field("base_url", &self.base_url)
            .field("retry", &self.retry)
            .finish_non_exhaustive()
    }
}

/// A pre-content failure that may be retried, or a final one.
enum Failure {
    Retryable { err: AskError, wait: Option<Duration> },
    Fatal(AskError),
}

impl AnthropicClient {
    /// Client over the production `reqwest` transport.
    pub fn new(api_key: SecretString) -> Result<Self, AskError> {
        let http = ReqwestTransport::new().map_err(|e| AskError::Stream(format!("HTTP client setup failed: {e}")))?;
        Self::with_transport(api_key, Arc::new(http))
    }

    pub fn with_transport(api_key: SecretString, http: Arc<dyn Transport>) -> Result<Self, AskError> {
        if api_key.expose_secret().trim().is_empty() {
            return Err(AskError::MissingApiKey);
        }
        Ok(Self { http, api_key, base_url: DEFAULT_BASE_URL.to_owned(), retry: RetryPolicy::default() })
    }

    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        base_url.into().trim_end_matches('/').clone_into(&mut self.base_url);
        self
    }

    #[must_use]
    pub fn with_retry_policy(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    fn request(&self, body: &[u8], betas: &[&str]) -> HttpRequest {
        let mut headers = vec![
            ("anthropic-version".to_owned(), ANTHROPIC_VERSION.to_owned()),
            ("content-type".to_owned(), "application/json".to_owned()),
            ("accept".to_owned(), "text/event-stream".to_owned()),
        ];
        if !betas.is_empty() {
            headers.push(("anthropic-beta".to_owned(), betas.join(",")));
        }
        HttpRequest {
            url: format!("{}/v1/messages", self.base_url),
            headers,
            api_key: self.api_key.clone(),
            body: body.to_vec(),
        }
    }

    /// Sends a streaming request and assembles the full assistant message,
    /// reporting deltas to `sink` as they arrive.
    pub async fn stream_message(
        &self,
        body: &Value,
        betas: &[&str],
        cancel: &CancelFlag,
        sink: &mut (dyn FnMut(StreamUpdate<'_>) + Send),
    ) -> Result<AssistantMessage, AskError> {
        let bytes = serde_json::to_vec(body).map_err(|e| AskError::Stream(format!("request body: {e}")))?;
        let mut attempt = 0u32;
        loop {
            if cancel.is_cancelled() {
                return Err(AskError::Cancelled);
            }
            let outcome = tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(AskError::Cancelled),
                r = self.http.post(self.request(&bytes, betas)) => r,
            };
            let failure = match outcome {
                Err(e @ (TransportError::Connect(_) | TransportError::Timeout(_))) => {
                    Failure::Retryable { err: AskError::Stream(e.to_string()), wait: None }
                }
                Err(e) => Failure::Fatal(AskError::Stream(e.to_string())),
                Ok(resp) if (200..300).contains(&resp.status) => match self.consume(resp, cancel, sink).await {
                    Ok(msg) => return Ok(msg),
                    Err(f) => f,
                },
                Ok(resp) => status_failure(resp, cancel).await?,
            };
            match failure {
                Failure::Fatal(err) => return Err(err),
                Failure::Retryable { err, wait } => {
                    if attempt >= self.retry.max_retries {
                        return Err(err);
                    }
                    let delay = match wait {
                        Some(w) if w > self.retry.max_delay => return Err(err),
                        Some(w) => w,
                        None => self.retry.backoff(attempt),
                    };
                    tracing::warn!(error = %err, attempt = attempt + 1, ?delay, "retrying Messages API request");
                    attempt += 1;
                    tokio::select! {
                        biased;
                        () = cancel.cancelled() => return Err(AskError::Cancelled),
                        () = tokio::time::sleep(delay) => {}
                    }
                }
            }
        }
    }

    async fn consume(
        &self,
        resp: HttpResponse,
        cancel: &CancelFlag,
        sink: &mut (dyn FnMut(StreamUpdate<'_>) + Send),
    ) -> Result<AssistantMessage, Failure> {
        let mut body = resp.body;
        let mut parser = SseParser::new();
        let mut acc = MessageAccumulator::new();
        while !acc.stopped() {
            let chunk = tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(Failure::Fatal(AskError::Cancelled)),
                c = body.next() => c,
            };
            let bytes = match chunk {
                None => break,
                Some(Ok(b)) => b,
                Some(Err(e)) => {
                    let err = AskError::Stream(e.to_string());
                    return Err(if acc.started() { Failure::Fatal(err) } else { Failure::Retryable { err, wait: None } });
                }
            };
            for ev in parser.push(&bytes) {
                if let Err(e) = acc.apply(&ev, sink) {
                    return Err(event_failure(e, acc.started()));
                }
                if acc.stopped() {
                    break;
                }
            }
        }
        parser.finish();
        acc.finish().map_err(|e| event_failure(e, true))
    }
}

fn event_failure(e: EventError, started: bool) -> Failure {
    match e {
        EventError::Malformed(m) => Failure::Fatal(AskError::Stream(m)),
        EventError::Api { kind, message } => {
            let err = match kind.as_str() {
                "overloaded_error" => AskError::Overloaded,
                "rate_limit_error" => AskError::RateLimited { retry_after: None },
                _ => AskError::Stream(format!("{kind}: {message}")),
            };
            let retryable = matches!(kind.as_str(), "overloaded_error" | "rate_limit_error" | "api_error");
            // Once content has streamed to observers a retry would duplicate
            // it, so only pre-content errors are retried.
            if retryable && !started { Failure::Retryable { err, wait: None } } else { Failure::Fatal(err) }
        }
    }
}

fn parse_retry_after(v: &str) -> Option<Duration> {
    v.trim().parse::<f64>().ok().filter(|s| s.is_finite() && *s >= 0.0).map(Duration::from_secs_f64)
}

async fn status_failure(resp: HttpResponse, cancel: &CancelFlag) -> Result<Failure, AskError> {
    let status = resp.status;
    let retry_after = resp.header("retry-after").and_then(parse_retry_after);
    let request_id = resp.header("request-id").map(str::to_owned);
    let body = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(AskError::Cancelled),
        b = resp.collect_body() => b.unwrap_or_default(),
    };
    let message = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|v| {
            let e = v.get("error")?;
            let kind = e.get("type").and_then(Value::as_str).unwrap_or("error");
            let msg = e.get("message").and_then(Value::as_str).unwrap_or("");
            Some(format!("{kind}: {msg}"))
        })
        .unwrap_or_else(|| String::from_utf8_lossy(&body).chars().take(500).collect());
    tracing::warn!(status, request_id = request_id.as_deref().unwrap_or(""), %message, "Messages API error");
    let err = match status {
        429 => AskError::RateLimited { retry_after },
        529 => AskError::Overloaded,
        _ => AskError::Http { status, message },
    };
    Ok(if status == 429 || status >= 500 {
        Failure::Retryable { err, wait: retry_after }
    } else {
        Failure::Fatal(err)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_parsing() {
        assert_eq!(parse_retry_after("3"), Some(Duration::from_secs(3)));
        assert_eq!(parse_retry_after(" 0.5 "), Some(Duration::from_millis(500)));
        assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(parse_retry_after("-1"), None);
    }

    #[test]
    fn backoff_grows_and_caps() {
        let p = RetryPolicy::default();
        assert_eq!(p.backoff(0), Duration::from_millis(500));
        assert_eq!(p.backoff(1), Duration::from_secs(1));
        assert_eq!(p.backoff(10), Duration::from_secs(20));
    }

    #[test]
    fn empty_key_is_rejected() {
        struct Never;
        #[async_trait::async_trait]
        impl Transport for Never {
            async fn post(&self, _: HttpRequest) -> Result<HttpResponse, TransportError> {
                Err(TransportError::Other("unused".into()))
            }
        }
        let err = AnthropicClient::with_transport(SecretString::from("  "), Arc::new(Never)).unwrap_err();
        assert_eq!(err, AskError::MissingApiKey);
    }
}

//! HTTP abstraction so the client can be driven by canned SSE in tests.

use std::fmt;
use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use secrecy::{ExposeSecret, SecretString};

/// Response body as a stream of byte chunks.
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, TransportError>> + Send>>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// Could not connect (DNS, TCP, TLS). Safe to retry.
    #[error("connection failed: {0}")]
    Connect(String),
    #[error("timed out: {0}")]
    Timeout(String),
    #[error("{0}")]
    Other(String),
}

/// A POST to the Messages API. The API key travels separately so it is
/// never part of a loggable header list.
pub struct HttpRequest {
    pub url: String,
    /// Non-secret headers (`anthropic-version`, `anthropic-beta`, ...).
    pub headers: Vec<(String, String)>,
    /// Sent as `x-api-key`.
    pub api_key: SecretString,
    pub body: Vec<u8>,
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpRequest")
            .field("url", &self.url)
            .field("headers", &self.headers)
            .field("api_key", &"[REDACTED]")
            .field("body_len", &self.body.len())
            .finish()
    }
}

impl HttpRequest {
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: ByteStream,
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .finish_non_exhaustive()
    }
}

impl HttpResponse {
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// Reads the whole body (for error responses).
    pub async fn collect_body(self) -> Result<Vec<u8>, TransportError> {
        let mut out = Vec::new();
        let mut body = self.body;
        while let Some(chunk) = body.next().await {
            out.extend_from_slice(&chunk?);
        }
        Ok(out)
    }
}

#[async_trait]
pub trait Transport: Send + Sync {
    /// Sends the request and returns once response headers arrive; the body
    /// streams afterwards.
    async fn post(&self, request: HttpRequest) -> Result<HttpResponse, TransportError>;
}

/// Production transport over `reqwest` (rustls).
#[derive(Debug, Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> Result<Self, TransportError> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            // The API sends `ping` events while the model thinks, so a long
            // silence means the stream is dead.
            .read_timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| TransportError::Other(e.to_string()))?;
        Ok(Self { client })
    }
}

fn map_reqwest(e: &reqwest::Error) -> TransportError {
    if e.is_connect() {
        TransportError::Connect(e.to_string())
    } else if e.is_timeout() {
        TransportError::Timeout(e.to_string())
    } else {
        TransportError::Other(e.to_string())
    }
}

#[async_trait]
impl Transport for ReqwestTransport {
    async fn post(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        let mut headers = HeaderMap::new();
        for (k, v) in &request.headers {
            let name = HeaderName::from_bytes(k.as_bytes()).map_err(|e| TransportError::Other(e.to_string()))?;
            let value = HeaderValue::from_str(v).map_err(|e| TransportError::Other(e.to_string()))?;
            headers.insert(name, value);
        }
        let mut key = HeaderValue::from_str(request.api_key.expose_secret())
            .map_err(|_| TransportError::Other("API key contains characters not allowed in a header".into()))?;
        key.set_sensitive(true);
        headers.insert(HeaderName::from_static("x-api-key"), key);

        let resp = self
            .client
            .post(&request.url)
            .headers(headers)
            .body(request.body)
            .send()
            .await
            .map_err(|e| map_reqwest(&e))?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_owned(), v.to_owned())))
            .collect();
        let body = resp.bytes_stream().map(|r| r.map_err(|e| map_reqwest(&e)));
        Ok(HttpResponse { status, headers, body: Box::pin(body) })
    }
}

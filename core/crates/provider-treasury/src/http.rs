//! Small HTTP helpers. Duplicated per provider crate on purpose (no shared
//! helper crate); keep the copies in sync by hand.

use std::time::Duration;

use meridian_provider::ProviderError;
use reqwest::header::{HeaderMap, RETRY_AFTER};

pub(crate) const USER_AGENT: &str = "Meridian/0.1 (personal use)";
pub(crate) const TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The provider's one shared client.
pub(crate) fn client() -> Result<reqwest::Client, ProviderError> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|e| ProviderError::Network(format!("could not build HTTP client: {}", e.without_url())))
}

/// Converts a transport error. The URL is stripped so query-string secrets
/// can never reach an error message or log line.
pub(crate) fn network_error(e: reqwest::Error) -> ProviderError {
    let e = e.without_url();
    if e.is_timeout() {
        ProviderError::Network(format!("request timed out: {e}"))
    } else {
        ProviderError::Network(e.to_string())
    }
}

/// Parses `Retry-After` as delay-seconds or an HTTP date.
pub(crate) fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let v = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(secs) = v.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let when = chrono::DateTime::parse_from_rfc2822(v).ok()?;
    let delta = when.with_timezone(&chrono::Utc) - chrono::Utc::now();
    Some(delta.to_std().unwrap_or(Duration::ZERO))
}

/// Sends a request and returns the body text, mapping non-2xx statuses with
/// [`ProviderError::from_status`].
pub(crate) async fn send_text(req: reqwest::RequestBuilder) -> Result<String, ProviderError> {
    let resp = req.send().await.map_err(network_error)?;
    let status = resp.status();
    let wait = retry_after(resp.headers());
    let body = resp.text().await.map_err(network_error)?;
    if !status.is_success() {
        return Err(ProviderError::from_status(status.as_u16(), &body, wait));
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use reqwest::header::HeaderValue;

    use super::*;

    #[test]
    fn retry_after_seconds() {
        let mut h = HeaderMap::new();
        h.insert(RETRY_AFTER, HeaderValue::from_static("7"));
        assert_eq!(retry_after(&h), Some(Duration::from_secs(7)));
    }

    #[test]
    fn retry_after_past_date_is_zero() {
        let mut h = HeaderMap::new();
        h.insert(RETRY_AFTER, HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT"));
        assert_eq!(retry_after(&h), Some(Duration::ZERO));
    }

    #[test]
    fn retry_after_missing() {
        assert_eq!(retry_after(&HeaderMap::new()), None);
    }
}

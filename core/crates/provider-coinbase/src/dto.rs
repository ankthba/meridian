//! Private serde shapes of Coinbase Exchange payloads. Nothing outside this
//! crate sees them; `normalize` converts them into `meridian_types`.

use serde::Deserialize;

/// A JSON value Coinbase sends either as a decimal string (`"85515.62"`) or
/// as a number (candles).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub(crate) enum Num {
    Float(f64),
    Text(String),
}

impl Num {
    /// `None` when the value is not a finite number.
    pub(crate) fn to_f64(&self) -> Option<f64> {
        let v = match self {
            Num::Float(f) => *f,
            Num::Text(s) => s.trim().parse::<f64>().ok()?,
        };
        v.is_finite().then_some(v)
    }

    /// Empty strings mean "no value".
    pub(crate) fn is_blank(&self) -> bool {
        matches!(self, Num::Text(s) if s.trim().is_empty())
    }

    pub(crate) fn raw(&self) -> String {
        match self {
            Num::Float(f) => f.to_string(),
            Num::Text(s) => s.clone(),
        }
    }
}

/// `GET /products` element.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ProductDto {
    pub id: String,
    pub base_currency: String,
    pub quote_currency: String,
    #[serde(default)]
    pub quote_increment: Option<String>,
    /// `online`, `offline`, `internal`, or `delisted`.
    #[serde(default)]
    pub status: Option<String>,
}

/// `GET /currencies` element (only the fields we use).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CurrencyDto {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
}

/// `GET /products/{id}/ticker`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct TickerDto {
    #[serde(default)]
    pub ask: Option<Num>,
    #[serde(default)]
    pub bid: Option<Num>,
    /// 24 h volume, base currency.
    #[serde(default)]
    pub volume: Option<Num>,
    /// Last trade price.
    #[serde(default)]
    pub price: Option<Num>,
    /// Last trade size.
    #[serde(default)]
    pub size: Option<Num>,
    /// Last trade time, RFC 3339.
    #[serde(default)]
    pub time: Option<String>,
}

/// `GET /products/{id}/stats` (24 h figures).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct StatsDto {
    #[serde(default)]
    pub open: Option<Num>,
    #[serde(default)]
    pub high: Option<Num>,
    #[serde(default)]
    pub low: Option<Num>,
    #[serde(default)]
    pub volume: Option<Num>,
}

/// `GET /products/{id}/candles` row: `[time, low, high, open, close, volume]`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CandleRow(pub i64, pub Num, pub Num, pub Num, pub Num, pub Num);

/// WebSocket feed messages we act on, keyed by `type`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum WsMessage {
    Ticker(Box<WsTicker>),
    Heartbeat,
    Subscriptions,
    Error {
        #[serde(default)]
        message: Option<String>,
        #[serde(default)]
        reason: Option<String>,
    },
    #[serde(other)]
    Other,
}

/// `ticker` channel payload. All prices and sizes are decimal strings.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WsTicker {
    pub product_id: String,
    #[serde(default)]
    pub price: Option<Num>,
    #[serde(default)]
    pub open_24h: Option<Num>,
    #[serde(default)]
    pub volume_24h: Option<Num>,
    #[serde(default)]
    pub low_24h: Option<Num>,
    #[serde(default)]
    pub high_24h: Option<Num>,
    #[serde(default)]
    pub best_bid: Option<Num>,
    #[serde(default)]
    pub best_bid_size: Option<Num>,
    #[serde(default)]
    pub best_ask: Option<Num>,
    #[serde(default)]
    pub best_ask_size: Option<Num>,
    #[serde(default)]
    pub last_size: Option<Num>,
    #[serde(default)]
    pub time: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn num_accepts_strings_and_numbers() {
        let v: Vec<Num> = serde_json::from_str(r#"["1.5", 2.25, "", "abc"]"#).unwrap();
        assert_eq!(v[0].to_f64(), Some(1.5));
        assert_eq!(v[1].to_f64(), Some(2.25));
        assert!(v[2].is_blank());
        assert_eq!(v[3].to_f64(), None);
    }

    #[test]
    fn ws_message_tags() {
        let m: WsMessage =
            serde_json::from_str(include_str!("../tests/fixtures/ws_heartbeat.json")).unwrap();
        assert!(matches!(m, WsMessage::Heartbeat));
        let m: WsMessage =
            serde_json::from_str(include_str!("../tests/fixtures/ws_subscriptions.json")).unwrap();
        assert!(matches!(m, WsMessage::Subscriptions));
        let m: WsMessage = serde_json::from_str(r#"{"type":"status","products":[]}"#).unwrap();
        assert!(matches!(m, WsMessage::Other));
        let m: WsMessage =
            serde_json::from_str(include_str!("../tests/fixtures/ws_error.json")).unwrap();
        match m {
            WsMessage::Error { message, reason } => {
                assert_eq!(message.as_deref(), Some("Failed to subscribe"));
                assert_eq!(reason.as_deref(), Some("NOPE-USD is not a valid product"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}

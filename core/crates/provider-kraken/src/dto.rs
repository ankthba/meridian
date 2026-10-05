//! Private serde shapes of Kraken spot payloads. Nothing outside this crate
//! sees them; `normalize` converts them into `meridian_types`.

use serde::Deserialize;

/// A value Kraken sends either as a decimal string (REST) or a number (WS v2).
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

/// Every REST response: `{"error": [...], "result": ...}`. Errors arrive
/// with HTTP 200.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Envelope {
    #[serde(default)]
    pub error: Vec<String>,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
}

/// `AssetPairs?assetVersion=1` value; the map key is the display/WS v2
/// name (`BTC/USD`).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PairDto {
    /// REST pair name, e.g. `XBTUSD`.
    #[serde(default)]
    pub altname: Option<String>,
    /// Display asset code with `assetVersion=1` (`BTC`, not `XXBT`).
    pub base: String,
    pub quote: String,
    #[serde(default)]
    pub pair_decimals: Option<u32>,
    #[serde(default)]
    pub tick_size: Option<String>,
}

/// `Ticker` value. Arrays are `[today, last 24 hours]` unless noted.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct TickerDto {
    /// Ask `[price, whole lot volume, lot volume]`.
    #[serde(default)]
    pub a: Option<Vec<Num>>,
    /// Bid `[price, whole lot volume, lot volume]`.
    #[serde(default)]
    pub b: Option<Vec<Num>>,
    /// Last trade `[price, lot volume]`.
    #[serde(default)]
    pub c: Option<Vec<Num>>,
    /// Volume.
    #[serde(default)]
    pub v: Option<Vec<Num>>,
    /// Volume-weighted average price.
    #[serde(default)]
    pub p: Option<Vec<Num>>,
    /// Low.
    #[serde(default)]
    pub l: Option<Vec<Num>>,
    /// High.
    #[serde(default)]
    pub h: Option<Vec<Num>>,
}

/// `OHLC` row: `[time, open, high, low, close, vwap, volume, count]`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct OhlcRow(
    pub i64,
    pub Num,
    pub Num,
    pub Num,
    pub Num,
    pub Num,
    pub Num,
    pub serde_json::Value,
);

/// Any WS v2 frame. Channel messages carry `channel`; responses to requests
/// carry `method`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WsFrame {
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub success: Option<bool>,
    #[serde(default)]
    pub error: Option<String>,
}

/// `ticker` channel element. All figures are rolling 24 h.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WsTicker {
    pub symbol: String,
    #[serde(default)]
    pub bid: Option<Num>,
    #[serde(default)]
    pub bid_qty: Option<Num>,
    #[serde(default)]
    pub ask: Option<Num>,
    #[serde(default)]
    pub ask_qty: Option<Num>,
    #[serde(default)]
    pub last: Option<Num>,
    #[serde(default)]
    pub volume: Option<Num>,
    #[serde(default)]
    pub vwap: Option<Num>,
    #[serde(default)]
    pub low: Option<Num>,
    #[serde(default)]
    pub high: Option<Num>,
    /// 24 h price change in quote currency.
    #[serde(default)]
    pub change: Option<Num>,
    #[serde(default)]
    pub timestamp: Option<String>,
}

/// `status` channel element.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WsStatus {
    #[serde(default)]
    pub system: Option<String>,
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
    fn envelope_with_error() {
        let e: Envelope =
            serde_json::from_str(include_str!("../tests/fixtures/error_unknown_pair.json"))
                .unwrap();
        assert_eq!(e.error, vec!["EQuery:Unknown asset pair".to_owned()]);
        assert!(e.result.is_none());
    }

    #[test]
    fn ws_frames() {
        let f: WsFrame =
            serde_json::from_str(include_str!("../tests/fixtures/ws_heartbeat.json")).unwrap();
        assert_eq!(f.channel.as_deref(), Some("heartbeat"));
        let f: WsFrame =
            serde_json::from_str(include_str!("../tests/fixtures/ws_pong.json")).unwrap();
        assert_eq!(f.method.as_deref(), Some("pong"));
        let f: WsFrame =
            serde_json::from_str(include_str!("../tests/fixtures/ws_subscribe_error.json"))
                .unwrap();
        assert_eq!(f.success, Some(false));
        assert_eq!(
            f.error.as_deref(),
            Some("Currency pair not supported NOPE/USD")
        );
    }
}

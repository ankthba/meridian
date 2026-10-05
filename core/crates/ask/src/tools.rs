//! The tool interface ASK calls into. The engine crate supplies the actual
//! read-only tools (SQL over DuckDB views, quotes, fundamentals, `compute`,
//! ...); this crate only defines the contract and runs the loop.

use async_trait::async_trait;
use meridian_types::{Provenance, ProviderId, UnixNanos};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A client tool as declared to the model. `input_schema` must be a strict
/// JSON Schema object (`additionalProperties: false`, `required` listed),
/// because every tool is sent with `strict: true`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    /// Say when to call the tool, not just what it does.
    pub description: String,
    pub input_schema: Value,
}

/// Where a tool's data came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    pub provider: ProviderId,
    pub as_of: UnixNanos,
    /// URL, accession number, or vendor ID.
    pub source_ref: Option<String>,
    /// Mock data. The answer must say so.
    pub synthetic: bool,
}

impl From<&Provenance> for SourceRef {
    fn from(p: &Provenance) -> Self {
        Self { provider: p.provider.clone(), as_of: p.as_of, source_ref: p.source_ref.clone(), synthetic: p.synthetic }
    }
}

/// Audit record for one tool call: shown as a numbered SOURCE in the ASK
/// screen and persisted with the transcript.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolAudit {
    /// `tool_use` id. Set by the agent.
    pub tool_use_id: String,
    /// Tool name. Set by the agent.
    pub tool: String,
    /// The validated input as JSON (or the raw text when it was invalid).
    /// Set by the agent.
    pub input_json: String,
    /// SQL text actually executed, for `sql_query`-style tools.
    pub sql: Option<String>,
    pub sources: Vec<SourceRef>,
    pub rows: Option<u64>,
    /// Wall time of the call as measured by the agent.
    pub duration_ms: u64,
    /// Every numeric value in the result. The number verifier matches the
    /// answer against these. The agent fills it from `content` when the
    /// executor leaves it empty.
    pub numbers: Vec<f64>,
    /// The result went back to the model with `is_error: true`. Set by the
    /// agent.
    pub is_error: bool,
    /// FNV-1a 64-bit hash (hex) of the result text sent to the model. Set
    /// by the agent.
    pub result_hash: String,
}

/// What a tool returns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolOutcome {
    /// JSON text returned to the model.
    pub content: String,
    pub is_error: bool,
    pub audit: ToolAudit,
}

impl ToolOutcome {
    /// A successful result; the audit's `numbers` are extracted from `value`.
    #[must_use]
    pub fn json(value: &Value, mut audit: ToolAudit) -> Self {
        if audit.numbers.is_empty() {
            audit.numbers = extract_numbers_from_json(value);
        }
        Self { content: value.to_string(), is_error: false, audit }
    }

    /// An error result. The model sees `{"error": message}`.
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: serde_json::json!({ "error": message.into() }).to_string(),
            is_error: true,
            audit: ToolAudit::default(),
        }
    }
}

#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// The tool set. Read once when a session starts and frozen for its
    /// lifetime (the tools array is part of the cached prefix).
    fn definitions(&self) -> Vec<ToolDefinition>;

    /// Runs one call. `input` has been validated against the tool's schema.
    /// Failures should come back as `is_error: true` outcomes, not panics.
    async fn execute(&self, name: &str, input: Value) -> ToolOutcome;
}

/// Every numeric value in a JSON document: number values, strings that are
/// numbers (`"1,234.5"`, `"$12.30"`, `"4.5%"`), and numeric tokens inside
/// free text (headlines, notes), using the same tokenizer as the verifier.
/// Object keys are ignored. For a scaled or percent token both the displayed
/// mantissa and the normalized value are included.
#[must_use]
pub fn extract_numbers_from_json(value: &Value) -> Vec<f64> {
    let mut out = Vec::new();
    collect(value, &mut out);
    out
}

fn collect(v: &Value, out: &mut Vec<f64>) {
    match v {
        Value::Number(n) => {
            if let Some(f) = n.as_f64().filter(|f| f.is_finite()) {
                out.push(f);
            }
        }
        Value::String(s) => crate::verify::numbers_in_text(s, out),
        Value::Array(a) => a.iter().for_each(|x| collect(x, out)),
        Value::Object(m) => m.values().for_each(|x| collect(x, out)),
        Value::Bool(_) | Value::Null => {}
    }
}

/// FNV-1a 64-bit, hex. A cheap, stable fingerprint of a tool result for the
/// audit log (not a security hash).
#[must_use]
pub fn fnv1a_hex(data: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn extracts_numbers_and_numeric_strings() {
        let v = json!({
            "symbol": "AAPL",
            "price": 227.52,
            "volume": 51234567,
            "change_pct": "-1.25%",
            "mkt_cap": "$3.4T",
            "as_of": "2026-10-02",
            "rows": [{"d": "2026-10-01", "close": 230.1}],
            "headline": "Apple slips 1.3% as iPhone 17 demand cools",
            "flag": true,
            "none": null
        });
        let mut n = extract_numbers_from_json(&v);
        n.sort_by(f64::total_cmp);
        for want in [227.52, 51234567.0, -1.25, -0.0125, 3.4, 3.4e12, 230.1, 1.3, 0.013] {
            assert!(n.iter().any(|x| (x - want).abs() < 1e-9 * want.abs().max(1.0)), "missing {want} in {n:?}");
        }
        // Date parts never enter the pool.
        assert!(!n.contains(&2026.0) && !n.contains(&10.0) && !n.contains(&2.0), "{n:?}");
    }

    #[test]
    fn fnv_is_stable() {
        assert_eq!(fnv1a_hex(b""), "cbf29ce484222325");
        assert_eq!(fnv1a_hex(b"a"), "af63dc4c8601ec8c");
    }
}

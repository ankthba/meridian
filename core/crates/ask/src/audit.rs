//! Serializable record of an ASK session, persisted by the engine (SQLite)
//! and shown in the ASK screen as the answer plus numbered SOURCES.

use meridian_types::UnixNanos;
use serde::{Deserialize, Serialize};

use crate::message::Usage;
use crate::tools::ToolAudit;
use crate::verify::NumberCheck;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AskTranscript {
    pub session_id: String,
    pub turns: Vec<AskTurn>,
}

/// One question and everything that went into answering it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AskTurn {
    pub question: String,
    /// The answer text exactly as streamed to the observer (number-check
    /// spans index into it). Partial when `error` is set.
    pub answer: String,
    /// In call order; SOURCE [n] is `tool_calls[n - 1]`.
    pub tool_calls: Vec<ToolAudit>,
    pub number_checks: Vec<NumberCheck>,
    /// Summed over every request this question made.
    pub usage: Usage,
    /// Model that produced the final response (differs from
    /// `requested_model` when a refusal fallback served it).
    pub model: String,
    pub requested_model: String,
    pub served_by_fallback: bool,
    /// `stop_reason` of the last response, if one arrived.
    pub stop_reason: Option<String>,
    /// Set when the question failed; `AskError::code()` plus its message.
    pub error: Option<String>,
    pub started_at: UnixNanos,
    pub finished_at: UnixNanos,
}

impl AskTurn {
    #[must_use]
    pub fn unverified_count(&self) -> usize {
        self.number_checks.iter().filter(|c| !c.verified).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let t = AskTranscript {
            session_id: "s1".into(),
            turns: vec![AskTurn {
                question: "q".into(),
                answer: "a 1.5%".into(),
                tool_calls: vec![ToolAudit { tool_use_id: "toolu_1".into(), numbers: vec![0.015], ..ToolAudit::default() }],
                number_checks: vec![NumberCheck {
                    text: "1.5%".into(),
                    value: 0.015,
                    span: (2, 6),
                    verified: true,
                    matched_tool_call: Some("toolu_1".into()),
                }],
                usage: Usage { input_tokens: 10, output_tokens: 5, cache_creation_input_tokens: 0, cache_read_input_tokens: 900, requests: 2 },
                model: "claude-opus-5-5".into(),
                requested_model: "claude-opus-5-5".into(),
                served_by_fallback: false,
                stop_reason: Some("end_turn".into()),
                error: None,
                started_at: 1,
                finished_at: 2,
            }],
        };
        let s = serde_json::to_string(&t).unwrap();
        let back: AskTranscript = serde_json::from_str(&s).unwrap();
        assert_eq!(back, t);
        assert_eq!(back.turns[0].unverified_count(), 0);
    }
}

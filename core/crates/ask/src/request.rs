//! Messages API request body and beta headers.
//!
//! Everything ahead of `messages` (model, tools, system) is byte-stable for
//! the life of a session so the prompt cache and preserved thinking hold.
//! Volatile context (date, panel security) goes in user turns, never here.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::message::Message;
use crate::tools::ToolDefinition;

pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
pub const DEFAULT_MAX_TOKENS: u32 = 64_000;
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Beta for `fallbacks: "default"` (category-routed server-side fallback).
pub const BETA_FALLBACK_DEFAULT: &str = "server-side-fallback-2026-07-01";
/// Beta for the array form `fallbacks: [{"model": ...}]`. The two headers
/// are not interchangeable: pairing either with the other form is a 400.
pub const BETA_FALLBACK_MODELS: &str = "server-side-fallback-2026-06-01";
/// Beta for `thinking.display: "updates"` (progress notes between tool calls).
pub const BETA_THINKING_UPDATES: &str = "thinking-display-updates-2026-08-18";

/// `output_config.effort`. Claude Opus 5.5 defaults to `medium` when omitted,
/// so it is always sent explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    #[default]
    High,
    Xhigh,
    Max,
}

impl Effort {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::Xhigh => "xhigh",
            Effort::Max => "max",
        }
    }
}

/// Server-side refusal fallback. A safety-classifier decline is re-run on
/// another model inside the same request instead of ending the question.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FallbackMode {
    /// No fallback: a classifier decline ends the question with
    /// [`AskError::Refusal`](crate::AskError::Refusal).
    Off,
    /// `fallbacks: "default"` — Anthropic routes by refusal category.
    #[default]
    Default,
    /// `fallbacks: [{"model": ...}, ...]` — explicit targets (1–3, distinct,
    /// each in the requested model's `allowed_fallback_models`).
    Models(Vec<String>),
}

/// What the API returns in `thinking` blocks. Thinking itself is always on
/// (adaptive) for Claude Opus 5.5 and cannot be disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThinkingDisplay {
    /// Omit the `thinking` field: adaptive thinking, blocks come back with
    /// empty text (the model default).
    #[default]
    Omitted,
    /// `{"type":"adaptive","display":"summarized"}`.
    Summarized,
    /// `{"type":"adaptive","display":"updates"}` (beta): short progress
    /// notes between tool calls, reasoning stays hidden.
    Updates,
}

/// Per-session request settings. Changing any of these mid-session
/// invalidates the prompt cache, so they are fixed when a session starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestOptions {
    pub model: String,
    pub max_tokens: u32,
    pub effort: Effort,
    pub fallback: FallbackMode,
    pub thinking_display: ThinkingDisplay,
    /// Top-level automatic caching for the growing conversation tail, in
    /// addition to the explicit breakpoint on the system prompt.
    pub cache_conversation: bool,
}

impl Default for RequestOptions {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.to_owned(),
            max_tokens: DEFAULT_MAX_TOKENS,
            effort: Effort::High,
            fallback: FallbackMode::Default,
            thinking_display: ThinkingDisplay::Omitted,
            cache_conversation: true,
        }
    }
}

impl RequestOptions {
    /// `anthropic-beta` values this configuration needs.
    #[must_use]
    pub fn betas(&self) -> Vec<&'static str> {
        let mut b = Vec::new();
        match &self.fallback {
            FallbackMode::Off => {}
            FallbackMode::Default => b.push(BETA_FALLBACK_DEFAULT),
            FallbackMode::Models(models) if models.is_empty() => {}
            FallbackMode::Models(_) => b.push(BETA_FALLBACK_MODELS),
        }
        if self.thinking_display == ThinkingDisplay::Updates {
            b.push(BETA_THINKING_UPDATES);
        }
        b
    }
}

/// Serializes one tool definition. Every client tool is strict (the API
/// guarantees schema-valid arguments) and streams its input eagerly (so the
/// client validates inputs itself; see `agent.rs`).
#[must_use]
pub fn tool_json(t: &ToolDefinition) -> Value {
    json!({
        "name": t.name,
        "description": t.description,
        "input_schema": t.input_schema,
        "strict": true,
        "eager_input_streaming": true,
    })
}

/// Builds the streaming `POST /v1/messages` body.
#[must_use]
pub fn build_body(opts: &RequestOptions, system: &str, tools: &[ToolDefinition], messages: &[Message]) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), json!(opts.model));
    body.insert("max_tokens".into(), json!(opts.max_tokens));
    body.insert("stream".into(), json!(true));
    body.insert("output_config".into(), json!({ "effort": opts.effort.as_str() }));
    match opts.thinking_display {
        // Adaptive is the default and the only mode Claude Opus 5.5
        // accepts; never send `disabled` or `budget_tokens`.
        ThinkingDisplay::Omitted => {}
        ThinkingDisplay::Summarized => {
            body.insert("thinking".into(), json!({"type": "adaptive", "display": "summarized"}));
        }
        ThinkingDisplay::Updates => {
            body.insert("thinking".into(), json!({"type": "adaptive", "display": "updates"}));
        }
    }
    body.insert(
        "system".into(),
        json!([{ "type": "text", "text": system, "cache_control": {"type": "ephemeral"} }]),
    );
    if !tools.is_empty() {
        body.insert("tools".into(), Value::Array(tools.iter().map(tool_json).collect()));
        // Forced tool choice (`any`/`tool`) is a 400 on this model.
        body.insert("tool_choice".into(), json!({"type": "auto"}));
    }
    match &opts.fallback {
        FallbackMode::Off => {}
        FallbackMode::Default => {
            body.insert("fallbacks".into(), json!("default"));
        }
        FallbackMode::Models(models) if models.is_empty() => {}
        FallbackMode::Models(models) => {
            let entries: Vec<Value> = models.iter().map(|m| json!({ "model": m })).collect();
            body.insert("fallbacks".into(), Value::Array(entries));
        }
    }
    if opts.cache_conversation {
        body.insert("cache_control".into(), json!({"type": "ephemeral"}));
    }
    body.insert("messages".into(), serde_json::to_value(messages).unwrap_or(Value::Array(Vec::new())));
    Value::Object(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Message;

    fn tool() -> ToolDefinition {
        ToolDefinition {
            name: "get_quote".into(),
            description: "Latest quote for a security.".into(),
            input_schema: json!({"type":"object","properties":{"symbol":{"type":"string"}},
                "required":["symbol"],"additionalProperties":false}),
        }
    }

    #[test]
    fn default_body_shape() {
        let opts = RequestOptions::default();
        let msgs = vec![Message::user(vec![json!({"type":"text","text":"hi"})])];
        let body = build_body(&opts, "SYS", &[tool()], &msgs);
        assert_eq!(body["model"], "claude-opus-5-5");
        assert_eq!(body["max_tokens"], 64000);
        assert_eq!(body["stream"], true);
        assert_eq!(body["output_config"], json!({"effort":"high"}));
        assert!(body.get("thinking").is_none(), "thinking must be omitted by default");
        assert_eq!(body["system"][0]["cache_control"], json!({"type":"ephemeral"}));
        assert_eq!(body["tools"][0]["strict"], true);
        assert_eq!(body["tools"][0]["eager_input_streaming"], true);
        assert_eq!(body["tool_choice"], json!({"type":"auto"}));
        assert_eq!(body["fallbacks"], "default");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(opts.betas(), vec![BETA_FALLBACK_DEFAULT]);
    }

    #[test]
    fn fallback_forms_pick_matching_beta() {
        let mut opts = RequestOptions { fallback: FallbackMode::Models(vec!["claude-opus-5".into()]), ..Default::default() };
        let body = build_body(&opts, "S", &[], &[]);
        assert_eq!(body["fallbacks"], json!([{"model":"claude-opus-5"}]));
        assert_eq!(opts.betas(), vec![BETA_FALLBACK_MODELS]);
        assert!(body.get("tools").is_none() && body.get("tool_choice").is_none());

        opts.fallback = FallbackMode::Off;
        assert!(build_body(&opts, "S", &[], &[]).get("fallbacks").is_none());
        assert!(opts.betas().is_empty());
    }

    #[test]
    fn thinking_display_updates_adds_beta() {
        let opts = RequestOptions { thinking_display: ThinkingDisplay::Updates, ..Default::default() };
        let body = build_body(&opts, "S", &[], &[]);
        assert_eq!(body["thinking"], json!({"type":"adaptive","display":"updates"}));
        assert!(opts.betas().contains(&BETA_THINKING_UPDATES));
    }

    #[test]
    fn body_is_deterministic() {
        let opts = RequestOptions::default();
        let a = build_body(&opts, "S", &[tool()], &[]).to_string();
        let b = build_body(&opts, "S", &[tool()], &[]).to_string();
        assert_eq!(a, b);
    }
}

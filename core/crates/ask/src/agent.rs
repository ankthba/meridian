//! The manual tool-use loop.
//!
//! One [`AskSession`] is one conversation. Each [`AskSession::ask`] appends a
//! user turn and loops: stream a response, run every requested tool
//! concurrently, return all results in one user message, and repeat until
//! the model ends its turn. The history only ever grows; a failed question
//! is rolled back to the last completed one so the next request is still a
//! valid, unedited prefix.

use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Instant;

use chrono::NaiveDate;
use futures_util::FutureExt;
use futures_util::future::join_all;
use meridian_types::{Clock, SecurityKey, SystemClock};
use serde_json::{Value, json};

use crate::audit::{AskTranscript, AskTurn};
use crate::cancel::CancelFlag;
use crate::client::AnthropicClient;
use crate::error::AskError;
use crate::message::{AssistantMessage, InvalidToolInput, Message, StreamUpdate, Usage, block_type};
use crate::prompt::SYSTEM_PROMPT;
use crate::request::{RequestOptions, build_body};
use crate::schema;
use crate::tools::{ToolAudit, ToolDefinition, ToolExecutor, ToolOutcome, extract_numbers_from_json, fnv1a_hex};
use crate::verify::verify_numbers;

pub const DEFAULT_MAX_ITERATIONS: u32 = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskConfig {
    pub request: RequestOptions,
    /// Maximum Messages API requests per question.
    pub max_iterations: u32,
}

impl Default for AskConfig {
    fn default() -> Self {
        Self { request: RequestOptions::default(), max_iterations: DEFAULT_MAX_ITERATIONS }
    }
}

/// Volatile context for one question. Sent in the user turn, never in the
/// system prompt, so the cached prefix stays byte-stable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskContext {
    /// Security loaded in the panel ASK was invoked from.
    pub panel_security: Option<SecurityKey>,
    /// The user's current date.
    pub date: NaiveDate,
}

impl AskContext {
    fn render(&self) -> String {
        let security = self.panel_security.as_ref().map_or_else(|| "none".to_owned(), ToString::to_string);
        format!("<context>\ndate: {}\npanel_security: {security}\n</context>", self.date.format("%Y-%m-%d (%A)"))
    }
}

/// Result of a completed question.
#[derive(Debug, Clone, PartialEq)]
pub struct AskOutcome {
    pub turn: AskTurn,
    /// Messages API requests made.
    pub iterations: u32,
}

/// Receives streaming progress. Called from the task running
/// [`AskSession::ask`]; tool results may arrive from concurrent tool tasks.
pub trait AskObserver: Send + Sync {
    /// Answer text as it streams. The concatenation of every delta equals
    /// `AskTurn::answer`, which number-check spans index into. If the
    /// question then fails (refusal, truncation, ...), discard it.
    fn on_text_delta(&self, text: &str);

    /// Non-empty thinking text: progress notes under
    /// `ThinkingDisplay::Updates`, summaries under `Summarized`.
    fn on_thinking_update(&self, _text: &str) {}

    /// A tool call finished streaming. It runs only if the response stops
    /// with `tool_use`.
    fn on_tool_call(&self, id: &str, name: &str, input_json: &str);

    fn on_tool_result(&self, id: &str, audit: &ToolAudit, is_error: bool);

    fn on_done(&self, outcome: AskOutcome);

    fn on_error(&self, error: AskError);
}

/// Per-question accumulation.
#[derive(Debug, Default)]
struct Run {
    answer: String,
    tool_calls: Vec<ToolAudit>,
    usage: Usage,
    model: String,
    served_by_fallback: bool,
    stop_reason: Option<String>,
}

/// A `tool_use` block waiting to run.
struct PendingCall {
    id: String,
    name: String,
    input: Value,
}

pub struct AskSession {
    client: Arc<AnthropicClient>,
    executor: Arc<dyn ToolExecutor>,
    /// Frozen at session start, sorted by name: the tools array is part of
    /// the cached prefix that preserved thinking binds to.
    tools: Vec<ToolDefinition>,
    config: AskConfig,
    clock: Arc<dyn Clock>,
    history: Vec<Message>,
    transcript: AskTranscript,
}

impl AskSession {
    pub fn new(
        client: Arc<AnthropicClient>,
        executor: Arc<dyn ToolExecutor>,
        config: AskConfig,
        session_id: impl Into<String>,
    ) -> Self {
        let mut tools = executor.definitions();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        for t in &tools {
            if t.input_schema.get("additionalProperties") != Some(&Value::Bool(false)) {
                tracing::warn!(tool = %t.name, "tool schema lacks additionalProperties: false; strict mode may reject it");
            }
        }
        Self {
            client,
            executor,
            tools,
            config,
            clock: Arc::new(SystemClock),
            history: Vec::new(),
            transcript: AskTranscript { session_id: session_id.into(), turns: Vec::new() },
        }
    }

    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// The conversation as it will be sent on the next request.
    #[must_use]
    pub fn history(&self) -> &[Message] {
        &self.history
    }

    #[must_use]
    pub fn transcript(&self) -> &AskTranscript {
        &self.transcript
    }

    #[must_use]
    pub fn tools(&self) -> &[ToolDefinition] {
        &self.tools
    }

    #[must_use]
    pub fn config(&self) -> &AskConfig {
        &self.config
    }

    /// Answers one question. The outcome (or error) is also delivered to
    /// `observer.on_done` / `observer.on_error`, and recorded in the
    /// transcript either way.
    pub async fn ask(
        &mut self,
        question: &str,
        context: &AskContext,
        observer: Arc<dyn AskObserver>,
        cancel: CancelFlag,
    ) -> Result<AskOutcome, AskError> {
        let started_at = self.clock.now();
        let committed = self.history.len();
        let mut run = Run { model: self.config.request.model.clone(), ..Run::default() };
        let result = self.run_loop(question, context, &observer, &cancel, &mut run).await;
        let finished_at = self.clock.now();
        let (number_checks, error) = match &result {
            Ok(_) => (verify_numbers(&run.answer, &run.tool_calls), None),
            Err(e) => (Vec::new(), Some(format!("{}: {e}", e.code()))),
        };
        let turn = AskTurn {
            question: question.to_owned(),
            answer: run.answer,
            tool_calls: run.tool_calls,
            number_checks,
            usage: run.usage,
            model: run.model,
            requested_model: self.config.request.model.clone(),
            served_by_fallback: run.served_by_fallback,
            stop_reason: run.stop_reason,
            error,
            started_at,
            finished_at,
        };
        self.transcript.turns.push(turn.clone());
        match result {
            Ok(iterations) => {
                let outcome = AskOutcome { turn, iterations };
                observer.on_done(outcome.clone());
                Ok(outcome)
            }
            Err(e) => {
                // Drop the unfinished question so the history stays a valid
                // sequence of completed turns. Nothing before `committed`
                // is touched.
                self.history.truncate(committed);
                observer.on_error(e.clone());
                Err(e)
            }
        }
    }

    async fn run_loop(
        &mut self,
        question: &str,
        context: &AskContext,
        observer: &Arc<dyn AskObserver>,
        cancel: &CancelFlag,
        run: &mut Run,
    ) -> Result<u32, AskError> {
        self.history.push(Message::user(vec![
            json!({"type": "text", "text": context.render()}),
            json!({"type": "text", "text": question}),
        ]));
        let betas = self.config.request.betas();
        for iteration in 1..=self.config.max_iterations {
            if cancel.is_cancelled() {
                return Err(AskError::Cancelled);
            }
            let body = build_body(&self.config.request, SYSTEM_PROMPT, &self.tools, &self.history);
            let msg = {
                let had_text = !run.answer.is_empty();
                let mut separated = false;
                let answer = &mut run.answer;
                let obs = observer.as_ref();
                let mut sink = |u: StreamUpdate<'_>| match u {
                    StreamUpdate::TextDelta { text, .. } if !text.is_empty() => {
                        // Text from a later response in the same question
                        // starts a new paragraph.
                        if had_text && !separated {
                            separated = true;
                            answer.push_str("\n\n");
                            obs.on_text_delta("\n\n");
                        }
                        answer.push_str(text);
                        obs.on_text_delta(text);
                    }
                    StreamUpdate::ThinkingDelta { text, .. } if !text.is_empty() => obs.on_thinking_update(text),
                    StreamUpdate::BlockStop { block, invalid_input, .. } if block_type(block) == "tool_use" => {
                        let field = |k: &str| block.get(k).and_then(Value::as_str).unwrap_or("");
                        let input = invalid_input.map_or_else(|| block["input"].to_string(), |bad| bad.raw.clone());
                        obs.on_tool_call(field("id"), field("name"), &input);
                    }
                    _ => {}
                };
                self.client.stream_message(&body, &betas, cancel, &mut sink).await?
            };
            run.usage.add(&msg.usage);
            msg.served_model().clone_into(&mut run.model);
            run.served_by_fallback |= msg.served_by_fallback;
            run.stop_reason.clone_from(&msg.stop_reason);
            tracing::info!(
                iteration,
                model = %run.model,
                stop_reason = msg.stop_reason.as_deref().unwrap_or(""),
                input_tokens = msg.usage.input_tokens,
                output_tokens = msg.usage.output_tokens,
                cache_read_input_tokens = msg.usage.cache_read_input_tokens,
                cache_creation_input_tokens = msg.usage.cache_creation_input_tokens,
                "ASK response"
            );

            match msg.stop_reason.as_deref() {
                Some("end_turn" | "stop_sequence") => {
                    let content = msg.replay_content();
                    if !content.is_empty() {
                        self.history.push(Message::assistant(content));
                    }
                    return Ok(iteration);
                }
                Some("tool_use") => {
                    let content = msg.replay_content();
                    let calls = pending_calls(&content);
                    if calls.is_empty() {
                        return Err(AskError::Stream("stop_reason tool_use without a tool_use block".into()));
                    }
                    self.history.push(Message::assistant(content));
                    let results = self.run_tools(calls, &msg, observer, cancel, run).await?;
                    // All results for the round go back in ONE user message.
                    self.history.push(Message::user(results));
                }
                Some("pause_turn") => {
                    // Re-send with the paused turn appended; the API resumes.
                    let content = msg.replay_content();
                    if content.is_empty() {
                        return Err(AskError::Stream("pause_turn with no content to resume from".into()));
                    }
                    self.history.push(Message::assistant(content));
                }
                // A truncated tool_use input can still parse as a valid
                // partial object, so nothing from this turn is run.
                Some("max_tokens" | "model_context_window_exceeded") => return Err(AskError::Truncated),
                Some("refusal") => return Err(AskError::Refusal { category: msg.refusal_category() }),
                other => return Err(AskError::Stream(format!("unexpected stop_reason {other:?}"))),
            }
        }
        Err(AskError::IterationLimit)
    }

    /// Runs every call concurrently and returns the `tool_result` blocks in
    /// call order.
    async fn run_tools(
        &self,
        calls: Vec<PendingCall>,
        msg: &AssistantMessage,
        observer: &Arc<dyn AskObserver>,
        cancel: &CancelFlag,
        run: &mut Run,
    ) -> Result<Vec<Value>, AskError> {
        let first_source = run.tool_calls.len() + 1;
        let invalid = &msg.invalid_tool_inputs;
        let futures = calls
            .into_iter()
            .enumerate()
            .map(|(i, call)| self.run_one(call, first_source + i, invalid, observer.as_ref()));
        let done = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(AskError::Cancelled),
            done = join_all(futures) => done,
        };
        let mut blocks = Vec::with_capacity(done.len());
        for (block, audit) in done {
            blocks.push(block);
            run.tool_calls.push(audit);
        }
        Ok(blocks)
    }

    async fn run_one(
        &self,
        call: PendingCall,
        source: usize,
        invalid: &BTreeMap<String, InvalidToolInput>,
        observer: &dyn AskObserver,
    ) -> (Value, ToolAudit) {
        let started = Instant::now();
        let def = self.tools.iter().find(|t| t.name == call.name);
        let (outcome, input_json) = if let Some(bad) = invalid.get(&call.id) {
            (invalid_json(&bad.raw, &bad.error), bad.raw.clone())
        } else if let Some(def) = def {
            let input_json = call.input.to_string();
            if let Err(reason) = schema::validate(&def.input_schema, &call.input) {
                (invalid_json(&input_json, &reason), input_json)
            } else {
                let fut = self.executor.execute(&call.name, call.input);
                let outcome = AssertUnwindSafe(fut).catch_unwind().await.unwrap_or_else(|_| {
                    tracing::error!(tool = %call.name, "tool panicked");
                    ToolOutcome::error(format!("tool `{}` failed internally", call.name))
                });
                (outcome, input_json)
            }
        } else {
            let content = json!({ "UNKNOWN_TOOL": call.name }).to_string();
            (ToolOutcome { content, is_error: true, audit: ToolAudit::default() }, call.input.to_string())
        };

        let ToolOutcome { mut content, is_error, mut audit } = outcome;
        if content.is_empty() {
            "{}".clone_into(&mut content); // empty text blocks are rejected
        }
        audit.tool_use_id.clone_from(&call.id);
        audit.tool.clone_from(&call.name);
        audit.input_json = input_json;
        audit.is_error = is_error;
        audit.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        audit.result_hash = fnv1a_hex(content.as_bytes());
        if is_error {
            audit.numbers.clear();
        } else if audit.numbers.is_empty() {
            audit.numbers = match serde_json::from_str::<Value>(&content) {
                Ok(v) => extract_numbers_from_json(&v),
                Err(_) => extract_numbers_from_json(&Value::String(content.clone())),
            };
        }
        tracing::debug!(tool = %call.name, source, is_error, ms = audit.duration_ms, "tool call finished");
        observer.on_tool_result(&call.id, &audit, is_error);

        let mut block = json!({
            "type": "tool_result",
            "tool_use_id": call.id,
            "content": [
                {"type": "text", "text": format!("SOURCE [{source}]")},
                {"type": "text", "text": content},
            ],
        });
        if is_error {
            block["is_error"] = Value::Bool(true);
        }
        (block, audit)
    }
}

fn invalid_json(raw: &str, reason: &str) -> ToolOutcome {
    ToolOutcome {
        content: json!({ "INVALID_JSON": raw, "error": reason }).to_string(),
        is_error: true,
        audit: ToolAudit::default(),
    }
}

fn pending_calls(content: &[Value]) -> Vec<PendingCall> {
    content
        .iter()
        .filter(|b| block_type(b) == "tool_use")
        .map(|b| PendingCall {
            id: b.get("id").and_then(Value::as_str).unwrap_or_default().to_owned(),
            name: b.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
            input: b.get("input").cloned().unwrap_or_else(|| json!({})),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use meridian_types::MarketSector;

    use super::*;

    #[test]
    fn context_renders_date_and_security() {
        let ctx = AskContext {
            panel_security: Some(SecurityKey::new("aapl", Some("us"), MarketSector::Equity)),
            date: NaiveDate::from_ymd_opt(2026, 10, 5).unwrap(),
        };
        assert_eq!(ctx.render(), "<context>\ndate: 2026-10-05 (Monday)\npanel_security: AAPL US Equity\n</context>");
        let none = AskContext { panel_security: None, ..ctx };
        assert!(none.render().contains("panel_security: none"));
    }
}

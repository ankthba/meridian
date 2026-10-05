//! Messages API conversation types and the streaming accumulator.
//!
//! Content blocks are kept as raw JSON (`serde_json::Value`) so everything
//! the API sends back (thinking text and signatures, citations, fallback
//! markers, block types this crate doesn't know) is replayed unchanged on the
//! next request. The history is append-only: preserved thinking binds each
//! thinking block to the exact prefix that produced it, so rewriting an
//! earlier turn would invalidate every later block (and the prompt cache).

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::sse::SseEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// One entry of the request `messages` array.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<Value>,
}

impl Message {
    #[must_use]
    pub fn user(content: Vec<Value>) -> Self {
        Self { role: Role::User, content }
    }

    #[must_use]
    pub fn assistant(content: Vec<Value>) -> Self {
        Self { role: Role::Assistant, content }
    }
}

/// Token usage, summed over however many requests the caller adds up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    /// Number of Messages API requests these totals cover.
    pub requests: u32,
}

impl Usage {
    pub fn add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
        self.requests += other.requests;
    }

    /// Overwrites each field present in a `usage` object. The streaming API
    /// reports cumulative values, so later events replace earlier ones.
    fn merge_json(&mut self, usage: &Value) {
        let field = |name: &str| usage.get(name).and_then(Value::as_u64);
        if let Some(v) = field("input_tokens") {
            self.input_tokens = v;
        }
        if let Some(v) = field("output_tokens") {
            self.output_tokens = v;
        }
        if let Some(v) = field("cache_creation_input_tokens") {
            self.cache_creation_input_tokens = v;
        }
        if let Some(v) = field("cache_read_input_tokens") {
            self.cache_read_input_tokens = v;
        }
    }
}

/// A tool input the client could not accept: the streamed JSON failed to
/// parse, or parsed to something other than an object. (With
/// `eager_input_streaming` the API does not validate inputs, so the client
/// must.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidToolInput {
    /// Exactly what was streamed.
    pub raw: String,
    pub error: String,
}

/// A complete assistant response assembled from the stream.
#[derive(Debug, Clone, PartialEq)]
pub struct AssistantMessage {
    pub id: String,
    /// Model named in `message_start`.
    pub model: String,
    /// Every content block exactly as received, in index order.
    pub content: Vec<Value>,
    pub stop_reason: Option<String>,
    pub stop_details: Option<Value>,
    pub usage: Usage,
    /// Tool inputs that could not be accepted, keyed by `tool_use` id.
    /// Their blocks carry `"input": {}` so the turn can still be replayed.
    pub invalid_tool_inputs: BTreeMap<String, InvalidToolInput>,
    /// A server-side refusal fallback produced (part of) this response.
    pub served_by_fallback: bool,
}

pub(crate) fn block_type(block: &Value) -> &str {
    block.get("type").and_then(Value::as_str).unwrap_or("")
}

impl AssistantMessage {
    /// Concatenated text of every `text` block.
    #[must_use]
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter(|b| block_type(b) == "text")
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect()
    }

    /// The model that actually produced the response: the target of the last
    /// fallback switch, else the model from `message_start`.
    #[must_use]
    pub fn served_model(&self) -> &str {
        self.content
            .iter()
            .rev()
            .find(|b| block_type(b) == "fallback")
            .and_then(|b| b.pointer("/to/model"))
            .and_then(Value::as_str)
            .unwrap_or(&self.model)
    }

    #[must_use]
    pub fn refusal_category(&self) -> Option<String> {
        self.stop_details
            .as_ref()
            .and_then(|d| d.get("category"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    /// The content to append to the history for the next request.
    ///
    /// Unchanged, except after a mid-output server-side fallback: blocks the
    /// declined model produced before the final `fallback` marker that the
    /// fallback model cannot continue from (thinking, redacted thinking,
    /// client `tool_use`, unpaired server-tool blocks, unknown types) are
    /// omitted, as the refusal-fallback docs require. Text, paired server
    /// tool blocks, the markers, and everything after the boundary are kept.
    #[must_use]
    pub fn replay_content(&self) -> Vec<Value> {
        let Some(boundary) = self.content.iter().rposition(|b| block_type(b) == "fallback") else {
            return self.content.clone();
        };
        let before = &self.content[..boundary];
        let id_of = |b: &Value, key: &str| b.get(key).and_then(Value::as_str).map(str::to_owned);
        let server_use_ids: HashSet<String> =
            before.iter().filter(|b| block_type(b) == "server_tool_use").filter_map(|b| id_of(b, "id")).collect();
        let result_ids: HashSet<String> = before
            .iter()
            .filter(|b| block_type(b).ends_with("_tool_result"))
            .filter_map(|b| id_of(b, "tool_use_id"))
            .collect();
        let mut out: Vec<Value> = before
            .iter()
            .filter(|b| match block_type(b) {
                "text" | "fallback" => true,
                "server_tool_use" => id_of(b, "id").is_some_and(|id| result_ids.contains(&id)),
                t if t.ends_with("_tool_result") => {
                    id_of(b, "tool_use_id").is_some_and(|id| server_use_ids.contains(&id))
                }
                _ => false,
            })
            .cloned()
            .collect();
        out.extend_from_slice(&self.content[boundary..]);
        out
    }
}

/// Incremental notifications while a response streams.
#[derive(Debug, Clone, Copy)]
pub enum StreamUpdate<'a> {
    MessageStart { model: &'a str },
    TextDelta { index: usize, text: &'a str },
    ThinkingDelta { index: usize, text: &'a str },
    /// A block finished. For tool-use blocks `input` is final here; when
    /// the streamed input was unusable, `invalid_input` holds the raw text
    /// and `input` is `{}`.
    BlockStop { index: usize, block: &'a Value, invalid_input: Option<&'a InvalidToolInput> },
}

/// An `error` event or a stream that doesn't follow the documented shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EventError {
    /// `event: error` with the API's error type, e.g. `overloaded_error`.
    Api { kind: String, message: String },
    Malformed(String),
}

#[derive(Debug, Default)]
struct BlockState {
    value: Map<String, Value>,
    partial_json: String,
    has_json: bool,
    closed: bool,
}

/// Folds decoded SSE events into an [`AssistantMessage`].
#[derive(Debug, Default)]
pub struct MessageAccumulator {
    id: String,
    model: String,
    started: bool,
    stopped: bool,
    blocks: Vec<Option<BlockState>>,
    stop_reason: Option<String>,
    stop_details: Option<Value>,
    usage: Usage,
    invalid: BTreeMap<String, InvalidToolInput>,
    fallback_iteration: bool,
}

fn append_str(map: &mut Map<String, Value>, key: &str, piece: &str) {
    match map.get_mut(key) {
        Some(Value::String(s)) => s.push_str(piece),
        _ => {
            map.insert(key.to_owned(), Value::String(piece.to_owned()));
        }
    }
}

fn has_fallback_iteration(usage: &Value) -> bool {
    usage
        .get("iterations")
        .and_then(Value::as_array)
        .is_some_and(|it| it.iter().any(|e| block_type(e) == "fallback_message"))
}

impl MessageAccumulator {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `message_start` has been seen (content may be flowing to observers).
    #[must_use]
    pub fn started(&self) -> bool {
        self.started
    }

    /// `message_stop` has been seen.
    #[must_use]
    pub fn stopped(&self) -> bool {
        self.stopped
    }

    pub(crate) fn apply(
        &mut self,
        ev: &SseEvent,
        sink: &mut (dyn FnMut(StreamUpdate<'_>) + Send),
    ) -> Result<(), EventError> {
        let data: Value = serde_json::from_str(&ev.data)
            .map_err(|e| EventError::Malformed(format!("event data is not JSON ({e}): {:.200}", ev.data)))?;
        // The `type` inside the data is authoritative; the `event:` line
        // mirrors it.
        let kind = data.get("type").and_then(Value::as_str).or(ev.event.as_deref()).unwrap_or("");
        match kind {
            "message_start" => {
                let msg = data.get("message").ok_or_else(|| malformed("message_start without message"))?;
                self.started = true;
                msg.get("id").and_then(Value::as_str).unwrap_or_default().clone_into(&mut self.id);
                msg.get("model").and_then(Value::as_str).unwrap_or_default().clone_into(&mut self.model);
                if let Some(u) = msg.get("usage") {
                    self.usage.merge_json(u);
                    self.fallback_iteration |= has_fallback_iteration(u);
                }
                if let Some(blocks) = msg.get("content").and_then(Value::as_array) {
                    for (i, b) in blocks.iter().enumerate() {
                        if let Value::Object(m) = b {
                            self.put_block(i, BlockState { value: m.clone(), closed: true, ..BlockState::default() });
                        }
                    }
                }
                sink(StreamUpdate::MessageStart { model: &self.model });
            }
            "content_block_start" => {
                let index = index_of(&data)?;
                let Some(Value::Object(block)) = data.get("content_block") else {
                    return Err(malformed("content_block_start without an object content_block"));
                };
                self.put_block(index, BlockState { value: block.clone(), ..BlockState::default() });
                // Normally empty, but forward any initial text so observers
                // see exactly what ends up in the block.
                let st = self.block(index)?;
                match block_type_map(&st.value) {
                    "text" => {
                        if let Some(t) = st.value.get("text").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                            sink(StreamUpdate::TextDelta { index, text: t });
                        }
                    }
                    "thinking" => {
                        if let Some(t) = st.value.get("thinking").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                            sink(StreamUpdate::ThinkingDelta { index, text: t });
                        }
                    }
                    _ => {}
                }
            }
            "content_block_delta" => {
                let index = index_of(&data)?;
                let delta = data.get("delta").ok_or_else(|| malformed("content_block_delta without delta"))?;
                let st = self.block_mut(index)?;
                let s = |key: &str| delta.get(key).and_then(Value::as_str).unwrap_or("");
                match block_type(delta) {
                    "text_delta" => {
                        let t = s("text");
                        append_str(&mut st.value, "text", t);
                        sink(StreamUpdate::TextDelta { index, text: t });
                    }
                    "thinking_delta" => {
                        let t = s("thinking");
                        append_str(&mut st.value, "thinking", t);
                        sink(StreamUpdate::ThinkingDelta { index, text: t });
                    }
                    "signature_delta" => append_str(&mut st.value, "signature", s("signature")),
                    "input_json_delta" => {
                        st.partial_json.push_str(s("partial_json"));
                        st.has_json = true;
                    }
                    "citations_delta" => {
                        if let Some(c) = delta.get("citation") {
                            match st.value.get_mut("citations") {
                                Some(Value::Array(a)) => a.push(c.clone()),
                                _ => {
                                    st.value.insert("citations".into(), Value::Array(vec![c.clone()]));
                                }
                            }
                        }
                    }
                    other => tracing::debug!(delta = other, "ignoring unknown content_block_delta type"),
                }
            }
            "content_block_stop" => {
                let index = index_of(&data)?;
                self.close_block(index)?;
                let st = self.block(index)?;
                let block = Value::Object(st.value.clone());
                let invalid_input = block.get("id").and_then(Value::as_str).and_then(|id| self.invalid.get(id));
                sink(StreamUpdate::BlockStop { index, block: &block, invalid_input });
            }
            "message_delta" => {
                if let Some(delta) = data.get("delta") {
                    if let Some(r) = delta.get("stop_reason").and_then(Value::as_str) {
                        self.stop_reason = Some(r.to_owned());
                    }
                    if let Some(d) = delta.get("stop_details").filter(|d| !d.is_null()) {
                        self.stop_details = Some(d.clone());
                    }
                }
                if let Some(d) = data.get("stop_details").filter(|d| !d.is_null()) {
                    self.stop_details = Some(d.clone());
                }
                if let Some(u) = data.get("usage") {
                    self.usage.merge_json(u);
                    self.fallback_iteration |= has_fallback_iteration(u);
                }
            }
            "message_stop" => self.stopped = true,
            "ping" => {}
            "error" => {
                let err = data.get("error");
                let field = |k: &str| err.and_then(|e| e.get(k)).and_then(Value::as_str).unwrap_or("").to_owned();
                return Err(EventError::Api { kind: field("type"), message: field("message") });
            }
            other => tracing::debug!(event = other, "ignoring unknown stream event"),
        }
        Ok(())
    }

    fn put_block(&mut self, index: usize, st: BlockState) {
        if self.blocks.len() <= index {
            self.blocks.resize_with(index + 1, || None);
        }
        self.blocks[index] = Some(st);
    }

    fn block(&self, index: usize) -> Result<&BlockState, EventError> {
        self.blocks
            .get(index)
            .and_then(Option::as_ref)
            .ok_or_else(|| EventError::Malformed(format!("event for unknown content block {index}")))
    }

    fn block_mut(&mut self, index: usize) -> Result<&mut BlockState, EventError> {
        self.blocks
            .get_mut(index)
            .and_then(Option::as_mut)
            .ok_or_else(|| EventError::Malformed(format!("event for unknown content block {index}")))
    }

    /// Finalizes a block: strictly parses streamed tool input.
    fn close_block(&mut self, index: usize) -> Result<(), EventError> {
        let st = self.block_mut(index)?;
        if st.closed {
            return Ok(());
        }
        st.closed = true;
        if !st.has_json {
            return Ok(());
        }
        let raw = std::mem::take(&mut st.partial_json);
        let parsed = if raw.trim().is_empty() {
            // A tool with no parameters may stream nothing.
            Ok(Value::Object(Map::new()))
        } else {
            match serde_json::from_str::<Value>(&raw) {
                Ok(v @ Value::Object(_)) => Ok(v),
                Ok(_) => Err("tool input is not a JSON object".to_owned()),
                Err(e) => Err(e.to_string()),
            }
        };
        match parsed {
            Ok(v) => {
                st.value.insert("input".into(), v);
            }
            Err(error) => {
                st.value.insert("input".into(), Value::Object(Map::new()));
                let id = st.value.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
                self.invalid.insert(id, InvalidToolInput { raw, error });
            }
        }
        Ok(())
    }

    /// Completes the message. Fails if `message_stop` never arrived.
    pub(crate) fn finish(mut self) -> Result<AssistantMessage, EventError> {
        if !self.started || !self.stopped {
            return Err(malformed("stream ended before message_stop"));
        }
        for i in 0..self.blocks.len() {
            if self.blocks[i].is_some() {
                self.close_block(i)?;
            }
        }
        let content: Vec<Value> = self.blocks.into_iter().flatten().map(|b| Value::Object(b.value)).collect();
        let fallback_block = content.iter().any(|b| block_type(b) == "fallback");
        let mut usage = self.usage;
        usage.requests = 1;
        Ok(AssistantMessage {
            id: self.id,
            model: self.model,
            content,
            stop_reason: self.stop_reason,
            stop_details: self.stop_details,
            usage,
            invalid_tool_inputs: self.invalid,
            served_by_fallback: self.fallback_iteration || fallback_block,
        })
    }
}

fn block_type_map(m: &Map<String, Value>) -> &str {
    m.get("type").and_then(Value::as_str).unwrap_or("")
}

fn malformed(msg: &str) -> EventError {
    EventError::Malformed(msg.to_owned())
}

fn index_of(data: &Value) -> Result<usize, EventError> {
    data.get("index")
        .and_then(Value::as_u64)
        .map(|i| i as usize)
        .ok_or_else(|| malformed("content block event without index"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn run(events: &[Value]) -> (AssistantMessage, Vec<String>) {
        let mut acc = MessageAccumulator::new();
        let mut log = Vec::new();
        for e in events {
            let ev = SseEvent { event: e["type"].as_str().map(str::to_owned), data: e.to_string() };
            acc.apply(&ev, &mut |u| match u {
                StreamUpdate::TextDelta { text, .. } => log.push(format!("text:{text}")),
                StreamUpdate::ThinkingDelta { text, .. } => log.push(format!("thinking:{text}")),
                StreamUpdate::BlockStop { block, invalid_input, .. } => {
                    log.push(format!("stop:{}:{}", block["type"], invalid_input.is_some()));
                }
                StreamUpdate::MessageStart { model } => log.push(format!("start:{model}")),
            })
            .unwrap();
        }
        (acc.finish().unwrap(), log)
    }

    fn start(model: &str) -> Value {
        json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":model,
            "content":[],"stop_reason":null,"stop_sequence":null,
            "usage":{"input_tokens":100,"cache_creation_input_tokens":0,"cache_read_input_tokens":2048,"output_tokens":1}}})
    }

    #[test]
    fn assembles_thinking_text_and_tool_use() {
        let (msg, log) = run(&[
            start("claude-opus-5-5"),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"EqQBsig=="}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Checking "}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"AAPL."}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"get_quote","input":{}}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":""}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"symbol\": \"AA"}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"PL\"}"}}),
            json!({"type":"content_block_stop","index":2}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":57}}),
            json!({"type":"message_stop"}),
        ]);
        assert_eq!(msg.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(msg.text(), "Checking AAPL.");
        assert_eq!(msg.content[0], json!({"type":"thinking","thinking":"","signature":"EqQBsig=="}));
        assert_eq!(msg.content[2]["input"], json!({"symbol":"AAPL"}));
        assert!(msg.invalid_tool_inputs.is_empty());
        assert_eq!(msg.usage.output_tokens, 57);
        assert_eq!(msg.usage.input_tokens, 100);
        assert_eq!(msg.usage.cache_read_input_tokens, 2048);
        assert_eq!(log[0], "start:claude-opus-5-5");
        assert!(log.contains(&"text:Checking ".to_owned()));
    }

    #[test]
    fn invalid_tool_json_is_recorded_and_replaced() {
        let (msg, _) = run(&[
            start("claude-opus-5-5"),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_x","name":"sql_query","input":{}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"sql\": \"SELECT \"x\"\"}"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}),
            json!({"type":"message_stop"}),
        ]);
        assert_eq!(msg.content[0]["input"], json!({}));
        let bad = &msg.invalid_tool_inputs["toolu_x"];
        assert_eq!(bad.raw, "{\"sql\": \"SELECT \"x\"\"}");
    }

    #[test]
    fn non_object_input_is_invalid() {
        let (msg, _) = run(&[
            start("m"),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"n","input":{}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"[1,2]"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":3}}),
            json!({"type":"message_stop"}),
        ]);
        assert!(msg.invalid_tool_inputs["t"].error.contains("not a JSON object"));
    }

    #[test]
    fn missing_message_stop_is_an_error() {
        let mut acc = MessageAccumulator::new();
        let ev = SseEvent { event: None, data: start("m").to_string() };
        acc.apply(&ev, &mut |_| {}).unwrap();
        assert!(acc.finish().is_err());
    }

    #[test]
    fn error_event_surfaces_kind() {
        let mut acc = MessageAccumulator::new();
        let ev = SseEvent {
            event: Some("error".into()),
            data: json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}).to_string(),
        };
        let err = acc.apply(&ev, &mut |_| {}).unwrap_err();
        assert_eq!(err, EventError::Api { kind: "overloaded_error".into(), message: "Overloaded".into() });
    }

    #[test]
    fn refusal_stop_details_are_captured() {
        let (msg, _) = run(&[
            start("claude-opus-5-5"),
            json!({"type":"message_delta","delta":{"stop_reason":"refusal","stop_details":{"type":"refusal","category":"cyber","explanation":null}},"usage":{"output_tokens":0}}),
            json!({"type":"message_stop"}),
        ]);
        assert_eq!(msg.refusal_category().as_deref(), Some("cyber"));
        assert!(msg.content.is_empty());
    }

    #[test]
    fn replay_after_mid_stream_fallback_drops_declined_internals() {
        let msg = AssistantMessage {
            id: "m".into(),
            model: "claude-opus-5-5".into(),
            content: vec![
                json!({"type":"thinking","thinking":"","signature":"a"}),
                json!({"type":"text","text":"Partial "}),
                json!({"type":"tool_use","id":"toolu_dead","name":"x","input":{}}),
                json!({"type":"fallback","from":{"model":"claude-opus-5-5"},"to":{"model":"claude-opus-5"}}),
                json!({"type":"thinking","thinking":"","signature":"b"}),
                json!({"type":"text","text":"continued."}),
            ],
            stop_reason: Some("end_turn".into()),
            stop_details: None,
            usage: Usage::default(),
            invalid_tool_inputs: BTreeMap::new(),
            served_by_fallback: true,
        };
        assert_eq!(msg.served_model(), "claude-opus-5");
        let replay = msg.replay_content();
        let types: Vec<&str> = replay.iter().map(block_type).collect();
        assert_eq!(types, ["text", "fallback", "thinking", "text"]);
        assert_eq!(msg.text(), "Partial continued.");
    }
}

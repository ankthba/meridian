//! Offline tests of the full tool-use loop. A fake `Transport` replays
//! canned SSE streams built from the documented Messages API event shapes;
//! nothing here touches the network.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use chrono::NaiveDate;
use meridian_ask::{
    AnthropicClient, AskConfig, AskContext, AskError, AskObserver, AskOutcome, AskSession, CancelFlag, HttpRequest,
    HttpResponse, RetryPolicy, ToolAudit, ToolDefinition, ToolExecutor, ToolOutcome, Transport, TransportError,
};
use meridian_types::{FixedClock, MarketSector, SecurityKey};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};

// ---------------------------------------------------------------- SSE ----

fn sse(events: &[Value]) -> String {
    let mut s = String::new();
    for e in events {
        writeln!(s, "event: {}\ndata: {e}\n", e["type"].as_str().unwrap()).unwrap();
    }
    s
}

fn message_start(model: &str) -> Value {
    json!({"type":"message_start","message":{"id":"msg_01","type":"message","role":"assistant","model":model,
        "content":[],"stop_reason":null,"stop_sequence":null,
        "usage":{"input_tokens":1200,"cache_creation_input_tokens":0,"cache_read_input_tokens":3000,"output_tokens":1}}})
}

fn thinking(index: usize, signature: &str) -> Vec<Value> {
    vec![
        json!({"type":"content_block_start","index":index,"content_block":{"type":"thinking","thinking":"","signature":""}}),
        json!({"type":"content_block_delta","index":index,"delta":{"type":"signature_delta","signature":signature}}),
        json!({"type":"content_block_stop","index":index}),
    ]
}

fn text(index: usize, pieces: &[&str]) -> Vec<Value> {
    let mut v = vec![json!({"type":"content_block_start","index":index,"content_block":{"type":"text","text":""}})];
    for p in pieces {
        v.push(json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":p}}));
    }
    v.push(json!({"type":"content_block_stop","index":index}));
    v
}

fn tool_use(index: usize, id: &str, name: &str, json_pieces: &[&str]) -> Vec<Value> {
    let mut v = vec![json!({"type":"content_block_start","index":index,
        "content_block":{"type":"tool_use","id":id,"name":name,"input":{}}})];
    for p in json_pieces {
        v.push(json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":p}}));
    }
    v.push(json!({"type":"content_block_stop","index":index}));
    v
}

fn finish(stop_reason: &str) -> Vec<Value> {
    vec![
        json!({"type":"message_delta","delta":{"stop_reason":stop_reason,"stop_sequence":null},"usage":{"output_tokens":80}}),
        json!({"type":"message_stop"}),
    ]
}

fn stream(model: &str, blocks: Vec<Vec<Value>>, stop_reason: &str) -> String {
    let mut events = vec![message_start(model), json!({"type":"ping"})];
    events.extend(blocks.into_iter().flatten());
    events.extend(finish(stop_reason));
    sse(&events)
}

// ---------------------------------------------------------- transport ----

enum Canned {
    Sse(String),
    Status { status: u16, headers: Vec<(String, String)>, body: String },
}

#[derive(Debug, Clone)]
struct Recorded {
    headers: Vec<(String, String)>,
    api_key: String,
    url: String,
    body: Value,
}

#[derive(Default)]
struct FakeTransport {
    responses: Mutex<VecDeque<Canned>>,
    requests: Mutex<Vec<Recorded>>,
}

impl FakeTransport {
    fn new(responses: Vec<Canned>) -> Arc<Self> {
        Arc::new(Self { responses: Mutex::new(responses.into()), requests: Mutex::default() })
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl Transport for FakeTransport {
    async fn post(&self, req: HttpRequest) -> Result<HttpResponse, TransportError> {
        self.requests.lock().unwrap().push(Recorded {
            headers: req.headers.clone(),
            api_key: req.api_key.expose_secret().to_owned(),
            url: req.url.clone(),
            body: serde_json::from_slice(&req.body).expect("request body is JSON"),
        });
        let next = self.responses.lock().unwrap().pop_front().expect("no canned response left");
        let (status, headers, body) = match next {
            Canned::Sse(s) => (200, vec![("content-type".into(), "text/event-stream".into())], s),
            Canned::Status { status, headers, body } => (status, headers, body),
        };
        // Odd-sized chunks so events straddle chunk boundaries.
        let chunks: Vec<Result<Bytes, TransportError>> =
            body.into_bytes().chunks(7).map(|c| Ok(Bytes::copy_from_slice(c))).collect();
        Ok(HttpResponse { status, headers, body: Box::pin(futures_util::stream::iter(chunks)) })
    }
}

// ----------------------------------------------------------- executor ----

struct Tools {
    calls: AtomicUsize,
    /// When set, every call waits here, so a sequential executor would
    /// deadlock (caught by a timeout).
    barrier: Option<tokio::sync::Barrier>,
}

impl Tools {
    fn new() -> Arc<Self> {
        Arc::new(Self { calls: AtomicUsize::new(0), barrier: None })
    }
}

fn object_schema(props: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":props,"required":required,"additionalProperties":false})
}

#[async_trait]
impl ToolExecutor for Tools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "get_quote".into(),
                description: "Latest quote. Call for any current price question.".into(),
                input_schema: object_schema(json!({"symbol":{"type":"string"}}), &["symbol"]),
            },
            ToolDefinition {
                name: "compute".into(),
                description: "Named analytics function. Call for every derived number.".into(),
                input_schema: object_schema(
                    json!({"function":{"type":"string"},"args":{"type":"array","items":{"type":"number"}}}),
                    &["function", "args"],
                ),
            },
        ]
    }

    async fn execute(&self, name: &str, input: Value) -> ToolOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(b) = &self.barrier {
            b.wait().await;
        }
        match name {
            "get_quote" => {
                let sym = input["symbol"].as_str().unwrap_or_default();
                let price = if sym == "AAPL" { 227.52 } else { 415.10 };
                ToolOutcome::json(
                    &json!({"symbol": sym, "last": price, "change_pct": -1.3, "as_of": "2026-10-02"}),
                    ToolAudit::default(),
                )
            }
            _ => ToolOutcome::error("unsupported"),
        }
    }
}

// ----------------------------------------------------------- observer ----

#[derive(Default)]
struct Log {
    events: Mutex<Vec<String>>,
    text: Mutex<String>,
    thinking: Mutex<Vec<String>>,
}

impl Log {
    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
}

impl AskObserver for Log {
    fn on_text_delta(&self, text: &str) {
        self.text.lock().unwrap().push_str(text);
    }
    fn on_thinking_update(&self, text: &str) {
        self.thinking.lock().unwrap().push(text.to_owned());
    }
    fn on_tool_call(&self, id: &str, name: &str, input_json: &str) {
        self.events.lock().unwrap().push(format!("call {id} {name} {input_json}"));
    }
    fn on_tool_result(&self, id: &str, audit: &ToolAudit, is_error: bool) {
        self.events.lock().unwrap().push(format!("result {id} {} error={is_error}", audit.tool));
    }
    fn on_done(&self, outcome: AskOutcome) {
        self.events.lock().unwrap().push(format!("done {}", outcome.iterations));
    }
    fn on_error(&self, error: AskError) {
        self.events.lock().unwrap().push(format!("error {}", error.code()));
    }
}

// ------------------------------------------------------------ helpers ----

const MODEL: &str = "claude-opus-5-5";

fn session(transport: Arc<FakeTransport>, tools: Arc<Tools>, config: AskConfig) -> AskSession {
    let client = AnthropicClient::with_transport(SecretString::from("sk-test"), transport)
        .unwrap()
        .with_retry_policy(RetryPolicy {
            max_retries: 2,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_secs(1),
        });
    AskSession::new(Arc::new(client), tools, config, "session-1").with_clock(Arc::new(FixedClock(42)))
}

fn ctx() -> AskContext {
    AskContext {
        panel_security: Some(SecurityKey::new("AAPL", Some("US"), MarketSector::Equity)),
        date: NaiveDate::from_ymd_opt(2026, 10, 5).unwrap(),
    }
}

fn header<'a>(r: &'a Recorded, name: &str) -> Option<&'a str> {
    r.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

async fn ask(s: &mut AskSession, q: &str, log: &Arc<Log>) -> Result<AskOutcome, AskError> {
    s.ask(q, &ctx(), log.clone(), CancelFlag::new()).await
}

// -------------------------------------------------------------- tests ----

#[tokio::test]
async fn text_only_answer() {
    let t = FakeTransport::new(vec![Canned::Sse(stream(
        MODEL,
        vec![thinking(0, "sig-a"), text(1, &["NOT AVAILABLE \u{2014} ", "no tool covers that; the answer is 42."])],
        "end_turn",
    ))]);
    let tools = Tools::new();
    let mut s = session(t.clone(), tools.clone(), AskConfig::default());
    let log = Arc::new(Log::default());
    let out = ask(&mut s, "What is the meaning of life?", &log).await.unwrap();

    assert_eq!(out.iterations, 1);
    assert_eq!(out.turn.answer, "NOT AVAILABLE \u{2014} no tool covers that; the answer is 42.");
    assert_eq!(*log.text.lock().unwrap(), out.turn.answer);
    assert_eq!(out.turn.number_checks.len(), 1);
    assert!(!out.turn.number_checks[0].verified, "42 came from no tool");
    assert_eq!(out.turn.usage.cache_read_input_tokens, 3000);
    assert_eq!(out.turn.usage.output_tokens, 80);
    assert_eq!(out.turn.usage.requests, 1);
    assert_eq!(out.turn.model, MODEL);
    assert_eq!(out.turn.started_at, 42);
    assert_eq!(tools.calls.load(Ordering::SeqCst), 0);
    assert_eq!(log.events(), ["done 1"]);

    let reqs = t.requests();
    assert_eq!(reqs.len(), 1);
    let r = &reqs[0];
    assert_eq!(r.url, "https://api.anthropic.com/v1/messages");
    assert_eq!(r.api_key, "sk-test");
    assert_eq!(header(r, "anthropic-version"), Some("2023-06-01"));
    assert_eq!(header(r, "content-type"), Some("application/json"));
    assert_eq!(header(r, "anthropic-beta"), Some("server-side-fallback-2026-07-01"));
    let b = &r.body;
    assert_eq!(b["model"], MODEL);
    assert_eq!(b["max_tokens"], 64000);
    assert_eq!(b["stream"], true);
    assert!(b.get("thinking").is_none());
    assert_eq!(b["output_config"], json!({"effort": "high"}));
    assert_eq!(b["fallbacks"], "default");
    assert_eq!(b["tool_choice"], json!({"type": "auto"}));
    assert_eq!(b["system"][0]["cache_control"], json!({"type": "ephemeral"}));
    assert!(!b["system"][0]["text"].as_str().unwrap().contains("2026"), "date must not be in the system prompt");
    // Tools sorted by name, strict, eager.
    let names: Vec<&str> = b["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["compute", "get_quote"]);
    assert!(b["tools"].as_array().unwrap().iter().all(|t| t["strict"] == true && t["eager_input_streaming"] == true));
    // Date and panel context ride in the user turn.
    let user = &b["messages"][0];
    assert_eq!(user["role"], "user");
    assert!(user["content"][0]["text"].as_str().unwrap().contains("date: 2026-10-05 (Monday)"));
    assert!(user["content"][0]["text"].as_str().unwrap().contains("panel_security: AAPL US Equity"));
    assert_eq!(user["content"][1]["text"], "What is the meaning of life?");

    // The assistant turn is in history, thinking signature intact.
    assert_eq!(s.history().len(), 2);
    assert_eq!(s.history()[1].content[0], json!({"type":"thinking","thinking":"","signature":"sig-a"}));
    assert_eq!(s.transcript().turns.len(), 1);
}

#[tokio::test]
async fn tool_use_then_final_answer_replays_assistant_turn_unchanged() {
    let t = FakeTransport::new(vec![
        Canned::Sse(stream(
            MODEL,
            vec![
                thinking(0, "sig-1"),
                text(1, &["Pulling the quote."]),
                tool_use(2, "toolu_1", "get_quote", &["", "{\"sym", "bol\": \"AAPL\"}"]),
            ],
            "tool_use",
        )),
        Canned::Sse(stream(
            MODEL,
            vec![thinking(0, "sig-2"), text(1, &["AAPL last traded at $227.52 [1], ", "down 1.3%."])],
            "end_turn",
        )),
    ]);
    let tools = Tools::new();
    let mut s = session(t.clone(), tools.clone(), AskConfig::default());
    let log = Arc::new(Log::default());
    let out = ask(&mut s, "Where is AAPL trading?", &log).await.unwrap();

    assert_eq!(out.iterations, 2);
    assert_eq!(out.turn.answer, "Pulling the quote.\n\nAAPL last traded at $227.52 [1], down 1.3%.");
    assert_eq!(*log.text.lock().unwrap(), out.turn.answer);
    assert_eq!(tools.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        log.events(),
        ["call toolu_1 get_quote {\"symbol\":\"AAPL\"}", "result toolu_1 get_quote error=false", "done 2"]
    );

    // Audit.
    let audit = &out.turn.tool_calls[0];
    assert_eq!(audit.tool_use_id, "toolu_1");
    assert_eq!(audit.tool, "get_quote");
    assert_eq!(audit.input_json, "{\"symbol\":\"AAPL\"}");
    assert!(!audit.is_error);
    assert!(audit.numbers.contains(&227.52));
    assert_eq!(audit.result_hash.len(), 16);
    // Verifier: both numbers came from the tool.
    let checks: Vec<(&str, bool, Option<&str>)> = out
        .turn
        .number_checks
        .iter()
        .map(|c| (c.text.as_str(), c.verified, c.matched_tool_call.as_deref()))
        .collect();
    assert_eq!(checks, [("$227.52", true, Some("toolu_1")), ("1.3%", true, Some("toolu_1"))]);
    assert_eq!(out.turn.usage.requests, 2);

    let reqs = t.requests();
    assert_eq!(reqs.len(), 2);
    let m1 = reqs[0].body["messages"].as_array().unwrap();
    let m2 = reqs[1].body["messages"].as_array().unwrap();
    // Append-only: request 2 starts with request 1's messages, byte for byte.
    assert_eq!(&m2[..m1.len()], &m1[..]);
    assert_eq!(m2.len(), 3);
    // The assistant content goes back exactly as streamed.
    assert_eq!(
        m2[1],
        json!({"role":"assistant","content":[
            {"type":"thinking","thinking":"","signature":"sig-1"},
            {"type":"text","text":"Pulling the quote."},
            {"type":"tool_use","id":"toolu_1","name":"get_quote","input":{"symbol":"AAPL"}}
        ]})
    );
    // One user message holding the tool result, with its source marker.
    assert_eq!(m2[2]["role"], "user");
    let results = m2[2]["content"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["type"], "tool_result");
    assert_eq!(results[0]["tool_use_id"], "toolu_1");
    assert!(results[0].get("is_error").is_none());
    assert_eq!(results[0]["content"][0]["text"], "SOURCE [1]");
    let payload: Value = serde_json::from_str(results[0]["content"][1]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["last"], 227.52);
    // Prefix (system + tools) is byte-identical across requests.
    assert_eq!(reqs[0].body["system"], reqs[1].body["system"]);
    assert_eq!(reqs[0].body["tools"], reqs[1].body["tools"]);

    // History holds the whole turn for follow-ups.
    assert_eq!(s.history().len(), 4);
}

#[tokio::test]
async fn parallel_tool_calls_run_concurrently_and_return_in_one_message() {
    let t = FakeTransport::new(vec![
        Canned::Sse(stream(
            MODEL,
            vec![
                tool_use(0, "toolu_a", "get_quote", &["{\"symbol\": \"AAPL\"}"]),
                tool_use(1, "toolu_b", "get_quote", &["{\"symbol\": \"MSFT\"}"]),
            ],
            "tool_use",
        )),
        Canned::Sse(stream(MODEL, vec![text(0, &["AAPL $227.52 [1], MSFT $415.10 [2]."])], "end_turn")),
    ]);
    let tools = Arc::new(Tools { calls: AtomicUsize::new(0), barrier: Some(tokio::sync::Barrier::new(2)) });
    let mut s = session(t.clone(), tools.clone(), AskConfig::default());
    let log = Arc::new(Log::default());
    let out = tokio::time::timeout(Duration::from_secs(5), ask(&mut s, "AAPL vs MSFT?", &log))
        .await
        .expect("tools ran sequentially (barrier never released)")
        .unwrap();

    assert_eq!(tools.calls.load(Ordering::SeqCst), 2);
    let ids: Vec<&str> = out.turn.tool_calls.iter().map(|a| a.tool_use_id.as_str()).collect();
    assert_eq!(ids, ["toolu_a", "toolu_b"]);
    assert!(out.turn.number_checks.iter().all(|c| c.verified));
    assert_eq!(out.turn.number_checks[1].matched_tool_call.as_deref(), Some("toolu_b"));

    let m2 = t.requests()[1].body["messages"].clone();
    let m2 = m2.as_array().unwrap();
    assert_eq!(m2.len(), 3, "exactly one user message carries the results");
    let results = m2[2]["content"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["tool_use_id"], "toolu_a");
    assert_eq!(results[0]["content"][0]["text"], "SOURCE [1]");
    assert_eq!(results[1]["tool_use_id"], "toolu_b");
    assert_eq!(results[1]["content"][0]["text"], "SOURCE [2]");
}

#[tokio::test]
async fn invalid_tool_input_returns_is_error_without_running() {
    let t = FakeTransport::new(vec![
        Canned::Sse(stream(
            MODEL,
            vec![
                // Unparseable JSON (eager streaming does not validate).
                tool_use(0, "toolu_bad", "get_quote", &["{\"symbol\": \"AA", "PL\" \"x\"}"]),
                // Parses, but violates the schema.
                tool_use(1, "toolu_schema", "get_quote", &["{\"symbol\": 5}"]),
                // Not a declared tool.
                tool_use(2, "toolu_unknown", "get_news", &["{}"]),
            ],
            "tool_use",
        )),
        Canned::Sse(stream(MODEL, vec![text(0, &["NOT AVAILABLE \u{2014} the quote tool rejected my input."])], "end_turn")),
    ]);
    let tools = Tools::new();
    let mut s = session(t.clone(), tools.clone(), AskConfig::default());
    let log = Arc::new(Log::default());
    let out = ask(&mut s, "AAPL?", &log).await.unwrap();

    assert_eq!(tools.calls.load(Ordering::SeqCst), 0, "no invalid call reaches the executor");
    assert!(out.turn.tool_calls.iter().all(|a| a.is_error && a.numbers.is_empty()));
    assert_eq!(out.turn.tool_calls[0].input_json, "{\"symbol\": \"AAPL\" \"x\"}");

    let reqs = t.requests();
    let m2 = reqs[1].body["messages"].as_array().unwrap().clone();
    // The bad block is replayed with `{}` so the turn stays valid.
    assert_eq!(m2[1]["content"][0]["input"], json!({}));
    let results = m2[2]["content"].as_array().unwrap();
    assert_eq!(results.len(), 3);
    for r in results {
        assert_eq!(r["is_error"], true);
    }
    let bad: Value = serde_json::from_str(results[0]["content"][1]["text"].as_str().unwrap()).unwrap();
    assert_eq!(bad["INVALID_JSON"], "{\"symbol\": \"AAPL\" \"x\"}");
    let schema: Value = serde_json::from_str(results[1]["content"][1]["text"].as_str().unwrap()).unwrap();
    assert!(schema["error"].as_str().unwrap().contains("$.symbol: expected string"));
    let unknown: Value = serde_json::from_str(results[2]["content"][1]["text"].as_str().unwrap()).unwrap();
    assert_eq!(unknown["UNKNOWN_TOOL"], "get_news");
    assert!(log.events().contains(&"result toolu_bad get_quote error=true".to_owned()));
}

#[tokio::test]
async fn refusal_stops_and_rolls_back() {
    // Mid-stream decline: some text streamed, then the classifier stopped it.
    let mut events = vec![message_start(MODEL)];
    events.extend(text(0, &["Partial "]));
    events.push(json!({"type":"message_delta","delta":{"stop_reason":"refusal","stop_sequence":null,
        "stop_details":{"type":"refusal","category":"cyber","explanation":null}},"usage":{"output_tokens":3}}));
    events.push(json!({"type":"message_stop"}));
    let t = FakeTransport::new(vec![Canned::Sse(sse(&events))]);
    let tools = Tools::new();
    let mut s = session(t, tools, AskConfig::default());
    let log = Arc::new(Log::default());
    let err = ask(&mut s, "question", &log).await.unwrap_err();

    assert_eq!(err, AskError::Refusal { category: Some("cyber".into()) });
    assert_eq!(log.events(), ["error refusal"]);
    assert!(s.history().is_empty(), "failed question is rolled back");
    let turn = &s.transcript().turns[0];
    assert_eq!(turn.stop_reason.as_deref(), Some("refusal"));
    assert!(turn.error.as_deref().unwrap().starts_with("refusal:"));
    assert!(turn.number_checks.is_empty());
}

#[tokio::test]
async fn max_tokens_with_partial_tool_use_is_truncated_and_nothing_runs() {
    let t = FakeTransport::new(vec![Canned::Sse(stream(
        MODEL,
        vec![tool_use(0, "toolu_cut", "get_quote", &["{\"symbol\": \"AA"])],
        "max_tokens",
    ))]);
    let tools = Tools::new();
    let mut s = session(t.clone(), tools.clone(), AskConfig::default());
    let log = Arc::new(Log::default());
    let err = ask(&mut s, "AAPL?", &log).await.unwrap_err();

    assert_eq!(err, AskError::Truncated);
    assert_eq!(tools.calls.load(Ordering::SeqCst), 0);
    assert_eq!(t.requests().len(), 1);
    assert!(s.history().is_empty());
    assert_eq!(log.events().last().unwrap(), "error truncated");
}

#[tokio::test]
async fn follow_up_question_extends_history_without_editing_it() {
    let t = FakeTransport::new(vec![
        Canned::Sse(stream(MODEL, vec![thinking(0, "s1"), text(1, &["First."])], "end_turn")),
        Canned::Sse(stream(MODEL, vec![text(0, &["Second."])], "end_turn")),
    ]);
    let mut s = session(t.clone(), Tools::new(), AskConfig::default());
    let log = Arc::new(Log::default());
    ask(&mut s, "one", &log).await.unwrap();
    ask(&mut s, "two", &log).await.unwrap();
    let reqs = t.requests();
    let m1 = reqs[0].body["messages"].as_array().unwrap().clone();
    let m2 = reqs[1].body["messages"].as_array().unwrap().clone();
    assert_eq!(m2.len(), 3);
    assert_eq!(&m2[..1], &m1[..]);
    assert_eq!(m2[1]["content"][0]["signature"], "s1");
    assert_eq!(m2[2]["content"][1]["text"], "two");
    assert_eq!(s.transcript().turns.len(), 2);
}

#[tokio::test]
async fn retries_overload_and_rate_limit_before_streaming() {
    let ok = stream(MODEL, vec![text(0, &["ok"])], "end_turn");
    let t = FakeTransport::new(vec![
        Canned::Status {
            status: 529,
            headers: vec![],
            body: json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}).to_string(),
        },
        Canned::Status {
            status: 429,
            headers: vec![("retry-after".into(), "0".into())],
            body: json!({"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}).to_string(),
        },
        Canned::Sse(ok),
    ]);
    let mut s = session(t.clone(), Tools::new(), AskConfig::default());
    let out = ask(&mut s, "q", &Arc::new(Log::default())).await.unwrap();
    assert_eq!(out.turn.answer, "ok");
    assert_eq!(t.requests().len(), 3);
}

#[tokio::test]
async fn retries_are_capped_and_client_errors_are_not_retried() {
    let overloaded = || Canned::Status { status: 529, headers: vec![], body: String::new() };
    let t = FakeTransport::new(vec![overloaded(), overloaded(), overloaded()]);
    let mut s = session(t.clone(), Tools::new(), AskConfig::default());
    let err = ask(&mut s, "q", &Arc::new(Log::default())).await.unwrap_err();
    assert_eq!(err, AskError::Overloaded);
    assert_eq!(t.requests().len(), 3, "initial attempt + 2 retries");

    let t = FakeTransport::new(vec![Canned::Status {
        status: 400,
        headers: vec![],
        body: json!({"type":"error","error":{"type":"invalid_request_error","message":"bad"}}).to_string(),
    }]);
    let mut s = session(t.clone(), Tools::new(), AskConfig::default());
    let err = ask(&mut s, "q", &Arc::new(Log::default())).await.unwrap_err();
    assert_eq!(err, AskError::Http { status: 400, message: "invalid_request_error: bad".into() });
    assert_eq!(t.requests().len(), 1);
}

#[tokio::test]
async fn error_event_before_message_start_is_retried() {
    let early_error = sse(&[json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}})]);
    let t = FakeTransport::new(vec![
        Canned::Sse(early_error),
        Canned::Sse(stream(MODEL, vec![text(0, &["ok"])], "end_turn")),
    ]);
    let mut s = session(t.clone(), Tools::new(), AskConfig::default());
    assert_eq!(ask(&mut s, "q", &Arc::new(Log::default())).await.unwrap().turn.answer, "ok");
    assert_eq!(t.requests().len(), 2);
}

#[tokio::test]
async fn iteration_limit() {
    let looping = || {
        Canned::Sse(stream(MODEL, vec![tool_use(0, "toolu_l", "get_quote", &["{\"symbol\":\"AAPL\"}"])], "tool_use"))
    };
    let t = FakeTransport::new(vec![looping(), looping()]);
    let config = AskConfig { max_iterations: 2, ..AskConfig::default() };
    let mut s = session(t.clone(), Tools::new(), config);
    let err = ask(&mut s, "q", &Arc::new(Log::default())).await.unwrap_err();
    assert_eq!(err, AskError::IterationLimit);
    assert_eq!(t.requests().len(), 2);
    assert!(s.history().is_empty());
}

#[tokio::test]
async fn cancelled_before_start() {
    let t = FakeTransport::new(vec![]);
    let mut s = session(t.clone(), Tools::new(), AskConfig::default());
    let cancel = CancelFlag::new();
    cancel.cancel();
    let err = s.ask("q", &ctx(), Arc::new(Log::default()), cancel).await.unwrap_err();
    assert_eq!(err, AskError::Cancelled);
    assert!(t.requests().is_empty());
}

#[tokio::test]
async fn mid_stream_fallback_is_reported_and_replayed_per_rules() {
    let events = vec![
        message_start(MODEL),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"declined-sig"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"The quote "}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"content_block_start","index":2,"content_block":{"type":"fallback",
            "from":{"model":MODEL},"to":{"model":"claude-opus-5"}}}),
        json!({"type":"content_block_stop","index":2}),
        json!({"type":"content_block_start","index":3,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":3,"delta":{"type":"text_delta","text":"is unavailable."}}),
        json!({"type":"content_block_stop","index":3}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},
            "usage":{"output_tokens":12,"iterations":[{"type":"message"},{"type":"fallback_message"}]}}),
        json!({"type":"message_stop"}),
    ];
    let t = FakeTransport::new(vec![Canned::Sse(sse(&events))]);
    let mut s = session(t, Tools::new(), AskConfig::default());
    let out = ask(&mut s, "q", &Arc::new(Log::default())).await.unwrap();
    assert_eq!(out.turn.answer, "The quote is unavailable.");
    assert_eq!(out.turn.model, "claude-opus-5");
    assert_eq!(out.turn.requested_model, MODEL);
    assert!(out.turn.served_by_fallback);
    let types: Vec<&str> = s.history()[1].content.iter().map(|b| b["type"].as_str().unwrap()).collect();
    assert_eq!(types, ["text", "fallback", "text"], "declined model's thinking is not echoed");
}

#[tokio::test]
async fn progress_updates_reach_the_observer_when_enabled() {
    let events = vec![
        message_start(MODEL),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Checking the quote cache."}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"s"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Done."}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":9}}),
        json!({"type":"message_stop"}),
    ];
    let t = FakeTransport::new(vec![Canned::Sse(sse(&events))]);
    let mut config = AskConfig::default();
    config.request.thinking_display = meridian_ask::ThinkingDisplay::Updates;
    let mut s = session(t.clone(), Tools::new(), config);
    let log = Arc::new(Log::default());
    ask(&mut s, "q", &log).await.unwrap();
    assert_eq!(*log.thinking.lock().unwrap(), ["Checking the quote cache."]);
    let r = &t.requests()[0];
    assert_eq!(header(r, "anthropic-beta"), Some("server-side-fallback-2026-07-01,thinking-display-updates-2026-08-18"));
    assert_eq!(r.body["thinking"], json!({"type":"adaptive","display":"updates"}));
    // The thinking text is replayed verbatim.
    assert_eq!(s.history()[1].content[0]["thinking"], "Checking the quote cache.");
}

/// The engine spawns `ask` on a multi-threaded runtime, so its future must
/// be `Send`. This only needs to compile.
#[allow(dead_code)]
fn ask_future_is_send(s: &mut AskSession, c: &AskContext, o: Arc<dyn AskObserver>) {
    fn assert_send<T: Send>(_: &T) {}
    let fut = s.ask("q", c, o, CancelFlag::new());
    assert_send(&fut);
}

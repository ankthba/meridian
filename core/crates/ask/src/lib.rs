//! ASK: the AI analyst.
//!
//! Answers natural-language questions by tool use over local data, shows
//! every tool call it made as a numbered source, and checks every number in
//! the answer against the tool results.
//!
//! - [`client`]: raw-HTTP Messages API client with an SSE parser (there is
//!   no official Rust SDK), behind a [`Transport`] trait.
//! - [`request`]: request body (model, effort, strict tools, refusal
//!   fallback, prompt caching).
//! - [`agent`]: the manual tool-use loop ([`AskSession`]).
//! - [`tools`]: the [`ToolExecutor`] contract the engine implements.
//! - [`verify`]: the number verifier.
//! - [`audit`]: the persisted transcript.

pub mod agent;
pub mod audit;
pub mod cancel;
pub mod client;
pub mod error;
pub mod message;
pub mod prompt;
pub mod request;
pub mod schema;
pub mod sse;
pub mod tools;
pub mod transport;
pub mod verify;

pub use agent::{AskConfig, AskContext, AskObserver, AskOutcome, AskSession, DEFAULT_MAX_ITERATIONS};
pub use audit::{AskTranscript, AskTurn};
pub use cancel::CancelFlag;
pub use client::{AnthropicClient, DEFAULT_BASE_URL, RetryPolicy};
pub use error::AskError;
pub use message::{AssistantMessage, Message, Role, Usage};
pub use request::{DEFAULT_MODEL, Effort, FallbackMode, RequestOptions, ThinkingDisplay};
pub use tools::{SourceRef, ToolAudit, ToolDefinition, ToolExecutor, ToolOutcome, extract_numbers_from_json};
pub use transport::{ByteStream, HttpRequest, HttpResponse, ReqwestTransport, Transport, TransportError};
pub use verify::{NumberCheck, verify_numbers};

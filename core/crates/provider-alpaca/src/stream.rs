//! Real-time stock stream: `wss://stream.data.alpaca.markets/v2/{iex|sip}`.
//!
//! Protocol (docs.alpaca.markets/us/docs/streaming-market-data, checked
//! 2026-10-05): the server greets with `[{"T":"success","msg":"connected"}]`;
//! the client sends `{"action":"auth","key":…,"secret":…}` within 10 s and gets
//! `[{"T":"success","msg":"authenticated"}]`; then
//! `{"action":"subscribe","trades":[…],"quotes":[…]}` (and `unsubscribe`)
//! changes the subscription set, answered by a `subscription` message. Data
//! arrives as JSON arrays of objects tagged by `T` (`t` trade, `q` quote, …).
//!
//! One tokio task owns one connection. It keeps the desired subscription set,
//! reconnects with exponential backoff and jitter, re-authenticates and
//! re-subscribes, and reports state changes as `StreamEvent::Status`.

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures_util::{Sink, SinkExt, StreamExt};
use meridian_provider::{StreamHandle, StreamSink};
use meridian_types::{ProviderId, QuoteUpdate, SecurityKey, StreamEvent, datetime_to_nanos};
use rand::Rng;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::time::{Instant, timeout};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::USER_AGENT;
use tokio_tungstenite::tungstenite::{self, Message};

use crate::normalize::{PROVIDER_ID, alpaca_symbol};

/// Symbols per subscribe/unsubscribe message, to keep frames small.
const SUBSCRIBE_CHUNK: usize = 500;
/// A session that stayed authenticated this long resets the backoff.
const HEALTHY_AFTER: Duration = Duration::from_secs(30);

/// Commands from the handle to the connection task.
#[derive(Debug)]
pub(crate) enum Cmd {
    Subscribe(Vec<SecurityKey>),
    Unsubscribe(Vec<SecurityKey>),
    Close,
}

/// Handle returned by `connect`. Dropping it closes the stream, because the
/// task stops when the command channel closes.
pub(crate) struct Handle {
    tx: UnboundedSender<Cmd>,
}

impl StreamHandle for Handle {
    fn subscribe(&self, keys: &[SecurityKey]) {
        let _ = self.tx.send(Cmd::Subscribe(keys.to_vec()));
    }

    fn unsubscribe(&self, keys: &[SecurityKey]) {
        let _ = self.tx.send(Cmd::Unsubscribe(keys.to_vec()));
    }

    fn close(&self) {
        let _ = self.tx.send(Cmd::Close);
    }
}

/// Everything the connection task needs.
#[derive(Clone)]
pub(crate) struct StreamConfig {
    pub url: String,
    /// `IEX` or `SIP`, for status messages.
    pub feed_label: &'static str,
    pub key_id: SecretString,
    pub secret_key: SecretString,
    /// Plan limit on streamed symbols (Basic: 30); `None` = unlimited.
    pub max_symbols: Option<usize>,
    pub backoff_base: Duration,
    pub backoff_cap: Duration,
    /// Client ping interval; also how often staleness is checked.
    pub ping_every: Duration,
    /// Reconnect if nothing (data, pong, ping) arrived for this long.
    pub stale_after: Duration,
    /// Limit for TCP/TLS/WebSocket connect, and for each handshake step.
    pub handshake_timeout: Duration,
}

impl StreamConfig {
    pub(crate) fn new(
        url: String,
        feed_label: &'static str,
        key_id: SecretString,
        secret_key: SecretString,
        max_symbols: Option<usize>,
    ) -> Self {
        Self {
            url,
            feed_label,
            key_id,
            secret_key,
            max_symbols,
            backoff_base: Duration::from_secs(1),
            backoff_cap: Duration::from_secs(30),
            ping_every: Duration::from_secs(30),
            stale_after: Duration::from_secs(90),
            handshake_timeout: Duration::from_secs(10),
        }
    }
}

/// Spawns the connection task and returns its handle immediately.
pub(crate) fn spawn(cfg: StreamConfig, sink: StreamSink) -> Handle {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(run(cfg, sink, rx));
    Handle { tx }
}

// ---------------------------------------------------------------------------
// Wire messages
// ---------------------------------------------------------------------------

/// A decoded server message.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum WsMsg {
    Success(String),
    Error {
        code: i64,
        msg: String,
    },
    Subscription {
        trades: Vec<String>,
        quotes: Vec<String>,
    },
    Data {
        symbol: String,
        update: QuoteUpdate,
    },
    /// Bars, corrections, cancels, statuses, LULDs, imbalances: not mapped.
    Ignored,
}

#[derive(Deserialize)]
#[serde(tag = "T")]
enum Raw {
    #[serde(rename = "success")]
    Success { msg: String },
    #[serde(rename = "error")]
    Error {
        code: i64,
        #[serde(default)]
        msg: String,
    },
    #[serde(rename = "subscription")]
    Subscription {
        #[serde(default)]
        trades: Vec<String>,
        #[serde(default)]
        quotes: Vec<String>,
    },
    #[serde(rename = "t")]
    Trade {
        #[serde(rename = "S")]
        symbol: String,
        p: f64,
        s: f64,
        t: DateTime<Utc>,
    },
    #[serde(rename = "q")]
    Quote {
        #[serde(rename = "S")]
        symbol: String,
        bp: Option<f64>,
        bs: Option<f64>,
        ap: Option<f64>,
        #[serde(rename = "as")]
        ask_size: Option<f64>,
        t: DateTime<Utc>,
    },
    #[serde(other)]
    Other,
}

fn positive(v: Option<f64>) -> Option<f64> {
    v.filter(|p| p.is_finite() && *p > 0.0)
}

impl From<Raw> for WsMsg {
    fn from(raw: Raw) -> Self {
        match raw {
            Raw::Success { msg } => WsMsg::Success(msg),
            Raw::Error { code, msg } => WsMsg::Error { code, msg },
            Raw::Subscription { trades, quotes } => WsMsg::Subscription { trades, quotes },
            Raw::Trade { symbol, p, s, t } => WsMsg::Data {
                symbol,
                update: QuoteUpdate {
                    last: Some(p),
                    last_size: Some(s),
                    volume_increment: Some(s),
                    ts_event: datetime_to_nanos(t),
                    ..QuoteUpdate::default()
                },
            },
            Raw::Quote { symbol, bp, bs, ap, ask_size, t } => {
                // A zero price means "no active bid/ask"; a partial update
                // can't express removal, so that side is left unchanged.
                let bid = positive(bp);
                let ask = positive(ap);
                WsMsg::Data {
                    symbol,
                    update: QuoteUpdate {
                        bid,
                        ask,
                        bid_size: bid.and(bs),
                        ask_size: ask.and(ask_size),
                        ts_event: datetime_to_nanos(t),
                        ..QuoteUpdate::default()
                    },
                }
            }
            Raw::Other => WsMsg::Ignored,
        }
    }
}

/// Decodes one text frame. Elements that don't match the documented schema
/// are skipped (and logged) rather than failing the whole frame.
pub(crate) fn parse_frame(text: &str) -> Vec<WsMsg> {
    // Fast path: the whole frame matches the schema (unknown `T` values
    // decode as `Other`). Otherwise fall back to element-by-element.
    if let Ok(raws) = serde_json::from_str::<Vec<Raw>>(text) {
        return raws.into_iter().map(WsMsg::from).collect();
    }
    let values = match serde_json::from_str::<Value>(text) {
        Ok(Value::Array(items)) => items,
        Ok(other) => vec![other],
        Err(e) => {
            tracing::warn!(error = %e, "Alpaca stream: undecodable frame");
            return Vec::new();
        }
    };
    values
        .into_iter()
        .filter_map(|v| match serde_json::from_value::<Raw>(v) {
            Ok(raw) => Some(WsMsg::from(raw)),
            Err(e) => {
                tracing::debug!(error = %e, "Alpaca stream: skipped message not matching the documented schema");
                None
            }
        })
        .collect()
}

/// `{"action":"auth","key":…,"secret":…}`. Never log the result.
pub(crate) fn auth_message(key_id: &str, secret: &str) -> String {
    json!({ "action": "auth", "key": key_id, "secret": secret }).to_string()
}

/// `{"action":"subscribe"|"unsubscribe","trades":[…],"quotes":[…]}`.
pub(crate) fn subscription_message(action: &str, symbols: &[String]) -> String {
    json!({ "action": action, "trades": symbols, "quotes": symbols }).to_string()
}

/// What an error code means for the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorAction {
    /// Keys or plan are wrong; retrying won't help. Stop the task.
    Stop,
    /// Drop the connection and retry after the maximum backoff.
    RetryAtCap,
    /// Drop the connection and retry with normal backoff.
    Reconnect,
    /// Report it and keep the connection.
    Report,
}

pub(crate) fn error_action(code: i64) -> ErrorAction {
    match code {
        // auth failed, insufficient subscription
        402 | 409 => ErrorAction::Stop,
        // connection limit exceeded (another client holds the only slot)
        406 => ErrorAction::RetryAtCap,
        // not authenticated, auth timeout, slow client
        401 | 404 | 407 => ErrorAction::Reconnect,
        // invalid syntax, already authenticated, symbol limit, bad channel,
        // internal error
        _ => ErrorAction::Report,
    }
}

fn error_text(feed: &str, code: i64, msg: &str) -> String {
    match code {
        402 => format!(
            "Alpaca {feed} stream: authentication failed ({msg}). Check the Alpaca API key in Settings; stream stopped"
        ),
        409 => format!(
            "Alpaca {feed} stream: insufficient subscription ({msg}). The {feed} feed needs Algo Trader Plus; stream stopped"
        ),
        406 => format!(
            "Alpaca {feed} stream: connection limit exceeded ({msg}). Another app may be using this key's stream connection"
        ),
        405 => format!("Alpaca {feed} stream: symbol limit exceeded ({msg})"),
        _ => format!("Alpaca {feed} stream error {code}: {msg}"),
    }
}

// ---------------------------------------------------------------------------
// Subscription bookkeeping
// ---------------------------------------------------------------------------

/// Desired subscriptions in insertion order. With a plan limit, only the
/// first `limit` symbols are streamed; the rest wait until a slot frees up.
#[derive(Debug, Default)]
pub(crate) struct Subscriptions {
    order: Vec<String>,
    keys: HashMap<String, Vec<SecurityKey>>,
    limit: Option<usize>,
}

impl Subscriptions {
    pub(crate) fn new(limit: Option<usize>) -> Self {
        Self { order: Vec::new(), keys: HashMap::new(), limit }
    }

    /// Adds keys; returns how many were ignored as not US equities.
    pub(crate) fn add(&mut self, keys: &[SecurityKey]) -> usize {
        let mut ignored = 0;
        for k in keys {
            let Some(sym) = alpaca_symbol(k) else {
                ignored += 1;
                continue;
            };
            let entry = self.keys.entry(sym.clone()).or_default();
            if entry.is_empty() {
                self.order.push(sym);
            }
            if !entry.contains(k) {
                entry.push(k.clone());
            }
        }
        ignored
    }

    pub(crate) fn remove(&mut self, keys: &[SecurityKey]) {
        for k in keys {
            let Some(sym) = alpaca_symbol(k) else { continue };
            if let Some(v) = self.keys.get_mut(&sym) {
                v.retain(|x| x != k);
                if v.is_empty() {
                    self.keys.remove(&sym);
                    self.order.retain(|s| *s != sym);
                }
            }
        }
    }

    /// Symbols that should be subscribed on the server.
    pub(crate) fn active(&self) -> &[String] {
        let n = self.limit.map_or(self.order.len(), |l| l.min(self.order.len()));
        &self.order[..n]
    }

    /// Desired symbols that don't fit under the plan limit.
    pub(crate) fn overflow(&self) -> usize {
        self.order.len() - self.active().len()
    }

    pub(crate) fn keys_for(&self, symbol: &str) -> &[SecurityKey] {
        self.keys.get(symbol).map_or(&[], Vec::as_slice)
    }
}

// ---------------------------------------------------------------------------
// Backoff
// ---------------------------------------------------------------------------

/// Exponential backoff with "equal jitter": the delay is uniform in
/// `[ceiling/2, ceiling]`, where the ceiling doubles from `base` up to `cap`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Backoff {
    base: Duration,
    cap: Duration,
    attempt: u32,
}

impl Backoff {
    pub(crate) fn new(base: Duration, cap: Duration) -> Self {
        Self { base, cap, attempt: 0 }
    }

    pub(crate) fn ceiling(&self) -> Duration {
        let factor = 1u32.checked_shl(self.attempt.min(31)).unwrap_or(u32::MAX);
        self.base.saturating_mul(factor).min(self.cap)
    }

    pub(crate) fn next_delay(&mut self, rng: &mut impl Rng) -> Duration {
        let ceiling = self.ceiling();
        self.attempt = self.attempt.saturating_add(1);
        let half = ceiling / 2;
        let jitter = rng.random_range(0.0..=1.0_f64);
        half + ceiling.saturating_sub(half).mul_f64(jitter)
    }

    pub(crate) fn cap(&self) -> Duration {
        self.cap
    }

    pub(crate) fn reset(&mut self) {
        self.attempt = 0;
    }
}

// ---------------------------------------------------------------------------
// Connection task
// ---------------------------------------------------------------------------

enum End {
    /// `close()` was called or the handle was dropped.
    Closed,
    /// Retrying can't help (bad keys, plan doesn't include the feed).
    Fatal(String),
    Lost {
        reason: String,
        healthy: bool,
        at_cap: bool,
    },
}

fn lost(reason: impl Into<String>) -> End {
    End::Lost { reason: reason.into(), healthy: false, at_cap: false }
}

struct Ctx<'a> {
    cfg: &'a StreamConfig,
    sink: &'a StreamSink,
    pid: ProviderId,
}

impl Ctx<'_> {
    fn status(&self, connected: bool, message: String) {
        self.sink.send(&self.pid, StreamEvent::Status { connected, message });
    }

    fn overflow_status(&self, subs: &Subscriptions) {
        let n = subs.overflow();
        if n > 0 {
            let limit = subs.limit.unwrap_or_default();
            self.status(
                true,
                format!(
                    "Alpaca {} stream: plan limit is {limit} symbols; {n} subscribed symbol(s) are not streamed",
                    self.cfg.feed_label
                ),
            );
        }
    }
}

async fn run(cfg: StreamConfig, sink: StreamSink, mut rx: UnboundedReceiver<Cmd>) {
    let ctx = Ctx { cfg: &cfg, sink: &sink, pid: ProviderId::new(PROVIDER_ID) };
    let mut subs = Subscriptions::new(cfg.max_symbols);
    let mut backoff = Backoff::new(cfg.backoff_base, cfg.backoff_cap);
    loop {
        match session(&ctx, &mut rx, &mut subs).await {
            End::Closed => {
                ctx.status(false, format!("Alpaca {} stream closed", cfg.feed_label));
                return;
            }
            End::Fatal(message) => {
                tracing::warn!(feed = cfg.feed_label, "Alpaca stream stopped: {message}");
                ctx.status(false, message);
                return;
            }
            End::Lost { reason, healthy, at_cap } => {
                if healthy {
                    backoff.reset();
                }
                let delay = if at_cap { backoff.cap() } else { backoff.next_delay(&mut rand::rng()) };
                tracing::info!(feed = cfg.feed_label, %reason, ?delay, "Alpaca stream disconnected");
                ctx.status(
                    false,
                    format!(
                        "Alpaca {} stream disconnected: {reason}; reconnecting in {:.1} s",
                        cfg.feed_label,
                        delay.as_secs_f64()
                    ),
                );
                if !wait(delay, &mut rx, &mut subs).await {
                    ctx.status(false, format!("Alpaca {} stream closed", cfg.feed_label));
                    return;
                }
            }
        }
    }
}

/// Sleeps for `delay` while still applying subscription changes. Returns
/// `false` if the stream was closed meanwhile.
async fn wait(delay: Duration, rx: &mut UnboundedReceiver<Cmd>, subs: &mut Subscriptions) -> bool {
    let sleep = tokio::time::sleep(delay);
    tokio::pin!(sleep);
    loop {
        tokio::select! {
            () = &mut sleep => return true,
            cmd = rx.recv() => match cmd {
                Some(Cmd::Subscribe(keys)) => {
                    subs.add(&keys);
                }
                Some(Cmd::Unsubscribe(keys)) => subs.remove(&keys),
                Some(Cmd::Close) | None => return false,
            },
        }
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn open(url: &str) -> Result<Ws, tungstenite::Error> {
    let mut req = url.into_client_request()?;
    req.headers_mut().insert(USER_AGENT, HeaderValue::from_static(crate::http::USER_AGENT));
    let (ws, _response) = tokio_tungstenite::connect_async(req).await?;
    Ok(ws)
}

/// Reads frames until one carries a message `pick` accepts. Pings are
/// answered; anything else is ignored.
async fn next_matching<W, R>(write: &mut W, read: &mut R, mut pick: impl FnMut(&WsMsg) -> bool) -> Result<WsMsg, String>
where
    W: Sink<Message, Error = tungstenite::Error> + Unpin,
    R: futures_util::Stream<Item = Result<Message, tungstenite::Error>> + Unpin,
{
    loop {
        let text = match read.next().await {
            None => return Err("server closed the connection".into()),
            Some(Err(e)) => return Err(format!("read error: {e}")),
            Some(Ok(Message::Text(t))) => t.to_string(),
            Some(Ok(Message::Binary(b))) => String::from_utf8_lossy(&b).into_owned(),
            Some(Ok(Message::Ping(p))) => {
                write.send(Message::Pong(p)).await.map_err(|e| format!("write error: {e}"))?;
                continue;
            }
            Some(Ok(Message::Close(frame))) => return Err(close_reason(frame.as_ref())),
            Some(Ok(_)) => continue,
        };
        if let Some(m) = parse_frame(&text).into_iter().find(|m| pick(m)) {
            return Ok(m);
        }
    }
}

fn close_reason(frame: Option<&tungstenite::protocol::CloseFrame>) -> String {
    match frame {
        Some(f) if !f.reason.is_empty() => format!("server closed the connection ({})", f.reason),
        _ => "server closed the connection".into(),
    }
}

/// Brings the server's subscription set in line with `subs.active()`.
async fn sync<W>(write: &mut W, subs: &Subscriptions, server: &mut BTreeSet<String>) -> Result<(), tungstenite::Error>
where
    W: Sink<Message, Error = tungstenite::Error> + Unpin,
{
    let target: BTreeSet<String> = subs.active().iter().cloned().collect();
    let remove: Vec<String> = server.difference(&target).cloned().collect();
    let add: Vec<String> = target.difference(server).cloned().collect();
    for chunk in remove.chunks(SUBSCRIBE_CHUNK) {
        write.send(Message::text(subscription_message("unsubscribe", chunk))).await?;
    }
    for chunk in add.chunks(SUBSCRIBE_CHUNK) {
        write.send(Message::text(subscription_message("subscribe", chunk))).await?;
    }
    *server = target;
    Ok(())
}

async fn session(ctx: &Ctx<'_>, rx: &mut UnboundedReceiver<Cmd>, subs: &mut Subscriptions) -> End {
    let cfg = ctx.cfg;
    let feed = cfg.feed_label;
    let ws = match timeout(cfg.handshake_timeout, open(&cfg.url)).await {
        Ok(Ok(ws)) => ws,
        Ok(Err(e)) => return lost(format!("connect failed: {e}")),
        Err(_) => return lost("connect timed out"),
    };
    let (mut write, mut read) = ws.split();

    // Welcome message, then authenticate.
    let welcome = timeout(
        cfg.handshake_timeout,
        next_matching(&mut write, &mut read, |m| {
            matches!(m, WsMsg::Error { .. }) || *m == WsMsg::Success("connected".into())
        }),
    )
    .await;
    match welcome {
        Ok(Ok(WsMsg::Error { code, msg })) => return handshake_error(feed, code, &msg),
        Ok(Ok(_)) => {}
        Ok(Err(reason)) => return lost(reason),
        Err(_) => return lost("no welcome message from server"),
    }
    let auth = auth_message(cfg.key_id.expose_secret(), cfg.secret_key.expose_secret());
    if let Err(e) = write.send(Message::text(auth)).await {
        return lost(format!("write error: {e}"));
    }
    let authed = timeout(
        cfg.handshake_timeout,
        next_matching(&mut write, &mut read, |m| {
            matches!(m, WsMsg::Error { .. }) || *m == WsMsg::Success("authenticated".into())
        }),
    )
    .await;
    match authed {
        Ok(Ok(WsMsg::Error { code, msg })) => return handshake_error(feed, code, &msg),
        Ok(Ok(_)) => {}
        Ok(Err(reason)) => return lost(reason),
        Err(_) => return lost("authentication timed out"),
    }

    let started = Instant::now();
    let lost_now =
        |reason: String, at_cap: bool| End::Lost { reason, healthy: started.elapsed() >= HEALTHY_AFTER, at_cap };

    let mut server: BTreeSet<String> = BTreeSet::new();
    if let Err(e) = sync(&mut write, subs, &mut server).await {
        return lost_now(format!("write error: {e}"), false);
    }
    ctx.status(true, format!("Alpaca {feed} stream connected"));
    ctx.overflow_status(subs);

    let mut ping = tokio::time::interval_at(Instant::now() + cfg.ping_every, cfg.ping_every);
    let mut last_rx = Instant::now();
    loop {
        tokio::select! {
            frame = read.next() => {
                last_rx = Instant::now();
                let msgs = match frame {
                    None => return lost_now("server closed the connection".into(), false),
                    Some(Err(e)) => return lost_now(format!("read error: {e}"), false),
                    Some(Ok(Message::Text(t))) => parse_frame(&t),
                    Some(Ok(Message::Binary(b))) => parse_frame(&String::from_utf8_lossy(&b)),
                    Some(Ok(Message::Ping(p))) => {
                        if let Err(e) = write.send(Message::Pong(p)).await {
                            return lost_now(format!("write error: {e}"), false);
                        }
                        continue;
                    }
                    Some(Ok(Message::Close(frame))) => return lost_now(close_reason(frame.as_ref()), false),
                    Some(Ok(_)) => continue,
                };
                for msg in msgs {
                    match msg {
                        WsMsg::Data { symbol, update } => {
                            for key in subs.keys_for(&symbol) {
                                ctx.sink.send(&ctx.pid, StreamEvent::Quote { key: key.clone(), update: update.clone() });
                            }
                        }
                        WsMsg::Error { code, msg } => {
                            let text = error_text(feed, code, &msg);
                            match error_action(code) {
                                ErrorAction::Stop => return End::Fatal(text),
                                ErrorAction::RetryAtCap => return lost_now(text, true),
                                ErrorAction::Reconnect => return lost_now(text, false),
                                ErrorAction::Report => {
                                    tracing::warn!(feed, code, "Alpaca stream error: {msg}");
                                    ctx.status(true, text);
                                }
                            }
                        }
                        WsMsg::Subscription { trades, quotes } => {
                            tracing::debug!(feed, trades = trades.len(), quotes = quotes.len(), "Alpaca subscription updated");
                        }
                        WsMsg::Success(_) | WsMsg::Ignored => {}
                    }
                }
            }
            cmd = rx.recv() => {
                let before = subs.overflow();
                match cmd {
                    Some(Cmd::Subscribe(keys)) => {
                        let ignored = subs.add(&keys);
                        if ignored > 0 {
                            tracing::debug!(ignored, "Alpaca stream: ignored keys that aren't US equities");
                        }
                    }
                    Some(Cmd::Unsubscribe(keys)) => subs.remove(&keys),
                    Some(Cmd::Close) | None => {
                        let _ = timeout(Duration::from_secs(1), write.send(Message::Close(None))).await;
                        return End::Closed;
                    }
                }
                if let Err(e) = sync(&mut write, subs, &mut server).await {
                    return lost_now(format!("write error: {e}"), false);
                }
                if subs.overflow() != before {
                    ctx.overflow_status(subs);
                }
            }
            _ = ping.tick() => {
                if last_rx.elapsed() >= cfg.stale_after {
                    return lost_now(format!("no data from server for {} s", cfg.stale_after.as_secs()), false);
                }
                if let Err(e) = write.send(Message::Ping(tungstenite::Bytes::new())).await {
                    return lost_now(format!("write error: {e}"), false);
                }
            }
        }
    }
}

fn handshake_error(feed: &str, code: i64, msg: &str) -> End {
    let text = error_text(feed, code, msg);
    match error_action(code) {
        ErrorAction::Stop => End::Fatal(text),
        ErrorAction::RetryAtCap => End::Lost { reason: text, healthy: false, at_cap: true },
        ErrorAction::Reconnect | ErrorAction::Report => lost(text),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use meridian_provider::EventSink;
    use parking_lot::Mutex;
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use tokio::net::TcpListener;

    use super::*;

    fn ns(s: &str) -> i64 {
        datetime_to_nanos(DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc))
    }

    #[test]
    fn parses_documented_session() {
        let lines: Vec<&str> = include_str!("../tests/fixtures/ws_session.txt").lines().collect();
        assert_eq!(parse_frame(lines[0]), vec![WsMsg::Success("connected".into())]);
        assert_eq!(parse_frame(lines[1]), vec![WsMsg::Success("authenticated".into())]);
        assert_eq!(
            parse_frame(lines[2]),
            vec![WsMsg::Subscription { trades: vec!["AAPL".into()], quotes: vec!["AMD".into(), "CLDR".into()] }]
        );

        let quote = parse_frame(lines[3]);
        assert_eq!(
            quote,
            vec![WsMsg::Data {
                symbol: "AMD".into(),
                update: QuoteUpdate {
                    bid: Some(91.95),
                    ask: Some(91.98),
                    bid_size: Some(2.0),
                    ask_size: Some(1.0),
                    ts_event: ns("2023-04-06T11:54:21.670905508Z"),
                    ..QuoteUpdate::default()
                },
            }]
        );

        let trades = parse_frame(lines[4]);
        assert_eq!(trades.len(), 3);
        assert_eq!(
            trades[2],
            WsMsg::Data {
                symbol: "AAPL".into(),
                update: QuoteUpdate {
                    last: Some(162.91),
                    last_size: Some(71.0),
                    volume_increment: Some(71.0),
                    ts_event: ns("2023-04-06T11:54:26.83915973Z"),
                    ..QuoteUpdate::default()
                },
            }
        );
    }

    #[test]
    fn parses_errors_and_ignores_other_channels() {
        let errors: Vec<WsMsg> =
            include_str!("../tests/fixtures/ws_errors.txt").lines().flat_map(parse_frame).collect();
        assert_eq!(
            errors,
            vec![
                WsMsg::Error { code: 406, msg: "connection limit exceeded".into() },
                WsMsg::Error { code: 402, msg: "auth failed".into() },
                WsMsg::Error { code: 405, msg: "symbol limit exceeded".into() },
                WsMsg::Error { code: 409, msg: "insufficient subscription".into() },
            ]
        );
        assert_eq!(error_action(402), ErrorAction::Stop);
        assert_eq!(error_action(409), ErrorAction::Stop);
        assert_eq!(error_action(406), ErrorAction::RetryAtCap);
        assert_eq!(error_action(404), ErrorAction::Reconnect);
        assert_eq!(error_action(405), ErrorAction::Report);

        let other: Vec<WsMsg> =
            include_str!("../tests/fixtures/ws_other_messages.txt").lines().flat_map(parse_frame).collect();
        assert_eq!(other, vec![WsMsg::Ignored; 4]);

        assert!(parse_frame("not json").is_empty());
        // A malformed element is skipped, the rest of the frame survives.
        assert_eq!(
            parse_frame(r#"[{"T":"t","S":"X"},{"T":"success","msg":"connected"}]"#),
            vec![WsMsg::Success("connected".into())]
        );
    }

    #[test]
    fn zero_quote_prices_leave_sides_unchanged() {
        let m = parse_frame(r#"[{"T":"q","S":"X","bp":0,"bs":0,"ap":1.5,"as":300,"t":"2026-01-02T15:00:00Z"}]"#);
        let WsMsg::Data { update, .. } = &m[0] else { panic!("{m:?}") };
        assert_eq!((update.bid, update.bid_size), (None, None));
        assert_eq!((update.ask, update.ask_size), (Some(1.5), Some(300.0)));
    }

    #[test]
    fn auth_and_subscribe_shapes() {
        let auth: Value = serde_json::from_str(&auth_message("TEST-KEY", "TEST-SECRET")).unwrap();
        assert_eq!(auth, json!({"action": "auth", "key": "TEST-KEY", "secret": "TEST-SECRET"}));
        let sub: Value =
            serde_json::from_str(&subscription_message("subscribe", &["AAPL".into(), "BRK.B".into()])).unwrap();
        assert_eq!(sub, json!({"action": "subscribe", "trades": ["AAPL", "BRK.B"], "quotes": ["AAPL", "BRK.B"]}));
        let unsub: Value = serde_json::from_str(&subscription_message("unsubscribe", &["AAPL".into()])).unwrap();
        assert_eq!(unsub, json!({"action": "unsubscribe", "trades": ["AAPL"], "quotes": ["AAPL"]}));
    }

    #[test]
    fn backoff_doubles_to_cap_with_jitter_and_resets() {
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(30));
        let mut rng = StdRng::seed_from_u64(7);
        let expected = [1, 2, 4, 8, 16, 30, 30, 30];
        for secs in expected {
            let ceiling = Duration::from_secs(secs);
            assert_eq!(b.ceiling(), ceiling);
            let d = b.next_delay(&mut rng);
            assert!(d >= ceiling / 2 && d <= ceiling, "{d:?} vs {ceiling:?}");
        }
        for _ in 0..100 {
            assert!(b.next_delay(&mut rng) <= Duration::from_secs(30));
        }
        b.reset();
        assert_eq!(b.ceiling(), Duration::from_secs(1));
        assert_eq!(b.cap(), Duration::from_secs(30));
    }

    #[test]
    fn subscriptions_respect_plan_limit() {
        let mut s = Subscriptions::new(Some(2));
        let ignored = s.add(&[
            SecurityKey::equity("AAPL"),
            SecurityKey::equity("MSFT"),
            SecurityKey::equity("NVDA"),
            SecurityKey::currency("EURUSD"),
        ]);
        assert_eq!(ignored, 1);
        assert_eq!(s.active(), ["AAPL", "MSFT"]);
        assert_eq!(s.overflow(), 1);
        // Two keys for one symbol share a slot.
        s.add(&["AAPL Equity".parse().unwrap()]);
        assert_eq!(s.keys_for("AAPL").len(), 2);
        assert_eq!(s.overflow(), 1);
        // Freeing a slot promotes the waiting symbol.
        s.remove(&[SecurityKey::equity("MSFT")]);
        assert_eq!(s.active(), ["AAPL", "NVDA"]);
        assert_eq!(s.overflow(), 0);
        s.remove(&[SecurityKey::equity("AAPL")]);
        assert_eq!(s.active(), ["AAPL", "NVDA"]);
        s.remove(&["AAPL Equity".parse().unwrap()]);
        assert_eq!(s.active(), ["NVDA"]);
        assert!(s.keys_for("AAPL").is_empty());

        let unlimited = {
            let mut u = Subscriptions::new(None);
            u.add(&[SecurityKey::equity("A"), SecurityKey::equity("B")]);
            u
        };
        assert_eq!(unlimited.active().len(), 2);
    }

    // --- Local WebSocket server tests (no network beyond 127.0.0.1) ---

    #[derive(Default)]
    struct Collect {
        events: Mutex<Vec<StreamEvent>>,
    }

    impl EventSink for Collect {
        fn send(&self, _provider: &ProviderId, event: StreamEvent) {
            self.events.lock().push(event);
        }
    }

    impl Collect {
        async fn wait_for(&self, what: &str, pred: impl Fn(&[StreamEvent]) -> bool) {
            let ok = timeout(Duration::from_secs(5), async {
                loop {
                    if pred(&self.events.lock()) {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await;
            assert!(ok.is_ok(), "timed out waiting for {what}; events: {:?}", self.events.lock());
        }
    }

    fn test_config(port: u16) -> StreamConfig {
        let mut cfg = StreamConfig::new(
            format!("ws://127.0.0.1:{port}/v2/iex"),
            "IEX",
            SecretString::from("TEST-KEY-ID"),
            SecretString::from("TEST-SECRET"),
            Some(30),
        );
        cfg.backoff_base = Duration::from_millis(10);
        cfg.backoff_cap = Duration::from_millis(50);
        cfg
    }

    type ServerWs = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

    async fn next_text(ws: &mut ServerWs) -> Value {
        loop {
            match ws.next().await.expect("client closed").expect("read") {
                Message::Text(t) => return serde_json::from_str(&t).unwrap(),
                Message::Close(_) => panic!("client closed"),
                _ => {}
            }
        }
    }

    /// Accepts one client and runs the documented welcome/auth exchange.
    async fn accept_and_auth(listener: &TcpListener) -> ServerWs {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        ws.send(Message::text(r#"[{"T":"success","msg":"connected"}]"#)).await.unwrap();
        let auth = next_text(&mut ws).await;
        assert_eq!(auth, json!({"action": "auth", "key": "TEST-KEY-ID", "secret": "TEST-SECRET"}));
        ws.send(Message::text(r#"[{"T":"success","msg":"authenticated"}]"#)).await.unwrap();
        ws
    }

    fn quotes_for<'a>(events: &'a [StreamEvent], symbol: &str) -> Vec<&'a QuoteUpdate> {
        events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::Quote { key, update } if key.symbol == symbol => Some(update),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn streams_trades_and_quotes_then_closes() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let sink = Arc::new(Collect::default());
        let handle = spawn(test_config(port), sink.clone());
        handle.subscribe(&[SecurityKey::equity("AAPL"), SecurityKey::equity("AMD")]);

        let mut ws = accept_and_auth(&listener).await;
        let mut subscribed: Vec<String> = Vec::new();
        while subscribed.len() < 2 {
            let sub = next_text(&mut ws).await;
            assert_eq!(sub["action"], "subscribe");
            assert_eq!(sub["trades"], sub["quotes"]);
            subscribed.extend(sub["trades"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()));
        }
        subscribed.sort();
        assert_eq!(subscribed, ["AAPL", "AMD"]);

        for line in include_str!("../tests/fixtures/ws_session.txt").lines().skip(2) {
            ws.send(Message::text(line)).await.unwrap();
        }
        sink.wait_for("connected status", |ev| {
            ev.iter().any(|e| matches!(e, StreamEvent::Status { connected: true, .. }))
        })
        .await;
        sink.wait_for("all data", |ev| quotes_for(ev, "AAPL").len() == 3 && quotes_for(ev, "AMD").len() == 2).await;
        {
            let events = sink.events.lock();
            let aapl = quotes_for(&events, "AAPL");
            assert_eq!(aapl[0].last, Some(162.92));
            assert_eq!(aapl[2].volume_increment, Some(71.0));
            let amd = quotes_for(&events, "AMD");
            assert_eq!(amd[1].bid, Some(91.9));
            // CLDR is in the server's subscription message but not ours.
            assert!(quotes_for(&events, "CLDR").is_empty());
        }

        handle.unsubscribe(&[SecurityKey::equity("AMD")]);
        let unsub = next_text(&mut ws).await;
        assert_eq!(unsub, json!({"action": "unsubscribe", "trades": ["AMD"], "quotes": ["AMD"]}));

        handle.close();
        sink.wait_for("closed status", |ev| {
            matches!(ev.last(), Some(StreamEvent::Status { connected: false, message }) if message.contains("closed"))
        })
        .await;
    }

    #[tokio::test]
    async fn reconnects_and_resubscribes() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let sink = Arc::new(Collect::default());
        let handle = spawn(test_config(port), sink.clone());
        handle.subscribe(&[SecurityKey::equity("AAPL")]);

        let mut first = accept_and_auth(&listener).await;
        assert_eq!(next_text(&mut first).await["trades"], json!(["AAPL"]));
        drop(first);

        let mut second = accept_and_auth(&listener).await;
        assert_eq!(next_text(&mut second).await["trades"], json!(["AAPL"]));
        sink.wait_for("disconnect then reconnect", |ev| {
            let statuses: Vec<bool> = ev
                .iter()
                .filter_map(|e| match e {
                    StreamEvent::Status { connected, .. } => Some(*connected),
                    StreamEvent::Quote { .. } => None,
                })
                .collect();
            statuses.starts_with(&[true, false, true])
        })
        .await;
        drop(handle);
        // Dropping the handle stops the task, which closes the socket.
        let end = timeout(Duration::from_secs(5), async {
            loop {
                match second.next().await {
                    None | Some(Err(_) | Ok(Message::Close(_))) => return,
                    Some(Ok(_)) => {}
                }
            }
        })
        .await;
        assert!(end.is_ok(), "client did not close after the handle was dropped");
    }

    #[tokio::test]
    async fn auth_failure_stops_without_retrying() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let sink = Arc::new(Collect::default());
        let _handle = spawn(test_config(port), sink.clone());

        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        ws.send(Message::text(r#"[{"T":"success","msg":"connected"}]"#)).await.unwrap();
        let _auth = next_text(&mut ws).await;
        ws.send(Message::text(r#"[{"T":"error","code":402,"msg":"auth failed"}]"#)).await.unwrap();

        sink.wait_for("auth failure status", |ev| {
            ev.iter().any(|e| {
                matches!(e, StreamEvent::Status { connected: false, message } if message.contains("authentication failed"))
            })
        })
        .await;
        // No reconnect attempt, even though the backoff in this config is 10 ms.
        assert!(timeout(Duration::from_millis(300), listener.accept()).await.is_err());
        let events = sink.events.lock();
        assert!(events.iter().all(|e| !matches!(e, StreamEvent::Status { connected: true, .. })));
        // The key never appears in a status message.
        assert!(events.iter().all(|e| !format!("{e:?}").contains("TEST-SECRET")));
    }

    #[tokio::test]
    async fn plan_limit_reported_and_enforced() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let sink = Arc::new(Collect::default());
        let mut cfg = test_config(port);
        cfg.max_symbols = Some(1);
        let handle = spawn(cfg, sink.clone());
        handle.subscribe(&[SecurityKey::equity("AAPL"), SecurityKey::equity("MSFT")]);

        let mut ws = accept_and_auth(&listener).await;
        assert_eq!(next_text(&mut ws).await["trades"], json!(["AAPL"]));
        sink.wait_for("limit status", |ev| {
            ev.iter().any(|e| matches!(e, StreamEvent::Status { message, .. } if message.contains("plan limit is 1")))
        })
        .await;
        handle.unsubscribe(&[SecurityKey::equity("AAPL")]);
        assert_eq!(next_text(&mut ws).await["action"], "unsubscribe");
        let sub = next_text(&mut ws).await;
        assert_eq!(sub, json!({"action": "subscribe", "trades": ["MSFT"], "quotes": ["MSFT"]}));
        handle.close();
    }
}

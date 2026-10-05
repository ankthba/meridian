//! WebSocket v2 `ticker` feed (`wss://ws.kraken.com/v2`).
//!
//! `connect` spawns one task per connection. The task keeps the desired
//! subscription set, connects only while that set is non-empty, reconnects
//! with exponential backoff and jitter, re-subscribes everything after a
//! reconnect, and reports state changes through the sink as
//! `StreamEvent::Status`. Kraken closes connections after about a minute of
//! inactivity, so the task sends an application-level `ping` every 20 s.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use meridian_provider::{StreamHandle, StreamSink};
use meridian_types::{Clock, ProviderId, SecurityKey, StreamEvent, SystemClock, UnixNanos};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, header};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::dto::{WsFrame, WsStatus, WsTicker};
use crate::symbols::{resolve, statically_covered};
use crate::{Inner, PROVIDER_ID, normalize};

pub(crate) const WS_URL: &str = "wss://ws.kraken.com/v2";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Application-level ping cadence (keeps the connection alive).
const PING_EVERY: Duration = Duration::from_secs(20);
/// No inbound frame (data, heartbeat, or pong) for this long = dead.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// A connection that stayed up this long resets the backoff.
const HEALTHY_AFTER: Duration = Duration::from_secs(30);
const BACKOFF_BASE: Duration = Duration::from_secs(1);
const BACKOFF_CAP: Duration = Duration::from_secs(30);

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug)]
pub(crate) enum Cmd {
    Subscribe(Vec<SecurityKey>),
    Unsubscribe(Vec<SecurityKey>),
    Close,
}

/// Returned by `connect`. Dropping it closes the channel, which stops the
/// task just like `close()`.
pub(crate) struct Handle {
    tx: UnboundedSender<Cmd>,
}

impl Handle {
    fn send(&self, cmd: Cmd) {
        if self.tx.send(cmd).is_err() {
            tracing::debug!("kraken stream task already stopped");
        }
    }
}

impl StreamHandle for Handle {
    fn subscribe(&self, keys: &[SecurityKey]) {
        self.send(Cmd::Subscribe(keys.to_vec()));
    }

    fn unsubscribe(&self, keys: &[SecurityKey]) {
        self.send(Cmd::Unsubscribe(keys.to_vec()));
    }

    fn close(&self) {
        self.send(Cmd::Close);
    }
}

pub(crate) fn spawn(inner: Arc<Inner>, sink: StreamSink) -> Handle {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(run(inner, sink, rx));
    Handle { tx }
}

/// Reconnect delay: nominal `1 s * 2^attempt` capped at 30 s, scaled into
/// `[nominal / 2, nominal]` by `jitter` in `[0, 1]` ("equal jitter").
pub(crate) fn backoff_delay(attempt: u32, jitter: f64) -> Duration {
    let nominal = (BACKOFF_BASE.as_secs_f64() * 2f64.powi(attempt.min(16) as i32))
        .min(BACKOFF_CAP.as_secs_f64());
    Duration::from_secs_f64(nominal * (0.5 + 0.5 * jitter.clamp(0.0, 1.0)))
}

#[derive(Debug, Default)]
pub(crate) struct Backoff {
    attempt: u32,
}

impl Backoff {
    pub(crate) fn next_delay(&mut self, jitter: f64) -> Duration {
        let d = backoff_delay(self.attempt, jitter);
        self.attempt = self.attempt.saturating_add(1);
        d
    }

    pub(crate) fn reset(&mut self) {
        self.attempt = 0;
    }
}

/// Sends events and reports connection state only when it changes.
struct Reporter {
    sink: StreamSink,
    id: ProviderId,
    connected: Option<bool>,
}

impl Reporter {
    fn new(sink: StreamSink) -> Self {
        Self {
            sink,
            id: ProviderId::new(PROVIDER_ID),
            connected: None,
        }
    }

    fn status(&mut self, connected: bool, message: String) {
        if self.connected == Some(connected) {
            tracing::debug!(connected, %message, "kraken stream status unchanged");
            return;
        }
        self.connected = Some(connected);
        self.sink
            .send(&self.id, StreamEvent::Status { connected, message });
    }

    fn event(&self, event: StreamEvent) {
        self.sink.send(&self.id, event);
    }
}

/// One subscribed pair.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Sub {
    pub key: SecurityKey,
    /// Pair price decimals, used to round the derived 24 h reference price.
    pub decimals: Option<u8>,
}

/// Subscribe/unsubscribe request for the `ticker` channel. `event_trigger`
/// is the documented default (`trades`): one update per trade, like the
/// Coinbase ticker.
pub(crate) fn subscription_msg(method: &str, symbols: &[String], req_id: u64) -> String {
    let mut params =
        serde_json::json!({ "channel": "ticker", "symbol": symbols, "event_trigger": "trades" });
    if method == "subscribe" {
        params["snapshot"] = serde_json::Value::Bool(true);
    }
    serde_json::json!({ "method": method, "params": params, "req_id": req_id }).to_string()
}

pub(crate) fn ping_msg(req_id: u64) -> String {
    serde_json::json!({ "method": "ping", "req_id": req_id }).to_string()
}

/// What a text frame means for the session.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Inbound {
    Quotes(Vec<StreamEvent>),
    /// `status` channel: the exchange's system state (`online`,
    /// `maintenance`, `cancel_only`, `post_only`).
    System(String),
    /// Heartbeats, pongs, acks, and anything else.
    Nothing,
}

pub(crate) fn handle_text(
    text: &str,
    active: &BTreeMap<String, Sub>,
    received_at: UnixNanos,
) -> Inbound {
    let frame: WsFrame = match serde_json::from_str(text) {
        Ok(f) => f,
        Err(e) => {
            tracing::debug!(error = %e, "kraken: dropping unparseable message");
            return Inbound::Nothing;
        }
    };
    if let Some(method) = frame.method.as_deref() {
        if frame.success == Some(false) {
            tracing::warn!(
                method,
                error = frame.error.as_deref().unwrap_or(""),
                "kraken request failed"
            );
        }
        return Inbound::Nothing;
    }
    match (frame.channel.as_deref(), frame.data) {
        (Some("ticker"), Some(data)) => {
            let items: Vec<WsTicker> = match serde_json::from_value(data) {
                Ok(v) => v,
                Err(e) => {
                    tracing::debug!(error = %e, "kraken: dropping malformed ticker frame");
                    return Inbound::Nothing;
                }
            };
            let events = items
                .iter()
                .filter_map(|t| {
                    let Some(sub) = active.get(&t.symbol) else {
                        tracing::debug!(symbol = %t.symbol, "kraken: ticker for an untracked pair");
                        return None;
                    };
                    match normalize::ws_ticker(t, sub.decimals, received_at) {
                        Ok(update) => Some(StreamEvent::Quote { key: sub.key.clone(), update }),
                        Err(e) => {
                            tracing::debug!(error = %e, symbol = %t.symbol, "kraken: dropping malformed ticker");
                            None
                        }
                    }
                })
                .collect();
            Inbound::Quotes(events)
        }
        (Some("status"), Some(data)) => serde_json::from_value::<Vec<WsStatus>>(data)
            .ok()
            .and_then(|v| v.into_iter().find_map(|s| s.system))
            .map_or(Inbound::Nothing, Inbound::System),
        _ => Inbound::Nothing,
    }
}

/// Applies a command while disconnected. `false` means stop.
fn apply(desired: &mut BTreeSet<SecurityKey>, cmd: Cmd) -> bool {
    match cmd {
        Cmd::Subscribe(keys) => {
            for k in keys {
                if statically_covered(&k) {
                    desired.insert(k);
                } else {
                    tracing::warn!(key = %k, "kraken: key not covered; ignoring subscription");
                }
            }
            true
        }
        Cmd::Unsubscribe(keys) => {
            for k in &keys {
                desired.remove(k);
            }
            true
        }
        Cmd::Close => false,
    }
}

/// Maps desired keys to WS pair names, dropping keys that aren't pairs.
fn resolve_all(inner: &Inner, desired: &mut BTreeSet<SecurityKey>) -> BTreeMap<String, Sub> {
    let cat = inner.catalog_now();
    let mut active = BTreeMap::new();
    desired.retain(|k| {
        let Some(p) = resolve(k, cat.as_deref()) else {
            tracing::warn!(key = %k, "kraken: not a Kraken pair; ignoring subscription");
            return false;
        };
        active.insert(
            p.symbol,
            Sub {
                key: k.clone(),
                decimals: p.price_decimals,
            },
        );
        true
    });
    active
}

enum SessionEnd {
    /// `close()` or the handle was dropped.
    Closed,
    /// Every subscription was removed; reconnect when there are new ones.
    Idle,
    /// Lost or failed connection; reconnect after a backoff.
    Dropped(String),
}

async fn run(inner: Arc<Inner>, sink: StreamSink, mut rx: UnboundedReceiver<Cmd>) {
    let mut rep = Reporter::new(sink);
    let mut desired: BTreeSet<SecurityKey> = BTreeSet::new();
    let mut backoff = Backoff::default();
    let mut catalog_tried = false;
    loop {
        if desired.is_empty() {
            let keep_going = rx.recv().await.is_some_and(|cmd| apply(&mut desired, cmd));
            if !keep_going {
                break;
            }
            continue;
        }
        if !catalog_tried && inner.catalog_now().is_none() {
            // Best effort: with the pair list, unknown pairs are filtered
            // out before subscribing and derived prices are rounded to the
            // pair's decimals.
            catalog_tried = true;
            if let Err(e) = inner.catalog().await {
                tracing::debug!(error = %e, "kraken: pair list unavailable; subscribing by symbol");
            }
        }
        let active = resolve_all(&inner, &mut desired);
        if active.is_empty() {
            continue;
        }
        let started = Instant::now();
        let end = match connect(&inner.ws_url).await {
            Ok(ws) => {
                rep.status(true, format!("connected to {}", inner.ws_url));
                session(ws, &inner, &mut desired, active, &mut rx, &rep).await
            }
            Err(e) => SessionEnd::Dropped(format!("connect failed: {e}")),
        };
        match end {
            SessionEnd::Closed => break,
            SessionEnd::Idle => {
                rep.status(false, "idle: no subscriptions".to_owned());
                backoff.reset();
            }
            SessionEnd::Dropped(reason) => {
                if started.elapsed() >= HEALTHY_AFTER {
                    backoff.reset();
                }
                let delay = backoff.next_delay(rand::random::<f64>());
                tracing::warn!(%reason, ?delay, "kraken stream disconnected");
                rep.status(
                    false,
                    format!("{reason}; reconnecting in {:.1} s", delay.as_secs_f64()),
                );
                if !wait(delay, &mut rx, &mut desired).await {
                    break;
                }
            }
        }
    }
    rep.status(false, "closed".to_owned());
}

/// Sleeps for `delay` while still applying commands. `false` means stop.
async fn wait(
    delay: Duration,
    rx: &mut UnboundedReceiver<Cmd>,
    desired: &mut BTreeSet<SecurityKey>,
) -> bool {
    let sleep = tokio::time::sleep(delay);
    tokio::pin!(sleep);
    loop {
        tokio::select! {
            () = &mut sleep => return true,
            cmd = rx.recv() => {
                if !cmd.is_some_and(|c| apply(desired, c)) {
                    return false;
                }
            }
        }
    }
}

async fn connect(url: &str) -> Result<Ws, String> {
    let mut req = url.into_client_request().map_err(|e| e.to_string())?;
    req.headers_mut().insert(
        header::USER_AGENT,
        HeaderValue::from_static(crate::http::USER_AGENT),
    );
    match tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(req)).await {
        Ok(Ok((ws, _response))) => Ok(ws),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!("timed out after {} s", CONNECT_TIMEOUT.as_secs())),
    }
}

fn close_reason(frame: Option<&CloseFrame>) -> String {
    match frame {
        Some(f) => format!(
            "server closed the connection ({} {})",
            u16::from(f.code),
            f.reason
        ),
        None => "server closed the connection".to_owned(),
    }
}

async fn session(
    ws: Ws,
    inner: &Inner,
    desired: &mut BTreeSet<SecurityKey>,
    mut active: BTreeMap<String, Sub>,
    rx: &mut UnboundedReceiver<Cmd>,
    rep: &Reporter,
) -> SessionEnd {
    let (mut write, mut read) = ws.split();
    let mut req_id: u64 = 1;
    let symbols: Vec<String> = active.keys().cloned().collect();
    if let Err(e) = write
        .send(Message::text(subscription_msg(
            "subscribe",
            &symbols,
            req_id,
        )))
        .await
    {
        return SessionEnd::Dropped(format!("subscribe failed: {e}"));
    }
    let mut ping = tokio::time::interval_at(Instant::now() + PING_EVERY, PING_EVERY);
    let mut last_seen = Instant::now();
    let mut system: Option<String> = None;
    loop {
        tokio::select! {
            cmd = rx.recv() => {
                let Some(cmd) = cmd else {
                    let _ = write.send(Message::Close(None)).await;
                    return SessionEnd::Closed;
                };
                match cmd {
                    Cmd::Close => {
                        let _ = write.send(Message::Close(None)).await;
                        return SessionEnd::Closed;
                    }
                    Cmd::Subscribe(keys) => {
                        let cat = inner.catalog_now();
                        let mut added = Vec::new();
                        for k in keys {
                            if !statically_covered(&k) {
                                tracing::warn!(key = %k, "kraken: key not covered; ignoring subscription");
                                continue;
                            }
                            let Some(p) = resolve(&k, cat.as_deref()) else {
                                tracing::warn!(key = %k, "kraken: not a Kraken pair; ignoring subscription");
                                continue;
                            };
                            desired.insert(k.clone());
                            if !active.contains_key(&p.symbol) {
                                active.insert(p.symbol.clone(), Sub { key: k, decimals: p.price_decimals });
                                added.push(p.symbol);
                            }
                        }
                        req_id += 1;
                        if !added.is_empty()
                            && let Err(e) = write.send(Message::text(subscription_msg("subscribe", &added, req_id))).await
                        {
                            return SessionEnd::Dropped(format!("subscribe failed: {e}"));
                        }
                    }
                    Cmd::Unsubscribe(keys) => {
                        let mut removed = Vec::new();
                        for k in &keys {
                            desired.remove(k);
                            active.retain(|symbol, sub| {
                                let keep = sub.key != *k;
                                if !keep {
                                    removed.push(symbol.clone());
                                }
                                keep
                            });
                        }
                        if active.is_empty() {
                            let _ = write.send(Message::Close(None)).await;
                            return SessionEnd::Idle;
                        }
                        req_id += 1;
                        if !removed.is_empty()
                            && let Err(e) = write.send(Message::text(subscription_msg("unsubscribe", &removed, req_id))).await
                        {
                            return SessionEnd::Dropped(format!("unsubscribe failed: {e}"));
                        }
                    }
                }
            }
            msg = read.next() => {
                let msg = match msg {
                    None => return SessionEnd::Dropped("connection ended".to_owned()),
                    Some(Err(e)) => return SessionEnd::Dropped(format!("read error: {e}")),
                    Some(Ok(m)) => m,
                };
                last_seen = Instant::now();
                match msg {
                    Message::Text(text) => match handle_text(text.as_str(), &active, SystemClock.now()) {
                        Inbound::Quotes(events) => {
                            for event in events {
                                rep.event(event);
                            }
                        }
                        Inbound::System(state) => {
                            // The first `online` after connecting is routine;
                            // any other change is worth surfacing.
                            let routine = system.is_none() && state == "online";
                            if system.as_deref() != Some(state.as_str()) && !routine {
                                rep.event(StreamEvent::Status { connected: true, message: format!("Kraken system status: {state}") });
                            }
                            system = Some(state);
                        }
                        Inbound::Nothing => {}
                    },
                    Message::Ping(payload) => {
                        if let Err(e) = write.send(Message::Pong(payload)).await {
                            return SessionEnd::Dropped(format!("pong failed: {e}"));
                        }
                    }
                    Message::Close(frame) => return SessionEnd::Dropped(close_reason(frame.as_ref())),
                    Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
                }
            }
            _ = ping.tick() => {
                if last_seen.elapsed() >= IDLE_TIMEOUT {
                    return SessionEnd::Dropped(format!("no data for {} s", IDLE_TIMEOUT.as_secs()));
                }
                req_id += 1;
                if let Err(e) = write.send(Message::text(ping_msg(req_id))).await {
                    return SessionEnd::Dropped(format!("ping failed: {e}"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active() -> BTreeMap<String, Sub> {
        BTreeMap::from([
            (
                "BTC/USD".to_owned(),
                Sub {
                    key: SecurityKey::currency("BTCUSD"),
                    decimals: Some(1),
                },
            ),
            (
                "ETH/USD".to_owned(),
                Sub {
                    key: SecurityKey::currency("ETHUSD"),
                    decimals: Some(2),
                },
            ),
        ])
    }

    #[test]
    fn subscribe_message_shape() {
        let m: serde_json::Value = serde_json::from_str(&subscription_msg(
            "subscribe",
            &["BTC/USD".into(), "ETH/USD".into()],
            7,
        ))
        .unwrap();
        assert_eq!(
            m,
            serde_json::json!({
                "method": "subscribe",
                "params": { "channel": "ticker", "symbol": ["BTC/USD", "ETH/USD"], "event_trigger": "trades", "snapshot": true },
                "req_id": 7
            })
        );
        let m: serde_json::Value =
            serde_json::from_str(&subscription_msg("unsubscribe", &["BTC/USD".into()], 8)).unwrap();
        assert_eq!(
            m,
            serde_json::json!({
                "method": "unsubscribe",
                "params": { "channel": "ticker", "symbol": ["BTC/USD"], "event_trigger": "trades" },
                "req_id": 8
            })
        );
        let m: serde_json::Value = serde_json::from_str(&ping_msg(9)).unwrap();
        assert_eq!(m, serde_json::json!({ "method": "ping", "req_id": 9 }));
    }

    #[test]
    fn ticker_frames_become_quote_events() {
        let Inbound::Quotes(events) = handle_text(
            include_str!("../tests/fixtures/ws_ticker_snapshot.json"),
            &active(),
            0,
        ) else {
            panic!("expected quotes");
        };
        assert_eq!(events.len(), 1);
        let StreamEvent::Quote { key, update } = &events[0] else {
            panic!("expected a quote")
        };
        assert_eq!(*key, SecurityKey::currency("BTCUSD"));
        assert_eq!(update.last, Some(85599.2));
        assert_eq!(update.prev_close, Some(85309.2));

        let Inbound::Quotes(events) = handle_text(
            include_str!("../tests/fixtures/ws_ticker_update.json"),
            &active(),
            0,
        ) else {
            panic!("expected quotes");
        };
        let StreamEvent::Quote { key, .. } = &events[0] else {
            panic!("expected a quote")
        };
        assert_eq!(*key, SecurityKey::currency("ETHUSD"));
        // Untracked pair -> no events.
        assert_eq!(
            handle_text(
                include_str!("../tests/fixtures/ws_ticker_update.json"),
                &BTreeMap::new(),
                0
            ),
            Inbound::Quotes(vec![])
        );
    }

    #[test]
    fn control_frames() {
        assert_eq!(
            handle_text(
                include_str!("../tests/fixtures/ws_status.json"),
                &active(),
                0
            ),
            Inbound::System("online".into())
        );
        for text in [
            include_str!("../tests/fixtures/ws_heartbeat.json"),
            include_str!("../tests/fixtures/ws_pong.json"),
            include_str!("../tests/fixtures/ws_subscribe_ack.json"),
            include_str!("../tests/fixtures/ws_subscribe_error.json"),
            "not json",
            r#"{"channel":"ticker","type":"update","data":"garbage"}"#,
        ] {
            assert_eq!(handle_text(text, &active(), 0), Inbound::Nothing, "{text}");
        }
        // A malformed element is dropped.
        let bad =
            r#"{"channel":"ticker","type":"update","data":[{"symbol":"BTC/USD","last":"x"}]}"#;
        assert_eq!(handle_text(bad, &active(), 0), Inbound::Quotes(vec![]));
    }

    #[test]
    fn backoff_grows_and_caps() {
        let secs = |a| backoff_delay(a, 1.0).as_secs_f64();
        assert_eq!(secs(0), 1.0);
        assert_eq!(secs(1), 2.0);
        assert_eq!(secs(4), 16.0);
        assert_eq!(secs(5), 30.0);
        assert_eq!(secs(40), 30.0);
        assert_eq!(backoff_delay(0, 0.0).as_secs_f64(), 0.5);
        let mut b = Backoff::default();
        assert_eq!(b.next_delay(1.0), Duration::from_secs(1));
        assert_eq!(b.next_delay(1.0), Duration::from_secs(2));
        b.reset();
        assert_eq!(b.next_delay(1.0), Duration::from_secs(1));
    }

    #[test]
    fn commands_update_desired_set() {
        let mut d = BTreeSet::new();
        assert!(apply(
            &mut d,
            Cmd::Subscribe(vec![
                SecurityKey::currency("BTCUSD"),
                SecurityKey::currency("EURUSD")
            ])
        ));
        assert_eq!(d.len(), 1, "EURUSD is not a crypto pair");
        assert!(apply(
            &mut d,
            Cmd::Unsubscribe(vec![SecurityKey::currency("BTCUSD")])
        ));
        assert!(d.is_empty());
        assert!(!apply(&mut d, Cmd::Close));
    }
    // --- Local feed: exercises the real task against a local server. ---

    use meridian_provider::EventSink;
    use tokio::net::TcpListener;

    #[derive(Default)]
    struct Collect(parking_lot::Mutex<Vec<StreamEvent>>);

    impl EventSink for Collect {
        fn send(&self, _provider: &ProviderId, event: StreamEvent) {
            self.0.lock().push(event);
        }
    }

    fn statuses_of(events: &[StreamEvent]) -> Vec<bool> {
        events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::Status { connected, .. } => Some(*connected),
                StreamEvent::Quote { .. } => None,
            })
            .collect()
    }

    impl Collect {
        fn statuses(&self) -> Vec<bool> {
            statuses_of(&self.0.lock())
        }

        async fn wait_for(&self, what: &str, pred: impl Fn(&[StreamEvent]) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !pred(&self.0.lock()) {
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for {what}; events: {:?}",
                    self.0.lock()
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    }

    type ServerWs = WebSocketStream<TcpStream>;

    async fn accept(listener: &TcpListener) -> ServerWs {
        let (tcp, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
        tokio_tungstenite::accept_async(tcp).await.unwrap()
    }

    async fn next_frame(ws: &mut ServerWs) -> Message {
        tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
    }

    async fn next_json(ws: &mut ServerWs) -> serde_json::Value {
        match next_frame(ws).await {
            Message::Text(t) => serde_json::from_str(t.as_str()).unwrap(),
            other => panic!("expected text, got {other:?}"),
        }
    }

    fn req(method: &str, symbols: &[&str], req_id: u64) -> serde_json::Value {
        let symbols: Vec<String> = symbols.iter().map(|s| (*s).to_owned()).collect();
        serde_json::from_str(&subscription_msg(method, &symbols, req_id)).unwrap()
    }

    async fn local_feed() -> (TcpListener, Arc<Collect>, Handle) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let sink = Arc::new(Collect::default());
        let handle = spawn(Arc::new(Inner::for_tests(url)), sink.clone());
        (listener, sink, handle)
    }

    #[tokio::test]
    async fn local_feed_reconnects_resubscribes_and_closes() {
        let (listener, sink, handle) = local_feed().await;
        // Unknown pairs are filtered against the pair list.
        handle.subscribe(&[
            SecurityKey::currency("BTCUSD"),
            SecurityKey::currency("NOPEUSD"),
        ]);

        let mut ws = accept(&listener).await;
        assert_eq!(next_json(&mut ws).await, req("subscribe", &["BTC/USD"], 1));
        ws.send(Message::text(include_str!(
            "../tests/fixtures/ws_status.json"
        )))
        .await
        .unwrap();
        ws.send(Message::text(include_str!(
            "../tests/fixtures/ws_subscribe_ack.json"
        )))
        .await
        .unwrap();
        ws.send(Message::Ping(b"hb".to_vec().into())).await.unwrap();
        match next_frame(&mut ws).await {
            Message::Pong(p) => assert_eq!(&p[..], b"hb"),
            other => panic!("expected pong, got {other:?}"),
        }
        ws.send(Message::text(include_str!(
            "../tests/fixtures/ws_ticker_snapshot.json"
        )))
        .await
        .unwrap();
        ws.send(Message::text(include_str!(
            "../tests/fixtures/ws_heartbeat.json"
        )))
        .await
        .unwrap();
        sink.wait_for("a quote", |ev| {
            ev.iter().any(|e| matches!(e, StreamEvent::Quote { .. }))
        })
        .await;
        // The routine "online" status produced no event; a change does.
        let maintenance = include_str!("../tests/fixtures/ws_status.json")
            .replace(r#""system":"online""#, r#""system":"maintenance""#);
        ws.send(Message::text(maintenance)).await.unwrap();
        sink.wait_for("system status", |ev| {
            ev.iter().any(|e| matches!(e, StreamEvent::Status { connected: true, message } if message == "Kraken system status: maintenance"))
        })
        .await;

        // Server drops the socket: the task reconnects (first backoff <= 1 s)
        // and re-subscribes everything.
        drop(ws);
        let mut ws = accept(&listener).await;
        assert_eq!(next_json(&mut ws).await, req("subscribe", &["BTC/USD"], 1));
        sink.wait_for("reconnected", |ev| {
            statuses_of(ev).iter().filter(|c| **c).count() >= 3
        })
        .await;
        assert_eq!(sink.statuses(), vec![true, true, false, true]);

        handle.subscribe(&[
            SecurityKey::currency("ETHUSD"),
            SecurityKey::currency("BTCUSD"),
        ]);
        assert_eq!(next_json(&mut ws).await, req("subscribe", &["ETH/USD"], 2));
        handle.unsubscribe(&[SecurityKey::currency("ETHUSD")]);
        assert_eq!(
            next_json(&mut ws).await,
            req("unsubscribe", &["ETH/USD"], 3)
        );

        handle.close();
        assert!(matches!(next_frame(&mut ws).await, Message::Close(_)));
        sink.wait_for("closed", |ev| matches!(ev.last(), Some(StreamEvent::Status { connected: false, message }) if message == "closed"))
            .await;
    }

    #[tokio::test]
    async fn local_feed_goes_idle_without_subscriptions_and_stops_on_drop() {
        let (listener, sink, handle) = local_feed().await;
        handle.subscribe(&[SecurityKey::currency("BTCUSD")]);
        let mut ws = accept(&listener).await;
        assert_eq!(next_json(&mut ws).await, req("subscribe", &["BTC/USD"], 1));
        handle.unsubscribe(&[SecurityKey::currency("BTCUSD")]);
        assert!(matches!(next_frame(&mut ws).await, Message::Close(_)));
        sink.wait_for("idle", |ev| matches!(ev.last(), Some(StreamEvent::Status { connected: false, message }) if message.starts_with("idle"))).await;

        handle.subscribe(&[SecurityKey::currency("ETHUSD")]);
        let mut ws = accept(&listener).await;
        assert_eq!(next_json(&mut ws).await, req("subscribe", &["ETH/USD"], 1));
        drop(handle);
        assert!(matches!(next_frame(&mut ws).await, Message::Close(_)));
        sink.wait_for("closed", |ev| matches!(ev.last(), Some(StreamEvent::Status { message, .. }) if message == "closed")).await;
    }
}

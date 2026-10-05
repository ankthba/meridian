use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, TrySendError};
use meridian_provider::{EventSink, Provider, StreamHandle, asset_hint};
use meridian_types::{ProviderId, Quote, QuoteUpdate, SecurityKey, StreamEvent};
use parking_lot::Mutex;

use crate::registry::{InstrumentId, InstrumentRegistry};
use crate::state::{FeedIndex, MarketState, NO_FEED};

const CHANNEL_CAPACITY: usize = 65_536;

#[allow(clippy::large_enum_variant)] // see StreamEvent
enum Msg {
    Event(FeedIndex, StreamEvent),
    Shutdown,
}

/// Counters exposed for diagnostics and benchmarks.
#[derive(Debug, Default)]
pub struct HubStats {
    pub events: AtomicU64,
    pub conflated: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HubStatsSnapshot {
    pub events: u64,
    pub conflated: u64,
}

/// Connection state of one streaming feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedStatus {
    pub provider: ProviderId,
    pub connected: bool,
    pub message: String,
}

struct Feed {
    provider: Arc<dyn Provider>,
    handle: Box<dyn StreamHandle>,
}

/// Merges updates per key when the channel is full, so a slow apply thread
/// never blocks socket readers and never loses the latest value.
#[derive(Default)]
struct Overflow {
    pending: HashMap<(FeedIndex, SecurityKey), QuoteUpdate>,
}

fn merge(into: &mut QuoteUpdate, u: &QuoteUpdate) {
    macro_rules! take {
        ($($f:ident),*) => { $( if u.$f.is_some() { into.$f = u.$f; } )* };
    }
    take!(bid, ask, bid_size, ask_size, last, last_size, open, high, low, prev_close, volume, vwap);
    if let Some(inc) = u.volume_increment {
        into.volume_increment = Some(into.volume_increment.unwrap_or(0.0) + inc);
    }
    into.ts_event = into.ts_event.max(u.ts_event);
}

/// The sink handed to streaming providers.
struct HubSink {
    tx: Sender<Msg>,
    overflow: Arc<Mutex<Overflow>>,
    feed_index: Arc<Mutex<HashMap<ProviderId, FeedIndex>>>,
    stats: Arc<HubStats>,
}

impl EventSink for HubSink {
    fn send(&self, provider: &ProviderId, event: StreamEvent) {
        let idx = self.feed_index.lock().get(provider).copied().unwrap_or(NO_FEED);
        match self.tx.try_send(Msg::Event(idx, event)) {
            Ok(()) | Err(TrySendError::Disconnected(_) | TrySendError::Full(Msg::Shutdown)) => {}
            Err(TrySendError::Full(Msg::Event(idx, StreamEvent::Quote { key, update }))) => {
                self.stats.conflated.fetch_add(1, Ordering::Relaxed);
                let mut o = self.overflow.lock();
                o.pending.entry((idx, key)).and_modify(|e| merge(e, &update)).or_insert(update);
            }
            Err(TrySendError::Full(Msg::Event(idx, ev @ StreamEvent::Status { .. }))) => {
                // Status changes are rare and must not be lost.
                let _ = self.tx.send(Msg::Event(idx, ev));
            }
        }
    }
}

/// Owns market state, the apply thread, open feeds, and subscription
/// reference counts.
pub struct StreamHub {
    registry: Arc<InstrumentRegistry>,
    state: Arc<MarketState>,
    tx: Sender<Msg>,
    overflow: Arc<Mutex<Overflow>>,
    feed_index: Arc<Mutex<HashMap<ProviderId, FeedIndex>>>,
    feeds: Mutex<Vec<Feed>>,
    statuses: Arc<Mutex<Vec<FeedStatus>>>,
    refcounts: Mutex<HashMap<InstrumentId, u32>>,
    /// Which feed streams each active instrument (None = needs polling).
    routed: Mutex<HashMap<InstrumentId, Option<FeedIndex>>>,
    stats: Arc<HubStats>,
    apply_thread: Mutex<Option<JoinHandle<()>>>,
}

impl StreamHub {
    #[must_use]
    pub fn new(registry: Arc<InstrumentRegistry>) -> Arc<Self> {
        let (tx, rx) = crossbeam_channel::bounded(CHANNEL_CAPACITY);
        let state = Arc::new(MarketState::new());
        let overflow = Arc::new(Mutex::new(Overflow::default()));
        let stats = Arc::new(HubStats::default());
        let statuses = Arc::new(Mutex::new(Vec::new()));
        let hub = Arc::new(Self {
            registry: registry.clone(),
            state: state.clone(),
            tx,
            overflow: overflow.clone(),
            feed_index: Arc::new(Mutex::new(HashMap::new())),
            feeds: Mutex::new(Vec::new()),
            statuses: statuses.clone(),
            refcounts: Mutex::new(HashMap::new()),
            routed: Mutex::new(HashMap::new()),
            stats: stats.clone(),
            apply_thread: Mutex::new(None),
        });
        let handle = std::thread::Builder::new()
            .name("meridian-apply".into())
            .spawn(move || apply_loop(&rx, &registry, &state, &overflow, &stats, &statuses))
            .ok();
        *hub.apply_thread.lock() = handle;
        hub
    }

    #[must_use]
    pub fn registry(&self) -> &Arc<InstrumentRegistry> {
        &self.registry
    }

    #[must_use]
    pub fn state(&self) -> &Arc<MarketState> {
        &self.state
    }

    /// Reserves a feed index for `provider` and returns the sink to pass to
    /// its `StreamingProvider::connect`.
    pub fn sink_for(&self, provider: &ProviderId) -> Arc<dyn EventSink> {
        let mut map = self.feed_index.lock();
        let next = FeedIndex::try_from(map.len()).unwrap_or(NO_FEED - 1);
        map.entry(provider.clone()).or_insert(next);
        self.statuses.lock().push(FeedStatus { provider: provider.clone(), connected: false, message: "idle: no subscriptions yet".into() });
        Arc::new(HubSink {
            tx: self.tx.clone(),
            overflow: self.overflow.clone(),
            feed_index: self.feed_index.clone(),
            stats: self.stats.clone(),
        })
    }

    /// Registers an open feed. Instruments already subscribed and covered by
    /// this feed are subscribed on it immediately.
    pub fn add_feed(&self, provider: Arc<dyn Provider>, handle: Box<dyn StreamHandle>) {
        let idx = self.feed_index.lock().get(&provider.id()).copied().unwrap_or(NO_FEED);
        let mut newly = Vec::new();
        {
            let mut routed = self.routed.lock();
            for (id, feed) in routed.iter_mut() {
                if feed.is_none()
                    && let Some(key) = self.registry.key(*id)
                    && feed_covers(provider.as_ref(), &key)
                {
                    *feed = Some(idx);
                    newly.push(key);
                }
            }
        }
        if !newly.is_empty() {
            handle.subscribe(&newly);
        }
        self.feeds.lock().push(Feed { provider, handle });
    }

    /// Increments reference counts and subscribes newly active instruments
    /// on the first feed that covers them. Returns their IDs in input order.
    pub fn subscribe(&self, keys: &[SecurityKey]) -> Vec<InstrumentId> {
        let ids: Vec<InstrumentId> = keys.iter().map(|k| self.registry.intern(k)).collect();
        let mut activate = Vec::new();
        {
            let mut rc = self.refcounts.lock();
            for (id, key) in ids.iter().zip(keys) {
                let n = rc.entry(*id).or_insert(0);
                *n += 1;
                if *n == 1 {
                    activate.push((*id, key.clone()));
                }
            }
        }
        if activate.is_empty() {
            return ids;
        }
        let feeds = self.feeds.lock();
        let index = self.feed_index.lock().clone();
        let mut per_feed: Vec<Vec<SecurityKey>> = vec![Vec::new(); feeds.len()];
        let mut routed = self.routed.lock();
        for (id, key) in activate {
            let chosen = feeds.iter().position(|f| feed_covers(f.provider.as_ref(), &key));
            match chosen {
                Some(i) => {
                    per_feed[i].push(key);
                    routed.insert(id, index.get(&feeds[i].provider.id()).copied());
                }
                None => {
                    routed.insert(id, None);
                }
            }
        }
        for (feed, keys) in feeds.iter().zip(per_feed) {
            if !keys.is_empty() {
                feed.handle.subscribe(&keys);
            }
        }
        ids
    }

    /// Decrements reference counts; instruments reaching zero are
    /// unsubscribed from their feed.
    pub fn unsubscribe(&self, ids: &[InstrumentId]) {
        let mut deactivate = Vec::new();
        {
            let mut rc = self.refcounts.lock();
            for id in ids {
                if let Some(n) = rc.get_mut(id) {
                    *n = n.saturating_sub(1);
                    if *n == 0 {
                        rc.remove(id);
                        deactivate.push(*id);
                    }
                }
            }
        }
        if deactivate.is_empty() {
            return;
        }
        let feeds = self.feeds.lock();
        let index = self.feed_index.lock().clone();
        let mut routed = self.routed.lock();
        for id in deactivate {
            let Some(Some(feed_idx)) = routed.remove(&id) else { continue };
            let Some(key) = self.registry.key(id) else { continue };
            if let Some(f) = feeds.iter().find(|f| index.get(&f.provider.id()) == Some(&feed_idx)) {
                f.handle.unsubscribe(&[key]);
            }
        }
    }

    /// Active instruments that no streaming feed covers; the engine polls
    /// these through REST.
    #[must_use]
    pub fn unstreamed(&self) -> Vec<SecurityKey> {
        self.routed
            .lock()
            .iter()
            .filter(|(_, f)| f.is_none())
            .filter_map(|(id, _)| self.registry.key(*id))
            .collect()
    }

    /// Writes a REST snapshot into state.
    pub fn apply_snapshot(&self, quote: &Quote) {
        let id = self.registry.intern(&quote.key);
        let feed = self.feed_index.lock().get(&quote.provenance.provider).copied().unwrap_or(NO_FEED);
        self.state.apply_snapshot(id, quote, feed);
    }

    #[must_use]
    pub fn statuses(&self) -> Vec<FeedStatus> {
        self.statuses.lock().clone()
    }

    #[must_use]
    pub fn stats(&self) -> HubStatsSnapshot {
        HubStatsSnapshot {
            events: self.stats.events.load(Ordering::Relaxed),
            conflated: self.stats.conflated.load(Ordering::Relaxed),
        }
    }

    /// Closes feeds and stops the apply thread.
    pub fn shutdown(&self) {
        for f in self.feeds.lock().drain(..) {
            f.handle.close();
        }
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(h) = self.apply_thread.lock().take() {
            let _ = h.join();
        }
    }
}

impl Drop for StreamHub {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
    }
}

fn feed_covers(p: &dyn Provider, key: &SecurityKey) -> bool {
    let caps = p.capabilities();
    p.covers(key) && asset_hint(key).iter().any(|a| caps.supports(meridian_provider::Capability::Stream, Some(*a)))
}

fn apply_loop(
    rx: &Receiver<Msg>,
    registry: &InstrumentRegistry,
    state: &MarketState,
    overflow: &Mutex<Overflow>,
    stats: &HubStats,
    statuses: &Mutex<Vec<FeedStatus>>,
) {
    let handle = |idx: FeedIndex, ev: StreamEvent| match ev {
        StreamEvent::Quote { key, update } => {
            stats.events.fetch_add(1, Ordering::Relaxed);
            let id = registry.intern(&key);
            state.apply(id, &update, idx);
        }
        StreamEvent::Status { connected, message } => {
            if !connected {
                state.mark_feed_stale(idx);
            }
            let mut st = statuses.lock();
            if let Some(s) = st.get_mut(idx as usize) {
                s.connected = connected;
                s.message = message;
            }
        }
    };
    // Overflow is only filled while the channel is full, so queued messages
    // always wake this loop before pending updates could be stranded; the
    // timeout is a backstop, kept long so an idle hub doesn't spin.
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Msg::Event(idx, ev)) => {
                handle(idx, ev);
                // Drain whatever else is queued without sleeping.
                while let Ok(msg) = rx.try_recv() {
                    match msg {
                        Msg::Event(idx, ev) => handle(idx, ev),
                        Msg::Shutdown => return,
                    }
                }
            }
            Ok(Msg::Shutdown) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
        }
        let pending = std::mem::take(&mut overflow.lock().pending);
        for ((idx, key), update) in pending {
            handle(idx, StreamEvent::Quote { key, update });
        }
    }
}

/// A view's subscription: a fixed set of instruments polled at display rate.
/// Dropping it releases the instruments.
pub struct QuoteSubscription {
    hub: Arc<StreamHub>,
    ids: Vec<InstrumentId>,
    keys: Vec<SecurityKey>,
}

impl QuoteSubscription {
    #[must_use]
    pub fn new(hub: Arc<StreamHub>, keys: Vec<SecurityKey>) -> Self {
        let ids = hub.subscribe(&keys);
        Self { hub, ids, keys }
    }

    #[must_use]
    pub fn ids(&self) -> &[InstrumentId] {
        &self.ids
    }

    #[must_use]
    pub fn keys(&self) -> &[SecurityKey] {
        &self.keys
    }

    /// Packed rows for instruments changed after `since`, and the sequence
    /// number to pass next time.
    #[must_use]
    pub fn poll(&self, since: u64) -> (u64, Vec<u8>) {
        let mut out = Vec::new();
        let seq = self.hub.state.poll_into(&self.ids, since, &mut out);
        (seq, out)
    }
}

impl Drop for QuoteSubscription {
    fn drop(&mut self) {
        self.hub.unsubscribe(&self.ids);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use meridian_provider::{AiPolicy, CachePolicy, Capabilities, Capability, CapabilityEntry};
    use meridian_types::{AssetClass, DataDelay, FeedSource};

    use super::*;
    use crate::row::{QuoteRow, ROW_SIZE};

    struct FakeProvider(Capabilities);

    #[async_trait::async_trait]
    impl Provider for FakeProvider {
        fn id(&self) -> ProviderId {
            ProviderId::new("fake")
        }
        fn capabilities(&self) -> &Capabilities {
            &self.0
        }
    }

    #[derive(Default)]
    struct RecordingHandle(Mutex<Vec<String>>);
    struct H(Arc<RecordingHandle>);
    impl StreamHandle for H {
        fn subscribe(&self, keys: &[SecurityKey]) {
            for k in keys {
                self.0.0.lock().push(format!("+{k}"));
            }
        }
        fn unsubscribe(&self, keys: &[SecurityKey]) {
            for k in keys {
                self.0.0.lock().push(format!("-{k}"));
            }
        }
        fn close(&self) {}
    }

    fn caps() -> Capabilities {
        Capabilities {
            entries: vec![CapabilityEntry {
                capability: Capability::Stream,
                asset_classes: vec![AssetClass::Equity],
                delay: DataDelay::RealTime,
                source: FeedSource::Consolidated,
                history: None,
            }],
            rate_limit: None,
            max_stream_symbols: None,
            cache_policy: CachePolicy::Unrestricted,
            attribution: None,
            display_allowed: true,
            ai_policy: AiPolicy::Allowed,
            requires_credentials: false,
            terms_note: String::new(),
            docs_url: String::new(),
        }
    }

    fn wait_for(cond: impl Fn() -> bool) {
        let start = Instant::now();
        while !cond() {
            assert!(start.elapsed() < Duration::from_secs(2), "timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn events_flow_to_subscription() {
        let hub = StreamHub::new(Arc::new(InstrumentRegistry::new()));
        let pid = ProviderId::new("fake");
        let sink = hub.sink_for(&pid);
        let rec = Arc::new(RecordingHandle::default());
        hub.add_feed(Arc::new(FakeProvider(caps())), Box::new(H(rec.clone())));

        let sub = QuoteSubscription::new(hub.clone(), vec![SecurityKey::equity("AAPL"), SecurityKey::currency("EURUSD")]);
        assert_eq!(rec.0.lock().as_slice(), ["+AAPL US Equity"]);
        assert_eq!(hub.unstreamed(), vec![SecurityKey::currency("EURUSD")]);

        sink.send(&pid, StreamEvent::Quote {
            key: SecurityKey::equity("AAPL"),
            update: QuoteUpdate { last: Some(100.0), ..Default::default() },
        });
        wait_for(|| !sub.poll(0).1.is_empty());
        let (_, buf) = sub.poll(0);
        let row = QuoteRow::read_from(&buf[..ROW_SIZE]).unwrap();
        assert_eq!(row.last, 100.0);

        drop(sub);
        assert_eq!(rec.0.lock().last().map(String::as_str), Some("-AAPL US Equity"));
        hub.shutdown();
    }

    #[test]
    fn refcounted_subscriptions() {
        let hub = StreamHub::new(Arc::new(InstrumentRegistry::new()));
        let _ = hub.sink_for(&ProviderId::new("fake"));
        let rec = Arc::new(RecordingHandle::default());
        hub.add_feed(Arc::new(FakeProvider(caps())), Box::new(H(rec.clone())));
        let a = QuoteSubscription::new(hub.clone(), vec![SecurityKey::equity("AAPL")]);
        let b = QuoteSubscription::new(hub.clone(), vec![SecurityKey::equity("AAPL")]);
        drop(a);
        assert_eq!(rec.0.lock().len(), 1, "still referenced by b");
        drop(b);
        assert_eq!(rec.0.lock().len(), 2);
        hub.shutdown();
    }

    #[test]
    fn status_disconnect_marks_stale() {
        let hub = StreamHub::new(Arc::new(InstrumentRegistry::new()));
        let pid = ProviderId::new("fake");
        let sink = hub.sink_for(&pid);
        let key = SecurityKey::equity("AAPL");
        sink.send(&pid, StreamEvent::Quote { key: key.clone(), update: QuoteUpdate { last: Some(1.0), ..Default::default() } });
        sink.send(&pid, StreamEvent::Status { connected: false, message: "lost".into() });
        let id = hub.registry().intern(&key);
        wait_for(|| hub.state().snapshot(id).is_some_and(|r| r.flags & meridian_types::quote_flags::STALE != 0));
        assert!(!hub.statuses()[0].connected);
        hub.shutdown();
    }
}

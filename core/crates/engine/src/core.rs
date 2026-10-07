use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use meridian_alerts::{AlertEngine, AlertRule, QuoteView};
use meridian_provider::{Capabilities, InstrumentQuery, Provider, ProviderRouter};
use meridian_store::Stores;
use meridian_stream::{FeedStatus, InstrumentRegistry, QuoteSubscription, StreamHub};
use meridian_types::{Clock, FixedClock, Instrument, ProviderId, SecurityKey, SystemClock, UnixNanos};
use parking_lot::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::config::{DataMode, EngineConfig};
use crate::error::{EngineError, EngineResult};
use crate::events::{EngineEvent, EngineEvents};

/// AI features the engine can call when an ASK model is configured.
#[async_trait::async_trait]
pub trait AiService: Send + Sync {
    async fn summarize_filing(&self, filing: &meridian_types::Filing, doc: &meridian_types::FilingDocument) -> EngineResult<String>;
}

/// One provider's terms and capabilities, for the Data Sources screen.
#[derive(Debug, Clone)]
pub struct DataSourceInfo {
    pub provider: ProviderId,
    pub capabilities: Capabilities,
}

pub struct Engine {
    runtime: Mutex<Option<tokio::runtime::Runtime>>,
    handle: tokio::runtime::Handle,
    mode: DataMode,
    clock: Arc<dyn Clock>,
    /// Swapped wholesale when credentials change (`replace_providers`);
    /// readers take a cheap `Arc` clone.
    router: RwLock<Arc<ProviderRouter>>,
    hub: Arc<StreamHub>,
    stores: Arc<Stores>,
    events: Arc<dyn EngineEvents>,
    alerts: Arc<Mutex<AlertEngine>>,
    alert_sub: Mutex<Option<QuoteSubscription>>,
    universe: RwLock<Vec<Instrument>>,
    ai: RwLock<Option<Arc<dyn AiService>>>,
    instruments: RwLock<HashMap<SecurityKey, Instrument>>,
    /// Stories shown this session, by id, so opening one never depends on
    /// re-fetching the feed. In memory only (some providers forbid storing
    /// news on disk).
    pub(crate) recent_news: Mutex<HashMap<String, meridian_types::NewsItem>>,
    /// Company news for TODAY by key set: (fetched at, items). In memory
    /// only, like `recent_news`; a short TTL keeps the home screen fast.
    pub(crate) holdings_news: Mutex<HashMap<String, (UnixNanos, Vec<meridian_types::NewsItem>)>>,
    /// TODAY's slower sections still loading after the first paint, by the
    /// inputs they were started for. The quick refresh that follows picks
    /// the same task up instead of fetching again.
    pub(crate) today_pending: Mutex<Option<(String, tokio::task::JoinHandle<crate::screens::today::Slow>)>>,
    /// Fixed clock (tests, snapshots): screens wait for all their data so
    /// output is deterministic.
    pub(crate) deterministic: bool,
    cancel: CancellationToken,
}

impl Engine {
    /// Builds the engine. `providers` must all belong to `config.mode`: the
    /// composition root passes either the mock provider or real providers.
    pub fn new(config: &EngineConfig, providers: Vec<Arc<dyn Provider>>, events: Arc<dyn EngineEvents>) -> EngineResult<Arc<Self>> {
        Self::check_mode(config.mode, &providers)?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(config.worker_threads.max(1))
            .thread_name("meridian-io")
            .enable_all()
            .build()
            .map_err(|e| EngineError::Internal(e.to_string()))?;
        let handle = runtime.handle().clone();
        let clock: Arc<dyn Clock> = match config.fixed_clock {
            Some(t) => Arc::new(FixedClock(t)),
            None => Arc::new(SystemClock),
        };
        let stores = if config.in_memory { Stores::in_memory()? } else { Stores::open(&config.data_dir)? };
        let hub = StreamHub::new(Arc::new(InstrumentRegistry::new()));
        let engine = Arc::new(Self {
            runtime: Mutex::new(Some(runtime)),
            handle,
            mode: config.mode,
            clock,
            router: RwLock::new(Arc::new(ProviderRouter::new(providers))),
            hub,
            stores: Arc::new(stores),
            events,
            alerts: Arc::new(Mutex::new(AlertEngine::new())),
            alert_sub: Mutex::new(None),
            universe: RwLock::new(Vec::new()),
            ai: RwLock::new(None),
            instruments: RwLock::new(HashMap::new()),
            recent_news: Mutex::new(HashMap::new()),
            holdings_news: Mutex::new(HashMap::new()),
            today_pending: Mutex::new(None),
            deterministic: config.fixed_clock.is_some(),
            cancel: CancellationToken::new(),
        });
        engine.reload_alerts()?;
        engine.ensure_default_watchlist();
        Ok(engine)
    }

    /// Connects streaming feeds and starts background loops. Returns
    /// immediately; work happens on the engine runtime.
    pub fn start(self: &Arc<Self>) {
        let me = self.clone();
        self.handle.spawn(async move {
            me.clone().connect_feeds().await;
            me.feed_status_loop().await;
        });
        let me = self.clone();
        self.handle.spawn(async move { me.load_universe().await });
        let me = self.clone();
        self.handle.spawn(async move { me.poll_unstreamed_loop().await });
        let me = self.clone();
        self.handle.spawn(async move { me.alert_loop().await });
    }

    /// Stops background work and closes feeds.
    pub fn shutdown(&self) {
        self.cancel.cancel();
        self.hub.shutdown();
        if let Some(rt) = self.runtime.lock().take() {
            rt.shutdown_background();
        }
    }

    /// Installs (or removes) the AI service used by CF summaries etc.
    pub fn set_ai(&self, ai: Option<Arc<dyn AiService>>) {
        *self.ai.write() = ai;
    }

    #[must_use]
    pub fn ai(&self) -> Option<Arc<dyn AiService>> {
        self.ai.read().clone()
    }

    #[must_use]
    pub fn mode(&self) -> DataMode {
        self.mode
    }

    #[must_use]
    pub fn now(&self) -> UnixNanos {
        self.clock.now()
    }

    #[must_use]
    pub fn clock(&self) -> &Arc<dyn Clock> {
        &self.clock
    }

    #[must_use]
    pub fn router(&self) -> Arc<ProviderRouter> {
        self.router.read().clone()
    }

    /// Replaces the data sources (after the user changes credentials):
    /// swaps the router, closes the old feeds and connects the new ones, and
    /// reloads reference data. Live subscriptions carry over.
    pub fn replace_providers(self: &Arc<Self>, providers: Vec<Arc<dyn Provider>>) -> EngineResult<()> {
        Self::check_mode(self.mode, &providers)?;
        *self.router.write() = Arc::new(ProviderRouter::new(providers));
        self.hub.reset_feeds();
        let me = self.clone();
        self.handle.spawn(async move {
            me.clone().connect_feeds().await;
            me.load_universe().await;
        });
        Ok(())
    }

    fn check_mode(mode: DataMode, providers: &[Arc<dyn Provider>]) -> EngineResult<()> {
        for p in providers {
            let synthetic = p.id().as_str() == "mock";
            if synthetic != (mode == DataMode::Mock) {
                return Err(EngineError::Internal(format!(
                    "provider {} does not belong to {:?} mode; mock and live data must never mix",
                    p.id(),
                    mode
                )));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn hub(&self) -> &Arc<StreamHub> {
        &self.hub
    }

    #[must_use]
    pub fn stores(&self) -> &Arc<Stores> {
        &self.stores
    }

    #[must_use]
    pub fn handle(&self) -> &tokio::runtime::Handle {
        &self.handle
    }

    pub(crate) fn emit(&self, e: EngineEvent) {
        self.events.on_event(e);
    }

    /// Hint shown when a capability has no provider in the current mode.
    #[must_use]
    pub fn unavailable_hint(&self) -> &'static str {
        match self.mode {
            DataMode::Mock => "the mock provider does not generate this",
            DataMode::Live => "no configured data source provides it (see Data Sources)",
        }
    }

    // --- streaming ------------------------------------------------------

    /// Connects every streaming provider in the current router.
    async fn connect_feeds(self: Arc<Self>) {
        for p in self.router().providers() {
            let Some(streaming) = p.streaming() else { continue };
            let sink = self.hub.sink_for(&p.id());
            match streaming.connect(sink).await {
                Ok(handle) => {
                    self.hub.add_feed(p.clone(), handle);
                    tracing::info!(provider = %p.id(), "feed connected");
                }
                Err(e) => {
                    tracing::warn!(provider = %p.id(), error = %e, "feed connect failed");
                    self.emit(EngineEvent::FeedStatus { provider: p.id().to_string(), connected: false, message: e.to_string() });
                }
            }
        }
    }

    /// Forwards feed status changes to the app until shutdown.
    async fn feed_status_loop(self: Arc<Self>) {
        let mut last: Vec<FeedStatus> = Vec::new();
        loop {
            tokio::select! {
                () = self.cancel.cancelled() => return,
                () = tokio::time::sleep(Duration::from_millis(500)) => {}
            }
            let now = self.hub.statuses();
            for s in &now {
                if !last.iter().any(|l| l == s) {
                    self.emit(EngineEvent::FeedStatus { provider: s.provider.to_string(), connected: s.connected, message: s.message.clone() });
                }
            }
            last = now;
        }
    }

    /// Subscribes `keys` for live updates and seeds them with REST snapshots
    /// so values appear before the first tick.
    pub fn subscribe(self: &Arc<Self>, keys: Vec<SecurityKey>) -> QuoteSubscription {
        let sub = QuoteSubscription::new(self.hub.clone(), keys.clone());
        let me = self.clone();
        self.handle.spawn(async move {
            // Only seed instruments that have no data yet.
            let missing: Vec<SecurityKey> = keys
                .into_iter()
                .filter(|k| me.hub.registry().get(k).and_then(|id| me.hub.state().snapshot(id)).is_none())
                .collect();
            for chunk in missing.chunks(100) {
                if let Ok(quotes) = me.router().quotes(chunk).await {
                    for q in &quotes {
                        me.hub.apply_snapshot(q);
                    }
                }
            }
        });
        sub
    }

    async fn poll_unstreamed_loop(self: Arc<Self>) {
        let interval = match self.mode {
            DataMode::Mock => Duration::from_secs(5),
            DataMode::Live => Duration::from_secs(15),
        };
        loop {
            tokio::select! {
                () = self.cancel.cancelled() => return,
                () = tokio::time::sleep(interval) => {}
            }
            let keys = self.hub.unstreamed();
            for chunk in keys.chunks(100) {
                match self.router().quotes(chunk).await {
                    Ok(quotes) => {
                        for q in &quotes {
                            self.hub.apply_snapshot(q);
                        }
                    }
                    Err(e) => tracing::debug!(error = %e, "poll quotes failed"),
                }
            }
        }
    }

    // --- reference data -------------------------------------------------

    /// Loads reference data now (tests and snapshot rendering).
    pub async fn refresh_universe(self: &Arc<Self>) {
        self.clone().load_universe().await;
    }

    async fn load_universe(self: Arc<Self>) {
        let q = InstrumentQuery { text: String::new(), sector: None, limit: 200_000 };
        match self.router().search(q).await {
            Ok(list) => {
                let n = list.len() as u64;
                {
                    let mut map = self.instruments.write();
                    for i in &list {
                        map.insert(i.key.clone(), i.clone());
                    }
                }
                *self.universe.write() = list;
                self.on_universe_loaded();
                self.emit(EngineEvent::UniverseLoaded { instruments: n });
            }
            Err(e) => tracing::warn!(error = %e, "universe load failed"),
        }
    }

    /// Hook for derived indexes (autocomplete) after the universe loads.
    fn on_universe_loaded(&self) {
        crate::screens::rebuild_suggest_index(self);
    }

    /// Instruments known from providers' reference data.
    #[must_use]
    pub fn universe(&self) -> Vec<Instrument> {
        self.universe.read().clone()
    }

    /// Reference data for `key`, from memory or the router.
    pub async fn instrument(&self, key: &SecurityKey) -> EngineResult<Instrument> {
        if let Some(i) = self.instruments.read().get(key) {
            return Ok(i.clone());
        }
        let routed = self.router().instrument(key).await?;
        self.instruments.write().insert(key.clone(), routed.value.clone());
        Ok(routed.value)
    }

    /// Reference data already in memory (loaded with the universe or looked
    /// up earlier); never touches the network.
    #[must_use]
    pub fn instrument_in_memory(&self, key: &SecurityKey) -> Option<Instrument> {
        self.instruments.read().get(key).cloned()
    }

    /// Price decimals for display, defaulting by sector.
    #[must_use]
    pub fn price_decimals(&self, key: &SecurityKey) -> u8 {
        if let Some(i) = self.instruments.read().get(key) {
            return i.price_decimals;
        }
        match key.sector {
            meridian_types::MarketSector::Curncy => {
                if key.symbol.contains("JPY") { 2 } else { 4 }
            }
            _ => 2,
        }
    }

    #[must_use]
    pub fn data_sources(&self) -> Vec<DataSourceInfo> {
        self.router().capabilities().into_iter().map(|(provider, capabilities)| DataSourceInfo { provider, capabilities }).collect()
    }

    #[must_use]
    pub fn feed_statuses(&self) -> Vec<FeedStatus> {
        self.hub.statuses()
    }

    // --- watchlists -----------------------------------------------------

    fn ensure_default_watchlist(&self) {
        if let Ok(list) = self.stores.app.watchlists()
            && list.is_empty()
            && let Ok(id) = self.stores.app.create_watchlist("Default", self.now())
        {
            let items: Vec<String> = crate::universe::DEFAULT_WATCHLIST.iter().map(|s| (*s).to_string()).collect();
            let _ = self.stores.app.set_watchlist_items(id, &items);
        }
    }

    // --- alerts ---------------------------------------------------------

    /// Reloads alert rules from SQLite into the evaluator.
    pub fn reload_alerts(&self) -> EngineResult<()> {
        let records = self.stores.app.alerts()?;
        let rules: Vec<AlertRule> = records
            .iter()
            .filter_map(|r| {
                let mut rule: AlertRule = serde_json::from_str(&r.json).ok()?;
                rule.id = r.id;
                rule.enabled = r.enabled;
                Some(rule)
            })
            .collect();
        self.alerts.lock().set_rules(rules);
        // Resubscribe on the next alert-loop tick.
        *self.alert_sub.lock() = None;
        Ok(())
    }

    /// Saves (insert or update) a rule. Returns its id.
    pub fn save_alert(&self, rule: &AlertRule) -> EngineResult<i64> {
        let json = serde_json::to_string(rule).map_err(|e| EngineError::Internal(e.to_string()))?;
        let id = self.stores.app.upsert_alert((rule.id > 0).then_some(rule.id), &json, rule.enabled, self.now())?;
        self.reload_alerts()?;
        Ok(id)
    }

    pub fn delete_alert(&self, id: i64) -> EngineResult<()> {
        self.stores.app.delete_alert(id)?;
        self.reload_alerts()
    }

    #[must_use]
    pub fn alert_rules(&self) -> Vec<AlertRule> {
        self.alerts.lock().rules().to_vec()
    }

    async fn alert_loop(self: Arc<Self>) {
        let mut news_tick = 0u32;
        loop {
            tokio::select! {
                () = self.cancel.cancelled() => return,
                () = tokio::time::sleep(Duration::from_millis(500)) => {}
            }
            let watched = self.alerts.lock().watched_securities();
            {
                let mut sub = self.alert_sub.lock();
                let stale = sub.as_ref().is_none_or(|s| s.keys() != watched.as_slice());
                if stale {
                    *sub = (!watched.is_empty()).then(|| self.subscribe(watched.clone()));
                }
            }
            let now = self.now();
            let mut fired = Vec::new();
            for key in &watched {
                let Some(id) = self.hub.registry().get(key) else { continue };
                let Some(row) = self.hub.state().snapshot(id) else { continue };
                let view = QuoteView { last: row.last, pct_change: row.pct_change, volume: row.volume };
                fired.extend(self.alerts.lock().on_quote(key, view, now));
            }
            news_tick += 1;
            if news_tick >= 120 {
                news_tick = 0;
                fired.extend(self.check_news_alerts(now).await);
            }
            for f in fired {
                let _ = self.stores.app.record_alert_fired(f.rule_id, f.at, &f.message);
                if f.disable
                    && let Some(mut rule) = self.alert_rules().into_iter().find(|r| r.id == f.rule_id)
                {
                    rule.enabled = false;
                    let _ = self.save_alert(&rule);
                }
                self.emit(EngineEvent::AlertFired(f));
            }
        }
    }

    async fn check_news_alerts(&self, now: UnixNanos) -> Vec<meridian_alerts::FiredAlert> {
        let keys: Vec<SecurityKey> = self
            .alert_rules()
            .into_iter()
            .filter(|r| r.enabled && matches!(r.condition, meridian_alerts::AlertCondition::NewsKeyword { .. }))
            .map(|r| r.security)
            .collect();
        if keys.is_empty() {
            return Vec::new();
        }
        let q = meridian_provider::NewsQuery {
            scope: meridian_provider::NewsScope::Company,
            keys,
            text: None,
            from: Some(now - 10 * 60 * meridian_types::NANOS_PER_SEC),
            to: None,
            limit: 100,
        };
        let Ok(page) = self.router().news(q).await else { return Vec::new() };
        let mut out = Vec::new();
        let mut alerts = self.alerts.lock();
        for item in &page.items {
            out.extend(alerts.on_news(item, now));
        }
        out
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(rt) = self.runtime.get_mut().take() {
            rt.shutdown_background();
        }
    }
}

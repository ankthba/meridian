use std::sync::Arc;

use meridian_command::{ParsedCommand, SecurityNeed, SuggestionKind, registry};
use meridian_engine::screens::ScreenRequest;
use meridian_engine::{DataMode, Engine, EngineConfig, EngineEvent, EngineEvents};
use meridian_provider::{AiPolicy, CachePolicy};
use meridian_stream::QuoteSubscription;
use meridian_types::SecurityKey;

use crate::error::{CoreError, CoreResult};
use crate::providers;
use crate::types::{
    AskToolFfi, AskTurnFfi, CapabilityRowFfi, ChartDataFfi, ChartStudyFfi, CoreEventFfi, DataSourceFfi, FunctionInfoFfi, KeyValue,
    LayoutFieldFfi, ParsedCommandFfi, PollResultFfi, ScreenFfi, SuggestionFfi, SuggestionKindFfi, WatchlistFfi, WorkspaceSummaryFfi, from_kv,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DataModeFfi {
    Mock,
    Live,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct CoreConfigFfi {
    pub mode: DataModeFfi,
    /// `~/Library/Application Support/Meridian`.
    pub data_dir: String,
    /// Use in-memory stores (snapshot rendering, tests).
    pub in_memory: bool,
    /// Freeze the clock (snapshot rendering). Unix nanos.
    pub fixed_clock_ns: Option<i64>,
    pub mock_seed: u64,
    /// Extra synthetic equities for load testing.
    pub mock_extra_symbols: u32,
    /// Stream updates per symbol per second in mock mode.
    pub mock_update_rate: f64,
}

/// Swift implements this with the macOS Keychain. Rust asks lazily; keys
/// are never stored anywhere else.
#[uniffi::export(foreign)]
pub trait SecretSource: Send + Sync + std::fmt::Debug {
    fn secret(&self, provider: String, field: String) -> Option<String>;
}

/// Swift receives low-frequency events here and hops to the main actor.
#[uniffi::export(foreign)]
pub trait CoreEvents: Send + Sync + std::fmt::Debug {
    fn on_event(&self, event: CoreEventFfi);
}

struct EventBridge(Arc<dyn CoreEvents>);

impl EngineEvents for EventBridge {
    fn on_event(&self, event: EngineEvent) {
        let e = match event {
            EngineEvent::FeedStatus { provider, connected, message } => CoreEventFfi::FeedStatus { provider, connected, message },
            EngineEvent::AlertFired(f) => {
                CoreEventFfi::AlertFired { rule_id: f.rule_id, security: f.security.to_string(), message: f.message, at: f.at }
            }
            EngineEvent::UniverseLoaded { instruments } => CoreEventFfi::UniverseLoaded { instruments },
            EngineEvent::Status { message } => CoreEventFfi::Status { message },
            EngineEvent::Show { function, security, args } => CoreEventFfi::Show {
                function,
                security,
                args: args.into_iter().map(|(k, v)| KeyValue { key: k, value: v }).collect(),
            },
        };
        self.0.on_event(e);
    }
}

fn init_logging() {
    use tracing_subscriber::prelude::*;
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let filter = tracing_subscriber::EnvFilter::try_from_env("MERIDIAN_LOG").unwrap_or_else(|_| "info".into());
        let _ = tracing_subscriber::registry().with(filter).with(tracing_oslog::OsLogger::new("meridian.core", "default")).try_init();
        std::panic::set_hook(Box::new(|info| {
            let bt = std::backtrace::Backtrace::force_capture();
            tracing::error!(%info, backtrace = %bt, "panic in Rust core");
            crate::core::write_crash_report(&format!("{info}\n\n{bt}"));
        }));
    });
}

static CRASH_DIR: parking_lot::Mutex<Option<std::path::PathBuf>> = parking_lot::Mutex::new(None);

pub(crate) fn write_crash_report(text: &str) {
    if let Some(dir) = CRASH_DIR.lock().clone() {
        let _ = std::fs::create_dir_all(&dir);
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = std::fs::write(dir.join(format!("rust-panic-{ts}.txt")), text);
    }
}

/// Streams ASK progress to Swift. Called on Rust threads.
#[uniffi::export(foreign)]
pub trait AskObserverFfi: Send + Sync + std::fmt::Debug {
    fn on_text(&self, delta: String);
    fn on_tool_call(&self, id: String, name: String, input_json: String);
    fn on_tool_result(&self, tool: AskToolFfi);
}

struct AskObserverBridge(Arc<dyn AskObserverFfi>);

impl meridian_ask::AskObserver for AskObserverBridge {
    fn on_text_delta(&self, text: &str) {
        self.0.on_text(text.to_owned());
    }
    fn on_tool_call(&self, id: &str, name: &str, input_json: &str) {
        self.0.on_tool_call(id.to_owned(), name.to_owned(), input_json.to_owned());
    }
    fn on_tool_result(&self, _id: &str, audit: &meridian_ask::ToolAudit, _is_error: bool) {
        self.0.on_tool_result(audit.into());
    }
    fn on_done(&self, _outcome: meridian_ask::AskOutcome) {}
    fn on_error(&self, _error: meridian_ask::AskError) {}
}

#[derive(uniffi::Object)]
pub struct Core {
    engine: Arc<Engine>,
    /// Rebuilt by `reload_providers` when the Anthropic key changes.
    ask: parking_lot::RwLock<Option<Arc<meridian_engine::ask_tools::AskService>>>,
    config: CoreConfigFfi,
    econf: EngineConfig,
    secrets: Arc<dyn SecretSource>,
}

fn parse_key(s: &str) -> CoreResult<SecurityKey> {
    s.parse().map_err(|_| CoreError::InvalidInput { message: format!("not a security key: {s}") })
}

impl Core {
    async fn on_runtime<T: Send + 'static>(&self, f: impl std::future::Future<Output = T> + Send + 'static) -> CoreResult<T> {
        self.engine.handle().spawn(f).await.map_err(|e| CoreError::Internal { message: e.to_string() })
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl Core {
    #[uniffi::constructor]
    pub fn new(config: CoreConfigFfi, secrets: Arc<dyn SecretSource>, events: Arc<dyn CoreEvents>) -> CoreResult<Arc<Self>> {
        init_logging();
        let data_dir = std::path::PathBuf::from(&config.data_dir);
        *CRASH_DIR.lock() = Some(data_dir.join("crashes"));
        let mode = match config.mode {
            DataModeFfi::Mock => DataMode::Mock,
            DataModeFfi::Live => DataMode::Live,
        };
        let econf = EngineConfig {
            mode,
            data_dir: data_dir.clone(),
            in_memory: config.in_memory,
            fixed_clock: config.fixed_clock_ns,
            worker_threads: 4,
        };
        let built = providers::build(&config, &econf, secrets.as_ref());
        let engine = Engine::new(&econf, built.providers, Arc::new(EventBridge(events)))?;
        if let Some(ai) = built.ai {
            engine.set_ai(Some(ai));
        }
        let ask = built.anthropic.map(|c| Arc::new(meridian_engine::ask_tools::AskService::new(engine.clone(), c)));
        Ok(Arc::new(Self { engine, ask: parking_lot::RwLock::new(ask), config, econf, secrets }))
    }

    /// Rebuilds the data sources from the current Keychain credentials and
    /// provider settings and swaps them in without restarting: feeds
    /// reconnect, open subscriptions carry over. No-op in MOCK mode.
    pub fn reload_providers(&self) -> CoreResult<()> {
        if self.econf.mode == DataMode::Mock {
            return Ok(());
        }
        let built = providers::build(&self.config, &self.econf, self.secrets.as_ref());
        self.engine.replace_providers(built.providers)?;
        self.engine.set_ai(built.ai);
        *self.ask.write() = built.anthropic.map(|c| Arc::new(meridian_engine::ask_tools::AskService::new(self.engine.clone(), c)));
        Ok(())
    }

    /// Makes one cheap real request against a data source (`alpaca`,
    /// `edgar`, `fred`, `finnhub`, `anthropic`, `coinbase`, `kraken`,
    /// `frankfurter`, `treasury`, `rss`) with the current settings and
    /// describes the result. Fails with the source's own error message.
    pub async fn test_provider(&self, provider: String) -> CoreResult<String> {
        const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
        let timed_out = || CoreError::Provider { message: "No response within 20 seconds".into() };
        if provider == "anthropic" {
            let Some(client) = providers::anthropic(self.secrets.as_ref()) else {
                return Err(CoreError::NotAvailable { reason: "Anthropic API key not set".into() });
            };
            let fut = async move { client.verify_key(meridian_ask::DEFAULT_MODEL).await };
            return match self.on_runtime(async move { tokio::time::timeout(TIMEOUT, fut).await }).await? {
                Ok(Ok(name)) => Ok(format!("Connected · {name} available")),
                Ok(Err(meridian_ask::AskError::Http { status: 401, .. })) => Err(CoreError::Provider { message: "Anthropic rejected the API key".into() }),
                Ok(Err(e)) => Err(CoreError::Provider { message: e.to_string() }),
                Err(_) => Err(timed_out()),
            };
        }
        let p = match providers::single(&provider, &self.econf, self.secrets.as_ref()) {
            Some(Ok(p)) => p,
            Some(Err(e)) => return Err(CoreError::Provider { message: e }),
            None => return Err(CoreError::InvalidInput { message: format!("unknown data source {provider}") }),
        };
        let fut = async move { providers::probe(&provider, p).await };
        match self.on_runtime(async move { tokio::time::timeout(TIMEOUT, fut).await }).await? {
            Ok(Ok(msg)) => Ok(msg),
            Ok(Err(e)) => Err(CoreError::from(meridian_engine::EngineError::Provider(e))),
            Err(_) => Err(timed_out()),
        }
    }

    /// Connects feeds and starts background loops; returns immediately.
    pub fn start(&self) -> CoreResult<()> {
        self.engine.start();
        Ok(())
    }

    pub fn shutdown(&self) -> CoreResult<()> {
        self.engine.shutdown();
        Ok(())
    }

    pub fn data_mode(&self) -> CoreResult<DataModeFfi> {
        Ok(match self.engine.mode() {
            DataMode::Mock => DataModeFfi::Mock,
            DataMode::Live => DataModeFfi::Live,
        })
    }

    // --- command line ---------------------------------------------------

    /// What GO runs for the command line: plain language first (`aapl 5y`,
    /// `aapl filings`, `earnings this week` → `Run`), the mnemonic grammar
    /// unchanged otherwise. Resolves bare tickers against the loaded
    /// instrument index; memory only.
    pub fn parse_command(&self, input: String, loaded: Option<String>) -> CoreResult<ParsedCommandFfi> {
        let loaded: Option<SecurityKey> = loaded.as_deref().and_then(|s| s.parse().ok());
        Ok(match self.engine.interpret(&input, loaded.as_ref()) {
            ParsedCommand::Empty => ParsedCommandFfi::Empty,
            ParsedCommand::Security { security, function, args } => {
                ParsedCommandFfi::Security { security: security.to_string(), function, args }
            }
            ParsedCommand::Function { function, args } => ParsedCommandFfi::Function { function, args },
            ParsedCommand::MenuItem(n) => ParsedCommandFfi::MenuItem { number: n },
            ParsedCommand::Search(text) => ParsedCommandFfi::Search { text },
            ParsedCommand::Run(action) => ParsedCommandFfi::Run { action: action.into() },
        })
    }

    /// Rows for the completion popover, in display order (see `SuggestionFfi`).
    pub fn suggest(&self, input: String, loaded: Option<String>, limit: u32) -> CoreResult<Vec<SuggestionFfi>> {
        let loaded = loaded.as_deref().and_then(|s| s.parse().ok());
        Ok(self
            .engine
            .suggest(&input, loaded.as_ref(), limit as usize)
            .into_iter()
            .map(|s| SuggestionFfi {
                kind: match s.kind {
                    SuggestionKind::Security => SuggestionKindFfi::Security,
                    SuggestionKind::Function => SuggestionKindFfi::Function,
                },
                display: s.display,
                detail: s.detail,
                completion: s.completion,
                group: s.group.heading(),
                title: s.title,
                subtitle: s.subtitle,
                hint: s.hint,
                action: s.action.map(Into::into),
                best: s.best,
            })
            .collect())
    }

    pub fn functions(&self) -> CoreResult<Vec<FunctionInfoFfi>> {
        Ok(registry()
            .iter()
            .map(|f| FunctionInfoFfi {
                mnemonic: f.mnemonic.into(),
                title: f.title.into(),
                description: f.description.into(),
                needs_security: f.needs_security == SecurityNeed::Required,
                category: format!("{:?}", f.category),
                name: f.name.into(),
                summary: f.summary.into(),
            })
            .collect())
    }

    // --- screens --------------------------------------------------------

    pub async fn screen(&self, function: String, security: Option<String>, args: Vec<KeyValue>) -> CoreResult<ScreenFfi> {
        let security = security.as_deref().map(parse_key).transpose()?;
        let req = ScreenRequest { function: function.to_ascii_uppercase(), security, args: from_kv(&args) };
        let engine = self.engine.clone();
        let screen = self.on_runtime(async move { engine.screen(req).await }).await?;
        Ok(screen.into())
    }

    /// Bars and studies for a chart, packed for direct upload to the
    /// renderer. `studies` use `NAME[:p1[:p2]]`, e.g. `SMA:50`, `RSI:14`.
    pub async fn chart_data(&self, security: String, interval: String, range: String, studies: Vec<String>) -> CoreResult<ChartDataFfi> {
        let key = parse_key(&security)?;
        let iv = meridian_types::BarInterval::from_code(&interval)
            .ok_or_else(|| CoreError::InvalidInput { message: format!("bad interval {interval}") })?;
        let studies: Vec<_> = studies.iter().filter_map(|s| meridian_engine::screens::chart::Study::parse(s)).collect();
        let engine = self.engine.clone();
        let data = self.on_runtime(async move { engine.chart_data(&key, iv, &range, &studies).await }).await??;
        let (origin, bars) = meridian_engine::screens::chart::pack_bars(&data.series);
        Ok(ChartDataFfi {
            security,
            interval: data.series.interval.code(),
            count: data.series.len() as u32,
            origin,
            bars,
            studies: data
                .studies
                .iter()
                .map(|st| ChartStudyFfi {
                    name: st.name.clone(),
                    pane: st.pane,
                    values: meridian_engine::screens::chart::pack_values(&st.values, if st.pane == 0 { origin } else { 0.0 }),
                })
                .collect(),
            price_decimals: data.price_decimals,
            sources: data.sources.into_iter().map(Into::into).collect(),
            stale: data.stale,
        })
    }

    // --- ASK ------------------------------------------------------------

    /// Whether an Anthropic API key is configured.
    pub fn ask_available(&self) -> CoreResult<bool> {
        Ok(self.ask.read().is_some())
    }

    /// Answers a question with tool use over local data. Streams through
    /// `observer`; returns the finished turn (answer, sources, number checks).
    pub async fn ask(&self, session_id: String, question: String, security: Option<String>, observer: Arc<dyn AskObserverFfi>) -> CoreResult<AskTurnFfi> {
        let Some(svc) = self.ask.read().clone() else {
            return Err(CoreError::NotAvailable { reason: "add an Anthropic API key in Settings → API Keys, then restart".into() });
        };
        let security = security.as_deref().map(parse_key).transpose()?;
        let obs: Arc<dyn meridian_ask::AskObserver> = Arc::new(AskObserverBridge(observer));
        let sid = session_id.clone();
        let svc2 = svc.clone();
        let outcome = self.on_runtime(async move { svc2.ask(&sid, &question, security, obs).await }).await?;
        match outcome {
            Ok(o) => Ok((&o.turn).into()),
            Err(e) => Err(CoreError::Provider { message: e.user_message() }),
        }
    }

    pub fn ask_cancel(&self, session_id: String) -> CoreResult<()> {
        if let Some(s) = self.ask.read().clone() {
            s.cancel(&session_id);
        }
        Ok(())
    }

    // --- streaming ------------------------------------------------------

    pub fn subscribe(&self, securities: Vec<String>) -> CoreResult<Arc<QuoteSubscriptionFfi>> {
        let keys = securities.iter().map(|s| parse_key(s)).collect::<CoreResult<Vec<_>>>()?;
        Ok(Arc::new(QuoteSubscriptionFfi { inner: self.engine.subscribe(keys) }))
    }

    pub fn hot_row_layout(&self) -> CoreResult<Vec<LayoutFieldFfi>> {
        Ok(meridian_stream::row::LAYOUT.iter().map(|(n, o)| LayoutFieldFfi { name: (*n).into(), offset: *o as u32 }).collect())
    }

    /// Process-local ID for a security (matches `instrument` in hot rows).
    pub fn instrument_id(&self, security: String) -> CoreResult<u32> {
        Ok(self.engine.hub().registry().intern(&parse_key(&security)?))
    }

    // --- data sources ---------------------------------------------------

    pub fn data_sources(&self) -> CoreResult<Vec<DataSourceFfi>> {
        let statuses = self.engine.feed_statuses();
        Ok(self
            .engine
            .data_sources()
            .into_iter()
            .map(|d| {
                let c = d.capabilities;
                DataSourceFfi {
                    provider: d.provider.to_string(),
                    capabilities: c
                        .entries
                        .iter()
                        .map(|e| CapabilityRowFfi {
                            capability: e.capability.label().into(),
                            asset_classes: e.asset_classes.iter().map(|a| a.label()).collect::<Vec<_>>().join(", "),
                            delay: e.delay.badge(),
                            source: e.source.label(),
                            history: e.history.clone(),
                        })
                        .collect(),
                    terms_note: c.terms_note,
                    docs_url: c.docs_url,
                    attribution: c.attribution,
                    requires_credentials: c.requires_credentials,
                    cache_policy: match c.cache_policy {
                        CachePolicy::Unrestricted => "Cache allowed".into(),
                        CachePolicy::MaxAge(d) => format!("Cache max {} h", d.as_secs() / 3600),
                        CachePolicy::NoStore => "Never stored".into(),
                        CachePolicy::PurgeOnUnsubscribe => "Delete when subscription ends".into(),
                    },
                    ai_policy: match c.ai_policy {
                        AiPolicy::Allowed => "May be sent to ASK".into(),
                        AiPolicy::Forbidden => "Excluded from ASK".into(),
                        AiPolicy::Unreviewed => "Excluded from ASK (terms not reviewed)".into(),
                    },
                    connected: statuses.iter().find(|s| s.provider == d.provider).map(|s| s.connected),
                }
            })
            .collect())
    }

    pub fn purge_provider(&self, provider: String) -> CoreResult<String> {
        let r = self.engine.stores().purge_provider(&provider)?;
        Ok(format!("Deleted {} rows and {} files from {provider}", r.rows_deleted, r.files_deleted))
    }

    // --- app state ------------------------------------------------------

    pub fn watchlists(&self) -> CoreResult<Vec<WatchlistFfi>> {
        Ok(self.engine.stores().app.watchlists()?.into_iter().map(|w| WatchlistFfi { id: w.id, name: w.name, securities: w.securities }).collect())
    }

    pub fn create_watchlist(&self, name: String) -> CoreResult<i64> {
        Ok(self.engine.stores().app.create_watchlist(&name, self.engine.now())?)
    }

    pub fn set_watchlist(&self, id: i64, securities: Vec<String>) -> CoreResult<()> {
        Ok(self.engine.stores().app.set_watchlist_items(id, &securities)?)
    }

    pub fn delete_watchlist(&self, id: i64) -> CoreResult<()> {
        Ok(self.engine.stores().app.delete_watchlist(id)?)
    }

    pub fn save_workspace(&self, id: String, name: String, json: String) -> CoreResult<()> {
        Ok(self.engine.stores().app.save_workspace(&id, &name, &json, self.engine.now())?)
    }

    pub fn load_workspace(&self, id: String) -> CoreResult<Option<String>> {
        Ok(self.engine.stores().app.workspace(&id)?.map(|(_, json)| json))
    }

    pub fn workspaces(&self) -> CoreResult<Vec<WorkspaceSummaryFfi>> {
        Ok(self
            .engine
            .stores()
            .app
            .workspaces()?
            .into_iter()
            .map(|w| WorkspaceSummaryFfi { id: w.id, name: w.name, updated_at: w.updated_at })
            .collect())
    }

    pub fn delete_workspace(&self, id: String) -> CoreResult<()> {
        Ok(self.engine.stores().app.delete_workspace(&id)?)
    }

    pub fn push_history(&self, panel: String, command: String) -> CoreResult<()> {
        Ok(self.engine.stores().app.push_history(&panel, &command, self.engine.now())?)
    }

    pub fn history(&self, panel: String, limit: u32) -> CoreResult<Vec<String>> {
        Ok(self.engine.stores().app.history(&panel, limit as usize)?)
    }

    pub fn setting(&self, key: String) -> CoreResult<Option<String>> {
        Ok(self.engine.stores().app.setting(&key)?)
    }

    pub fn set_setting(&self, key: String, value: String) -> CoreResult<()> {
        Ok(self.engine.stores().app.set_setting(&key, &value)?)
    }

    pub fn provider_settings(&self, provider: String) -> CoreResult<Option<String>> {
        Ok(self.engine.stores().app.provider_settings(&provider)?)
    }

    pub fn set_provider_settings(&self, provider: String, json: String) -> CoreResult<()> {
        Ok(self.engine.stores().app.set_provider_settings(&provider, &json)?)
    }
}

/// A view's live quote subscription. Poll at display rate.
#[derive(uniffi::Object)]
pub struct QuoteSubscriptionFfi {
    inner: QuoteSubscription,
}

#[uniffi::export]
impl QuoteSubscriptionFfi {
    /// Rows changed since `since` (0 = everything with data) and the next
    /// `since` value.
    pub fn poll(&self, since: u64) -> CoreResult<PollResultFfi> {
        let (seq, rows) = self.inner.poll(since);
        Ok(PollResultFfi { seq, rows })
    }

    pub fn instrument_ids(&self) -> CoreResult<Vec<u32>> {
        Ok(self.inner.ids().to_vec())
    }
}

/// Reads non-secret provider settings straight from SQLite before the engine
/// opens its stores (the composition root needs them to build providers).
pub(crate) fn read_provider_settings(econf: &EngineConfig, provider: &str) -> Option<serde_json::Value> {
    if econf.in_memory {
        return None;
    }
    let store = meridian_store::AppStore::open(&econf.data_dir.join("app.sqlite")).ok()?;
    let json = store.provider_settings(provider).ok()??;
    serde_json::from_str(&json).ok()
}

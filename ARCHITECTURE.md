# Meridian — Architecture

Status: **implemented through the phases listed in CLAUDE.md**; research-verified 2026-10-05 (see `docs/research/`). Items marked **[DECIDE]** need a decision before or during the named phase; items marked **[VERIFY]** must be confirmed against current docs or by measurement during implementation.

Meridian is a personal-use native macOS financial terminal. The UI is SwiftUI + AppKit, the core is a Rust library exposed to Swift through UniFFI, analytics and history live in DuckDB and Parquet, and app state lives in SQLite.

---

## 1. Principles

1. **Rust owns all market-data state and all business logic.** Swift renders snapshots and diffs, routes keystrokes, and manages windows. SwiftUI views contain no computation beyond layout.
2. **One `Provider` trait.** Every data source (vendors, EDGAR, FRED, the mock) implements it. Nothing above the provider layer knows a vendor exists.
3. **Normalize at the boundary.** Providers convert vendor payloads into internal types (§5) before anything else sees them.
4. **Provenance on every datum.** Every payload carries where it came from, whether it is synthetic, and how delayed it is. The UI always shows mock and delayed data as such.
5. **No silent fallbacks.** If a capability is missing (no provider, not entitled, rate-limited, licensing), the screen shows `NOT AVAILABLE` with the reason. It never substitutes mock data or stale data without labeling it.
6. **Measure, don't guess.** Each performance budget has a benchmark (§12). A phase cannot close if any budget regresses.

---

## 2. Repository layout

```
.
├── CLAUDE.md                 Working rules + commands (read first)
├── ARCHITECTURE.md           This file
├── docs/
│   ├── DATA_PROVIDERS.md     Provider research, comparison table, budget stacks
│   └── adr/                  Architecture decision records (NNNN-title.md)
├── reference/                Fidelity spec: reference/<FUNCTION>/*.png
│   └── compare/              Committed side-by-side comparisons
├── core/                     Rust workspace
│   ├── Cargo.toml
│   ├── crates/
│   │   ├── types/            Normalized domain types. No I/O dependencies.
│   │   ├── provider/         Provider trait, capabilities, ProviderError, router, rate limiting
│   │   ├── provider-mock/    MockProvider: seeded synthetic data for every capability
│   │   ├── provider-<name>/  One crate per real source: coinbase, kraken, alpaca, finnhub, edgar, fred, treasury, frankfurter, rss
│   │   ├── stream/           Subscription hub, market-state cells, hot-path buffers
│   │   ├── store/            SQLite (app state), DuckDB (analytics), Parquet cache, migrations
│   │   ├── analytics/        Indicators, pricing models, greeks/IV, risk, stats, backtester
│   │   ├── command/          Command-line tokenizer/parser, function registry, autocomplete index
│   │   ├── ask/              Anthropic Messages API client, tool registry, audit log, number verifier
│   │   ├── alerts/           Alert rules engine
│   │   ├── import/           Broker CSV import: format detection, normalized portfolio transactions
│   │   ├── engine/           Orchestration: providers by mode, cache, stream/alert loops, screen models, ASK tools
│   │   ├── ffi/              UniFFI surface — the ONLY crate Swift links against
│   │   └── bench/            Load generators and cross-crate benches
│   └── xtask/                Build orchestration: bindings, XCFramework
├── app/
│   ├── project.yml           XcodeGen spec; .xcodeproj is generated
│   ├── Meridian/
│   │   ├── App/              Entry point, AppDelegate, window and screen management
│   │   ├── Shell/            Panels, command line, key routing, Launchpad, link groups
│   │   ├── Design/           Theme tokens, fonts, grid/text primitives, input cells
│   │   ├── Render/           Metal chart renderer, high-frequency grid view
│   │   ├── Functions/<MNEMONIC>/   One folder per function screen
│   │   ├── Bridge/           Swift wrappers over generated UniFFI bindings
│   │   └── Resources/        Fonts (freely licensed only), asset catalog
│   ├── Packages/MeridianCore/  SwiftPM package: binaryTarget(XCFramework) + generated Swift
│   ├── MeridianTests/        Unit + snapshot tests
│   └── MeridianUITests/      Keyboard-driven UI tests, launch-time metrics
├── scripts/                  build-core, capture, compare, palette extraction, name lint
└── .github/workflows/        CI
```

**Why an XcodeGen spec instead of a hand-maintained `.xcodeproj`:** project files are generated deterministically, diffs stay reviewable, and merge conflicts in `project.pbxproj` disappear. The generated project is still a normal Xcode project. Chosen over Tuist (Phase 1).

---

## 3. Module boundaries

```
             ┌─────────────────────────── Swift (app) ───────────────────────────┐
             │  Functions/*  ──►  Shell (panels, keys, links, Launchpad)         │
             │      │                     │                                      │
             │      ▼                     ▼                                      │
             │  Design + Render (grid view, Metal charts, theme)                 │
             │      ▲                                                            │
             │  Bridge (typed wrappers, hot-buffer decoders, event pump)         │
             └──────┼────────────────────────────────────────────────────────────┘
                    │  UniFFI (records, objects, async fns, callback traits, byte buffers)
             ┌──────┼──────────────────────── Rust (core) ───────────────────────┐
             │     ffi  ── thin facade, no logic                                 │
             │      │                                                            │
             │  command   ask   alerts   analytics   stream                      │
             │      │       │      │          │          │                       │
             │      └───────┴──────┴────┬─────┴──────────┘                       │
             │                       provider (router, rate limits, cache)       │
             │                 ┌────────┼──────────┐                             │
             │          provider-mock  provider-edgar  provider-<vendor> …       │
             │                          store (SQLite / DuckDB / Parquet)        │
             │                          types  (depended on by everything)       │
             └───────────────────────────────────────────────────────────────────┘
```

Dependency rules (enforced by crate dependencies, so violations fail to compile):

| Crate | May depend on | Must not depend on |
|---|---|---|
| `types` | std, serde, time | anything with I/O |
| `provider` | `types` | `store`, `stream`, `ffi`, any vendor crate |
| `provider-*` | `types`, `provider` | each other, `store`, `stream` |
| `store` | `types` | `provider-*` |
| `stream` | `types`, `provider` | `store` (persistence of ticks goes through a sink trait) |
| `analytics` | `types` | I/O of any kind. Pure functions over slices/arrays. |
| `command` | `types` | I/O. Pure parser plus an in-memory index. |
| `import` | `types` | I/O and clocks. Pure CSV reader; the caller passes text and the import date. |
| `ask` | `types` | `ffi`, `engine` (the engine supplies tools through the `ToolExecutor` trait) |
| `engine` | everything except `ffi` and concrete `provider-*` crates | `ffi` |
| `ffi` | everything above, incl. concrete providers (composition root) | — (nothing depends on `ffi`) |

On the Swift side:
- `Functions/*` may use `Shell`, `Design`, `Render`, `Bridge`. A function never imports another function.
- `Design` and `Render` know nothing about market data types. They render generic rows, cells, and series.
- `Bridge` is the only Swift code that touches generated UniFFI symbols.

---

## 4. Provider layer

### 4.1 The trait

One trait. Every method has a default that returns `ProviderError::Unsupported`, and the provider declares what it actually supports in `Capabilities`. Routing decisions use `Capabilities`, never trial and error.

```rust
#[async_trait]
pub trait Provider: Send + Sync + 'static {
    fn id(&self) -> ProviderId;
    fn capabilities(&self) -> &Capabilities;

    // Reference data
    async fn search_instruments(&self, q: &InstrumentQuery) -> ProviderResult<Vec<Instrument>>;
    async fn instrument(&self, key: &SecurityKey) -> ProviderResult<Instrument>;

    // Market data
    async fn quotes(&self, ids: &[InstrumentRef]) -> ProviderResult<Vec<Quote>>;
    async fn bars(&self, req: &BarsRequest) -> ProviderResult<BarSeries>;
    async fn option_chain(&self, req: &ChainRequest) -> ProviderResult<OptionChain>;

    // Company data
    async fn fundamentals(&self, req: &FundamentalsRequest) -> ProviderResult<Fundamentals>;
    async fn estimates(&self, req: &EstimatesRequest) -> ProviderResult<Vec<Estimate>>;
    async fn recommendations(&self, req: &RecsRequest) -> ProviderResult<Recommendations>;
    async fn holders(&self, req: &HoldersRequest) -> ProviderResult<Holders>;
    async fn dividends(&self, req: &DividendsRequest) -> ProviderResult<Vec<Dividend>>;
    async fn filings(&self, req: &FilingsRequest) -> ProviderResult<FilingsPage>;
    async fn filing_document(&self, id: &FilingId) -> ProviderResult<FilingDocument>;
    async fn news(&self, req: &NewsQuery) -> ProviderResult<NewsPage>;
    async fn transcripts(&self, req: &TranscriptRequest) -> ProviderResult<Vec<Transcript>>;

    // Macro
    async fn economic_series(&self, req: &SeriesRequest) -> ProviderResult<EconomicSeries>;
    async fn economic_calendar(&self, req: &CalendarRequest) -> ProviderResult<Vec<EconomicEvent>>;

    // Streaming. None if the provider has no push feed.
    fn streaming(&self) -> Option<&dyn StreamingProvider> { None }
}

#[async_trait]
pub trait StreamingProvider: Send + Sync {
    /// Opens the feed. Normalized events are written to `sink`; the provider
    /// owns reconnects and reports state changes through the same sink.
    async fn connect(&self, sink: StreamSink) -> ProviderResult<StreamHandle>;
}

pub trait StreamHandle: Send + Sync {
    fn subscribe(&self, ids: &[InstrumentRef], channels: StreamChannels) -> ProviderResult<()>;
    fn unsubscribe(&self, ids: &[InstrumentRef], channels: StreamChannels) -> ProviderResult<()>;
}
```

`Capabilities` declares, per dataset × asset class:
- whether it is supported;
- `DataDelay` (real-time / delayed N min / EOD) and the true source (e.g. `Sip`, `SingleVenue("IEX")`, `Modelled`). Many "real-time" retail feeds cover one venue only, so the UI must say so.
- history depth, update frequency, and rate limits (token-bucket parameters);
- max symbols and connections per stream;
- licensing terms:
  - `CachePolicy`: `Unrestricted`, `MaxAge(Duration)` (e.g. CoinGecko's 24 h refresh rule), `NoStore`, or `PurgeOnUnsubscribe`;
  - `attribution: Option<String>`: text that must be displayed (FRED, CoinGecko, FINRA);
  - `display_allowed`: some free tiers are "non-display";
  - `ai_allowed`: whether the data may be sent to the ASK model (§11.3).

The store enforces `CachePolicy`. The UI shows attributions on the screens that use the data and on a "Data sources" screen.

`#[async_trait]` is used because the trait must be object-safe (`Arc<dyn Provider>`). Native `async fn` in traits is not dyn-compatible yet. **[VERIFY Phase 1]** that this is still true on the toolchain in use.

### 4.2 Router

`ProviderRouter` is what the rest of the core calls. It
- picks a provider per (dataset, asset class) from user configuration, checked against `Capabilities`;
- enforces per-provider rate limits (token buckets) and concurrency limits;
- reads through the cache (DuckDB/Parquet) for history and fundamentals, with per-dataset TTLs;
- retries retryable errors with exponential backoff and jitter, and opens a per-provider circuit breaker after repeated failures;
- **never** falls back across the mock/live boundary.

Most datasets take the first provider that answers. Where sources complement each other, the caller asks each one: `providers_for(capability, key)` lists them in routing order and `dividends_from(provider, key)` calls one with the same rate limit, retries and breaker but no fall-through. DVD uses this to merge dividend events with ex-dates (a vendor's corporate actions) and per-share totals by fiscal period (SEC XBRL); each source is cached separately, and a failing source leaves only its part `NOT AVAILABLE`.

### 4.3 Data mode

The engine runs in exactly one **data mode**, fixed at launch and shown permanently in the status bar:

- **LIVE** — only real providers are registered. If a dataset has no provider, or its provider lacks credentials, the screen shows `NOT AVAILABLE — <reason>` naming what to add in Settings → Setup. **The app always runs LIVE**; the user does not want a demo mode (2026-10-05).
- **MOCK** — only `MockProvider` is registered and the status bar shows `MOCK DATA`. Reachable solely through `MERIDIAN_MODE=mock`, for snapshot tests (`scripts/capture.sh`, engine snapshot tests) and the perf harness (`scripts/perf-app.sh`). Never offered in the UI.

The two are never mixed in one process (`Engine::new` rejects a mixed provider set). That makes "never silently mix mock and real data" structural rather than a convention.

### 4.4 MockProvider

- Deterministic: seeded RNG (`rand_chacha`) and an injectable clock, so snapshot tests are stable.
- Realistic: GBM with stochastic volatility and jumps for prices; realistic tick sizes, spreads, intraday volume curve (U-shape), market hours and holidays; option chains priced with our own Black-Scholes using a skewed vol surface; ten years of fundamentals with consistent accounting identities (assets = liabilities + equity, etc.); news and filings generated from templates and **visibly marked synthetic**.
- Load mode: N symbols × M updates/sec for the streaming benchmark (§12).
- Synthetic instruments use real-looking tickers, but `Instrument.is_synthetic = true` and every payload's `Provenance.synthetic = true`.

---

## 5. Data contracts (normalized types)

All in `core/crates/types`. Each type is `serde`-serializable for storage and tests. The FFI layer exposes UniFFI records mirroring these types, plus packed buffers for hot paths (§7.3).

### 5.1 Primitives

| Type | Representation | Notes |
|---|---|---|
| `UnixNanos` | `i64` ns since Unix epoch, UTC | All timestamps. Exchange-local time is derived from the instrument's exchange calendar. |
| Prices, sizes | `f64` | Changed from the fixed-point draft: every source delivers binary floats or short decimal strings that `f64` represents exactly to display precision; display rounds to the instrument's `price_decimals`. Avoids conversions on every path. |
| `InstrumentId` | `u32` | Process-local interned handle. Never persisted. |
| `SecurityKey` | `{ symbol, exchange: Option<ExchangeCode>, sector: MarketSector }` | User-facing key, e.g. `AAPL US Equity`. Persisted. |
| `MarketSector` | enum | `Govt, Corp, Mtge, MMkt, Muni, Pfd, Equity, Cmdty, Index, Curncy` |
| `DataDelay` | enum | `RealTime`, `Delayed { minutes }`, `EndOfDay`, `Synthetic` |
| `Provenance` | `{ provider, synthetic, delay, as_of, source_ref }` | Attached to every response type. `source_ref` is a URL, accession number, or vendor ID for audit. |

### 5.2 Domain types (fields abbreviated)

- **Instrument**: `id, key, name, asset_class, currency, exchange_mic, figi?, cik?, tick_size, lot_size, multiplier, is_synthetic`.
- **Quote**: `instrument, bid, ask, bid_size, ask_size, last, last_size, open, high, low, prev_close, volume, vwap?, ts_event, ts_recv, flags (halted, stale, delayed), provenance`.
- **Bar**: `ts_start, interval, open, high, low, close, volume, vwap?, trades?`. `BarSeries` = `{ instrument, interval, adjustment (None | Splits | SplitsAndDividends), bars (columnar), provenance }`.
- **OptionContract**: `underlying, contract_symbol (OCC), expiry, strike, right (Call/Put), style (American/European), multiplier, nbbo?, last?, volume, open_interest, greeks?`. `Greeks` = `{ delta, gamma, theta, vega, rho, iv, source: Vendor | Computed { model, inputs_hash } }`.
- **Filing**: `cik, accession, form_type, filed_at, accepted_at, period_of_report?, items[], primary_document_url, documents[], provenance`.
- **NewsItem**: `id, source, headline, summary?, body? (only if licensed), url, published_at, received_at, tickers[], topics[], provenance`.
- **Estimate**: `instrument, metric (EPS, Revenue, EBITDA, …), fiscal_period, period_type (Q/FY), mean, median?, high?, low?, count?, actual?, as_of, provenance`.
- **Fundamentals**: `instrument, statements[]`. `Statement` = `{ kind (Income/Balance/CashFlow), period_type, fiscal_period, period_end, currency, lines[] }`. `Line` = `{ concept (normalized code), label, value, unit, as_reported_tag? }`.
- **Also**: `Holder`, `Dividend` (event with ex-date), `PeriodDividend` (per-share total for a fiscal period), `ReportedSplit` (split ratio disclosed in filings, no ex-date; also on `Fundamentals`), `CorporateAction`, `Recommendation`, `EconomicSeries`, `Observation`, `EconomicEvent`, `Transcript`, `Portfolio`, `Position`, `Transaction`, `Alert`.

### 5.3 Errors

```rust
#[derive(thiserror::Error, Debug)]
pub enum ProviderError {
    Unsupported { capability: Capability },          // provider doesn't offer it
    NotEntitled { capability: Capability, plan: String }, // offered, but not on this plan
    RateLimited { retry_after: Option<Duration> },
    Unauthorized,                                     // missing or invalid key
    NotFound,
    Network(String),
    Http { status: u16, body_snippet: String },
    Parse { context: String },                        // vendor changed format: bug, not user error
    Upstream(String),
}
```

---

## 6. Storage

| Store | Engine | Contents | Location |
|---|---|---|---|
| App state | SQLite (WAL) via `rusqlite` | workspaces, layouts, panel state, link groups, watchlists, portfolios, transactions, alerts, settings, command history, ASK transcripts and audit log | `~/Library/Application Support/Meridian/app.sqlite` |
| Analytics | DuckDB via `duckdb` crate | normalized bars, fundamentals, estimates, filing and news indexes, economic series, option snapshots, vol surfaces, derived tables | `…/Meridian/market.duckdb` |
| Bulk history | Parquet (hive-partitioned) | intraday bars and long daily histories: `cache/parquet/<dataset>/provider=<p>/symbol=<s>/year=<y>/*.parquet` | `…/Meridian/cache/` |

- Parquet stays in Application Support, not `~/Library/Caches`, because offline mode depends on it and macOS may purge Caches.
- DuckDB reads Parquet in place through views (`read_parquet(..., hive_partitioning = true)`).
- **Concurrency:** one DuckDB database instance per process. `duckdb::Connection` and `rusqlite::Connection` are `Send` but `!Sync`, so each thread owns its own connection via `try_clone()`. One writer thread serializes writes; readers use per-thread clones on the blocking pool. ASK gets a **separate read-only connection** limited to an allowlist of views.
- **DuckDB build:** the `bundled`, `parquet`, and `appender-arrow` features are statically linked, and extension autoinstall/autoload is disabled. Hardened Runtime library validation would block downloaded extensions anyway. Arrow types come from the `duckdb::arrow` re-export (duckdb pins arrow 58), never a separately versioned `arrow` crate.
- **Migrations:** embedded, numbered SQL files. SQLite uses `PRAGMA user_version`; DuckDB uses a `schema_version` table. Migrations run at startup before any service starts, and a failed migration aborts startup with a clear error rather than running on a half-migrated schema.
- **Every cached row carries its `provider` column, and the Parquet path includes `provider=`.** Some vendor terms (e.g. Finnhub) require deleting their data when the subscription ends. `store` exposes `purge_provider(id)`, which removes that provider's rows and files from all three stores, and Settings exposes it per provider.
- Secrets never go in any of these stores (§10).

---

## 7. Swift ↔ Rust boundary

### 7.1 Shape

UniFFI with proc-macros (no UDL). The `ffi` crate exposes:

- `Core` (object): created once at launch with `CoreConfig` (data mode, paths, log level). Owns the tokio runtime, router, stores, and services. `Core::new` is synchronous and must return in under 100 ms. Anything slower happens asynchronously after first paint.
- Service objects obtained from `Core`: `MarketService`, `ReferenceService`, `CompanyService`, `NewsService`, `FilingService`, `AnalyticsService`, `CommandService`, `AskService`, `AlertService`, `WorkspaceStore`, `SecretsService`.
- **Records** for low-frequency data (instrument details, statements, news pages, filing metadata).
- **Async functions** for anything doing I/O. They map to Swift `async throws`.
- **Callback interfaces** (Swift implements, Rust calls) for low-frequency push events: `CoreEventSink` (provider status, alert fired, news arrived, cache progress) and `AskObserver` (streamed tokens and tool-call events).
- **Packed byte buffers** for high-frequency data (§7.3).

Build (UniFFI 0.32.x, verified 2026-10-05; `cargo-swift` does not support 0.32 yet, so it isn't used):
1. `cargo build --release --target aarch64-apple-darwin -p meridian-ffi`, producing a `staticlib`. Arm64 only; macOS 28 drops Intel.
2. `uniffi-bindgen-swift` in library mode generates the Swift sources, headers, and modulemap.
3. `xcodebuild -create-xcframework` produces `MeridianCoreFFI.xcframework`.
4. `Packages/MeridianCore` contains a `binaryTarget` for the XCFramework and a Swift target for the generated bindings. That target uses **`nonisolated` default actor isolation**, so a MainActor-default app target can't capture the generated code.
5. The app links libc++, because DuckDB is C++. Exact flags come from `--print native-static-libs`.

`xtask build-ffi` wraps all five steps.

### 7.2 Rules

1. **Sync FFI calls must not block.** A synchronous exported function may only touch memory. Target < 1 ms; hot-path polls < 0.2 ms. Anything that can wait on I/O or a lock held across I/O is `async`.
2. **Async calls run on our tokio runtime.** Exported async functions use `#[uniffi::export(async_runtime = "tokio")]`. Any work that must survive the call is spawned onto `Core`'s runtime.
3. **Nothing crosses the FFI per tick.** Streaming data is pulled at display rate as batched buffers. Rust never calls Swift once per market-data update.
4. **No large record lists.** Lifting `Vec<Record>` in Swift costs ~3.7 µs per record (UniFFI issue #3013). Any result that can exceed ~500 rows (bars, chains, screener results, holders, history tables) is returned as a packed columnar `Vec<u8>` with a documented layout, decoded in `Bridge/`.
5. **Callbacks hop to the main actor.** Callback implementations on the Swift side immediately dispatch to `@MainActor` (or an actor they own). They never do work on the Rust thread that delivered the event. Foreign traits implement `From<UnexpectedUniFFICallbackError>`. Callback objects must not hold strong references back to Rust objects, because cycles leak.
6. **Every export returns `Result`.** A panic inside a non-throwing export is an uncatchable fatal error in Swift. With `Result`, a panic surfaces as a Swift error. A panic is still a bug: the panic hook logs a backtrace, and the crash reporter (Phase 9) captures it.
7. **Explicit cancellation.** UniFFI does not propagate Swift task cancellation into Rust. Long or streaming async calls take a `CancelToken` object; Bridge wraps each call in `withTaskCancellationHandler { … } onCancel: { token.cancel() }`.
8. Swift 6.4 language mode with strict concurrency. UniFFI's generated async code is not fully `Sendable` yet (issue #2448). `Bridge/` wraps it where needed.

### 7.3 Hot path: streaming quotes

Market state lives in `stream` as a table of **cells**, one per `InstrumentId`. Each cell holds the latest normalized quote and a `version: u64` stamped from a global atomic sequence on every write.

```
provider WS ──► normalize ──► ingest channel (bounded) ──► apply thread ──► cells[id] (version = ++global_seq)
                                                                                 ▲
Swift display link (per window, ≤ 120 Hz) ── subscription.poll(since_seq) ───────┘
                                              returns QuoteBatch { seq, bytes }
```

- **Subscription**: a Swift view (monitor, panel) creates a `QuoteSubscription` with its instrument IDs. `poll(since)` walks those IDs and copies cells whose `version > since` into a packed buffer. With 2,000 IDs this is an atomic load per ID plus a copy per changed cell, in the low microseconds.
- **Coalescing is automatic**: 50 ticks between two frames produce one row with the latest values.
- **Buffer layout** (`core/crates/stream/src/row.rs`): 128 bytes per row, little-endian — `instrument u32, changed u32, ts_event i64`, then `bid ask last open high low prev_close volume bid_size ask_size last_size net_change pct_change` as `f64` (NaN = missing), then `flags u32`. Rust exports the offsets (`hot_row_layout()`); Swift verifies them at startup and in a unit test, so the sides cannot drift silently. `changed` carries per-field change bits since the poll's `since` (used to flash cells).

- **Backpressure**: the ingest channel is bounded. On overflow the apply thread conflates (keeps latest per instrument) rather than dropping or blocking the socket reader. The overflow count is a metric.
- **Rendering**: streaming grids use a custom layer-backed `NSView` (`Render/TerminalGridView`). Each visible row (± 3 rows of overscan) is its own `CALayer` with an 8-bit (`RGBA8Uint`), opaque backing store; a live update calls `setNeedsDisplay` on that row's layer only, so AppKit never unions dirty rects into large repaints. Text is drawn as cached `CTLine`s (keyed by text, color, weight) on fixed monospace cell metrics; number formatting on this path avoids `NumberFormatter`/`DateFormatter`. Cell flashes expire on one shared 0.1 s timer. `NSTableView` and SwiftUI `List` are not used for streaming grids. Core Text holds the budget (§12), so the Metal glyph-atlas fallback was not needed.

### 7.4 Hot path: chart series

- `Core.chart_data(...)` returns packed columns: `ts i64[n]`, OHLC as `f32` relative to an `f64` origin (precision), volume `f32`, plus study lines (`f32`, overlays relative to origin, lower panes absolute) computed in `analytics`.
- The renderer (`Render/ChartView.swift`) decimates per pixel column at draw time (min/max of each bucket), so primitives per frame stay ≈ 2× the pixel width regardless of series length; pan and zoom only change the visible index window.
- **Renderer decision (resolved)**: GP/GIP use Core Graphics (`PriceChartNSView`) with the per-pixel decimation above. Because work per frame is bounded by pixel width, 1M bars render well inside the 16.7 ms frame budget (§12), so Metal was not needed. Swift Charts is used only for small static charts (ERN, payoff and curve plots), where Apple publishes no throughput figures and a DTS forum case shows ~2,900 scrolling points saturating a core.

---

### 7.5 Screen model

Function screens are built in Rust (`engine/src/screens/*`) as a generic `Screen`: a title, numbered menu (`Action`s), source badges, a status (`Ok` / `NotAvailable{reason}` / `Error`), and blocks — `Fields`, `Table` (columns may bind to a live hot-row field), `Text`, `Inputs` (editable amber cells; changing one re-requests the screen with that argument), `Notice`, `Chart` (spec; Swift fetches `chart_data`), `Xy` (small embedded charts; bar charts may carry category labels shown instead of numeric x ticks), `Heat`, and `Diff`. Swift renders every function with one renderer (`Render/ScreenView.swift`, `TerminalGridView`), so screens get consistent look and keyboard behavior and all logic stays testable in Rust. Swift-native views exist only where interaction demands it: the price chart, ASK, and Launchpad.

## 8. Threading model

| Thread / pool | Owner | Runs | Must never |
|---|---|---|---|
| Main thread | Swift | AppKit/SwiftUI, key routing, display-link polls, Metal encode | do I/O, decode large payloads, wait on Rust async |
| Display link | Swift (`NSView.displayLink` / `CADisplayLink` on macOS 14+) | triggers `poll()` per visible subscription | — |
| `meridian-io` (tokio, multi-thread) | Rust | provider REST/WS, router, ASK HTTP/SSE, async FFI calls | run CPU-heavy work > 1 ms |
| `meridian-apply` (1 dedicated OS thread) | Rust | applies normalized stream events to cells | block on I/O |
| Blocking pool (`spawn_blocking`) | Rust | DuckDB/SQLite reads, Parquet I/O | — |
| `meridian-db-writer` (1 dedicated thread) | Rust | serialized DuckDB writes and appends | — |
| Compute pool (rayon) | Rust | backtests, Monte Carlo VaR, IV surface fits, correlation matrices | — |

Default tokio worker count is `min(4, performance cores)`. It is configurable and will be tuned against the CPU budget.

Cancellation: Swift task cancellation does **not** reach Rust on its own (§7.2 rule 7). Cancellable calls take a `CancelToken`. Rust side: `tokio_util::sync::CancellationToken` for async work, plus periodic checks in rayon jobs (backtests, Monte Carlo).

---

## 9. Shell, command line, and keyboard

### 9.1 Panels

- A **panel** is the unit of work: its own command line, function stack (for MENU/back), loaded security, and link group.
- The default main window holds a 2×2 grid of panels. **Launchpad** windows host floating or tiled components (monitors, charts, news, function panels) across multiple pages and displays. Layouts persist in SQLite.
- **Link groups**: each panel or component may join a group, labeled by letter (A, B, C…), as in the incumbent's Group Manager. There are two kinds: *security groups* (members follow one security) and *monitor groups* (a monitor's selected row drives news/chart members). Loading a security in one member publishes `(group, SecurityKey)` and the other members load it. The link bus lives in `Shell` (Swift), because it is UI coordination, not market data. Group membership is persisted via `WorkspaceStore`.

### 9.2 Command line

- **Plain language first (1.1):** `aapl`, `aapl 5y`, `aapl filings`, `aapl vs msft 5y`, `earnings this week`, `cpi`, `ask <question>`. `command::interpret` reads plain phrases on top of the mnemonic grammar and returns `ParsedCommand::Run(Action)` (function, security, named arguments) for anything it resolves; input the grammar already gives a meaning keeps it. Bare tickers need the instrument index, so the engine calls `interpret` with its `SuggestIndex` as the `SecurityResolver`; `command` stays pure. The vocabulary, precedence and ambiguity rules are in `core/crates/command/src/plain.rs` and `docs/FUNCTIONS.md`.
- Grammar: `[<SECURITY> [<EXCHANGE>] <SECTOR>] [<FUNCTION>] [<ARGS>…] <GO>`, e.g. `AAPL US <EQUITY> DES <GO>`. `DES <GO>` alone applies to the panel's loaded security. Unchanged by plain language.
- Parser and autocomplete live in Rust (`command`): pure functions plus an in-memory index of instruments (ticker, name, aliases) and functions (mnemonic, title, keywords). The budget is < 50 ms from keystroke to rendered suggestions; the parser and index lookup target < 2 ms for mnemonic queries and < 5 ms for plain-language ones (measured ≤ 0.32 ms over 20,000 instruments, `benches/suggest.rs`), so most of the budget is left for rendering.
- **Suggestions** feed a grouped popover (`docs/DESIGN.md`): each row has a group (`Security`, `On AAPL`, `Commands`, `Functions`, `Compare AAPL with`), a plain title and subtitle, a right-side hint (the mnemonic), the Tab completion, an optional resolved action for Return, and a `best` flag on the row Return runs by default. When `interpret` resolves the input, that row is the best one, so Return on the highlight and GO on the text agree. Rows arrive in display order.
- **App actions:** `SETTINGS` and `IMPORT` come back as `Run` actions but have no engine screen; the app handles them.
- **Function registry**: Rust holds metadata (mnemonic, title, security types and sectors accepted, argument schema). Swift maps mnemonic → view factory. A test fails if the two sets differ, so a function can't exist on one side only.
- Numbered menu items: `<n> <GO>` selects item *n* on the current screen.

### 9.3 Keys

Standard mapping verified from public keyboard guides (sources in `docs/FUNCTIONS.md`):

| Action | Default binding | Notes |
|---|---|---|
| GO | Return | Standard |
| CANCEL | Esc | Standard |
| HELP | F1 and ⌘? | F1 is standard; ⌘? per brief. Pressed twice opens help index. Pressed after typed words, it searches. |
| MENU (back / related functions) | ⌘[ · End · Delete on an empty command line | No standard key equivalent exists; all three are bound |
| END/BACK | End | Standard on newer keyboards; previous screen |
| PAGE FWD / PAGE BACK | PgDn / PgUp, ⌘↓ / ⌘↑ | `<n>` + PAGE FWD jumps *n* pages |
| PANEL (cycle panels) | ⌃Tab / ⌃⇧Tab (⌘1–⌘4 jump) | No standard key equivalent |
| Sector keys | F2 GOVT, F3 CORP, F4 MTGE, F5 M-MKT, F6 MUNI, F7 PFD, F8 EQUITY, F9 CMDTY, F10 INDEX, F11 CRNCY | Configurable. Mac keyboards send media keys on the F-row unless fn is held or the system setting is changed, so offer alternatives (e.g. ⌃F-key or ⌥1…⌥0) |
| Keyboard overlay | ⌘/ | Ours |

Key routing is an `NSEvent` local monitor in `Shell` that translates physical keys into semantic `TerminalKey` events before any view sees them. That keeps the mapping configurable and testable. Every workflow is covered by keyboard-only UI tests.

---

## 10. Secrets

- API keys live in the macOS **login keychain** as generic-password items (service `meridian.provider.<id>`, account = field). The data-protection keychain was the first plan, but its access group requires a provisioning profile; the login keychain does not, and with stable signing its ACL survives rebuilds. Keys are never stored in SQLite, config files, logs, environment variables, or the repo.
- **Swift owns Keychain I/O** (`Bridge/Keychain.swift`, `SecItem*`). Rust asks for a key through the `SecretSource` foreign trait when the composition root builds providers (`ffi/src/providers.rs`).
- In Rust memory, keys are held as `secrecy::SecretString` (zeroized on drop, redacted `Debug`).
- **Signing:** the app is signed with a stable Apple Development identity from Phase 1. Ad-hoc signing changes the designated requirement on every build, which would re-prompt for keychain access on every rebuild (TN3127).
- **No App Sandbox** for this personal build (it is only required for the Mac App Store). On macOS 27 a sandbox container would block CLI inspection of the DuckDB/Parquet files. Hardened Runtime stays on.
- `scripts/` and CI contain no keys. Live-provider integration tests are opt-in and run inside the app's test host, which has the keychain entitlement.

---

## 11. ASK (AI analyst)

### 11.1 Flow

```
user question ─► AskService ─► Messages API (streaming SSE)
                    ▲   │            │
                    │   │      tool_use blocks
                    │   ▼            ▼
                    │  tool registry (validate input against JSON Schema) ─► local tools
                    │                                                         │
                    └──────── tool_result blocks + audit records ◄────────────┘
final text ─► number verifier ─► ASK screen (answer + numbered SOURCES list)
```

- **Transport**: there is no official Anthropic Rust SDK, so `ask` calls `POST https://api.anthropic.com/v1/messages` directly (`reqwest` + an SSE parser), with headers `x-api-key`, `anthropic-version: 2023-06-01`, `content-type: application/json`. The manual tool-use loop runs until `stop_reason` is `end_turn`.
- **Model**: `claude-opus-5-5` by default, configurable. Thinking is adaptive (always on for this model); `output_config.effort` is user-configurable (default `high`). Forced `tool_choice` (`any`/`tool`) is rejected by this model, so tool use is `auto`, tools use `strict: true`, and the system prompt says which tools to use.
- **Streaming**: SSE events are forwarded to Swift via `AskObserver`. Client tools set `eager_input_streaming: true`, which means **we** must validate every tool input against its schema before running it, check `stop_reason` for `max_tokens` and `refusal` before executing tools, and return `is_error: true` results for invalid input.
- **Refusal fallback**: send the server-side `fallbacks` parameter with its beta header, and show which model answered. **[VERIFY Phase 8]** exact parameter form and header version against current docs.
- **Prompt caching**: the system prompt and tool definitions are byte-stable and cached (explicit `cache_control` on the last system block). The current date and the panel context go in the user turn, never in the system prompt. Cache hits are verified via `usage.cache_read_input_tokens` and logged.

### 11.2 Tools (all read-only)

| Tool | Does |
|---|---|
| `sql_query` | Runs SQL on the read-only DuckDB connection, restricted to allowlisted views, with row limit and timeout. The SQL text is shown to the user. |
| `get_quote`, `get_bars`, `get_option_chain` | Market data via the router |
| `get_fundamentals`, `get_estimates`, `get_holders`, `get_dividends` | Company data via the router |
| `search_news`, `search_filings`, `get_filing_section` | News and filings |
| `compute` | Named `analytics` functions (returns, CAGR, correlation, regression, VaR, Black-Scholes, etc.). All derived numbers must come from here. |
| `get_watchlist`, `get_portfolio` | User data |
| `show_chart`, `show_table` | Push a series or table into a target panel. UI-only; no data changes. |

### 11.3 Honesty guarantees

1. **Audit log**: every tool call is recorded (tool, validated input, SQL text, provider calls with provenance, row count, duration, result hash) and shown in the ASK screen as numbered SOURCES.
2. **Number verifier**: after the final answer, Rust extracts every numeric token and checks it against the numbers in that turn's tool results (with rounding tolerance and unit/scale normalization). Unmatched numbers are highlighted as `UNVERIFIED` in the UI rather than hidden. Dates, fiscal years, and tickers are excluded by rule.
3. Derived figures must be computed with `compute`, so they appear in tool results and pass the verifier.
4. **Data leaving the machine**: ASK sends tool results (market data) to Anthropic. Some vendors' terms may restrict passing their data to third-party services. Per-provider flags in `Capabilities` can exclude a provider's data from ASK. **[VERIFY before Phase 8]** each chosen vendor's terms.

---

## 12. Performance budgets and measurement

| Budget | Target | How it is measured |
|---|---|---|
| Cold launch | < 1.5 s to first interactive frame | `XCTApplicationLaunchMetric` UI test, plus an `os_signpost` from process start to first command-line focus |
| Command line response | < 50 ms keystroke → suggestions rendered; GO → function shell painted | signposts around key event → frame commit; criterion bench for parser/index (target < 2 ms) |
| Streaming | 2,000 symbols, 60 fps, < 10% CPU (whole process) | MockProvider load mode (2,000 symbols, realistic tick rates); frame-time histogram from display-link timestamps; CPU via `task_info` sampling over 60 s |
| Charts | Pan/zoom 10y daily and 1M intraday bars at 60 fps | Scripted pan/zoom in a UI test, frame-time p99 < 16.6 ms |

**How the app budgets are measured now:** `scripts/perf-app.sh` runs the release app with `MERIDIAN_PERF=1` (`App/PerfHarness.swift`): process start → first panel loaded; suggestion latency over 8 inputs; GO → screen loaded for 4 commands; 2,000 synthetic symbols at 3 updates/s each into a live grid in an on-screen 1600×1000 window for 20 s (frames from the 60 Hz display clock, CPU from `getrusage` for the whole process, the main window's four live panels included); chart frames via `cacheDisplay` over 240 pan/zoom steps.

**Measured 2026-10-05** (Apple M5 Pro, release build, three runs; `bench/results/app-2026-10-05-run{1,2,3}.json`):

| Budget | Result | Status |
|---|---|---|
| Cold launch < 1.5 s | 0.66 s first run, 0.17 s warm | Met |
| Suggestions < 50 ms | ≤ 0.07 ms | Met |
| GO → screen loaded < 50 ms | 10–47 ms; the first command within ~0.5 s of launch took 83–92 ms in 2 of 3 runs while startup work was still running | Met after startup; first command can exceed |
| 2,000 symbols at 60 fps, < 10% CPU | 60.0 fps; median 8.6% CPU over seven runs (2026-10-05 and 10-07; range 7.2–11.5%, two runs slightly over, the later ones with a second Meridian instance running); feed alone 4.5–6.6%; main-loop interval p99 ≈ 18 ms | Met at the median; at the edge |
| Charts 60 fps (p99 < 16.7 ms) | 1M bars p99 8.1–9.0 ms; 10y daily p99 4.4–5.1 ms | Met |

What got streaming CPU from ~25% to under 10%: number/time formatting without `NumberFormatter`/`DateFormatter`, cached `CTLine`s, one 8-bit `CALayer` per visible row instead of AppKit dirty-rect unions, the mock feed ticking at 25 ms instead of 10 ms, and the hub's idle wakeup raised from 10 ms to 100 ms.

- Rust benches use `criterion` (parser, cell apply/poll, BS/binomial, IV solver, decimation, DuckDB query templates).
- Results go to `bench/results/<date>.json`; `scripts/bench-check` compares against `bench/baseline.json` and fails on a regression beyond noise threshold (10% for every metric).
- CI runs build and tests on every push. Hosted runners aren't M-series-class or quiet enough for performance gating, so budgets are gated locally at the end of each phase and the results are committed.

---

## 13. Error handling and observability

- **Rust**: `thiserror` enums per crate; `anyhow` only in tests, benches, and `xtask`. Each crate's errors convert into the FFI `CoreError`, which becomes a Swift `throws`:

```rust
#[derive(uniffi::Error, thiserror::Error, Debug)]
pub enum CoreError {
    NotAvailable { capability: String, reason: String }, // unsupported / not entitled / licensing
    RateLimited  { provider: String, retry_after_ms: Option<u64> },
    Unauthorized { provider: String },
    NotFound     { what: String },
    InvalidInput { field: String, message: String },
    Network      { provider: String, message: String },
    Storage      { message: String },
    Cancelled,
    Internal     { message: String }, // bug: logged with context
}
```

- **Every function screen has explicit states**: `loading`, `loaded`, `empty`, `notAvailable(reason)`, `error(code, message)`, `stale(as_of)`. There is no blank screen and no fabricated placeholder data.
- **Streams**: reconnect with backoff; after reconnect, instruments are marked stale until their first fresh update, and the UI shows it.
- **Logging**: Rust `tracing` → unified logging (`os_log`) under subsystem `meridian.core`; Swift `Logger` under `meridian.app`. Both show up together in Console.app. Secrets are never logged (`SecretString` redacts).
- **Crash reporting (Phase 9)**: local only. MetricKit diagnostics plus a Rust panic hook writing to `…/Meridian/crashes/`. No third-party crash service.

---

## 14. Testing

| Layer | Approach |
|---|---|
| Rust analytics and parsers | Unit tests with known reference values (e.g. Black-Scholes against published textbook values, binomial converging to BS for European options); property tests (`proptest`) for parsers and invariants |
| Providers | Recorded-fixture tests (vendor JSON in `tests/fixtures`) for normalization; live tests opt-in only |
| FFI | Swift tests that exercise each exported API against MockProvider |
| Function screens | Snapshot tests rendering each screen offscreen at fixed size with seeded mock data and a fixed clock |
| Fidelity | `scripts/capture` (`screencapture -l <window>`) + `scripts/compare` → `reference/compare/<FUNCTION>.png` (reference, ours, diff), committed |
| Keyboard | UI tests that drive every workflow with keys only |
| Performance | §12 |

---

## 15. Visual system (pending references)

- Theme tokens (colors, font sizes, row heights, grid metrics) live in `Design/Theme.swift`, generated from `Design/tokens.json`.
- **Colors are not chosen by eye.** `scripts/palette` samples reference PNGs and emits candidate hex values with pixel counts. Tokens are filled from that output once references exist.
- Font: **Iosevka Fixed, SS08 stylistic set** (SIL OFL 1.1, no Reserved Font Name), fetched by `scripts/fetch-fonts.sh` from the official release (not committed, size) and bundled with its license notice. It is the only freely licensed monospace that is natively narrow (0.500 em vs the usual 0.600) and can be built narrower. It covers box-drawing, block, and Braille glyphs, which are useful for grids and sparklines. Phase 2 starts from the prebuilt family and makes a custom narrower build (renamed family) only if the references call for it. Fallback: IBM Plex Mono. The incumbent's font is proprietary and is not used. Research: `docs/research/tech-stack.md`.
- Panels have no rounded corners, shadows, translucency, or SF-style controls. Editable fields use a distinct input-cell style taken from the references.
- `scripts/check-names` fails CI if the forbidden brand name appears anywhere under `app/`, `core/`, or bundled assets.

---

## 16. Decisions

Resolved during the uninterrupted build (2026-10-05); the user asked not to be consulted per phase, so documented defaults were taken.

| # | Decision | Resolution |
|---|---|---|
| 1 | Data budget tier | **Free ($0).** Ask before any paid integration |
| 1a | Demo mode | **None in the app** (user, 2026-10-05). MOCK only for tests and benchmarks (§4.3) |
| 2 | Reference screenshots | **Still open.** None provided; screens follow documented conventions until references exist |
| 3 | MENU and PANEL keys | MENU = ⌘[ / End / Delete on empty line; PANEL = ⌃Tab (§9) |
| 3a | Quote monitor form | Both: `W` worksheet and the Launchpad Monitor component |
| 4 | Project generation | XcodeGen |
| 5 | Signing identity | Apple Development, team H7T2D2GL7U, hardened runtime, no sandbox |
| 6 | Chart renderer | Core Graphics with per-pixel decimation (§7.4); Swift Charts for small static charts |
| 7 | Non-standard mnemonics | `ASK` is Meridian's own (the incumbent uses ASKB); flagged in `docs/FUNCTIONS.md` |

# Meridian — working notes for Claude

Native macOS financial terminal for personal use. SwiftUI + AppKit UI, Rust core via UniFFI, DuckDB / SQLite / Parquet storage, Anthropic API for the ASK analyst. Read `ARCHITECTURE.md` before changing module boundaries, data contracts, threading, or the FFI.

**Current phase:** 1.1.0 (2026-10-07): original "Instrument" redesign (user decision, 2026-10-07: the 1.0 look read as a copy of another terminal) plus Today home, broker CSV import, Calendar, Filings inbox, Compare and plain-language commands. 1.0.0 released 2026-10-07; phases 1–9 implemented. The user asked for the whole build without per-phase check-ins, so defaults were taken where decisions were open: free data tier only, `W` and Launchpad Monitor both, MENU = ⌘[ / End / Delete on an empty line, PANEL = ⌃Tab, layouts from documented conventions (no references exist). **The app runs on real data only** (user decision, 2026-10-05); see Hard rules. Remaining work is in Known gaps.

Key docs: `docs/DATA_PROVIDERS.md` (provider comparison and budget stacks), `docs/research/*` (cited research), `docs/FUNCTIONS.md` (mnemonic registry and status), `reference/README.md` (reference inventory).

## Hard rules

- **Never use the Bloomberg name, logo, wordmarks, or icons** in the app, its code identifiers, strings, or assets. Docs may mention it only to describe UX conventions. `scripts/check-names` enforces this in CI (from Phase 1).
- **Never fake data or stub success.** If something can't be done (licensing, API limits, missing data), say so and propose the closest alternative. Screens show `NOT AVAILABLE — <reason>` instead of placeholders.
- **Real data only in the app.** The user does not want a demo mode. The app always starts LIVE; `MockProvider` is reachable only via `MERIDIAN_MODE=mock` for snapshot tests and the perf harness, never from the UI, and is labeled `MOCK DATA` when running. Mock and live never mix in one process (ARCHITECTURE §4.3).
- **Personal use only.** Don't design features that redistribute market data.
- **Secrets only in the macOS Keychain.** Never in code, config, logs, env files, or tests.
- **Don't guess APIs.** Check the vendor's current docs before writing integration code; cite the doc URL in the provider crate's README.
- **Original design only.** Follow `docs/DESIGN.md` (the Instrument design system). Never imitate another terminal's look: no black-and-amber palette, colored function bars or yellow-key chrome. No generic AI-styled UI: no gradients, glass, purple accents, emoji or pill badges everywhere.
- **Commands:** existing mnemonics stay for compatibility, but plain language comes first (`aapl 5y`, `aapl filings`, `earnings this week`). New features get plain-word function ids (e.g. `TODAY`, `FILINGS`, `CALENDAR`), not new cryptic mnemonics.
- **Don't integrate a paid provider.** The chosen tier is free ($0); ask before adding anything paid (see `docs/DATA_PROVIDERS.md`).
- **Don't fill in the user's personal details** (e.g. their email as the SEC EDGAR contact). They enter them in Settings → Setup.

## Process

1. **Plan before each phase.** (Waived for the initial build at the user's request; resume for new work of similar size.)
2. Implement, then test: Rust unit tests for all analytics and parsers; snapshot tests for every function screen.
3. **Design check** for each screen: build → render with `scripts/capture` → compare against `docs/DESIGN.md` and the mockup → fix spacing, type and color until it matches.
4. **Measure budgets** (ARCHITECTURE §12). A phase fails if any budget regresses.
5. **End of phase:** update this file and `ARCHITECTURE.md`, list known gaps, say what's next, commit.

## Phases

1. Skeleton: Xcode project, Rust workspace, UniFFI bridge, MockProvider, CI build
2. Shell: command line + parser, sector/special keys, panels, Launchpad, visual system from references
3. Core market: quote monitor, watchlists, GP/GIP/HP from mock, then the first real provider
4. News, fundamentals, filings
5. Derivatives (Black-Scholes + binomial in Rust, tested against known values)
6. Analytics
7. Macro and cross-asset
8. ASK + alerts
9. Perf pass, fidelity pass, crash reporting, offline mode from cache

## Conventions

**Rust (`core/`)**
- Edition 2024, workspace lints: `clippy::pedantic` warn, `unsafe_code` deny except in `ffi` and `stream` (hot buffers), each `unsafe` block with a `// SAFETY:` comment.
- Errors: `thiserror` in libraries, `anyhow` only in tests/benches/xtask. No `unwrap()`/`expect()` outside tests except on proven invariants with a comment.
- Timestamps `i64` Unix nanos UTC; prices `i64` fixed-point 1e-9 (ARCHITECTURE §5.1). Convert to `f64` only inside `analytics`.
- `analytics` and `command` are pure: no I/O, no clocks, no randomness without an injected seed.
- Every provider crate has `README.md` with the doc URLs it was built against and its rate limits.
- **FFI rules** (ARCHITECTURE §7.2):
  - Every `#[uniffi::export]` returns `Result`. A panic in a non-throwing export is a fatal crash.
  - Never return `Vec<Record>` that can exceed ~500 rows; use a packed columnar `Vec<u8>`.
  - Sync exports must not block.
  - Cancellable async calls take a `CancelToken`; UniFFI doesn't propagate Swift cancellation.
- Pinned (verified 2026-10-05):
  - `uniffi 0.32`
  - `tokio ~1.53` (LTS)
  - `duckdb 1.10506` (`bundled`, `parquet`, `appender-arrow`; use `duckdb::arrow`, never a separate `arrow`)
  - `rusqlite 0.40` (`bundled`)
  - `tokio-tungstenite 0.30` (`rustls-tls-native-roots`)
- DuckDB extension autoinstall/autoload is disabled.

**Swift (`app/`)**
- Swift 6 language mode, strict concurrency. The generated UniFFI bindings live in their own SwiftPM target with `nonisolated` default isolation.
- Keychain I/O lives in Swift (`Bridge/Keychain.swift`, data protection keychain). Rust pulls keys via the `SecretSource` foreign trait.
- Sign with a stable Apple Development identity, never ad-hoc. Ad-hoc signing re-prompts for keychain access on every build. No App Sandbox; Hardened Runtime on.
- Views contain layout only. Formatting goes through `Design/TerminalNumberFormatter` using display hints from Rust.
- Only `Bridge/` imports the generated UniFFI module.
- Streaming grids use `Render/TerminalGridView`, not `List`/`Table`/`NSTableView`.
- Inside panes follow `docs/DESIGN.md`: no cards, fills or rounded boxes; the only shadow and rounding is on popovers. Settings stays a native macOS window.

**General**
- Commits: imperative subject, scope prefix (`core:`, `app:`, `docs:`, `ci:`), body says why.
- New dependencies need a one-line justification in the commit body. Licenses must be permissive (MIT/Apache/BSD/OFL/zlib).

## Commands

```
# Rust core
cargo test --manifest-path core/Cargo.toml --workspace
cargo clippy --manifest-path core/Cargo.toml --workspace --all-targets -- -D warnings
scripts/bench-check.sh                 # criterion benches vs bench/baseline.json

# Core → XCFramework + Swift bindings (debug by default; perf runs need release)
scripts/build-core.sh [release]

# Full app build (fonts, core, xcodegen, xcodebuild)
scripts/build-app.sh [debug|release]
xcodebuild -project app/Meridian.xcodeproj -scheme Meridian -destination 'platform=macOS,arch=arm64' test

# Screens: snapshot tests (core) and rendered PNGs (app, mock data)
UPDATE_SNAPSHOTS=1 cargo test --manifest-path core/Cargo.toml -p meridian-engine --test screens
scripts/capture.sh "DES|AAPL US Equity;W" [out_dir] [WxH]
scripts/compare.sh                     # needs reference/<FUNCTION>/*.png

# Install to ~/Applications (stable Dock/Spotlight path); --build, --dock
scripts/install-app.sh --build

# App icon (renders app/Meridian/Assets.xcassets/AppIcon.appiconset)
swift scripts/make-icon.swift

# App budgets (release build): writes bench/results/app-<date>.json
scripts/perf-app.sh

# Name check
scripts/check-names.sh
```

## Environment (verified 2026-10-05)

macOS 27.0.1 · Xcode 27.0 (Swift 6.4) · rustc 1.93.0 (CI pins the same) · Apple M5 Pro · xcodegen (Homebrew). `uniffi-bindgen-swift` is built from the workspace by `scripts/build-core.sh`. Signing: Apple Development, team H7T2D2GL7U. Keychain items live in the login keychain (the data-protection keychain needs a provisioning profile), service `meridian.provider.<name>`.

## Known gaps

- Screens are checked against `docs/DESIGN.md` by rendering them with mock data (`scripts/capture.sh`, plus `MERIDIAN_SNAPSHOT_IMPORT=<csv>` for the importer); there is no automated pixel comparison.
- **Free tier limits (live):** US equities are IEX-only (single exchange) with 30 streamed symbols on Alpaca Basic; options are Alpaca's indicative feed; no consensus estimates (EE), holders (HDS) or transcripts from any free source. WEI shows US-listed ETF proxies, not index levels.
- **ASK has not run against the live API** (no Anthropic key yet); it's covered by recorded-fixture tests. Alpaca, SEC EDGAR, FRED, Finnhub, Coinbase, Kraken, Frankfurter, Treasury and RSS have all run live.
- FA per-share values before a stock split are as reported (a notice says so). CORP/MUNI/MTGE bond pricing is not obtainable on the free tier.
- Launchpad components are tiled in one window; no floating windows or multi-monitor persistence.
- Crash reporting is the Rust panic hook (writes reports to the data directory); no MetricKit.
- The release app is signed with an Apple Development certificate but not notarized (needs a paid Developer ID).
- The 2,000-symbol streaming budget is measured with the synthetic load generator; real data at that scale needs a consolidated (SIP) plan. 1.1 re-runs (`bench/results/app-2026-10-07-v1.1-run*.json`) were taken with another app using most of a core: 8–14% total, with the grid's share over the feed-only baseline unchanged from 1.0 (2–6 points). Re-measure on an idle machine.
- **FRED's release calendar is sometimes slow:** `releases/dates` answered in 0.27 s at one point and 13–16 s twenty minutes later (2026-10-07). TODAY shows "Coming up: loading…" and fills in; CALENDAR waits for all three kinds, so it spins meanwhile. Next step: give CALENDAR the same per-section loading as TODAY.
- **Not yet run live in 1.1:** the Finnhub earnings calendar and Alpaca corporate-actions calendar paths, and TODAY's progressive load against real network latency (fixtures and mock only).
- **Broker CSV import** (`core/crates/import`, sources in its README): Robinhood, Fidelity, Schwab, Vanguard, positions snapshots and a column-mapping path; no broker publishes a spec, so formats come from parsers and published exports. Options, short sales, mergers, bonds and 401(k) rows are warnings, not imports; no FX conversion; holdings rebuilt from a date-limited history can have gaps (PORT flags them). The import sheet (`App/ImportSheet.swift`) has been run end to end on fixtures and a positions file in mock mode, not on a real broker export.

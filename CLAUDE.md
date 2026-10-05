# Meridian — working notes for Claude

Native macOS financial terminal for personal use. SwiftUI + AppKit UI, Rust core via UniFFI, DuckDB / SQLite / Parquet storage, Anthropic API for the ASK analyst. Read `ARCHITECTURE.md` before changing module boundaries, data contracts, threading, or the FFI.

**Current phase:** Pre-code done (research + design docs). Waiting on the user for: budget tier, reference screenshots, and the open decisions in `ARCHITECTURE.md` §16. Phase 1 has not started.

Key docs: `docs/DATA_PROVIDERS.md` (provider comparison and budget stacks), `docs/research/*` (cited research), `docs/FUNCTIONS.md` (mnemonic registry and status), `reference/README.md` (reference inventory).

## Hard rules

- **Never use the Bloomberg name, logo, wordmarks, or icons** in the app, its code identifiers, strings, or assets. Docs may mention it only to describe UX conventions. `scripts/check-names` enforces this in CI (from Phase 1).
- **Never fake data or stub success.** If something can't be done (licensing, API limits, missing data), say so and propose the closest alternative. Screens show `NOT AVAILABLE — <reason>` instead of placeholders.
- **Mock data is always labeled.** The app runs in MOCK or LIVE mode, never both (ARCHITECTURE §4.3).
- **Personal use only.** Don't design features that redistribute market data.
- **Secrets only in the macOS Keychain.** Never in code, config, logs, env files, or tests.
- **Don't guess APIs.** Check the vendor's current docs before writing integration code; cite the doc URL in the provider crate's README.
- **Don't invent layouts.** Every function screen is built from `reference/<FUNCTION>/*.png`. If there's no reference, ask.
- **Don't invent mnemonics.** Use standard ones; if unsure, flag it in `docs/FUNCTIONS.md` and ask.
- **Don't integrate a paid provider** until the user has picked a budget tier (see `docs/DATA_PROVIDERS.md`).

## Process

1. **Plan before each phase.** Show the plan and wait for approval.
2. Implement, then test: Rust unit tests for all analytics and parsers; snapshot tests for every function screen.
3. **Fidelity loop** for each screen: build → render → `scripts/capture` → `scripts/compare` against the reference → list differences → iterate until only data differs (not layout, typography, or color). Commit the side-by-side to `reference/compare/`.
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
- No rounded corners, shadows, materials, or system-styled controls inside panels.

**General**
- Commits: imperative subject, scope prefix (`core:`, `app:`, `docs:`, `ci:`), body says why.
- New dependencies need a one-line justification in the commit body. Licenses must be permissive (MIT/Apache/BSD/OFL/zlib).

## Commands

Available from Phase 1. Until then these are the planned entry points.

```
# Rust core
cargo test --manifest-path core/Cargo.toml --workspace
cargo clippy --manifest-path core/Cargo.toml --workspace --all-targets -- -D warnings
cargo bench --manifest-path core/Cargo.toml -p meridian-bench

# Build XCFramework + Swift bindings
cargo run --manifest-path core/Cargo.toml -p xtask -- build-ffi

# App
xcodegen generate --spec app/project.yml
xcodebuild -project app/Meridian.xcodeproj -scheme Meridian -destination 'platform=macOS' build test

# Fidelity
scripts/capture <FUNCTION>
scripts/compare <FUNCTION>

# Budgets
scripts/bench-check
```

## Environment (verified 2026-10-05)

macOS 27.0.1 · Xcode 27.0 (Swift 6.4) · rustc 1.93.0 · Apple M5 Pro. Not yet installed: `uniffi-bindgen-swift`, `duckdb` CLI, `xcodegen`.

## Known gaps

- No reference screenshots exist yet (`reference/README.md` lists what's needed).
- No budget tier chosen, so no paid provider may be integrated.
- MENU and PANEL key bindings undecided; quote-monitor form (`W` vs Launchpad Monitor) undecided.
- Index data (WEI) not yet researched. CORP/MUNI/MTGE bond pricing is not obtainable at the planned budgets.
- Vendor terms on sending data to the ASK model are unchecked (gate before Phase 8).

# Research: tech stack versions, FFI behavior, fonts, platform

Verified 2026-10-05 via crates.io, `cargo info`, official docs, and by parsing published font files (downloaded to a scratch directory only). UNVERIFIED means no official source was found. Toolchain: every crate below declares an MSRV ≤ 1.89, so rustc 1.93.0 is fine. Xcode 27 ships Swift 6.4.

## UniFFI 0.32.2 (2026-09-23)

Sources: https://crates.io/crates/uniffi · https://github.com/mozilla/uniffi-rs/blob/v0.32.2/CHANGELOG.md · https://mozilla.github.io/uniffi-rs/latest/

| Topic | Finding |
|---|---|
| Async | Rust `async fn` → Swift `async`. `#[uniffi::export(async_runtime = "tokio")]` (feature `tokio`) wraps futures in `async_compat::Compat`; works on trait exports since 0.32.0 |
| **Cancellation** | **Not supported.** Swift task cancellation does not drop the Rust future. Docs say to build your own cancel mechanism |
| Foreign traits | `#[uniffi::export(foreign)]`; traits must be `Send + Sync + Debug`. Implement `From<uniffi::UnexpectedUniFFICallbackError>` or generated code panics. Rust↔Swift reference cycles leak |
| Objects | `Arc<T>` → Swift protocol + class; passed as a `u64` handle since 0.30 |
| Errors | `Result<T, E: Error enum>` → Swift `throws` |
| **Panics** | A panic in a **non-throwing** export becomes an uncatchable Swift fatal error. **Every export must return `Result`** |
| Swift 6 | "Partial support… most generated code will conform to `Sendable`… async code will not" (issue #2448 open). Generated protocols are `Sendable` since 0.29.1. Put the bindings in their own SwiftPM target with `nonisolated` default isolation so MainActor-default (SE-0466) doesn't capture them |
| Build | cargo-swift 0.11.1 supports UniFFI ≤ 0.31.1 only, so don't use it. Pipeline: staticlib → `uniffi-bindgen-swift --swift-sources / --headers / --modulemap` (library mode) → `xcodebuild -create-xcframework` → SPM `binaryTarget` + Swift target |
| **Perf** | Records and sequences are serialized through `RustBuffer`. **Issue #3013 (2026-09-27):** lifting `Vec<Record>` in Swift costs ~3.7 µs/record on M2 Pro (Kotlin: 0.2–0.3 µs), so 50k records ≈ 185 ms. Per-call scalar overhead: unmeasured, so measure it |
| Bytes | Zero-copy Swift `Data` → Rust `&[u8]` since 0.32.0 (sync functions only). Rust → Swift `Vec<u8>` is copied once through `RustBuffer` |
| Linking | DuckDB is C++, so the app links libc++. Get the exact flags from `cargo rustc --release -- --print native-static-libs` |

## Crates

| Crate | Version | Notes |
|---|---|---|
| `duckdb` | 1.10506.0 (= DuckDB 1.5.6, 2026-09-30) | Features: `bundled`, `parquet`, `json`, `appender-arrow`, `vtab-arrow`. **Depends on `arrow ^58`**: use the `duckdb::arrow` re-export or pin arrow to 58.x. `Connection` is `Send + !Sync`; `try_clone()` gives one connection per thread. Within one process, appends never conflict |
| `rusqlite` | 0.40.2 | `bundled` (SQLite 3.53.x). `Connection` is `Send + !Sync` |
| `arrow` / `parquet` | 60.0.0 | Don't depend on these directly; go through DuckDB (version skew) |
| `tokio` | 1.53.2 | **1.53.x is LTS until Sept 2027**; pin `~1.53` |
| `tokio-tungstenite` | 0.30.0 | Most used. Has `rustls-tls-native-roots`; no permessage-deflate |
| `tokio-websockets` | 0.13.3 | SIMD (NEON), Autobahn-passing; benchmark candidate |
| `yawc` | 0.4.2 | Has deflate; MPL-2.0 |
| `security-framework` | 3.7.0 | Data protection keychain from Rust is possible |

## Keychain, signing, sandbox

- In-process library code gets keychain access from the **host app's entitlements** (TN3137), so Rust and Swift behave the same. Data-protection-keychain access groups need a provisioning profile.
- **Ad-hoc ("Sign to Run Locally") builds have a designated requirement tied to that exact build** (TN3127). Every rebuild re-prompts for keychain access. **Sign with a stable Apple Development identity from day one.**
- App Sandbox is optional outside the Mac App Store. If enabled, it needs `com.apple.security.network.client`. ATS doesn't cover Rust sockets.
- macOS 27 denies other teams' processes access to app containers by default, and CLI tools without a bundle ID can't be granted access. This argues for **no sandbox**, so the DuckDB/Parquet files stay inspectable with CLI tools.
- Under Hardened Runtime, library validation blocks downloaded DuckDB extensions. Link `parquet`/`json` statically and **disable extension autoinstall**.
- UserNotifications: call `requestAuthorization`; implement `willPresent` so alerts show while frontmost. `.timeSensitive` needs an entitlement in the provisioning profile; free Personal Team support UNVERIFIED.

## Charts

- Swift Charts vectorized plots (`LinePlot`, `AreaPlot`, `BarPlot`, `PointPlot`, `RectanglePlot`, `RulePlot`; macOS 15+) are for "large collections". Apple publishes **no** throughput figures. A DTS forum thread shows 2,880 scrolling points at 100% CPU, with advice to slice to the visible window.
- "1M points at 60 fps in Swift Charts" has no official support. Metal is the realistic path for GP/GIP; Swift Charts is fine for small/static charts.
- Sources: https://developer.apple.com/documentation/charts/lineplot · https://developer.apple.com/videos/play/wwdc2024/10155/ · https://developer.apple.com/forums/thread/763757

## Fonts

Under OFL, bundling is allowed with the license notice, and modified builds must drop any Reserved Font Name.

| Font | License | Width | Box/Block glyphs |
|---|---|---|---|
| **Iosevka** 34.9.0 | OFL, no RFN | **0.500 em** normal (others are 0.600); custom builds can go narrower (`shape`). Term/Fixed variants for grids, no ligatures | Box, block and Braille coverage documented |
| IBM Plex Mono | OFL (RFN "Plex") | 0.600 em only | Full |
| JetBrains Mono | OFL | 0.600 em | Full |
| Source Code Pro / Fira | OFL | 0.600 em | Full / near-full |
| Monaspace | OFL | 0.620–0.775 em (wider only) | Full |
| Martian Mono | OFL | `wdth` 75–112.5 (true condensed) | **No box drawing** |
| Input Mono | **Not OFL; app embedding requires a paid license** | Has condensed | — |
| Berkeley Mono, PragmataPro | **Commercial** | Condensed | — |

**Pick: Iosevka** (Fixed or Term, no ligatures). Start with the prebuilt 0.500 em; do a custom narrower build with a renamed family only if references demand it. Fallback: IBM Plex Mono.

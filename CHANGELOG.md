# Changelog

## 1.0.0 — 2026-10-07

First public release.

### Terminal
- Command line with the `<security> <yellow key> <function> <GO>` syntax, autocomplete for securities and functions, numbered selection, and MENU/back.
- Four linked panels (link groups A–D), F-key sectors (or ⌥1–⌥0), and Launchpad for tiled monitors, charts and news.
- 33 functions across equities (DES, GP, GIP, HP, FA, ERN, ANR, DVD), monitors (W, MOST, WEI, CRYP, FXC), news and filings (TOP, N, CN, CF), derivatives (OMON, OVDV, OVME), analytics (EQS, RV, CORR, PORT, BTST, ALRT), macro (ECO), navigation (SECF, BLP, MENU, HELP) and AI (ASK). EE and HDS say NOT AVAILABLE: no free source provides consensus estimates or holders.
- HELP: command syntax, the key map and a clickable directory of every function; `HELP <function>` explains one. F1 opens help for the current screen, twice for the directory.
- ASK: an analyst built on Claude that answers through tools over the terminal's own data and flags numbers it can't trace to a source.

### Data
- Real data only, from free official sources: Alpaca (US equities, IEX on the free plan; news; corporate actions; option chains), SEC EDGAR (company search, as-reported financials, filings, dividends per share), FRED, Finnhub, Coinbase, Kraken, ECB reference rates via Frankfurter, the U.S. Treasury par curve, and press-release RSS feeds.
- Dividends combine Alpaca corporate actions (ex, record and pay dates) with SEC per-share data and flag stock splits inside the window.
- World indices are shown through clearly labeled US-listed ETF proxies, since index levels aren't available from free sources.
- Financial statements use the most recently filed (restated) value for each period.
- Every screen shows its sources and their delay; anything no source provides says NOT AVAILABLE with the reason.

### App
- Settings with Data Sources (status per source, Save and Test with a real request, changes apply without restarting), General, Storage, Keyboard and About.
- API keys are stored only in the macOS Keychain.
- Local cache in DuckDB and SQLite with an offline fallback.
- Measured on an M5 Pro: cold launch 0.17–0.66 s, 2,000 streaming symbols at 60 fps on a median 8.6% of one core, 1M-bar charts at under 10 ms per frame.

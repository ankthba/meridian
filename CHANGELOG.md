# Changelog

## 1.1.0 — 2026-10-07

A new look of its own, and features built around your own portfolio.

### Design
- New Instrument design ([`docs/DESIGN.md`](docs/DESIGN.md)): graphite surfaces and bone text with no accent color; green and red only for up and down. SF Pro for interface text, SF Mono for numbers. Replaces the black-and-amber look and the bundled font.
- One command bar at the top of the window runs in the focused pane. Suggestions are grouped (the security, things to open on it, commands) and the best match runs on Return.
- Pane headers show a plain name (Overview, Chart, Financials), the security, the screen's sections as tabs, and the data sources with their delay.
- Line charts fit their axis to the data; chart, table and help typography redone.

### Features
- **Today**, the new home pane: portfolio value and today's change, markets, holdings (or your watchlist), upcoming earnings, ex-dividend dates and economic releases, and unread filings.
- **Broker import**: Robinhood, Fidelity, Charles Schwab and Vanguard activity exports, Fidelity and Schwab positions files, and any other CSV with a column mapping. Preview first (format, date range, counts, skipped rows with reasons), then import into a new or existing portfolio. Re-importing an overlapping export adds only new rows. Portfolios now track dividends, interest, fees, splits, transfers and cash, with realized P&L and income.
- **Calendar**: earnings with consensus estimates (Finnhub), ex-dividend and split dates (Alpaca corporate actions) and high-importance economic releases (FRED), for holdings, watchlists or everything.
- **Filings inbox**: new SEC filings from what you hold and watch, with read and unread state.
- **Compare**: up to eight securities rebased to 100, with returns, market cap, P/E, net margin and dividend yield.
- **Plain-language commands**: `aapl 5y`, `aapl filings`, `aapl vs msft`, `earnings this week`, `cpi`, `ask …`. Mnemonics still work.

### Changes
- The default layout is Today, a watchlist, a chart and the filings inbox. Layouts saved by 1.0 are reset once.
- `MERIDIAN_LAYOUT=showcase` opens Today, crypto, a 5-day NVDA chart and world indices.
- Today shows prices and holdings at once; the calendar, filings and news fill in as each arrives, and stay up while they refresh.
- The macro calendar no longer marks releases FRED dates every day as high importance (FRED lists the FOMC Press Release daily, so it isn't a meeting calendar).
- The local database gains a transaction ledger (schema v2) and filing read state (v3); 1.0 databases migrate on launch.

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

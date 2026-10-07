# Meridian user guide

Meridian is a market terminal for macOS that you drive from a command line. This guide covers installing it, connecting data sources, the command syntax and keys, and every function. Inside the app, `HELP <GO>` (or F1) shows the same material for the screen you're on.

## Install

**Download:** get `Meridian-1.0.0.zip` from the [latest release](https://github.com/ankthba/meridian/releases/latest), unzip it, and move `Meridian.app` to Applications. The app is signed but not notarized, so macOS blocks the first launch. Open it once, then go to **System Settings → Privacy & Security**, find the message about Meridian and click **Open Anyway**. You only have to do this once. (Alternatively, in Terminal: `xattr -dr com.apple.quarantine /Applications/Meridian.app`.)

**Build from source:** see [Building](../README.md#building) in the README.

Meridian needs macOS 15 or later on Apple silicon.

## Connect data sources

Meridian shows real data only. A screen whose source isn't connected says `NOT AVAILABLE` and names what to add; it never fills in. On first launch Settings opens on **Data Sources**. Each source has a page with what it unlocks, a link to get a key, and **Save and Test**, which stores the key in your macOS Keychain, connects it immediately (no restart), and makes one real request so you can see it works.

| Source | What you need | How to get it | Unlocks |
|---|---|---|---|
| Alpaca | Key ID and secret key | Create a free account at [alpaca.markets](https://alpaca.markets), then **Generate New Keys** on the dashboard. The secret is shown once. Paper-trading keys work for market data. | US stock quotes and streaming, charts, history, news, dividends, world-index ETF proxies, option chains |
| SEC EDGAR | Your name and email | Nothing to sign up for. The SEC's fair-access policy requires apps to identify themselves; Meridian sends this line with each request. | Company search, descriptions, financial statements, filings, dividends per share |
| FRED | API key | Request a free key at [fred.stlouisfed.org](https://fred.stlouisfed.org/docs/api/api_key.html). | Economic series and release calendar |
| Finnhub | API key | Free tier at [finnhub.io](https://finnhub.io). | Market headlines, company news, analyst ratings, earnings history |
| Anthropic | API key | [Get an API key](https://platform.claude.com/docs/en/get-api-key). Billed per use; testing the key costs nothing. | ASK and AI filing summaries |

Coinbase, Kraken, the ECB (via Frankfurter), the U.S. Treasury and press-release feeds need nothing and are always on.

**About the free stock feed:** Alpaca's free plan gives real-time prices from the IEX exchange only, not the consolidated market, and streams up to 30 symbols at once (the rest refresh every 15 seconds). Screens label these prices `IEX`. Alpaca's paid plan switches the feed to SIP in Settings.

## The command line

Every panel has a command line. Type what you want in plain words, then press Return (GO):

```
aapl                          the overview
aapl 5y                       chart over a range (1d 5d 1m 3m 6m ytd 1y 2y 5y 10y max)
aapl filings                  or news, financials, balance sheet, earnings, analysts, dividends,
                              options, volatility, peers, history, backtest, 10-k
aapl vs msft 5y               performance and key figures side by side
earnings this week            the earnings calendar; also dividends next week
cpi · jobs · gdp · rates      economic series and the Treasury yield curve
news · crypto · fx · world    markets
today · portfolio · settings  your home screen, holdings and setup
ask <question>                the AI analyst
```

A security can be a ticker (`aapl`, `brk.b`), a coin (`btc`) or a currency pair (`eurusd`). A word on its own like `chart` or `options` applies to the panel's security. Anything Meridian doesn't recognize searches for securities. The full vocabulary and how ambiguous words are read (for example `gdp` when a ticker GDP exists) are in [FUNCTIONS.md](FUNCTIONS.md#plain-language-commands-11).

Mnemonics still work exactly as before:

```
<security> <yellow key> <function> <GO>      AAPL US <EQUITY> DES <GO>
<function> <GO>                              DES <GO>   (on the panel's loaded security)
<security> <yellow key> <GO>                 loads the security and opens its function menu
<n> <GO>                                     selects numbered item n on the screen
any other text <GO>                          searches for securities (SECF)
```

- **Securities** are `SYMBOL [EXCHANGE] <yellow key>`. US equities don't need the exchange: `AAPL <EQUITY>` and `AAPL US Equity` both work. Currencies and crypto use `<CRNCY>`: `EURUSD <CRNCY>`, `BTCUSD <CRNCY>`.
- **Yellow keys** are the F-keys F2–F11, or ⌥1–⌥0 (`GOVT CORP MTGE M-MKT MUNI PFD EQUITY CMDTY INDEX CRNCY`). You can also type the word in angle brackets.
- **Autocomplete** suggests securities, things to open on a security (`aapl fil` → Filings), commands and functions as you type: ↑ ↓ to choose, Tab to accept.
- **Arguments** follow the function: `GP 5Y` sets the range, `CN rate cut` filters news, `FA BS` opens the balance sheet, `HELP GP` explains GP. Screens also have amber input fields; edit them and press GO.
- **MENU** (⌘[, End, or Delete on an empty line) goes back to the previous screen.

## Panels, linking and Launchpad

The main window has four panels. ⌃Tab moves between them and ⌘1–⌘4 jumps to one. Each panel has a link group letter (A–D) in its corner: panels in the same group follow each other's security, so loading MSFT in one updates the others. **Launchpad** (`BLP <GO>` or ⇧⌘L) opens a separate window of tiled monitors, charts and news across pages. Layouts are saved when you quit; **Settings → General** can reset them.

**Showcase:** to open four live screens for a quick tour (DES on Apple, the crypto monitor, a 5-day NVDA chart and world indices) without touching your saved layout, quit Meridian and run:

```bash
open -a Meridian --env MERIDIAN_LAYOUT=showcase
```

`MERIDIAN_LAYOUT` also takes your own screens, e.g. `"DES|MSFT US Equity;W;GP|TSLA US Equity|range=1Y;TOP"`. The layout lasts for that session only.

## Keys

| Key | Action |
|---|---|
| Return | GO |
| Esc | CANCEL |
| ⌘[ · End · Delete on an empty line | MENU (back) |
| F1 · ⌘? | HELP for the current screen; press twice for the function directory |
| PgDn / PgUp · ⌘↓ / ⌘↑ | Page forward / back |
| ⌃Tab / ⌃⇧Tab · ⌘1–⌘4 | Next / previous panel, or jump to one |
| F2–F11 · ⌥1–⌥0 | Yellow keys (GOVT … CRNCY) |
| ⌘/ | Keyboard reference |
| ⌘, | Settings |

Mac F-keys send media keys unless you hold fn or turn on **Use F1, F2, etc. keys as standard function keys** in System Settings → Keyboard. ⌥1–⌥0 always work.

## Functions

`HELP <function> <GO>` shows each function's arguments and data sources inside the app.

| Area | Function | What it shows | Example |
|---|---|---|---|
| Equities | `DES` | Description: price, valuation, financial highlights, chart, company info | `AAPL US <EQUITY> DES` |
| | `GP` | Price graph with studies (SMA, EMA, Bollinger, RSI, MACD, stochastic, ATR, OBV, VWAP), trend lines and pan/zoom | `AAPL US <EQUITY> GP 5Y` |
| | `GIP` | Intraday price graph | `AAPL US <EQUITY> GIP` |
| | `HP` | Historical prices, daily/weekly/monthly | `AAPL US <EQUITY> HP` |
| | `FA` | Income statement, balance sheet, cash flow, ratios (as reported to the SEC) | `MSFT US <EQUITY> FA` |
| | `ERN` | Earnings history against estimates | `NVDA US <EQUITY> ERN` |
| | `ANR` | Analyst rating distribution and consensus | `AAPL US <EQUITY> ANR` |
| | `DVD` | Dividend history with ex/record/pay dates, dividends per share, splits | `KO US <EQUITY> DVD` |
| | `EE` | Consensus estimates (not available from free sources) | `NVDA US <EQUITY> EE` |
| | `HDS` | Holders (not available from free sources) | `AAPL US <EQUITY> HDS` |
| Monitors | `W` | Worksheet: live watchlists you can add to and create | `W` |
| | `MOST` | Most active, gainers, losers | `MOST` |
| | `WEI` | World equity indices, shown through US-listed ETF proxies | `WEI` |
| | `CRYP` | Crypto monitor, real time | `CRYP` |
| | `FXC` | FX cross-rate matrix (ECB reference rates) | `FXC` |
| News and filings | `TOP` | Top market headlines | `TOP` |
| | `N` | News menu; `N` with Press Releases for company press releases | `N` |
| | `CN` | Company news | `AAPL US <EQUITY> CN` |
| | `CF` | SEC filings with an AI summary and a diff against the prior filing | `AAPL US <EQUITY> CF` |
| Derivatives | `OMON` | Option chain with implied volatility and Greeks computed by Meridian | `AAPL US <EQUITY> OMON` |
| | `OVDV` | Volatility surface and smile | `SPY US <EQUITY> OVDV` |
| | `OVME` | Option and strategy valuation (Black–Scholes and binomial) with payoff | `AAPL US <EQUITY> OVME` |
| Analytics | `EQS` | Equity screener on fundamentals and performance | `EQS` |
| | `RV` | Relative valuation against peers | `MSFT US <EQUITY> RV` |
| | `COMPARE` | Securities side by side: performance rebased to 100, price changes, market cap, P/E, net margin, dividend yield | `aapl vs msft 5y` |
| | `CORR` | Correlation matrix of returns | `CORR` |
| | `PORT` | Portfolio: transactions, holdings, performance and risk | `PORT` |
| | `BTST` | Backtester (SMA cross, RSI reversion, breakout, MACD, buy and hold) | `SPY US <EQUITY> BTST` |
| | `ALRT` | Price, % change, volume and news-keyword alerts, delivered as notifications | `ALRT` |
| Macro | `ECO` | Economic calendar and FRED series; `ECO` with view curve for the Treasury curve | `ECO` |
| Navigation | `SECF` | Security finder | `SECF APPLE` |
| | `BLP` | Launchpad | `BLP` |
| | `MENU` | Related functions for the loaded security | `MENU` |
| | `HELP` | Syntax, keys and the function directory; with a topic, one function | `HELP GP` |
| AI | `ASK` | Analyst built on Claude | `ASK` |

Options data on Alpaca's free plan is its "indicative" feed: derived quotes, with trades delayed 15 minutes. Implied volatility and Greeks are computed by Meridian from the bid/ask midpoint, with the 3-month Treasury yield as the rate.

## ASK

ASK answers questions about markets and securities from the terminal's own data. It calls tools (quotes, bars, financials, filings, news, economic series, a read-only SQL query over the local database) and lists its sources. Every number in an answer is checked against the data it retrieved; numbers it can't trace are highlighted. Sources whose terms don't allow AI use are never sent to the model. ASK needs an Anthropic API key.

## Data, delays and the cache

- Each screen shows a badge per source with its delay: `RT` (real time), `DLY 15` (delayed), `EOD` (end of day), and the venue, e.g. `IEX`.
- Data is cached locally in DuckDB according to each provider's terms. If you're offline, cached data is shown with an `OFFLINE — showing cached data from …` notice.
- **Settings → Storage** shows the data folder and deletes cached data per source. API keys are never stored there; they live only in the macOS Keychain.

## Troubleshooting

| Symptom | Fix |
|---|---|
| A screen says `NOT AVAILABLE — … key not set` | Add that source in Settings → Data Sources. |
| `NOT AVAILABLE — … not found` | The source has no data for that security, e.g. an ETF has no SEC company filings or analyst ratings. |
| Settings says **Can't connect** | The message under **Connection** is the provider's own error, e.g. a rejected key. Paste the key again and press Save and Test. |
| Stock prices differ from your broker | The free Alpaca feed is IEX only. Spreads can look wide outside market hours. |
| F-keys change volume or brightness | Hold fn, change the Keyboard setting above, or use ⌥1–⌥0. |
| "Meridian can't be opened" on first launch | System Settings → Privacy & Security → **Open Anyway** (once). |

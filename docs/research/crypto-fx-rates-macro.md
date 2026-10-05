# Research: crypto, FX, rates, macro and economic calendar

Researched 2026-10-05 against official pages. "search-only" means only a search-result summary was available (the page was JS-rendered, 403/503, or timed out). UNVERIFIED means not confirmed.

## Crypto

| Provider | Price/mo | Real-time | Streaming | Limits | History | Terms | Sources |
|---|---|---|---|---|---|---|---|
| **Coinbase Advanced Trade WS** / Exchange feed | Free, no account | Yes | `wss://advanced-trade-ws.coinbase.com` (level2, ticker, ticker_batch, market_trades, candles; most channels need no auth); `wss://ws-feed.exchange.coinbase.com` | 8 connections and 8 unauthenticated msgs/s per IP; must subscribe within 5 s. Exchange REST 10 req/s | Exchange candles 300 per request | Data-use terms UNVERIFIED | docs.cdp.coinbase.com/coinbase-app/advanced-trade-apis/websocket/websocket-overview ; …/websocket-rate-limits ; docs.cdp.coinbase.com/exchange/websocket-feed/overview |
| **Kraken WS v2** | Free, no account | Yes | `wss://ws.kraken.com/v2` (ticker, book, ohlc) | ~150 connect attempts / 10 min per IP | REST OHLC is the last 720 candles only. **Free CSV OHLCVT dumps** cover full history to 2026-06-30, updated quarterly | — | docs.kraken.com/api/docs/guides/spot-ws-intro ; support.kraken.com/articles/360047124832 |
| Binance.com | — | — | — | — | — | **US is a restricted location** (search-only). Not used. | developers.binance.com |
| Binance.US | Free | Yes | `wss://stream.binance.us:9443` | 6,000 weight/min | UNVERIFIED | Unsupported in 16 states/territories incl. NY, TX, WA (search-only) | docs.binance.us |
| Bybit | — | — | — | — | — | US excluded (search-only). Not used. | — |
| **CoinGecko** | Demo $0; Basic $35; Analyst $129; Lite $499 | ~60 s freshness | WS from Basic (beta, 0.1 credit/msg) | Demo 10k credits/mo, 100/min | Demo 1 y; Basic 2 y; Analyst+ full | **Attribution required. Cache must refresh within 24 h; storing restricted.** | coingecko.com/en/api/pricing ; coingecko.com/en/api_terms |
| CoinDesk Data (ex-CryptoCompare) | **Free tier retired 2026-05-21**; sales pricing | — | — | — | — | — | data.coindesk.com/blogs/changes-to-coindesk-data-indices-api-free-tier-access |
| CoinMarketCap | $0 / $29 / $79 / $299 / $699 | 60 s on Basic | WS from $79 | 50–1,200/min | Basic ~1 mo intraday; Startup+ all-time | Commercial allowed | coinmarketcap.com/api/pricing/ |
| **Massive Currencies** (crypto + FX in one plan) | Basic $0 (EOD); **Starter $49** (real-time) | Starter | WS (trades, quotes, per-second/minute aggregates) + flat files | Starter unlimited | Starter 10+ y | Personal, non-business | massive.com/currencies ; massive.com/legal/individuals-terms-of-service |

## FX

| Provider | Price | Real-time | Streaming | Limits | History | Terms | Sources |
|---|---|---|---|---|---|---|---|
| **OANDA v20** | Free with a (demo) account | Yes | **HTTP chunked streaming**, not WS | 120 req/s, 20 streams per IP | 5,000 candles/request (search-only) | Personal token; data terms UNVERIFIED | developer.oanda.com/rest-live-v20/introduction/ ; …/best-practices/ |
| **Massive Currencies** | $49 Starter (shared with crypto) | Yes | WS | Unlimited | 10+ y | Personal | massive.com/currencies |
| Twelve Data | $0 / $79 / $229 / $999 | Yes | WS trial-only below Pro | 8/min free | UNVERIFIED | **Free = "internal non-display"**; display requires Grow | twelvedata.com/pricing ; twelvedata.com/terms |
| ECB reference rates | Free | Daily ~16:00 CET | — | — | Since 1999 | "Information purposes only" | ecb.europa.eu euro reference rates page |
| Frankfurter v2 | Free, no key | Daily | — | Abuse limits only | Back to 1948 | Pass `providers` for consistent series; self-hostable | frankfurter.dev/docs/ |
| ExchangeRate-API / exchangerate.host | $0–$100 | Daily → 60 s by tier | — | 100–500k/mo | Paid only | Attribution, non-commercial free tiers | exchangerate-api.com ; exchangerate.host/product |
| IBKR IDEALPRO | IBKR account | Yes | TWS socket | Pacing limits | ≤30 s bars: 6 months | FX data free without subscription (search-only) | interactivebrokers.github.io/tws-api/market_data.html |

## Rates / fixed income

| Source | Price | Coverage | Limits | Terms | Sources |
|---|---|---|---|---|---|
| **FRED / ALFRED** | Free, key | 800k+ series, vintages | v1 120/min; v2 2 req/s | **Mandatory notice:** "This product uses the FRED® API but is not endorsed or certified by the Federal Reserve Bank of St. Louis." Third-party copyrighted series are for personal use only. | fred.stlouisfed.org/docs/api/terms_of_use.html ; …/releases_dates.html |
| **Treasury par yield curve XML** | Free | Par (since 1990), bills, long-term, real; 1 Mo–30 Yr incl. 1.5 Mo | — | — | home.treasury.gov/treasury-daily-interest-rate-xml-feed |
| **Treasury FiscalData** | Free, no key | Auctions since 1979, average rates, debt, DTS, FX rates of exchange | Unspecified | Commercial allowed | fiscaldata.treasury.gov/api-documentation/ |
| TreasuryDirect | Free | Announced/auctioned securities | — | Docs moved and wouldn't render; use FiscalData | treasurydirect.gov/webapis/webapisecurities.htm |
| FINRA API (TRACE) | Public $0 | **Aggregates only** (weekly Treasury aggregates, breadth, capped volume) | 1,200/min | Non-commercial; credit FINRA | developer.finra.org/docs ; developer.finra.org/fees |
| MSRB EMMA | Subscriptions only (~$5.5k–$11k/yr per old snippets, UNVERIFIED) | Munis | — | **Scraping prohibited** | emma.msrb.org/AboutEmma/UserAgreement |
| ECB Data Portal | Free | Euro rates, curves, FX | UNVERIFIED (503) | — | data-api.ecb.europa.eu (search-only) |
| Bank of England IADB | Free | Gilts, SONIA, BoE rates | 300 series per CSV request | OGL v3; some FX series excluded | bankofengland.co.uk/boeapps/database/help.asp |
| **OpenFIGI** | Free (key raises limits) | Identifier mapping | 25 per 6 s with key | Open | openfigi.com/api/documentation |

## Macro and calendar

| Source | Price | Limits | Notes | Sources |
|---|---|---|---|---|
| BLS v2 | Free (registration) | 500 queries/day, 50 series × 20 y per query | Batch queries | bls.gov/developers/api_faqs.htm |
| BEA | Free UserID | 100 req/min | 429 + Retry-After | BEA API user guide (Apr 2026) |
| World Bank | Free | — | ~16k indicators | datahelpdesk.worldbank.org |
| IMF | Free | UNVERIFIED | SDMX 2.1/3.0 | data.imf.org |
| OECD | Free | **60 downloads/hour** (search-only) | Cache aggressively | oecd.org API best practices |
| **FRED release calendar** | Free | As FRED | Needs `include_release_dates_with_no_data=true` for future dates. Release dates are not necessarily when data lands on FRED | fred.stlouisfed.org/docs/api/fred/releases_dates.html |
| FMP economic calendar | ~$49/mo (search-only) | — | 150+ countries; tier UNVERIFIED | (403) |
| Trading Economics | Price UNVERIFIED (sales) | 2 req/s | Best calendar, with live-event subscription | docs.tradingeconomics.com |
| Finnhub economic calendar | ~All-in-One tier (search-only) | — | — | — |
| Nasdaq Data Link | Free key; datasets priced individually | 50k/day with key (search-only) | Docs URLs redirect | — |

## Gotchas

- Kraken REST is capped at 720 candles. Deep history comes from the quarterly CSV dumps, which **the user must download manually**.
- Coinbase drops sockets that don't subscribe within 5 s.
- Don't use Binance.com or Bybit (US excluded). Binance.US is unavailable in NY/TX/WA and other states.
- **CoinGecko's terms conflict with a long-lived local history cache:** refresh within 24 h, storage restricted.
- Twelve Data free is non-display; showing it on screen needs Grow.
- OANDA uses HTTP streaming, not WS.
- FRED attribution is mandatory. FINRA public data is aggregates only. EMMA forbids scraping.
- TreasuryDirect API docs have moved; use FiscalData. On fetch, the Treasury 2026 TextView's latest row was 2026-08-26, so verify feed currency during integration.

## Not verified

Finnhub (all), FMP (all), Trading Economics price, CoinDesk paid pricing, MSRB pricing, IMF auth/limits, ECB portal limits, OECD limit, Nasdaq Data Link limits, Binance/Bybit/OKX US terms (search-only), IBKR free FX data, OANDA candle cap/history, Coinbase total history and data terms, Twelve Data FX depth, exchangerate.host depth, Treasury publication time.

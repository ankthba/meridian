# Data providers: comparison and recommended stacks

Researched 2026-10-05. Every price and limit below comes from the provider's current official pages; the URLs are in the per-category research files:

- [equities-fundamentals.md](research/equities-fundamentals.md)
- [options-futures.md](research/options-futures.md)
- [crypto-fx-rates-macro.md](research/crypto-fx-rates-macro.md)
- [news-filings-transcripts.md](research/news-filings-transcripts.md)

Items marked † could only be confirmed from a search-engine extract of the official page (blocked or JS-rendered). Items marked ‡ are unverified. Confirm both before paying.

All plans below are **personal, non-professional** licenses. You qualify only as a natural person using data for yourself. **Registering the project under an LLC or company would make you "professional"** and multiply exchange fees.

## Facts that shape the choice

1. **Polygon.io is now Massive** (since 2025-10-30). **IEX Cloud shut down** (2024-08-31). **CoinDesk Data (ex-CryptoCompare) killed its free tier** (2026-05-21).
2. **Most cheap "real-time" US equity feeds are not the full market.** Alpaca Basic and Tiingo are IEX only, EODHD WebSocket is Cboe EDGX only, Twelve Data covers ~5% of volume, and Intrinio's is a modelled price. Only Alpaca Algo Trader Plus, Massive Stocks Advanced, and broker feeds (Tradier, tastytrade, IBKR) carry consolidated SIP data.
3. **Real-time options and futures for free require a funded brokerage account** (tastytrade dxLink is the strongest: real-time OPRA plus CME plus a Greeks stream).
4. **Consensus estimates, analyst price targets, holders, and transcripts are not available for free** from any legitimate source.
5. **SEC EDGAR is free, official, and near real-time** (about 1 s for submissions, under 1 min for XBRL). It covers filings, as-reported financials (XBRL company facts, plus "frames" across all filers, which makes a free screener possible), 13F, and Form 4.

## Master comparison

| Provider | Covers | Price (USD/mo) | Real-time quality | Streaming | Verdict |
|---|---|---|---|---|---|
| **Alpaca** | US equities, US options, corporate actions, news | Basic $0 · **Algo Trader Plus $99** | Basic: IEX only; options "indicative" (derived). **Plus: full SIP + OPRA** | WS: Basic 30 symbols; Plus unlimited equities / 1,000 option quotes | **Best value for real-time equities + options.** Bars since 2016; options history only since Feb 2024. Account required (paper-only eligibility for Plus‡) |
| **Massive** (ex-Polygon) | Stocks, options, futures, currencies (crypto + FX), Benzinga add-ons | Per product: Stocks $0/29/79/**199**; Options $0/29/79/**199**; Futures $0/**29**/79/199; Currencies $0/**49**; Benzinga add-ons $99 each | Real-time only on Advanced tiers (stocks/options/futures) and on Currencies Starter. Stocks: full SIP | WS from Starter | Deepest history (20+ y stocks, tick/quote data, flat files). Expensive when several products are stacked |
| **Databento** | OPRA options, CME futures (full order book), US equities (Mini) | OPRA Standard $199 · CME Standard $199 · ICE $2,500/venue | Raw exchange-grade; no Greeks/IV | Live API + **official Rust client** | Best raw data quality; you compute everything. Options from 2013, CME from 2010 |
| **ThetaData** | US options | FREE (EOD) · Value $40 · **Standard $80** · Pro $160 | Real-time from Value | Standard: 10k quote contracts | Best options history per dollar (tick from 2016 on Standard). **Needs a local Java 21+ sidecar** |
| **EODHD** | Fundamentals, estimates, ratings, holders, insiders, dividends, EOD | Fundamentals $59.99 · **All-in-One $99.99** | WS is EDGX only (not SIP) | WS 50 symbols | **Broadest single fundamentals source:** statements since 1985, earnings trend (estimates), ratings + targets, institutional/fund holders |
| **Sharadar** | Point-in-time fundamentals, 13F, insiders, prices | Bundle $29 / $49 / $69 (5 y / 10 y / full) | EOD | — | Cheap, point-in-time, includes delisted companies. No estimates |
| **Finnhub** | News, fundamentals, recommendations, estimates | Free · paid $49.99–$199.99†; Estimates $75–200† | Free WS (venue‡) | 1 WS connection per key | Free tier useful: company news (North America), as-reported financials, recommendation trends. **Terms: delete all data when the subscription ends** |
| **Tiingo** | EOD, IEX, fundamentals add-on, news | Power $30; fundamentals $9.99–49.99 | IEX only | WS on Power | News at $30 is decent (headline + description, real-time) |
| FMP | Everything | $0 / 29 / 69 / 139† | Source‡ | WS‡ | Site blocks verification and prices conflict. Not recommended until confirmed |
| Alpha Vantage / Twelve Data / Intrinio | Mixed | $49.99+ / $79+ / $150 | Unclear entitlement / ~5% volume / modelled | — / credits / WS | Not recommended (weak real-time, restrictive or unclear terms) |
| **tastytrade** | Equities, options (+ Greeks stream), CME futures | $0 with funded account | Real-time | dxLink WS, 25k subscriptions/session | **Best free real-time if you'll open and fund an account** |
| Tradier / IBKR / Schwab | Equities, options (IBKR: futures) | $0–$35 / ~$10–16† / $0‡ | Consolidated | WS / TWS socket / WS‡ | Alternatives if you already hold one of these accounts. Tradier Greeks update hourly; IBKR needs $500 equity |
| **Coinbase, Kraken** | Crypto spot | Free, no account | Real-time | Public WS | **Free real-time crypto.** Kraken: free CSV history dumps (manual download) |
| CoinGecko | Crypto metadata, market caps | Demo $0 · Basic $35 · Analyst $129 | ~60 s | WS from Basic | Attribution required; cache must refresh within 24 h |
| **OANDA v20** | FX | Free with demo account | Real-time | HTTP streaming (20 streams) | **Free real-time FX.** You create the demo account |
| Frankfurter / ECB | FX reference rates | Free | Daily | — | Daily history back to 1948 / 1999 |
| **SEC EDGAR** | Filings, XBRL financials, 13F, Form 4 | Free | ~1 s submissions; Atom "getcurrent" feed | — (poll) | **Core free source.** 10 req/s; requires a User-Agent with a contact email |
| sec-api.io | Filings stream, extractors | Personal $49 | < 300 ms stream | WS | Optional; EDGAR alone is near real-time |
| **FRED / ALFRED** | 800k+ macro series, release calendar | Free (key) | On release | — | **Core macro source.** Mandatory attribution notice |
| **Treasury** (par curve XML, FiscalData) | UST yields, auctions | Free | Daily | — | Core GOVT source |
| BLS, BEA, World Bank, IMF, OECD, ECB, BoE | Macro | Free | On release | — | Supplementary; OECD allows only 60 downloads/hour |
| **Benzinga via Massive** | Full-text news since 2009 | $99 | Real-time | REST | **Only verified full-text real-time news at this price.** Whether a base Massive plan is required is unclear‡ |
| Press-release RSS (GlobeNewswire, PR Newswire, Business Wire) | Company press releases | Free | Live | RSS | Headlines + summaries |
| **EarningsCall.biz** | Transcripts (+ audio on Ultimate) | Starter $60 · **Premium $69** · Ultimate $129 · Ultimate+ $155 ("start at" prices) | ~50% within 15 min | Notifications on Ultimate+ | Best transcripts value; history mostly from 2020 |
| API Ninjas | Transcripts | $39–$299 | ‡ | — | Alternative; history from 2005 |
| Not usable | Yahoo Finance (ToS bans automated access), Koyfin (no API), Seeking Alpha / Motley Fool (scraping banned), Stocktwits (closed), Binance.com / Bybit (US excluded), Kinetick (NinjaTrader-only), NewsAPI free (dev-only, 24 h delay), Twelve Data free (non-display license), MSRB EMMA (scraping banned) | | | | |

## Recommended stacks

### Free: $0/mo

| Need | Source |
|---|---|
| Equities real-time | Alpaca Basic: **IEX only**, 30 streamed symbols, the rest refreshed via REST within 200 calls/min. Labeled `IEX` in the UI |
| Equities history | Alpaca SIP bars since 2016 (~10 y daily, intraday) |
| Filings, financials, holders | SEC EDGAR: submissions, XBRL company facts and frames (as-reported; we standardize), 13F, Form 4 |
| Recommendations, company news | Finnhub free (recommendation counts, North America company news) + press-release RSS |
| Options | **EOD only** (Massive Options Basic). Real-time needs a brokerage account |
| Futures | Massive Futures Basic: 10-min delayed, REST at 5 calls/min |
| Crypto | Coinbase + Kraken WebSockets (real-time); Kraken CSV dumps for history |
| FX | OANDA v20 demo (real-time stream) + Frankfurter/ECB (daily history) |
| Rates, macro, calendar | FRED/ALFRED, Treasury par curve + FiscalData auctions, BLS, BEA |
| **Not available** | Real-time SIP equities, real-time options/futures, consensus estimates (EE), price targets, transcripts, full-text news |

**If you open a funded tastytrade account**, the free tier gains real-time equities, options (with a Greeks stream), and CME futures.

### ~$100/mo: **$99**

| Pick | $/mo | Adds |
|---|---|---|
| **Alpaca Algo Trader Plus** | 99 | Full **SIP** real-time equities with unlimited streamed symbols (enough to test the 2,000-symbol budget on real data), real **OPRA** options with chain snapshots and Greeks/IV, 10,000 calls/min |
| Everything in the free stack | 0 | |

Stretch options:
- **+$29** Sharadar 5-year bundle: point-in-time standardized fundamentals, 13F, insiders.
- **+$49** Massive Currencies: unified real-time crypto + FX with 10+ y history.

**Still not available at $99:** consensus estimates, price targets, transcripts, full-text news, real-time futures, options history before Feb 2024.

### ~$500/mo: **$525** (trim to $496 by dropping futures to the free delayed tier)

| Pick | $/mo | Adds |
|---|---|---|
| Alpaca Algo Trader Plus | 99 | Real-time SIP equities + OPRA options |
| ThetaData Options Standard | 80 | Options tick history from 2016, chain snapshots, vendor Greeks to validate our Rust models, historical vol surfaces (OVDV) |
| EODHD All-in-One | 99.99 | FA since 1985, **EE** (earnings trend), **ANR** (ratings + targets), **HDS** (institutional/fund holders, insiders), DVD |
| Massive Currencies Starter | 49 | Real-time crypto + FX WebSockets, 10+ y history |
| Massive Futures Starter | 29 | CME Group futures over WebSocket (10-min delayed), 2 y history |
| Benzinga News via Massive | 99 | **Full-text** real-time news since 2009 (CN, TOP, N) |
| EarningsCall Premium | 69 | Speaker-segmented transcripts, ~15 min after calls |
| **Total** | **524.99** | |

**Alternative ~$500, markets-first:** Alpaca Plus $99 + Databento OPRA $199 + Databento CME $199 = **$497**. This gives exchange-grade raw options (from 2013) and a real-time full CME order book through an official Rust client, but no paid fundamentals, news, or transcripts (free sources only).

### Separate from the data budget: ASK

Anthropic API usage is billed per token: `claude-opus-5-5` costs $4 input / $20 output per million tokens. Prompt caching cuts the repeated system and tool prefix to the cache-read rate.

## Function coverage by tier

| Function | Free | ~$100 | ~$500 |
|---|---|---|---|
| Quote monitor / W | IEX real-time (30 streamed) | **SIP real-time** | SIP real-time |
| GP / GIP / HP | Alpaca bars since 2016 | same + real-time | same |
| DES | EDGAR profile | same | + EODHD profile |
| FA | EDGAR XBRL (as-reported) | same | EODHD standardized since 1985 |
| EE | NOT AVAILABLE | NOT AVAILABLE | EODHD |
| ERN | Actuals only (EDGAR); consensus NOT AVAILABLE | same | EODHD |
| ANR | Recommendation counts (Finnhub); targets NOT AVAILABLE | same | EODHD ratings + targets |
| HDS | EDGAR 13F/Form 4 (we parse) | same | EODHD |
| DVD | Massive Basic / Alpaca corporate actions | same | EODHD |
| N / CN / TOP | Headlines + summaries (Finnhub, RSS) | same | **Full text** (Benzinga) |
| CF | EDGAR | EDGAR | EDGAR |
| OMON | EOD only | **Real-time OPRA** | Real-time + history |
| OVDV | From EOD chains | From real-time chains (history from Feb 2024) | History from 2016 |
| OVME | Our models | Our models | Our models |
| EQS / RV | EDGAR frames (as-reported, all filers) | same | + EODHD |
| CORR / PORT / BTST | From bars | From bars | From bars |
| WEI | **Not yet researched** (see gaps) | | |
| ECO | FRED values + release dates; consensus NOT AVAILABLE | same | same |
| FXC | OANDA real-time | same | + Massive Currencies |
| CRYP | Coinbase + Kraken real-time | same | + Massive Currencies |
| Futures | 10-min delayed REST | same | 10-min delayed WS (real-time: +$170 for Massive Futures Advanced, or Databento) |
| GOVT | Treasury par curve, auctions, FRED | same | same |
| Transcripts | NOT AVAILABLE | NOT AVAILABLE | EarningsCall |

## Not obtainable at any of these budgets

- **Corporate, muni, and mortgage bond pricing** (CORP / MUNI / MTGE sectors). FINRA's public API is aggregates only, trade-level TRACE is paid enterprise data, MSRB EMMA bans scraping, and its subscriptions are roughly $5.5k–$11k/yr‡. These sectors will show `NOT AVAILABLE`.
- **ICE / Eurex real-time futures** (Databento: $2,500/mo per venue). FirstRate offers historical ICE/Eurex bars.
- **Full-depth US equity order book** (Databento US Equities: $4,000/mo).
- **Economic consensus forecasts** for ECO. Trading Economics has them, but its price is unpublished‡. The FMP calendar tier is unverified.
- **Social sentiment** (Stocktwits closed to new registrations; Reddit requires pre-approval).

## Gaps in this research

- **Index data (WEI) was not researched.** Real-time index levels (S&P, Dow, Nasdaq, international) are separately licensed. I'll research this before Phase 7. Fallbacks are FRED's daily index series (with its licensing limits) or clearly labeled ETF proxies.
- Several providers blocked verification: Schwab (all), IBKR (prices), FMP (all), Finnhub paid tiers. See each research file's "not verified" list.

## Licensing constraints the design enforces

- Data is personal and non-display: don't screen-share vendor data with others.
- **Finnhub:** delete its data when the subscription ends → `store.purge_provider()`.
- **CoinGecko:** refresh cache within 24 h → `CachePolicy::MaxAge`.
- **FRED, CoinGecko, FINRA:** attribution text shown on screen.
- **EDGAR:** User-Agent must include a contact email. **You set it in Settings; I won't fill in your address myself.**
- **ASK sends retrieved data to Anthropic.** Before Phase 8 I'll check each chosen vendor's terms on passing data to a third-party AI service; a provider that forbids it is excluded from ASK tool results (`ai_allowed` capability flag).

## Confirm before paying

- **Alpaca:** that a paper-only account can subscribe to Algo Trader Plus (third-party sources say yes).
- **Massive:** whether Benzinga add-ons need a base plan, and whether OPRA/CME fees are passed through.
- **ThetaData:** which tier includes 2nd/3rd-order Greeks (its docs conflict).
- **EarningsCall:** final prices (listed as "start at").
- **Databento:** whether "no license fees" covers live OPRA/CME on Standard.

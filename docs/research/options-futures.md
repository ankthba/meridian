# Research: US options and futures

Researched 2026-10-05 against official pages. "search-index" means only a search-engine summary was available (IBKR, Schwab, and TradeStation pages returned 403, and the CME fee PDF timed out). UNVERIFIED means not confirmed.

## Options

| Provider | Price/mo | Real-time? | Chains / Greeks / IV | Streaming | Limits | History | Terms | Sources |
|---|---|---|---|---|---|---|---|---|
| **Massive Options** | $0 / $29 / $79 / **$199** | Basic EOD; Starter/Developer 15-min delayed; **Advanced real-time** | Chain snapshot (Starter+) with vendor Greeks/IV/OI (method undocumented; missing for some deep ITM). **NBBO quotes only on Advanced** | WS from Starter | Basic 5/min | 2 / 2 / 4 / 5+ y | Individual, non-pro; OPRA fee pass-through UNVERIFIED | massive.com/pricing?product=options ; massive.com/docs/rest/options/snapshots/option-chain-snapshot |
| **ThetaData** | FREE $0; Value $40; **Standard $80**; Pro $160 | FREE is EOD (1-day delay); Value+ real-time | IV on Value+; Black-Scholes Greeks (tier mapping conflicts between v2/v3 docs); chain snapshots on Standard+ | Standard: 10k quote + 15k trade contracts; FREE/Value: none | FREE 30/min; otherwise 2/4/8 concurrent requests | FREE from 2023-06; Value 2020 (1-min); **Standard 2016 tick**; Pro 2012 tick | Personal only. **Requires the local Java 21+ "Theta Terminal" sidecar** | thetadata.net/pricing ; thetadata.net/docs/Articles/Getting-Started/Subscriptions.html |
| ORATS | $199 delayed; $299 live; $599 intraday | Delayed or live | Vendor Greeks, IV, smoothed surface (`smvVol`), 500+ indicators | REST only | Monthly request caps | EOD from 2007; 1-min from 2020 | Terms UNVERIFIED | orats.com/data-api |
| Tradier | Brokerage $0–$35 | Real-time for account holders | Full chains; ORATS Greeks/IV updated **hourly** | WS/HTTP, 1 session | 120/min | UNVERIFIED | Account required | docs.tradier.com/docs/market-data |
| Cboe All Access | $2,499+ | With OPRA add-on | Calcs add-on | — | Points | 2012+ | **Buying OPRA through All Access classifies you as a professional subscriber** | datashop.cboe.com/cboe-all-access-api |
| **Databento OPRA.PILLAR** | **Standard $199**; Plus $1,750 | Live (portal attestation) | **Raw data only; no IV or Greeks.** All 18 US options exchanges | Live API + **official Rust client** (`databento-rs`) | — | From 2013-04; Standard: 12 months top-of-book included | "No license fees" on Standard (older posts say fees pass through; confirm) | databento.com/pricing ; databento.com/catalog/opra/OPRA.PILLAR ; github.com/databento/databento-rs |
| Intrinio Individual | $150 | Synthetic "OptionsEdge" midpoint, **not NBBO** | Vendor Greeks | WS | — | None | Real OPRA is Enterprise-only | intrinio.com/options/optionsedge |
| IBKR | $10 bundle + $4.50 + OPRA $1.50 (search-index) | Real-time | UNVERIFIED | TWS socket | Pacing | — | Account + $500 equity | (search-index) |
| **Alpaca** | Basic $0; **Algo Trader Plus $99** | Basic: "indicative" feed (derived, not OPRA quotes; trades delayed 15 min). **Plus: real OPRA feed** | Chain/snapshot with Alpaca Black-Scholes Greeks and IV | WS: 200 / 1,000 quote subscriptions | 200 / 10,000 per min | **Options only from Feb 2024** | Alpaca account required | docs.alpaca.markets/us/docs/historical-option-data ; docs.alpaca.markets/us/reference/optionchain |
| MarketData.app | $0–$250 | Trader+ real-time (OPRA agreement) | Live Greeks/IV; null on historical requests | UNVERIFIED | **1 credit per contract** on live chains | EOD from 2010 | Internal only | marketdata.app/pricing |
| **tastytrade dxLink** | $0 with a funded account | Real-time | Streams Greeks events (IV, delta, gamma, theta, vega, rho) | WS: 5 sessions × 25k subscriptions | — | Candles UNVERIFIED | Fully onboarded customer; no non-pro data fee listed | developer.tastytrade.com/docs/concepts/streaming |
| Schwab Trader API | $0 with account | UNVERIFIED | UNVERIFIED | WS L1/L2 (unofficial docs) | UNVERIFIED | — | Approved individual developer app | developer.schwab.com (403) |
| Unusual Whales | $150 / $375 | Real-time flow | Chains, Greeks, flow, GEX | Advanced: WS | 40k/day | 2 y | Personal | unusualwhales.com/public-api |
| FirstRate Data | ~$79/mo updates | Historical | EOD with IV + Greeks | — | — | 2010–2026 incl. delisted | UNVERIFIED | firstratedata.com/b/49/historical-options-data |

**OPRA fees** (official schedule): non-professional $1.25/mo; professional $31.50/device. Source: cdn.opraplan.com/documents/OPRA_Fee_Schedule.pdf

## Futures

| Provider | Price/mo | Real-time? | Exchanges | Streaming | History | Terms | Sources |
|---|---|---|---|---|---|---|---|
| **Massive Futures** (GA 2026-05-28) | $0 / **$29** / $79 / $199 | Basic–Developer 10-min delayed; Advanced real-time | CME, CBOT, NYMEX, COMEX (no ICE/Eurex); top of book; depth "coming soon" | WS from Starter | 2 / 2 / 5 / 7+ y | Non-pro; CME fee inclusion UNVERIFIED | massive.com/pricing?product=futures |
| **Databento GLBX.MDP3** | **Standard $199** | Live | CME Group, full MBO book, 650k+ symbols. ICE at $2,500/mo per venue | Live + Rust client | From 2010-06 | "No license fees" (confirm) | databento.com/catalog/cme/GLBX.MDP3 |
| IBKR | $10 bundle (search-index) | Real-time top of book | CME Group | TWS socket | — | Account + $500 equity | (search-index) |
| tastytrade | $0 funded account | Real-time | Futures + futures options | dxLink | UNVERIFIED | Account | developer.tastytrade.com |
| dxFeed retail | ~$99 (promo page) | Real-time | CME Group | Partner platforms; direct API UNVERIFIED | — | Non-pro | dxfeed.com |
| Kinetick | — | — | — | **NinjaTrader only; not usable** | — | — | kinetick.com |
| Barchart OnDemand | From $500 | UNVERIFIED | — | — | — | Sales | help.barchart.com |
| FirstRate Data | ~$59.95/mo updates | Historical bars | CME Group, **ICE, Eurex** (130 contracts) | — | From 2008 | UNVERIFIED | firstratedata.com/b/29/futures-most-active |

**CME non-professional fees** (Jan 2026 list, search-index): top of book $1.55 per exchange or $4.65 for all four; depth $12.10 per exchange or $36.50 for all four.

## Gotchas

- **Registering as an LLC/company makes you professional**: non-pro status requires a natural person using the data for personal use.
- Cboe All Access + OPRA makes you professional.
- Massive options below Advanced have **no NBBO quotes**.
- Databento has no Greeks or IV. That's fine, since we compute them in Rust.
- ThetaData needs a Java sidecar. Meridian would connect to it on localhost; the user runs it separately.
- MarketData.app's per-contract credit cost makes full chains expensive.
- Alpaca free options are indicative (derived), not OPRA.
- Free real-time options and futures all require a funded brokerage account (tastytrade, Tradier, IBKR, Schwab).

## Not verified

All IBKR and Schwab details; whether Massive, ThetaData, and MarketData.app pass through OPRA/CME fees; whether Databento's "no license fees" applies to live data; Cboe file prices; FirstRate licenses; dxFeed retail API; Tradovate API; tastytrade candle depth; Tradier history; OPRA non-display fee applicability; the official source for CME's 10-minute delay.

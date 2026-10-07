# meridian-provider-alpaca

Provider `alpaca`: Alpaca Market Data API for US stocks (snapshot quotes, historical bars, a real-time trade/quote WebSocket stream), US option chain snapshots with vendor Greeks, news, and corporate actions (cash dividends and splits for DVD, and ex-dates across many symbols for CALENDAR and TODAY).

Requires an Alpaca account and API key pair (a free paper account works for the Basic plan). Keys come from the Keychain through `AlpacaConfig`; the crate never reads them from anywhere else.

## Docs this was built against (checked 2026-10-05)

| Topic | URL |
|---|---|
| Plans, feeds, rate limits, auth headers | <https://docs.alpaca.markets/us/docs/about-market-data-api> |
| Authentication (hosts, `APCA-API-KEY-ID` / `APCA-API-SECRET-KEY`) | <https://docs.alpaca.markets/us/docs/authentication> |
| Historical API base URL | <https://docs.alpaca.markets/us/docs/historical-api> |
| Stock feeds (`iex`, `sip`, …) | <https://docs.alpaca.markets/us/docs/historical-stock-data-1> |
| Option feeds (`indicative`, `opra`) | <https://docs.alpaca.markets/us/docs/historical-option-data> |
| News (Benzinga, since 2015) | <https://docs.alpaca.markets/us/docs/historical-news-data> |
| Snapshots `GET /v2/stocks/snapshots` | <https://docs.alpaca.markets/us/reference/stocksnapshots-1> |
| Bars `GET /v2/stocks/{symbol}/bars` | <https://docs.alpaca.markets/us/reference/stockbarsingle-1> (multi-symbol: <https://docs.alpaca.markets/us/reference/stockbars>) |
| Option chain `GET /v1beta1/options/snapshots/{underlying}` | <https://docs.alpaca.markets/us/reference/optionchain> |
| News `GET /v1beta1/news` | <https://docs.alpaca.markets/us/reference/news-3> |
| Corporate actions `GET /v1/corporate-actions` (checked 2026-10-07; page updated 2026-05-27). `symbols`: "A comma-separated list of symbols", not required; `limit` "applies to the total number of data points, not the count per symbol" | <https://docs.alpaca.markets/us/reference/corporateactions-1> |
| Corporate actions: `region`, `isin` and `currency` added (2026-06-03) | <https://docs.alpaca.markets/us/v1.1/changelog/2026-06-03-market-data-9dddd18> |
| `end` filters on `process_date`, which can be days after the ex-date (Alpaca staff, community forum) | <https://forum.alpaca.markets/t/querying-corporate-actions-by-ex-date-rather-than-process-date/17724> |
| WebSocket protocol, errors, limits | <https://docs.alpaca.markets/us/docs/streaming-market-data> |
| Stock stream channels and schemas | <https://docs.alpaca.markets/us/docs/real-time-stock-pricing-data> |
| Option stream (not used: msgpack only) | <https://docs.alpaca.markets/us/docs/real-time-option-data> |
| 403s, SIP embargo, IEX vs SIP, Greeks gaps | <https://docs.alpaca.markets/us/docs/market-data-faq> |
| Quote sizes in shares since 2025-11-03 | <https://docs.alpaca.markets/us/changelog/marketdata-bid-and-ask-size-display-change> |
| Terms and Conditions (from the OpenAPI `termsOfService`) | <https://s3.amazonaws.com/files.alpaca.markets/disclosures/library/TermsAndConditions.pdf> |

The pages were read as raw Markdown (`<page>.md`), which includes each endpoint's OpenAPI definition; paths, parameters and field names below come from those definitions.

Two facts come from Alpaca staff on the community forum, not the docs:
- Class shares use a dot on the data API (`BRK.B`): <https://forum.alpaca.markets/t/how-to-pull-quotes-for-berkshire-hattaway/7303> (2021-11-10).
- IEX bars only go back to 2020, SIP bars to 2016, and staff recommend `feed=sip` for history: <https://forum.alpaca.markets/t/cannot-get-historical-bar-data-before-2020/12415> (2023-05-23).

## Authentication

- REST (`https://data.alpaca.markets`): headers `APCA-API-KEY-ID` and `APCA-API-SECRET-KEY`. Both header values are marked sensitive (`HeaderValue::set_sensitive`), so they are redacted from `Debug` output. Keys never go in URLs, so `Provenance.source_ref` (the request URL) is safe to store and show.
- Stream: after the server's `[{"T":"success","msg":"connected"}]`, the client sends `{"action":"auth","key":…,"secret":…}` (within 10 s) and expects `[{"T":"success","msg":"authenticated"}]`. The message is built in memory and never logged.
- If either key is missing or blank, every keyed call (`quotes`, `bars`, `option_chain`, `news`, `dividends`, `connect`) returns `ProviderError::Unauthorized("Alpaca API key not set — add it in Settings")` without any network I/O. `quotes` for keys Alpaca doesn't serve returns `Ok(vec![])` even without keys.

## Plans, feeds and what the capabilities say

`AlpacaConfig.feed` states which plan the keys have. Capabilities and provenance follow from it.

| | `AlpacaFeed::Iex` (Basic, free) | `AlpacaFeed::Sip` (Algo Trader Plus, $99/mo) |
|---|---|---|
| Quotes, stream | IEX only, real time: `FeedSource::SingleVenue("IEX")`, `RealTime`. IEX is ~2.5% of US volume, so `volume`/`vwap`/OHLC are IEX-only | SIP, all US exchanges: `Consolidated`, `RealTime` |
| Bars (daily and intraday) | **SIP**, `end` clamped to 16 min ago: `Consolidated`, `Delayed { 15 }` | SIP: `Consolidated`, `RealTime` |
| Option chain | `feed=indicative`: `FeedSource::Modelled`, `Delayed { 15 }` | `feed=opra`: `Consolidated`, `RealTime` |
| News | `Aggregated`, `RealTime` | same |
| Corporate actions (dividends, splits) | `Aggregated`, `EndOfDay` | same |
| REST rate limit | 200/min (`RateLimit::per_minute(200)`) | 10,000/min |
| Streamed symbols | 30 (`max_stream_symbols = Some(30)`) | unlimited (`None`) |
| History | Bars since 2016 (SIP) | same |

Why bars use SIP on Basic: the docs allow Basic to query SIP history as long as `end` is at least 15 minutes old, IEX bars only start in 2020 and carry ~2.5% of the volume, and Alpaca staff recommend `feed=sip` for history. The cost is that Basic intraday bars stop 16 minutes before now (one minute of margin for clock skew); the live stream covers the gap. If Basic keys request recent SIP data anyway, Alpaca answers 403 "subscription does not permit querying recent SIP data", which becomes `NotEntitled { plan: "Algo Trader Plus" }`.

Why indicative options are `Modelled` + `Delayed { 15 }`: the docs call the indicative feed "a free derivative of the original OPRA feed: the quotes are not actual OPRA quotes … The trades are also derivatives and they're delayed by 15 minutes." A chain carries one delay, so it gets the conservative one.

Not used: `delayed_sip` (15-min delayed SIP; a possible consolidated alternative for Basic quotes), `boats`/`overnight`, `otc`, the option stream (msgpack only), the news stream, assets/reference data (on the trading API host, which differs between paper and live accounts).

## Endpoints and field mappings

### `quotes` — `GET /v2/stocks/snapshots?symbols=A,B&feed=iex|sip`

- Served keys: sector `Equity` or `Pfd`, exchange `US` or none. `/` in a symbol becomes `.` (`BRK/B` → `BRK.B`). Other keys are skipped so the router can try another provider; symbols missing from the response are simply not returned. The caller's own `SecurityKey` is returned on each quote.
- Up to 100 symbols per request (the docs give no maximum; our choice).
- Response: an object keyed by symbol; each value may hold `latestTrade`, `latestQuote`, `minuteBar`, `dailyBar`, `prevDailyBar`.

| `Quote` | Alpaca |
|---|---|
| `last`, `last_size` | `latestTrade.p`, `latestTrade.s` |
| `bid`, `bid_size`, `ask`, `ask_size` | `latestQuote.bp`, `.bs`, `.ap`, `.as`. Price `0` means "no active bid/ask" → `None` (size too). Sizes as reported: shares since 2025-11-03, round lots before |
| `open`, `high`, `low`, `volume`, `vwap` | `dailyBar.o`, `.h`, `.l`, `.v`, `.vw` |
| `prev_close` | `prevDailyBar.c` |
| `ts_event` | newer of `latestTrade.t` and `latestQuote.t` (else `minuteBar.t`, `dailyBar.t`, else fetch time) |
| `ts_recv` | fetch time |
| `provenance.as_of` | `ts_event` |

The stream page still says quote sizes are "in round lots"; the REST schema and the changelog say shares since 2025-11-03. We pass sizes through unchanged.

### `bars` — `GET /v2/stocks/{symbol}/bars`

- Query: `timeframe`, `start`, `end` (RFC 3339, whole seconds), `limit=10000` (documented max), `adjustment`, `feed=sip`, `sort=asc`, `page_token`.
- `timeframe`: `Minute(1..=59)` → `{n}Min`; whole-hour minutes and `Hour(1..=23)` → `{n}Hour`; `Day` → `1Day`; `Week` → `1Week`; `Month` → `1Month`. Anything else (e.g. 90 minutes, 24 hours) → `Unsupported`.
- `adjustment`: `None` → `raw`, `Splits` → `split`, `SplitsAndDividends` → `all` (Alpaca's `all` also adjusts for spin-offs).
- Window: `from` is inclusive; `to` is exclusive and applied after fetching because Alpaca's `end` is inclusive. Without `from`: daily/weekly/monthly start 2016-01-01; intraday starts 30 days before `end`. Without `to`: now (Basic: now − 16 min).
- Pagination: repeat with `page_token = next_page_token` until it is null or empty. A repeated token is a `Parse` error; more than 2,000 pages is `Upstream`.
- Mapping: `t, o, h, l, c, v` → `ts, open, high, low, close, volume`; `vw` and `n` are dropped (`BarSeries` has no columns for them). Timestamps are kept as Alpaca sends them: the bar's left edge; daily bars are stamped at midnight New York time (e.g. `04:00Z`). Then `BarSeries::normalize()`.
- `provenance.as_of` is the fetch time; `source_ref` is the first page's URL.

### `option_chain` — `GET /v1beta1/options/snapshots/{underlying}`

- Query: `feed=indicative|opra`, `limit=1000` (documented max), `expiration_date=YYYY-MM-DD` when `ChainRequest.expiry` is set, `page_token`. All pages are fetched.
- Response: `snapshots` keyed by OCC symbol (unpadded, e.g. `AAPL240426C00162500`), each with optional `latestQuote`, `latestTrade`, `greeks {delta, gamma, rho, theta, vega}`, `impliedVolatility`, `dailyBar`, `minuteBar`, `prevDailyBar`.
- OCC parsing: root (1–6 alphanumerics) + `YYMMDD` + `C`/`P` + strike × 1000 in 8 digits. `contract_symbol` is the padded 21-character form from `OptionContract::occ_symbol`, built from the symbol's own root (so `SPXW` stays `SPXW`). Unparseable symbols are skipped and counted in a warning.
- `bid`/`ask`/sizes from `latestQuote` (`0` → `None`); `last` from `latestTrade.p`; `volume` from `dailyBar.v`; `open_interest` is always `None` (not in the response); style American, multiplier 100.
- Greeks: `Greeks { iv: impliedVolatility, delta, gamma, theta, vega, rho, source: Vendor }` when either is present. Alpaca computes them with Black-Scholes; per the FAQ they are missing for 0DTE contracts, contracts without a two-sided quote, or when IV doesn't converge.
- `underlying_price`: one extra snapshot request for the underlying, `latestTrade.p` from the configured **stock** feed (IEX on Basic). `None` if that request fails; the chain is still returned.
- `as_of` / `provenance.as_of`: newest quote or trade timestamp in the chain, else fetch time.

### `news` — `GET /v1beta1/news`

- Query: `sort=desc`, `include_content=false`, `limit` (1–50, documented max), `symbols` for company news, `start`/`end` from the query's `from`/`to`, `page_token`.
- Scopes: `Company` → `symbols` from the served keys (no served keys → empty page, no request); `Market` → no symbol filter; `Top` and `PressReleases` → `Unsupported` (Alpaca has neither category).
- Text filter: applied client-side (case-insensitive, headline and summary). With a filter or a limit above 50, up to 5 pages are read.
- Mapping: `id` → `id` (string); `source` (e.g. `benzinga`); `headline`; `summary` with HTML stripped and entities decoded (empty → `None`); `body` always `None`; `url` (null/empty → `None`); `created_at` → `published_at`; fetch time → `received_at`; `symbols` → `tickers`; `topics` empty. `provenance.as_of` = `updated_at`; `source_ref` = request URL. `NewsPage.next` is the vendor's `next_page_token` after the last page read.

### `dividends` — `GET /v1/corporate-actions`

**Plan.** Corporate actions are part of the Market Data API (`data.alpaca.markets`, same `APCA-API-KEY-ID`/`APCA-API-SECRET-KEY` headers). The plans page says Basic "serves as the default option for both Paper and Live trading accounts, ensuring all users can access essential data with zero cost" and limits Basic only on real-time coverage (IEX, indicative options), stream symbols, the latest 15 minutes of history and call rate; the corporate actions reference names no plan requirement. We found no statement restricting it to a paid plan, so it is used on both plans. **Not yet confirmed with a live Basic key** (`tests/live.rs` `live_corporate_actions`). A 403 is reported as is (`Unauthorized("HTTP 403: …")`), and DVD still shows the SEC EDGAR data.

- Query: `symbols=<symbol>`, `types=cash_dividend,forward_split,reverse_split`, `start` = today − 3,653 days, `end` = today + 90 days, `limit=1000` (documented maximum), `sort=desc`, `page_token`. Up to 10 pages. Default `data_quality=complete` (incomplete records without an ex-date are excluded by Alpaca).
- `start`/`end` filter on `process_date`, "the date when the corporate action is processed by Alpaca", which Alpaca staff say "can be several days (or more) after the `ex_date`"; the 90-day look-ahead picks up declared dividends that haven't been paid. The docs neither allow nor forbid a future `end`: if the request fails with 400/422, it is retried once with `end` = today. How far back Alpaca's history goes isn't documented.
- Alpaca warns it "has no guarantees on the creation time of corporate actions", so the capability is labelled `EndOfDay`, source `Aggregated`.

| `Dividend` | `cash_dividends[]` | `forward_splits[]` / `reverse_splits[]` |
|---|---|---|
| `kind` | `Special` if `special`, else `Regular` | `Split` |
| `amount` | `rate` (per share) | `new_rate / old_rate` (2 = 2-for-1, 0.1 = 1-for-10) |
| `ex_date` | `ex_date` (record skipped if missing/invalid) | `ex_date` |
| `record_date`, `pay_date` | `record_date`, `payable_date` | same |
| `declared_date`, `frequency` | `None` (not in the response) | `None` |
| `currency` | `currency` as sent; **empty stays empty** (the schema: "Empty value can mean USD, non-applicable … or unknown") | empty |

- Records for other symbols are ignored; records without a valid ex-date or amount are skipped and counted in a warning. `sub_type` (`interest`, `return_of_capital`), `foreign`, CUSIP/ISIN and the other action types (mergers, spin-offs, stock dividends, …) are not used.
- Events are sorted newest first. `per_period` and `reported_splits` stay empty (they come from SEC EDGAR).
- `provenance`: `EndOfDay`, `Aggregated`, `as_of` = fetch time, `source_ref` = first page URL (no credentials in it).

### `dividend_calendar` — `GET /v1/corporate-actions` for many symbols

For CALENDAR and TODAY: dividend and split events whose **ex-date** is in `[from, to]` for a set of securities, in as few requests as possible.

- Query: as for `dividends`, but `symbols=A,B,…` with up to 100 symbols per request (the docs give no maximum; our choice), or no `symbols` at all when the request has no keys (the parameter is optional: every symbol). Keys Alpaca doesn't serve are skipped; if none is served → `NotFound`, no request.
- Window: `start` = `from` − 7 days, `end` = `to` + 75 days. `start`/`end` filter on process date, which trails the ex-date (actions are processed around the pay date); the extra week before `from` covers large special dividends paid before their ex-date. The ex-date filter is applied here. If a future `end` is rejected (400/422), the request is retried once with `end` = today (announced future dividends are then missing).
- Records map exactly as for `dividends`; each is keyed back to the caller's key (`BRK.B` → `BRK/B US Equity`), or, without keys, `<symbol> US Equity`. Records without a symbol, ex-date or amount are skipped and counted in a warning. Up to 10 pages per request; oldest first; duplicates removed.
- Capability `DividendCalendar` (equities and ETFs), `EndOfDay`, `Aggregated`.

## Streaming

`connect(sink)` checks the keys, spawns one tokio task, and returns a `StreamHandle` at once. URL: `wss://stream.data.alpaca.markets/v2/iex` or `/v2/sip`.

- Subscriptions: `subscribe`/`unsubscribe` send commands over an unbounded channel. The task keeps the desired set in insertion order and sends `{"action":"subscribe"|"unsubscribe","trades":[…],"quotes":[…]}` diffs (at most 500 symbols per message).
- Plan limit: with a symbol limit (Basic: 30) only the first 30 subscribed symbols are streamed. The task reports `Status { connected: true, message: "… plan limit is 30 symbols; N subscribed symbol(s) are not streamed" }`. When a streamed symbol is unsubscribed, the next waiting one is subscribed. Several keys for the same symbol share one slot. Whether the server counts trades and quotes separately toward the 30 is not documented; we count symbols.
- Events: trade `t` → `QuoteUpdate { last: p, last_size: s, volume_increment: s, ts_event: t }`; quote `q` → `QuoteUpdate { bid: bp, bid_size: bs, ask: ap, ask_size: as, ts_event: t }` (a zero price leaves that side unchanged, because a partial update can't express "no bid"). Trade conditions are not interpreted. Bars, corrections (`c`), cancels (`x`), statuses, LULDs and imbalances are ignored, so a cancelled trade's volume isn't backed out.
- Status: `connected: true` after authentication and the first subscription sync; `connected: false` with the reason on every disconnect, and on close.
- Reconnect: exponential backoff with equal jitter (ceiling doubles from 1 s to a 30 s cap, delay uniform in [ceiling/2, ceiling]); reset after a session that stayed authenticated for 30 s. After reconnecting the task re-authenticates and re-subscribes the whole desired set.
- Keepalive: WebSocket pings are answered with pongs. The client pings every 30 s and reconnects if nothing arrived for 90 s.
- Errors: 402 auth failed and 409 insufficient subscription stop the task (retrying can't fix keys or a plan) with a `Status` telling the user what to do; reconnect via a new `connect` after fixing it. 406 connection limit exceeded (most plans allow **one** stream connection per key, so another app using the key blocks this one) retries at the 30 s cap. 401/404/407 reconnect with normal backoff. Others (400, 403, 405, 410, 500) are reported as `Status` and the connection is kept.
- Stops on `close()` or when the handle is dropped (the channel closes); a close frame is sent.

## Rate limits

Basic 200 requests/min, Algo Trader Plus 10,000/min (about-market-data-api). The value is declared in `Capabilities.rate_limit` for the router, and the provider also paces every HTTP request (including each pagination page and the chain's underlying snapshot) through its own token bucket at the same rate, since one routed call can make many requests. Alpaca reports `X-RateLimit-Limit/Remaining/Reset` headers; on 429 the shared `http.rs` only reads `Retry-After`, so the router's own backoff applies.

## Terms, caching, attribution, AI

- Alpaca Terms and Conditions: content is for personal, non-commercial use and may not be "copied, reproduced, republished, uploaded, posted, publicly displayed … transmitted or distributed … to any other computer, server, web site or other medium for publication or distribution" without consent. Making the data available to others through your own application needs 30 days' notice to Alpaca. Fine for this personal app; don't share screens of it.
- Caching: no storage limit found in the terms or docs → `CachePolicy::Unrestricted` (personal use only).
- Attribution: none required by the terms or docs → `None`. News items carry their source (`benzinga`) in `NewsItem.source`.
- AI: `AiPolicy::Unreviewed` (treated as forbidden). The transmission clause above must be reviewed before sending Alpaca data to the ASK model.
- Display: allowed (`display_allowed: true`).

## Tests

- `src/normalize.rs`, `src/occ.rs`, `src/stream.rs`: mappings against the documented examples in `tests/fixtures/` (see `tests/fixtures/SOURCES.md` for each file's origin and which ones are hand-built).
- `src/tests.rs`: the provider end to end against a scripted HTTP server on 127.0.0.1 (request paths, queries, auth headers, pagination, error mapping, missing keys making no requests).
- `src/stream.rs`: the stream task against a local WebSocket server (auth, subscribe/unsubscribe diffs, events, plan limit, reconnect and re-subscribe, auth failure stopping, close/drop).
- `tests/live.rs` (`live_dividend_calendar` added 2026-10-07): `#[ignore]`d smoke tests that read `ALPACA_KEY_ID` / `ALPACA_SECRET_KEY` (and optional `ALPACA_FEED=sip`) from the environment and skip when unset. **They have not been run**: there are no Alpaca keys for this project yet, and secrets belong in the Keychain, not in files.

Everything that could not be confirmed against a live response (quote size units on the stream, whether trades and quotes count separately toward the 30-symbol limit, the snapshot symbol-count limit, exact 403 texts for the OPRA feed on Basic) is noted above.

# meridian-provider-coinbase

Coinbase Exchange public market data for crypto pairs: product search and
reference data, REST quotes, OHLCV candles, and a streaming `ticker` feed.
Provider id `coinbase`. No account or API key.

## Docs this was built against (checked 2026-10-05)

| Topic | URL |
|---|---|
| Exchange APIs overview (links the Market Data Terms) | https://docs.cdp.coinbase.com/exchange/introduction/welcome |
| REST requests / errors (`{"message": ...}`, 4xx/5xx) | https://docs.cdp.coinbase.com/exchange/rest-api/requests |
| REST rate limits | https://docs.cdp.coinbase.com/exchange/rest-api/rate-limits |
| `GET /products` | https://docs.cdp.coinbase.com/api-reference/exchange-api/rest-api/products/get-all-known-trading-pairs |
| `GET /products/{id}/ticker` | https://docs.cdp.coinbase.com/api-reference/exchange-api/rest-api/products/get-product-ticker |
| `GET /products/{id}/stats` | https://docs.cdp.coinbase.com/api-reference/exchange-api/rest-api/products/get-product-stats |
| `GET /products/{id}/candles` | https://docs.cdp.coinbase.com/api-reference/exchange-api/rest-api/products/get-product-candles |
| `GET /currencies` (display names, e.g. "Bitcoin") | https://docs.cdp.coinbase.com/api-reference/exchange-api/rest-api/currencies/get-all-known-currencies |
| WebSocket overview (5 s subscribe rule) | https://docs.cdp.coinbase.com/exchange/websocket-feed/overview |
| WebSocket channels (`ticker`, `heartbeat`) | https://docs.cdp.coinbase.com/exchange/websocket-feed/channels |
| WebSocket rate limits | https://docs.cdp.coinbase.com/exchange/websocket-feed/rate-limits |
| Advanced Trade WS (considered, not used) | https://docs.cdp.coinbase.com/coinbase-app/advanced-trade-apis/websocket/websocket-overview |
| Market Data Terms of Use | https://www.coinbase.com/legal/market_data (403 to automated fetches; read via search summaries) |

## Auth

None. Only public endpoints are used: REST `https://api.exchange.coinbase.com`
and the WebSocket market data feed `wss://ws-feed.exchange.coinbase.com`.

## Rate limits

- REST, public: **10 requests/s per IP, bursts up to 15**. `Capabilities.rate_limit`
  is `{ burst: 15, per_second: 10 }` (what the router enforces per call). Because
  one `quotes` call makes two requests per key and one `bars` call can page, the
  provider also paces its own HTTP requests at 8/s (burst 8).
- WebSocket: 8 connection requests/s per IP (bursts to 20), 100 client messages/s
  per IP, and a socket that hasn't subscribed within 5 s is disconnected. We open
  one connection per `connect()` and send one subscribe message per change.
- No documented per-connection product limit, so `max_stream_symbols` is `None`.

## Endpoints used

| Method | Endpoint | Notes |
|---|---|---|
| `search`, `instrument`, coverage | `GET /products` | Cached in memory for 24 h; delisted products dropped. A failed refresh keeps the old list. |
| `search`, `instrument` (names) | `GET /currencies` | Optional: on failure names fall back to codes (`BTC/USD`). |
| `quotes` | `GET /products/{id}/ticker` + `GET /products/{id}/stats` | Two requests per key, fetched concurrently. |
| `bars` | `GET /products/{id}/candles?granularity=&start=&end=` | Unix-second `start`/`end`, both inclusive, max 300 candles per request. |
| stream | `wss://ws-feed.exchange.coinbase.com`, channel `ticker` | `{"type":"subscribe","product_ids":[...],"channels":["ticker"]}` |

## Why the Exchange feed, not Advanced Trade

Both have an unauthenticated `ticker` channel. The Exchange feed is used
because it is the same venue and product ids as the Exchange REST endpoints
used here, it takes one subscribe message for all products, and its ticker
carries `open_24h`, `last_size`, best bid/ask sizes and an RFC 3339 trade
time. The Advanced Trade ticker documents a 24 h percent change
(`price_percent_chg_24_h`) instead of a 24 h open, and subscriptions there
are per channel message.

## Key mapping and coverage

- `BTCUSD Curncy` <-> product `BTC-USD`. The symbol is split by its quote suffix;
  4-letter quotes are tried first, so `BTCUSDT` -> `BTC-USDT` and `USDCUSD` ->
  `USDC-USD` (which is not a Coinbase Exchange product, so it isn't covered once
  the list is loaded).
- `covers(key)` (inherent method, also `meridian_provider::Coverage`): sector
  `Curncy`, **no exchange code**, symbol `<BASE><QUOTE>` with QUOTE in
  **USD, USDT, USDC**, and a base that is not a fiat code (so `EURUSD Curncy`
  stays with the FX providers). Once `/products` has been loaded (by any REST
  call or the stream), the product must also be listed and not delisted.
  Before that only the static rule applies. `covers` never blocks or does I/O.
- Uncovered or unknown keys return `ProviderError::NotFound` so the router
  falls through; `quotes` skips them.

## Field mapping

All "session" figures are Coinbase's **rolling 24 h window**, not a trading
session. Crypto has no close; `prev_close` is defined as **the price 24 h ago**
(`open_24h` / stats `open`), so the net change on screen is the rolling 24 h
change.

| `Quote` / `QuoteUpdate` | REST source | Stream (`ticker`) source |
|---|---|---|
| `bid` / `ask` | ticker `bid` / `ask` | `best_bid` / `best_ask` |
| `bid_size` / `ask_size` | not provided (`None`) | `best_bid_size` / `best_ask_size` |
| `last` / `last_size` | ticker `price` / `size` | `price` / `last_size` |
| `open` | stats `open` (24 h ago) | `open_24h` |
| `prev_close` | stats `open` (24 h ago) | `open_24h` |
| `high` / `low` | stats `high` / `low` (24 h) | `high_24h` / `low_24h` |
| `volume` | ticker `volume` (24 h, base currency; stats `volume` if absent) | `volume_24h` (24 h, base currency) |
| `vwap` | not provided (`None`) | not provided (`None`) |
| `ts_event` | ticker `time` (last trade), else fetch time | `time` |
| `Provenance.as_of` | ticker `time`, else fetch time | (stream updates carry no provenance) |
| `Provenance.source_ref` | the ticker request URL | — |

Prices arrive as decimal strings and are parsed to `f64`. A missing or empty
field stays `None`. A malformed number is `ProviderError::Parse` on REST; on
the stream the message is logged at debug level and dropped.

Instruments: `Instrument::basic(key, "<currency name> / <quote>", Crypto, quote)`,
`tick_size` = `quote_increment`, `price_decimals` = decimals of
`quote_increment`, `exchange_name` = "Coinbase Exchange".

## Bars and history

- Native granularities: 60, 300, 900, 3600, 21600, 86400 s. Candle rows are
  `[time, low, high, open, close, volume]` (verified against the docs and live
  data; values arrive as JSON numbers).
- Other intervals use the largest native granularity that divides them and are
  aggregated client-side (open = first, high = max, low = min, close = last,
  volume = sum): e.g. 30m from 15m, 2h/4h from 1h, 12h from 6h. Fixed-length
  buckets are aligned to the Unix epoch, so 2h/4h/12h buckets start at 00:00 UTC.
  **Weeks start Monday 00:00 UTC** and **months are calendar months (UTC)**, both
  from daily candles. Only zero-length intervals are `Unsupported`.
- Paging: windows of 300 candles walk backwards from `to` (or the current candle)
  until `from`, an **empty page**, or **100 requests** (30,000 native candles:
  ~20 days of 1m, ~3.4 years of 1h; daily never hits it). An empty page is taken
  as "before listing"; an illiquid product with a 300-candle gap would stop
  early. If the page limit cuts the history, a partial leading aggregated bucket
  is dropped rather than shown with a wrong open.
- Output is ascending, `normalize()`d, filtered to `from <= ts < to`, and always
  `Adjustment::None` (crypto has no corporate actions). The newest candle may be
  the current, still-forming one; it is kept.
- Depth: daily candles go back to each product's listing (BTC-USD: 2015).
  Coinbase notes that no candle is published for intervals without trades.

## Streaming

- `connect(sink)` spawns one tokio task and returns a handle immediately.
  Subscriptions go to the task over an unbounded channel; `close()` or dropping
  the handle stops it.
- The task connects only while the subscription set is non-empty (Coinbase drops
  sockets that don't subscribe within 5 s), and closes the socket when the last
  subscription is removed (status `idle: no subscriptions`).
- Reconnects use exponential backoff with equal jitter: nominal 1 s doubling to
  a 30 s cap, actual delay in [nominal/2, nominal]; reset after a connection that
  stayed up 30 s. Everything is re-subscribed after a reconnect.
- Liveness: the client sends a WebSocket ping every 20 s; any inbound frame
  counts as alive, and 60 s of silence forces a reconnect. Server pings are
  answered with a pong carrying the same payload.
- `StreamEvent::Status { connected, message }` is emitted on state changes only
  (connected, disconnected + retry delay, idle, closed). Subscribe errors from
  Coinbase (e.g. unknown product) are logged.

## Terms, caching, AI

- Coinbase Market Data Terms of Use (via search summaries; the page returns 403
  to automated fetches): use is limited to **personal or research purposes**, and
  the data and derived works may not be redistributed, displayed, or disseminated
  to third parties without written consent. That fits a personal terminal.
- No explicit storage or retention limit was found, so `CachePolicy::Unrestricted`;
  `terms_note` says this is based on a summary, not the full text.
- No attribution requirement found (`attribution: None`).
- Sending the data to a third-party AI model is not addressed, and could be read
  as dissemination: `AiPolicy::Unreviewed` (treated as forbidden).

## Tests

- Unit tests parse fixtures captured from the live endpoints on 2026-10-05
  (`tests/fixtures/`, provenance in `tests/fixtures/SOURCES.md`). They cover DTO
  parsing, normalization, key mapping (incl. USDT/USDC and delisted products),
  candle paging windows, aggregation, WS message handling, subscribe message
  shape, and backoff.
- `stream::tests::local_feed_*` run the real stream task against a local
  WebSocket server: subscribe filtering, ping/pong, quote events, reconnect and
  re-subscribe after a dropped socket, incremental (un)subscribe, idle close,
  `close()` and handle drop.
- Live tests (network, ignored by default):

  ```
  cargo test --manifest-path core/Cargo.toml -p meridian-provider-coinbase --test live -- --ignored --nocapture --test-threads=1
  ```

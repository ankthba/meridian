# meridian-provider-kraken

Kraken spot public market data for crypto pairs: pair search and reference
data, REST quotes, OHLC candles (last 720 only), and a streaming WebSocket v2
`ticker` feed. Provider id `kraken`. No account or API key.

## Docs this was built against (checked 2026-10-05)

| Topic | URL |
|---|---|
| Spot REST introduction (base URL, `error` array, `E`/`W` severities) | https://docs.kraken.com/api/docs/guides/spot-rest-intro |
| REST errors | https://docs.kraken.com/api/docs/guides/spot-errors |
| REST rate limits (authenticated counters) | https://docs.kraken.com/api/docs/guides/spot-rest-ratelimits |
| Public endpoint rate limits | https://support.kraken.com/articles/206548367-what-are-the-api-rate-limits- |
| Historical data guidance (720 candles, ~1 s between calls) | https://docs.kraken.com/exchange/guides/general/historical-data |
| `AssetPairs` | https://docs.kraken.com/api/docs/rest-api/get-tradable-asset-pairs |
| `Ticker` | https://docs.kraken.com/api/docs/rest-api/get-ticker-information |
| `OHLC` | https://docs.kraken.com/api/docs/rest-api/get-ohlc-data |
| WebSocket v2 intro (endpoints, connection limits, idle timeout) | https://docs.kraken.com/api/docs/guides/spot-ws-intro |
| WS v2 `ticker` | https://docs.kraken.com/api/docs/websocket-v2/ticker |
| WS v2 `ping` | https://docs.kraken.com/api/docs/websocket-v2/ping |
| WS v2 `heartbeat` | https://docs.kraken.com/api/docs/websocket-v2/heartbeat |
| Global Terms of Service | https://www.kraken.com/legal/global-terms |

## Auth

None. REST `https://api.kraken.com/0/public/*` and WebSocket
`wss://ws.kraken.com/v2`.

## Rate limits

- Public REST: rate limited per IP (per IP and pair for `OHLC`/`Trades`); Kraken
  says **1 call per second or less** stays within the limits. `Capabilities.rate_limit`
  is `{ burst: 1, per_second: 1 }` and the provider paces its own requests the
  same way. `quotes` uses **one** `Ticker` call for all keys.
- WebSocket: Cloudflare allows ~**150 connection attempts per rolling 10 minutes
  per IP** (exceeding it is a 10-minute ban). Kraken recommends reconnecting
  quickly a few times, then no faster than every 5 s; our backoff (1 s doubling
  to 30 s) stays well inside both.
- No documented per-connection symbol limit: `max_stream_symbols` is `None`.

## Endpoints used

| Method | Endpoint | Notes |
|---|---|---|
| `search`, `instrument`, coverage, name mapping | `AssetPairs?assetVersion=1` | Cached 24 h. With `assetVersion=1` the keys are display names (`BTC/USD`) that equal the WS v2 symbols, and `base`/`quote` are display codes (`BTC`, not `XXBT`); `altname` (`XBTUSD`) is the REST pair name. |
| `quotes` | `Ticker?pair=XBTUSD,ETHUSD,...&assetVersion=1` | Response keyed by `BTC/USD`. |
| `bars` | `OHLC?pair=XBTUSD&interval=<min>&assetVersion=1` | |
| stream | `wss://ws.kraken.com/v2`, channel `ticker` | `{"method":"subscribe","params":{"channel":"ticker","symbol":[...],"event_trigger":"trades","snapshot":true},"req_id":n}` |

Errors arrive in the JSON `error` array with HTTP 200 and are mapped:
`EGeneral:Too many requests`, `EAPI:Rate limit exceeded`, `EService:Throttled`
-> `RateLimited`; `EQuery:Unknown asset pair` -> `NotFound`; any other `E...`
-> `Upstream`. `W...` warnings are logged only. Non-2xx HTTP statuses go
through `ProviderError::from_status`.

## Key mapping and coverage

- `BTCUSD Curncy` <-> WS v2 / display `BTC/USD` <-> REST `XBTUSD`, all resolved
  through `AssetPairs?assetVersion=1` (also `DOGEUSD` <-> `DOGE/USD` <-> `XDGUSD`).
  The symbol is split by quote suffix, 4-letter quotes first, so `BTCUSDT` ->
  `BTC/USDT` and `USDCUSD` -> `USDC/USD`.
- `covers(key)` (inherent method, also `meridian_provider::Coverage`): sector
  `Curncy`, **no exchange code**, symbol `<BASE><QUOTE>` with QUOTE in
  **USD, USDT, USDC, EUR** (EUR because most Kraken crypto is also listed
  against EUR), and a base that is not a fiat code. Kraken lists FX pairs such as
  `EUR/USD`; the fiat-base rule leaves `EURUSD Curncy` to the FX providers. Once
  the pair list is loaded, `BASE/QUOTE` must be listed. `covers` never blocks or
  does I/O.
- Uncovered or unknown keys return `ProviderError::NotFound`; `quotes` skips them.

## Field mapping

Everything maps to Kraken's **rolling 24 h** figures, the same semantics as
the Coinbase provider. `prev_close` means **the price 24 h ago**, so the net
change on screen is the rolling 24 h change.

| `Quote` / `QuoteUpdate` | REST `Ticker` | Stream `ticker` |
|---|---|---|
| `ask` / `ask_size` | `a[0]` / `a[2]` (lot volume) | `ask` / `ask_qty` |
| `bid` / `bid_size` | `b[0]` / `b[2]` (lot volume) | `bid` / `bid_qty` |
| `last` / `last_size` | `c[0]` / `c[1]` | `last` / not provided |
| `high` / `low` | `h[1]` / `l[1]` (last 24 h) | `high` / `low` (24 h) |
| `volume` | `v[1]` (last 24 h, base currency) | `volume` (24 h, base currency) |
| `vwap` | `p[1]` (last 24 h) | `vwap` (24 h) |
| `open`, `prev_close` | **`None`**: the REST ticker has no 24 h reference price | **derived**: `last - change` (Kraken's 24 h change), rounded to the pair's `pair_decimals` |
| `ts_event` | fetch time (the endpoint has no timestamp) | `timestamp` |
| `Provenance.as_of` | fetch time | (stream updates carry no provenance) |
| `Provenance.source_ref` | the `Ticker` request URL | — |

Kraken's REST `o` ("today's opening price", since 00:00 UTC) and the `[0]`
"today" elements are **not** mapped: they describe the UTC day, while every
other field here (and the stream) is rolling 24 h. A REST-only quote therefore
shows no net change; the first stream snapshot fills it in.

Numbers arrive as strings (REST) or JSON numbers (WS) and are parsed to `f64`;
missing fields stay `None`. Malformed numbers are `ProviderError::Parse` on
REST; on the stream the element is logged at debug level and dropped.

Instruments: `Instrument::basic(key, "BTC/USD", Crypto, quote)`, `tick_size` =
`tick_size` (or `10^-pair_decimals`), `price_decimals` = `pair_decimals`,
`exchange_name` = "Kraken". Kraken publishes no full asset names.

## Bars and history

- **Kraken's REST `OHLC` returns only the most recent 720 candles per interval**
  (plus the current, not-yet-committed one), regardless of `since`: daily ~2
  years, 1h 30 days, 1m 12 hours. Older data is not available from this provider.
- Native intervals (minutes): 1, 5, 15, 30, 60, 240, 1440, 10080, 21600. Rows are
  `[time, open, high, low, close, vwap, volume, count]`.
- Other intervals use the largest native interval that divides them, aggregated
  client-side (open first, high max, low min, close last, volume sum): 2h/6h from
  1h, 12h from 4h, 3m from 1m, 10m from 5m. Buckets are aligned to the Unix epoch
  (2h/6h/12h start at 00:00 UTC).
- **Week uses Kraken's native weekly candles, which start Thursday 00:00 UTC**
  (epoch-aligned) and reach back ~13 years (679 candles for BTC/USD on
  2026-10-05). This differs from the Coinbase provider's Monday weeks; aggregating
  from daily would give Monday weeks but only ~2 years.
- **Month** is aggregated from daily candles (calendar months, UTC), so it covers
  only the last ~23 months. Because the daily history is cut at 720 candles, the
  partial first month is dropped rather than shown with a wrong open.
- The last native candle is the current, uncommitted period; **it is kept** (as
  with Coinbase) so charts show the forming bar. Re-fetch to update it.
- Output is ascending, `normalize()`d, filtered to `from <= ts < to`, always
  `Adjustment::None`.
- Deep history: `docs/research/crypto-fx-rates-macro.md` lists Kraken's
  downloadable OHLCVT CSV files as a source, while the developer guide above says
  Kraken offers no bulk historical dump. Neither is used or verified here.

## Streaming

- `connect(sink)` spawns one tokio task and returns a handle immediately;
  subscriptions go over an unbounded channel; `close()` or dropping the handle
  stops the task.
- The task connects only while the subscription set is non-empty and closes the
  socket when the last subscription is removed (status `idle: no subscriptions`).
- `event_trigger` is `trades` (the documented default): one update per trade,
  like the Coinbase ticker. `snapshot: true` gives a full ticker right after
  subscribing.
- Kraken closes connections after about a minute of inactivity, so the client
  sends an application-level `{"method":"ping","req_id":n}` every 20 s. Any inbound
  frame (ticker, `heartbeat`, `pong`, status) counts as alive; 60 s of silence
  forces a reconnect. Protocol-level pings are answered with pongs.
- Reconnects: exponential backoff with equal jitter, nominal 1 s doubling to a
  30 s cap, actual delay in [nominal/2, nominal], reset after 30 s of healthy
  connection. Everything is re-subscribed after a reconnect.
- `StreamEvent::Status` is emitted on connection state changes, and also (with
  `connected: true`) when the `status` channel reports a system state other than
  the routine initial `online` (e.g. `maintenance`). Failed subscriptions (e.g.
  `Currency pair not supported`) are logged.

## Terms, caching, AI

- Kraken's Global Terms of Service (sections 8-9) forbid distributing, selling, or
  making Kraken content available to third parties and forbid data extraction by
  scraping or other automation outside the provided API. The public market-data
  API needs no account. Personal display in this terminal fits; redistribution
  does not.
- No retention limit was found, so `CachePolicy::Unrestricted`. The terms are
  general rather than API-specific, so `terms_note` says this is a reading, not
  an explicit grant.
- No attribution requirement found (`attribution: None`).
- Third-party AI processing is not addressed: `AiPolicy::Unreviewed` (treated as
  forbidden).

## Tests

- Unit tests parse fixtures captured from the live endpoints on 2026-10-05
  (`tests/fixtures/`, provenance in `tests/fixtures/SOURCES.md`): DTO parsing,
  error mapping, normalization (incl. the derived 24 h reference price), key
  mapping (USDT/USDC/EUR, `XBT`/`XDG` altnames, FX exclusion), aggregation, WS
  message handling, subscribe/ping message shapes, backoff.
- `stream::tests::local_feed_*` run the real stream task against a local
  WebSocket server: filtering, ping/pong, quote events, system-status events,
  reconnect + re-subscribe, incremental (un)subscribe with `req_id`, idle close,
  `close()` and handle drop.
- Live tests (network, ignored by default; keep one thread so the 1 req/s pacing
  holds):

  ```
  cargo test --manifest-path core/Cargo.toml -p meridian-provider-kraken --test live -- --ignored --nocapture --test-threads=1
  ```

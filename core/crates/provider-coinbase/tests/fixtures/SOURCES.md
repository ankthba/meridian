# Fixture sources

All captured 2026-10-05 from the public Coinbase Exchange endpoints (no key)
with `curl -A "Meridian/0.1 (personal use)"` (REST) or a minimal WebSocket
client (WS). Arrays were trimmed for size; values are unmodified.

| File | Origin | Trimming |
|---|---|---|
| `products.json` | `GET https://api.exchange.coinbase.com/products` | 9 of 839 products kept: BTC-USD, ETH-USD, BTC-USDT, BTC-USDC (delisted), USDT-USD, USDT-USDC, ETH-BTC, USDC-EUR, DNT-USDC (delisted) |
| `currencies.json` | `GET https://api.exchange.coinbase.com/currencies` | 6 of 508 currencies kept; the bulky `details`, `supported_networks`, `convertible_to` fields removed |
| `ticker_btc_usd.json` | `GET https://api.exchange.coinbase.com/products/BTC-USD/ticker` | none |
| `stats_btc_usd.json` | `GET https://api.exchange.coinbase.com/products/BTC-USD/stats` | none |
| `candles_btc_usd_1d.json` | `GET https://api.exchange.coinbase.com/products/BTC-USD/candles?granularity=86400` | first 10 of 350 rows (no start/end given) |
| `candles_btc_usd_1h.json` | `GET https://api.exchange.coinbase.com/products/BTC-USD/candles?granularity=3600&start=1791100800&end=1791136800` | none (11 rows) |
| `error_not_found.json` | `GET https://api.exchange.coinbase.com/products/NOPE-USD/ticker` (HTTP 404) | none |
| `ws_subscriptions.json` | `wss://ws-feed.exchange.coinbase.com` after `{"type":"subscribe","product_ids":["BTC-USD","ETH-USD"],"channels":["ticker","heartbeat"]}` | one message |
| `ws_ticker.json` | same session, a BTC-USD `ticker` message | one message |
| `ws_heartbeat.json` | `wss://ws-feed.exchange.coinbase.com` after subscribing `heartbeat` for BTC-USD | one message |
| `ws_error.json` | `wss://ws-feed.exchange.coinbase.com` after subscribing `ticker` for `NOPE-USD` | one message |

# Fixture sources

All captured 2026-10-05 from the public Kraken endpoints (no key) with
`curl -A "Meridian/0.1 (personal use)"` (REST) or a minimal WebSocket client
(WS). Maps/arrays were trimmed for size; values are unmodified.

| File | Origin | Trimming |
|---|---|---|
| `asset_pairs.json` | `GET https://api.kraken.com/0/public/AssetPairs?assetVersion=1` | 10 of 1,458 pairs kept: BTC/USD, ETH/USD, USDC/USD, USDT/USD, BTC/USDT, BTC/USDC, EUR/USD, DOGE/USD, BTC/EUR, ETH/BTC |
| `ticker.json` | `GET https://api.kraken.com/0/public/Ticker?pair=XBTUSD,ETHUSD,USDCUSD&assetVersion=1` | none |
| `ohlc_btc_usd_1440.json` | `GET https://api.kraken.com/0/public/OHLC?pair=XBTUSD&interval=1440&assetVersion=1` | last 10 of 721 rows; `last` kept |
| `error_unknown_pair.json` | `GET https://api.kraken.com/0/public/Ticker?pair=NOPEUSD` (HTTP 200) | none |
| `error_invalid_arguments.json` | `GET https://api.kraken.com/0/public/OHLC?pair=XBTUSD&interval=7` (HTTP 200) | none |
| `ws_status.json` | `wss://ws.kraken.com/v2`, first message after connecting | one message |
| `ws_subscribe_ack.json` | same session, after `{"method":"subscribe","params":{"channel":"ticker","symbol":["BTC/USD","ETH/USD"]},"req_id":1}` | one of two acks |
| `ws_ticker_snapshot.json` | same session, BTC/USD `ticker` snapshot | one message |
| `ws_ticker_update.json` | same session, ETH/USD `ticker` update | one message |
| `ws_pong.json` | same session, reply to `{"method":"ping","req_id":2}` | one message |
| `ws_heartbeat.json` | same session | one message |
| `ws_subscribe_error.json` | `wss://ws.kraken.com/v2` after subscribing `ticker` for `["BTC/USD","NOPE/USD"]` | one message |

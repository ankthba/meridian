# Fixture sources

All captured from the live public Frankfurter API (no key) on 2026-10-05 with `curl -A "Meridian/0.1 (personal use)"`. Responses are unmodified.

| File | Request |
|---|---|
| `rates_eur_usd_gbp_window.json` | `https://api.frankfurter.dev/v2/rates?base=EUR&quotes=USD,GBP&providers=ecb&from=2026-09-28` |
| `rates_usd_jpy_range.json` | `https://api.frankfurter.dev/v2/rates?base=USD&quotes=JPY&providers=ecb&from=2026-09-01&to=2026-10-05` |
| `rates_empty.json` | `https://api.frankfurter.dev/v2/rates?base=EUR&quotes=RUB&providers=ecb` (the ECB no longer publishes RUB) |
| `error_invalid_currency.json` | `https://api.frankfurter.dev/v2/rate/USD/XXX?providers=ecb` (HTTP 422 body) |

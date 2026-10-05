# meridian-provider-frankfurter

FX reference rates from the European Central Bank, served by the Frankfurter API. Provider id: `frankfurter`.

## Built against (checked 2026-10-05)

| What | URL |
|---|---|
| API docs (v2) | https://frankfurter.dev/docs/ |
| Overview, terms, fair use | https://frankfurter.dev/ |
| ECB provider record (`GET /v2/providers/ecb`) | https://api.frankfurter.dev/v2/providers/ecb |
| ECB reuse terms | https://www.ecb.europa.eu/services/using-our-site/disclaimer/html/index.en.html |
| ECB reference rates | https://www.ecb.europa.eu/stats/policy_and_exchange_rates/euro_reference_exchange_rates/html/index.en.html |

- **API version:** v2 (`https://api.frankfurter.dev/v2`). v1 is deprecated but still available.
- **Auth:** none.
- **Rate limits:** Frankfurter says "There are no quotas. Requests are rate-limited to prevent abuse, but there are no monthly or daily caps." We declare `RateLimit::per_minute(60)` to stay polite.
- **Source pinning:** v2 blends 100+ central banks by default. Every request passes `providers=ecb`, so all data is the ECB euro reference rate series (since 1999-01-04). Mixing providers would change the series' fixing time and methodology from one day to the next.

## Endpoints

| Use | Request |
|---|---|
| `quotes` | `GET /v2/rates?base=<B>&quotes=<Q1,Q2>&providers=ecb&from=<today-14d>` (one request per base currency) |
| `bars` | `GET /v2/rates?base=<B>&quotes=<Q>&providers=ecb&from=<d>&to=<d>` |

Responses are arrays of `{"date","base","quote","rate"}`. An unknown currency returns HTTP 422 `{"status":422,"message":"invalid currency: XXX"}`, mapped to `NotFound`. A currency the ECB no longer publishes (e.g. RUB) returns `[]`, so no quote is produced.

## Mapping

| Meridian | Source |
|---|---|
| Key | `BASEQUOTE Curncy` with both currencies in the ECB list (`covers()`), e.g. `EURUSD`, `USDJPY` |
| `Quote.last` | Latest ECB fixing |
| `Quote.prev_close` | The previous fixing (prior TARGET business day) |
| `Quote.ts_event` | Fixing date at 00:00 UTC. Fixings are dated, not timed. The ECB publishes around 16:00 CET |
| `Quote.flags` | `STALE` when the latest fixing is more than 7 days old |
| bid / ask / open / high / low / volume | `None`. A reference rate has none of these |
| Daily bar | open = high = low = close = the fixing; `volume = 0` (FX reference rates have no volume) |
| Weekly / monthly bar | Aggregated from daily fixings: open = first, high/low = extremes, close = last. Stamped at the period start (Monday / the 1st, 00:00 UTC). A range that starts mid-period gives a partial first bar |
| Intraday bars | `Unsupported` |
| `instrument` | `"<B>/<Q> ECB reference rate"`, asset class `Fx`, currency = quote currency |

Non-EUR pairs (e.g. USDJPY) are **cross rates Frankfurter computes from the ECB's EUR rates**. The ECB requires modifications to be stated, so the attribution says so.

## Terms

- **Attribution** (on `Capabilities` and every `Provenance`): "Source: European Central Bank euro foreign exchange reference rates, via Frankfurter. Pairs not quoted against EUR are cross rates derived from the ECB EUR rates." The ECB disclaimer allows free reuse if "the ECB must be cited as the source" and modifications are "stated explicitly".
- **Cache:** `Unrestricted`.
- **AI policy:** `Allowed`. The ECB grants free use of its published information subject only to citation and disclosure of modifications. Frankfurter says the rates "fall under each provider's terms".
- The ECB describes reference rates as being for information purposes only. They are not tradable quotes, which is why the provenance is `EndOfDay` / `Official`.

## Tests

- Unit tests parse fixtures captured from the live API (`tests/fixtures/`, see `SOURCES.md`).
- Live tests (ignored by default):
  `cargo test -p meridian-provider-frankfurter --test live -- --ignored --nocapture`

  Results on 2026-10-05: EURUSD last 1.1204 (prev 1.1225), USDJPY 158.23 (prev 157.67), both dated 2026-10-05. Daily EURUSD since 2025-01-01: 449 bars. Monthly: 22 bars. BTCUSD correctly not served.

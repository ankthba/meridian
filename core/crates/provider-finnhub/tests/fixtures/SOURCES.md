# Fixture sources

All fixtures were captured on **2026-10-05**. None came from a live API call (no key was used).

Finnhub's docs page (https://finnhub.io/docs/api) is rendered by JavaScript from Finnhub's own Swagger file, https://finnhub.io/static/swagger.json (linked from the docs' Introduction as "Swagger schema: Download"). Each fixture is the `sampleResponse` of the endpoint in that file. SHA-256 of the swagger file as downloaded: `2313bfb03d718257c705d5c48c5185772c86041e259188164d3f7eb3cfbe13e3`. The rendered docs page was spot-checked in a browser and shows the same samples.

| File | Endpoint (swagger path) | Changes |
|---|---|---|
| `finnhub_company_news.json` | `GET /company-news` (Company News) | The published sample ends with a trailing comma (`},\n]`); removed the comma. Otherwise verbatim. |
| `finnhub_market_news.json` | `GET /news` (Market News) | The published sample ends with an extra `}` (`}\n  }]`); removed it. Otherwise verbatim. |
| `finnhub_recommendation.json` | `GET /stock/recommendation` (Recommendation Trends) | Verbatim. |
| `finnhub_profile2.json` | `GET /stock/profile2` (Company Profile 2) | Verbatim. |
| `finnhub_earnings.json` | `GET /stock/earnings` (Earnings Surprises) | Verbatim. |
| `finnhub_earnings_calendar.json` | `GET /calendar/earnings` (Earnings Calendar) | Added 2026-10-07. The field values of the two documented `sampleResponse` rows (Apple, 2020-01-28 and 2019-10-30), read from the swagger file; re-indented, so whitespace is not byte-identical to the published sample. |

Not fixtures, but used inline in `src/tests.rs`, **hand-built** with obviously-test values:

- `{}` for an unknown symbol on `/stock/profile2` (widely reported behaviour; not in Finnhub's docs).
- `CALENDAR_EDGES` in `src/tests.rs`: earnings calendar rows for made-up symbols (`TSTA`, `TST.B`, …) covering the other documented `hour` values (`bmo`, `dmh`), an empty `hour`, nulls, and malformed rows.
- Error bodies `{"error": "..."}` for 401/403/429. Finnhub's docs only say a 429 is returned when the limit is exceeded; the exact error wording is **not verified**.

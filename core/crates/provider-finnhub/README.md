# meridian-provider-finnhub

Finnhub provider, id `finnhub`. **Free-tier endpoints only.**

Implements `news` (company and market news), `profile`, `recommendations`, `earnings`, and `earnings_calendar`. Everything else returns `Unsupported`.

## Documentation (all checked 2026-10-05)

Finnhub's docs page is rendered by JavaScript, so a plain fetch returns nothing. The docs are generated from Finnhub's own Swagger file, which was read directly; the rendered pages were then checked in a browser.

| Topic | URL | How verified |
|---|---|---|
| API reference | https://finnhub.io/docs/api | rendered in a browser |
| Swagger schema behind the docs | https://finnhub.io/static/swagger.json | downloaded and read (paths, schemas, `freeTier`/`premium` flags, samples) |
| Authentication | https://finnhub.io/docs/api/authentication | rendered page + swagger `extraDocs` |
| Rate limits | https://finnhub.io/docs/api/rate-limit | rendered page + swagger `extraDocs` |
| Company News | https://finnhub.io/docs/api/company-news | swagger + rendered page |
| Market News | https://finnhub.io/docs/api/market-news | swagger |
| Recommendation Trends | https://finnhub.io/docs/api/recommendation-trends | swagger |
| Company Profile 2 | https://finnhub.io/docs/api/company-profile2 | swagger + rendered page |
| Earnings Surprises | https://finnhub.io/docs/api/company-earnings | swagger + rendered page |
| Earnings Calendar (checked 2026-10-07) | https://finnhub.io/docs/api/earnings-calendar | swagger (`/calendar/earnings`, definitions `EarningsCalendar` and `EarningRelease`, `freeTier`, `sampleResponse`) |
| Plans | https://finnhub.io/pricing | rendered in a browser |
| Terms of service | https://finnhub.io/terms-of-service | rendered in a browser |

**Search-only / unverified:**

- Class shares as `BRK.B`: not stated in Finnhub's docs (its symbology note covers only `TICKER.EXCHANGE` for non-US listings). Mapped from `BRK/B` or `BRK.B` on third-party evidence.
- `{}` from `/stock/profile2` for unknown symbols: widely reported, not documented.
- Error-body wording (`{"error": "..."}`) for 401/403/429: not documented beyond "you will receive a response with status code 429".
- Units of `marketCapitalization` / `shareOutstanding`: the docs say only "Market Capitalization." and "Number of oustanding shares." Millions is inferred from the documented sample (Apple: `1415993` and `4375.48`).
- News latency: Finnhub calls these "real-time" endpoints; actual latency is unverified.
- Earnings calendar: whether a request without `symbol` is ever truncated (a row cap per response) is not documented. `tests/live.rs` prints the row count for a two-week window and checks per-symbol requests against the window.
- Earnings calendar: how far ahead "new updates" reach on the free tier is not stated; the swagger only says "1 month of historical earnings and new updates".

## What is free (verified)

| Endpoint | Free? | Free-tier limit | Evidence |
|---|---|---|---|
| `GET /company-news` | yes | "1 year of historical news and new updates"; "only available for North American companies" | swagger `freeTier`; pricing "Company News: 1 year and real-time updates" |
| `GET /news` | yes | latest batch only (no date parameters) | swagger `premium: null` |
| `GET /stock/recommendation` | yes | — | swagger `premium: null`; pricing check mark |
| `GET /stock/profile2` | yes | — | swagger: "the free version of Company Profile"; pricing "Company Profile: v2" |
| `GET /stock/earnings` | yes | "Last 4 quarters" | swagger `freeTier`; pricing "EPS Surprises: 4 quarters" |
| `GET /calendar/earnings` | yes | "1 month of historical earnings and new updates" | swagger `freeTier` (checked 2026-10-07), `premium: null` |
| `GET /stock/financials-reported` | yes, **not implemented** | — | Skipped: as-reported statements need a concept-mapping layer (that's the EDGAR provider's job). |
| `GET /stock/price-target` | **no** ("Premium required.") | — | Not called; `Recommendations.target_*` stay `None`. |
| Press releases | **no** (pricing) | — | `NewsScope::PressReleases` → `Unsupported`. |

## Authentication

- Free Finnhub API key from the Keychain via `FinnhubConfig { api_key: Option<SecretString> }`.
- The docs accept the key either as a `token` query parameter or as an `X-Finnhub-Token` header. We use the **header**, marked sensitive (`HeaderValue::set_sensitive`), so request URLs and `Provenance.source_ref` never contain the key. Error messages are still passed through `redact()`.
- No key, or a blank one: every keyed call returns `Unauthorized("Finnhub API key not set — add it in Settings")` before any HTTP.
- The key is never logged.

## Rate limits

- Free plan: "60 API calls/minute" (pricing page). Every plan also has a 30 calls/second cap (rate-limit docs). Over the limit: HTTP 429.
- `Capabilities.rate_limit = RateLimit::per_minute(60)`, plus an internal 60/min bucket per HTTP request, because company news makes one request per symbol.

## Endpoints and mappings

Base URL `https://finnhub.io/api/v1`.

### Symbols

Only US listings: `Equity` or `Pfd` keys with exchange `US` or none (`AAPL US Equity` → `AAPL`, `BRK/B US Equity` → `BRK.B`). Anything else → `NotFound` without a request (free company news is North-America-only, and other markets use different symbols).

### `news(NewsQuery)`

| Scope | Request |
|---|---|
| `Company` | `GET /company-news?symbol=<s>&from=<YYYY-MM-DD>&to=<YYYY-MM-DD>`, one request per distinct symbol. `from`/`to` are required by Finnhub: an open `to` is today (UTC), an open `from` is 7 days before `to`. No keys → empty page. Keys outside coverage are skipped; if none is covered → `NotFound`. |
| `Market` | `GET /news?category=general` |
| `Top`, `PressReleases` | `Unsupported` (no request) |

| `NewsItem` | From |
|---|---|
| `id` | `id` (int64 → string) |
| `source` | `source` |
| `headline` | `headline`, HTML-stripped and whitespace-collapsed |
| `summary` | `summary`, same cleaning; empty → `None` |
| `body` | `None` (no full text on this API) |
| `url` | `url` |
| `published_at` | `datetime` (Unix seconds) → nanos |
| `received_at` | fetch time |
| `tickers` | `related` split on commas; if empty, the requested symbol (company news) or `[]` (market news) |
| `topics` | `[category]` if non-empty |
| `provenance` | `RealTime`, `Aggregated`, `as_of` = `published_at`, `source_ref` = request URL |

Articles missing an `id`, a positive `datetime`, or a headline are dropped. Then, client-side: `from`/`to` bounds (inclusive, nanos), `text` filter (case-insensitive, headline or summary), de-duplication by ID, newest first, truncate to `limit` (0 = no limit).

### `recommendations(key)` → `GET /stock/recommendation?symbol=<s>`

Uses the period with the latest `period` date (not array position): `strongBuy`, `buy`, `hold`, `sell`, `strongSell` → counts; `as_of` and `provenance.as_of` = that period. `ratings` is empty (no per-broker data on the free tier) and targets are `None` (price targets are premium). A missing count is a `Parse` error, never 0. Empty array → `NotFound`.

### `profile(key)` → `GET /stock/profile2?symbol=<s>`

| `CompanyProfile` | From |
|---|---|
| `website` | `weburl` |
| `ipo_date` | `ipo` |
| `headquarters` | `country` (documented as "Country of company's headquarter"; a country code such as `US`) |
| `market_cap` | `marketCapitalization` × 1,000,000 (millions → USD). Dropped if the profile's `currency` is present and not USD, since `CompanyProfile` has no currency field. |
| `shares_outstanding` | `shareOutstanding` × 1,000,000 |
| everything else | `None` (`finnhubIndustry`, `exchange`, `logo`, `phone` have no field) |

`{}` (unknown symbol) → `NotFound`. `CompanyProfile` has no provenance field.

### `earnings(key)` → `GET /stock/earnings?symbol=<s>`

Each row → `EarningsRecord { fiscal_label: "Q{quarter} {yy}", period_end: period, announce_date: None, eps_actual: actual, eps_estimate: estimate, revenue_*: None }`, newest first. `eps_estimate` is Finnhub's consensus estimate as returned on the free tier. A missing/invalid `period` is a `Parse` error; empty array → `NotFound`. `provenance.as_of` = fetch time.

### `earnings_calendar(EventCalendarRequest)` → `GET /calendar/earnings?from=<YYYY-MM-DD>&to=<YYYY-MM-DD>[&symbol=<s>]`

The docs: "Get historical and coming earnings release. EPS and Revenue in this endpoint are non-GAAP"; estimates come from sell-side and buy-side analysts. Parameters `from`, `to`, `symbol` and `international` (default `false`, US only) are all optional; we always send `from`/`to` and never `international`.

- **Requests.** Keys outside coverage (non-US, non-equity) are skipped; if every key is outside → `NotFound` without a request. Up to 3 covered symbols: one request per symbol with `symbol=` (exact). More symbols, or no keys: **one** request for the window, filtered here to the requested symbols, so a long watchlist costs one call instead of dozens against 60 calls/minute.
- **Response** `{"earningsCalendar": [EarningRelease]}`. A body without the array is a `Parse` error (format change), not "no releases".

| `EarningsEvent` | `EarningRelease` |
|---|---|
| `key` | the requested key for `symbol` (`BRK.B` matches `BRK/B US Equity`); without keys, `symbol` → `<symbol> US Equity` with `.` → `/` |
| `date` | `date` (`YYYY-MM-DD`; rows without a symbol or a valid date are dropped and counted in a warning) |
| `session` | `hour`: `bmo` → before open, `amc` → after close, `dmh` → during market hours (documented values); anything else (usually `""`) → `None` |
| `fiscal_year`, `fiscal_quarter` | `year`, `quarter` as Finnhub reports them (fiscal, e.g. Apple's January release is Q1 2020); out-of-range values → `None` |
| `eps_estimate`, `eps_actual`, `revenue_estimate`, `revenue_actual` | `epsEstimate`, `epsActual`, `revenueEstimate` ("including Finnhub's proprietary estimates"), `revenueActual`; null → `None` |

Rows outside `[from, to]` are dropped, duplicates (same symbol and date) removed, oldest first. Provenance: `EndOfDay`, `Aggregated`, `as_of` = fetch time, `source_ref` = the first request URL.

### Errors

| Response | `ProviderError` |
|---|---|
| 401 | `Unauthorized` |
| 403, or any `{"error": "...access..."}` ("You don't have access to this resource.") | `NotEntitled { plan: "a paid Finnhub plan" }` |
| 429, or `{"error": "...limit..."}` | `RateLimited` |
| 2xx with `{"error": ...}` | classified as above, else `Upstream` |
| body not in the documented shape | `Parse` |

## Terms (terms-of-service, checked 2026-10-05)

- **Personal use only:** the terms forbid redistributing or sharing access to the data "or derived results" with anyone or any third party without Finnhub's written approval, and say every listed plan is strictly for personal use.
- **Delete on termination:** "All data must be deleted should your subscription to that data ends." → `CachePolicy::PurgeOnUnsubscribe` (the store's `purge_provider`).
- **Attribution:** none required → `attribution: None`.
- **Display:** no display restriction for the subscriber → `display_allowed: true`.
- **AI:** the terms don't mention AI. But the no-sharing clause plausibly covers sending Finnhub data (or results derived from it) to a third-party AI service. `AiPolicy::Unreviewed` (treated as forbidden); get written approval from Finnhub before enabling it.

## Tests

`cargo test -p meridian-provider-finnhub` runs offline:

- Normalization of the sample responses from Finnhub's docs (`tests/fixtures/`, provenance in `tests/fixtures/SOURCES.md`).
- End-to-end calls against a local in-process HTTP server (`src/test_server.rs`): the token goes in `X-Finnhub-Token` and never in the URL or `source_ref`; query parameters; de-duplication; error mapping.
- Missing key, unsupported scopes, and uncovered symbols make zero connections.

`tests/live.rs`: `#[ignore]`d smoke tests for the earnings calendar that read `FINNHUB_API_KEY` from the shell environment (never a file) and skip when it is unset. **They have not been run**: no Finnhub key is available to this project's tooling, and keys belong in the Keychain.

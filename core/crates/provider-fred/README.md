# meridian-provider-fred

FRED® (Federal Reserve Economic Data, Federal Reserve Bank of St. Louis) provider, id `fred`.

Implements `Provider::economic_series` (`EconomicSeries`) and `Provider::economic_calendar` (`EconomicCalendar`). Everything else returns `Unsupported`.

## Documentation (all checked 2026-10-05)

Each page below was read directly (fetched or rendered in a browser); fixture JSON was hashed against the page's example. Nothing here is search-only except the one item marked.

| Topic | URL |
|---|---|
| API overview (v1 and v2) | https://fred.stlouisfed.org/docs/api/fred/ |
| `fred/series` | https://fred.stlouisfed.org/docs/api/fred/series.html |
| `fred/series/observations` | https://fred.stlouisfed.org/docs/api/fred/series_observations.html |
| `fred/releases/dates` | https://fred.stlouisfed.org/docs/api/fred/releases_dates.html |
| `fred/release` (reference only, not called) | https://fred.stlouisfed.org/docs/api/fred/release.html |
| Errors and rate limit | https://fred.stlouisfed.org/docs/api/fred/errors.html |
| API keys | https://fred.stlouisfed.org/docs/api/api_key.html |
| Terms of use | https://fred.stlouisfed.org/docs/api/terms_of_use.html |
| Missing-value marker `.` | https://fred.stlouisfed.org/docs/api/fred/v2/release_observations.html (the v1 observations page doesn't mention it) |

**Search-only:** the error text for an unknown series, "Bad Request. The series does not exist.", comes from third-party reports, not FRED's docs. The code matches on "does not exist" inside FRED's documented error body.

The high-importance release IDs and names, and the representative series, were checked on fred.stlouisfed.org (`/release?rid=N` and `/series/<ID>`, which shows each series' release).

## Authentication

- A free FRED API key: a 32-character lower-case alphanumeric string (`api_key.html`). It comes from the Keychain via `FredConfig { api_key: Option<SecretString> }`.
- v1 documents only the `api_key` **query parameter** (v2 uses a header, but v2 has no series-metadata or release-dates endpoints we need). So:
  - the key is appended to the request last, and `Provenance.source_ref` is built from the URL **without** it;
  - every error message is passed through `redact()`, which removes the key, including a partial copy cut off by the 300-character body snippet;
  - transport errors have their URL stripped by `http::network_error`.
- No key, or a blank one: every call returns `Unauthorized("FRED API key not set — add it in Settings")` before any HTTP.
- The key is never logged.

## Rate limits

`errors.html`: up to 120 requests per minute, then HTTP 429; ignoring the throttling can get the key temporarily blocked. The terms also let FRED change limits at any time.

- `Capabilities.rate_limit = RateLimit::per_minute(120)` (the router's bucket, one token per routed call).
- One routed call can make several HTTP requests (series metadata + observation pages; calendar pages), so the provider also paces each HTTP request with its own 120/min `TokenBucket`.
- 429 → `RateLimited` (with `Retry-After` if sent).

## Endpoints and mappings

Base URL `https://api.stlouisfed.org/fred`. All requests send `file_type=json`.

### `economic_series(SeriesRequest { id, from, to })`

1. `GET /series?series_id=<id>` → `seriess[0]`: `id`, `title`, `units`, `frequency`, `seasonal_adjustment`, `last_updated`, `notes`. An empty `seriess` → `NotFound`.
2. `GET /series/observations?series_id=<id>&sort_order=asc&limit=100000&offset=<n>[&observation_start=<from>][&observation_end=<to>]` → `observations[{date, value}]`.

| `EconomicSeries` | From |
|---|---|
| `id` | `seriess[0].id` (FRED's canonical ID) |
| `title`, `units`, `frequency` | same-named fields |
| `seasonal_adjustment`, `notes` | same-named fields, trimmed; empty → `None` |
| `last_updated` | `last_updated` (`2013-07-31 09:26:16-05`, parsed with its UTC offset); unparseable → `None` |
| `observations` | `date` (`YYYY-MM-DD`), `value` parsed as `f64`. **`"."` → `None`** (FRED's missing-value marker; never 0). Any other non-number is a `Parse` error. Sorted by date ascending. |
| `provenance.as_of` | `last_updated`, else fetch time |
| `provenance.source_ref` | first observations request URL, without the key |

**Pagination:** `limit` max is 100000. The next offset is `offset + received` while that is below `count`; an empty page stops. More than 10 pages (1M observations) returns an error instead of partial data.

Observations are the current vintage only (no `realtime_start`/`realtime_end`, so no ALFRED vintages).

### `economic_calendar(CalendarRequest { from, to, countries })`

`GET /releases/dates?realtime_start=<from>&realtime_end=<to>&include_release_dates_with_no_data=true&limit=1000&order_by=release_date&sort_order=asc&offset=<n>`

- `include_release_dates_with_no_data=true` is needed to get scheduled future dates (default `false` "excludes release dates that do not have data").
- `limit` max is 1000; paginated with the same offset rule. More than 50 pages → error ("narrow the date range").
- Dates outside `[from, to]` are dropped as a guard.
- `countries`: FRED's calendar is US-centric. If the filter is non-empty and doesn't contain `US` (case-insensitive), the result is empty and no request is made.

| `EconomicEvent` | Value |
|---|---|
| `release_time` | **Midnight UTC** of FRED's `date` (`date_to_nanos`). FRED gives dates only. |
| `time_known` | `false` |
| `country` | `"US"`. FRED doesn't tag releases by country; a few releases FRED republishes are non-US (e.g. "National Accounts of Japan") and are still labelled US. |
| `event` | `release_name` |
| `period`, `actual`, `consensus`, `prior`, `unit` | `None`. FRED has no consensus forecasts, and this endpoint carries no values. |
| `importance` | `High` for the list below, `Medium` otherwise (FRED has no importance rating) |
| `series_id` | representative series for the `High` releases (below), else `None` |
| `provenance.as_of` | fetch time (the endpoint has no vendor timestamp) |

FRED's docs warn that release dates come from the data sources and may not match when the data appears on FRED or ALFRED.

**High-importance releases** (matched by release ID, or by exact name case-insensitively):

| ID | Release name | Series |
|---|---|---|
| 50 | Employment Situation | `PAYEMS` |
| 10 | Consumer Price Index | `CPIAUCSL` |
| 53 | Gross Domestic Product | `GDP` |
| 54 | Personal Income and Outlays | `PCEPI` |
| 9 | Advance Monthly Sales for Retail and Food Services | `RSAFS` |
| 46 | Producer Price Index | `PPIFIS` |
| 13 | G.17 Industrial Production and Capacity Utilization | `INDPRO` |
| 192 | Job Openings and Labor Turnover Survey | `JTSJOL` |
| 27 | New Residential Construction | `HOUST` |
| 101 | FOMC Press Release | `DFEDTARU` |

Not included on purpose: **H.15 Selected Interest Rates** (release 18) is published every business day, so rating it High would mark every day High. ISM surveys aren't on FRED.

Any listed release that FRED dates on three or more consecutive days in the requested window is rated Medium instead (`demote_daily_releases`). Seen live on 2026-10-07: `releases/dates` lists the FOMC Press Release (101) every day, Saturdays included, because its target-range series (`DFEDTARU`/`DFEDTARL`) update daily, so those dates don't mark FOMC meetings. A one- or two-day window can't show the pattern, so 101 can still appear as High there.

### Errors

FRED returns JSON bodies like `{"error_code":400,"error_message":"..."}` (with a `text/xml` content type).

| Response | `ProviderError` |
|---|---|
| 400, message contains "does not exist" | `NotFound` |
| 400, message mentions `api_key` (not set / not registered) | `Unauthorized` |
| 401 / 403 | `Unauthorized` |
| 404 | `NotFound` |
| 429 | `RateLimited` |
| other (incl. 423 Locked, 500) | `Http { status, message }` |
| body not in the documented shape | `Parse` |

## Terms (terms_of_use.html, checked 2026-10-05)

- **Attribution (mandatory):** "Place the following notice prominently on your application: 'This product uses the FRED® API but is not endorsed or certified by the Federal Reserve Bank of St. Louis.'" That exact text is `ATTRIBUTION`, set on `Capabilities.attribution` and on every `Provenance.attribution`.
- **Third-party series:** some series are owned by third parties and copyrighted (their notes contain "Copyright"). Using those for anything beyond your own personal use needs the owner's permission. Meridian is personal use only.
- **Caching:** the terms have no caching or retention clause → `CachePolicy::Unrestricted`.
- **AI:** the terms say nothing about AI or machine learning. `AiPolicy::Unreviewed` (treated as forbidden) until the user decides.
- **Limits:** FRED may set or change bandwidth and request limits at any time.
- `display_allowed: true`, `requires_credentials: true`.

## Tests

`cargo test -p meridian-provider-fred` runs offline:

- Normalization of FRED's documented example responses (`tests/fixtures/`, provenance in `tests/fixtures/SOURCES.md`).
- End-to-end calls against a local in-process HTTP server (`src/test_server.rs`) serving those fixtures: query parameters, pagination offsets, error mapping, attribution on every payload, and that the key never appears in `source_ref` or error text.
- Missing key → `Unauthorized` with zero connections made.

**No live tests.** No FRED key was available, and the project rule keeps keys only in the Keychain (not env vars or test files). Live verification belongs in the app's opt-in test host (ARCHITECTURE §10).

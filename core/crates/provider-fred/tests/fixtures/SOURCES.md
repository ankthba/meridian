# Fixture sources

All fixtures were captured on **2026-10-05**. None came from a live API call (no key was used).

| File | Source | Changes |
|---|---|---|
| `fred_series_gnpca.json` | JSON example on https://fred.stlouisfed.org/docs/api/fred/series.html | The published example is missing its final `}`; added it. Otherwise verbatim. |
| `fred_series_observations_gnpca.json` | JSON example on https://fred.stlouisfed.org/docs/api/fred/series_observations.html | Whitespace only: one observation per line. The canonical JSON (`JSON.stringify` form) has the same SHA-256 as the page's example: `63edd4bfb8553b07d8d5ca3af21189d691afd7849e73ac7e95528b0d4e24a9d5`. |
| `fred_releases_dates.json` | JSON example on https://fred.stlouisfed.org/docs/api/fred/releases_dates.html | The published example ends its array with a literal `...` line; removed it and the comma before it. Otherwise verbatim (including the double space in release 283's name). Canonical SHA-256 after that one repair: `ecee32914ef7844265a0e2dac9b1825bea8248d56a8a2c04162fb9a37a63466f`. |
| `fred_error_api_key.json` | JSON error example on https://fred.stlouisfed.org/docs/api/fred/errors.html | Verbatim. |
| `fred_observations_missing_values.json` | **Hand-built** to the documented `fred/series/observations` schema, with obviously-test values (dates 2001-01-01..03, values `1.5`, `.`, `2.25`). FRED's v1 docs don't show a missing value; the `.` marker is documented on https://fred.stlouisfed.org/docs/api/fred/v2/release_observations.html ("values for dates with missing observations are reported as a period '.'"). | — |
| `fred_error_series_does_not_exist.json` | **Hand-built** in the documented error-body shape. The message "Bad Request. The series does not exist." is **search-verified only** (third-party reports of FRED's response); it is not in FRED's docs. | — |

Tests that exercise pagination split the documented examples into pages in code (see `paged()` in `src/tests.rs`); the data itself is unchanged.

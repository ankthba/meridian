# Fixture sources

Written 2026-10-05. **No fixture was downloaded from sec.gov or data.sec.gov.**
The SEC requires every automated request to declare the requester's real name
and email in the User-Agent, and we must not invent one, so no live request
was made. Every fixture here is **hand-constructed** to the documented
structure, for a fictitious filer:

- Company "Example Corp", CIK 1 (`0000000001`), ticker `EXMP`; other rows use
  the made-up names "Sample Holdings Inc.", "Testex Industries Inc." and
  "Exa Example Trust".
- Financial values are round test numbers (1000, 1100, 4.6, …), labelled as
  test values in the payload's `description` fields. They are not real.
- Accession numbers use the fake filer-agent prefix `0000000001` / `0000000002`.

The SEC's API page documents the endpoints and the general structure but not a
field-level schema, so field names and types come from the SEC description
plus third-party descriptions (listed per file). The opt-in live tests in
`tests/live.rs` are the check against real responses.

| File | Endpoint it stands in for | Structure source |
|---|---|---|
| `company_tickers.json` | `https://www.sec.gov/files/company_tickers.json` | SEC landing page https://www.sec.gov/file/company-tickers (file exists, no schema). Row format `{"<row index>": {"cik_str": <number>, "ticker": "…", "title": "…"}}` from third-party descriptions, e.g. https://www.thefullstackaccountant.com/blog/intro-to-edgar (CIK as a number without leading zeros). Class shares written with `-` (`SMPL-A`, `SMPL-B`) as the SEC does for e.g. `BRK-B`. |
| `submissions_CIK0000000001.json` | `https://data.sec.gov/submissions/CIK0000000001.json` | https://www.sec.gov/search-filings/edgar-application-programming-interfaces (entity metadata, columnar `filings.recent`, links to older files). Field names/types from https://fundamentalshub.com/blog/data-sec-gov-submissions-json and the third-party OpenAPI description https://raw.githubusercontent.com/api-evangelist/sec/refs/heads/main/openapi/sec-submissions-api-openapi.yml (`accessionNumber`, `filingDate`, `reportDate`, `acceptanceDateTime`, `act`, `form`, `fileNumber`, `filmNumber`, `items`, `size`, `isXBRL`, `isInlineXBRL`, `primaryDocument`, `primaryDocDescription`; `files[]` with `name`, `filingCount`, `filingFrom`, `filingTo`; `addresses.business`/`mailing` with `city`, `stateOrCountry`, `stateOrCountryDescription`). `fiscalYearEnd` is `MMDD`. Rows are deliberately **not** in date order, to exercise sorting. |
| `CIK0000000001-submissions-001.json` | `https://data.sec.gov/submissions/CIK0000000001-submissions-001.json` (an older page named in `filings.files`) | Assumed to use the same columnar arrays as `filings.recent`, at the top level. The SEC page only says older filings are in "additional JSON files"; this layout is from third-party usage and is **unverified**. |
| `companyfacts_CIK0000000001.json` | `https://data.sec.gov/api/xbrl/companyfacts/CIK0000000001.json` | SEC API page (all concepts for a company; facts grouped by unit). Fact fields `start` (durations only), `end`, `val`, `accn`, `fy`, `fp`, `form`, `filed`, optional `frame`, per https://dealcharts.org/blog/sec-edgar-api-guide and https://fundamentalshub.com/blog/sec-data-json-format. Built to exercise: FY + Q1–Q3 facts (Q4 derivation), comparatives repeated in later filings with the *filing's* `fy`/`fp`, a restatement (FY2024 net income 400 → 380 in a later 10-K), year-to-date-only cash flow in 10-Qs, an 8-K fact and an EUR unit that must be ignored, an unmapped concept, and a `dei` block. |
| `example_10k.htm` | A 10-K primary document under `https://www.sec.gov/Archives/edgar/data/…` | Hand-made minimal HTML with an inline-XBRL `<ix:header>` block, a table of contents, a forward-looking-statements paragraph, item headings in three common layouts (table cells, bold paragraph, split spans), and a wrapped cross-reference. Text is placeholder prose. |

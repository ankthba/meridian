# meridian-provider-edgar

SEC EDGAR provider (id `edgar`): ticker ↔ CIK reference data and search,
company profiles and filing history, filing documents as sectioned text, and
as-reported financial statements from XBRL company facts.

## Documentation (checked 2026-10-05)

| Topic | Source |
|---|---|
| Data APIs (submissions, companyfacts), no keys, latency, no CORS | https://www.sec.gov/search-filings/edgar-application-programming-interfaces |
| Fair access: 10 requests/second per user | https://www.sec.gov/about/developer-resources |
| Declared User-Agent format and max rate | https://www.sec.gov/os/webmaster-faq#code-support |
| Archive paths, accession-number format | https://www.sec.gov/search-filings/edgar-search-assistance/accessing-edgar-data |
| Reuse of EDGAR content | https://www.sec.gov/os/webmaster-faq#reuse , https://www.sec.gov/about/privacy-information |
| Ticker file | https://www.sec.gov/file/company-tickers |

The SEC documents endpoints and general structure, not a field-level schema.
Field names were taken from third-party descriptions (listed in
`tests/fixtures/SOURCES.md`) and must be confirmed with the live tests.

## Access model

- **No API key.** The data APIs "do not require any authentication or API keys".
- **Declared User-Agent required.** The SEC's example is
  `Sample Company Name AdminContact@<sample company domain>.com`. We send
  `Meridian/0.1 <contact>`, where `contact` comes from
  `EdgarConfig { contact }` (e.g. `Jane Doe jane@example.com`, set by the user
  in Settings). The contact is configuration, not a credential
  (`requires_credentials: false`), but it is personal: it is never logged and
  never placed in provenance.
- **Missing contact.** If the contact is empty, has no `@`, or isn't printable
  ASCII, `EdgarProvider::new` still succeeds, but every call returns
  `ProviderError::Unauthorized("Set your contact name and email for SEC EDGAR
  in Settings (required by SEC fair-access policy)")` (or a message about
  invalid characters) and no request is sent.
- **Rate limit.** "No more than 10 requests per second, regardless of the
  number of machines". `Capabilities.rate_limit = RateLimit::per_second(10)`
  for the router, and the provider also acquires an internal
  `TokenBucket` (burst 2, 8 tokens/s) before **every** HTTP request, because
  one call can fan out (ticker file, submissions, older pages). Burst 2 + 8/s
  admits at most 10 requests in any one-second window.
- **Terms.** "Information presented on sec.gov is considered public
  information and may be copied or further distributed by users of the web
  site without the SEC's permission" (privacy-information page); "All
  Government-created content on sec.gov and EDGAR public filing content are
  free to access and reuse" (webmaster FAQ). Hence `CachePolicy::Unrestricted`,
  `AiPolicy::Allowed`, no attribution text (none is required).

## Endpoints used

| Endpoint | Used for |
|---|---|
| `https://www.sec.gov/files/company_tickers.json` | Ticker → CIK, search, instrument names. Fetched once, reused for 24 h; if a refresh fails the previous copy is reused. |
| `https://data.sec.gov/submissions/CIK##########.json` | Profile, filings (`filings.recent`), fiscal-year-end fallback |
| `https://data.sec.gov/submissions/<name from filings.files>` | Older filings, fetched only when the recent list can't satisfy `limit` (at most 5 pages per call) |
| `https://data.sec.gov/api/xbrl/companyfacts/CIK##########.json` | Fundamentals |
| `https://www.sec.gov/Archives/edgar/data/<cik>/<accession-no-dashes>/<primaryDocument>` | Filing documents |

CIKs are zero-padded to 10 digits in data.sec.gov paths and unpadded in
archive paths.

## Capabilities

| Capability | Asset classes | Delay | Notes |
|---|---|---|---|
| Search, Reference | Equity | EOD | From the ticker file; the SEC doesn't document its refresh cadence |
| Profile | Equity, ETF | RT | Submissions: "typical processing delay of less than a second" |
| Filings | Equity, ETF | RT | Per company only (`key: None` → `Unsupported`) |
| Fundamentals | Equity | RT | XBRL: "typical processing delay of under a minute" |

Source is `FeedSource::Official`; every payload's provenance is provider
`edgar`, `synthetic: false`, delay `RealTime`, `as_of` = fetch time,
`source_ref` = accession number (filings) or the request URL (fundamentals).
Dividends, quotes, bars, news, estimates, holders and the rest keep the
trait's `Unsupported` default.

## Reference data and search

- Keys must be `Equity` with no exchange, `US`, or a US venue code (`UN`,
  `UW`, `UQ`, `UR`, `UA`, `UP`, `UF`, `UV`). Other keys return `NotFound`, so
  the router falls through. Foreign listings are not mapped because the same
  symbol can name a different company abroad.
- The SEC writes class shares with `-` (`BRK-B`). Lookups map `.`, `/` and
  spaces to `-`, so `BRK.B` and `BRK/B` resolve.
- `instrument(key)` returns the requested key, the SEC title, `cik`, asset
  class `Equity`, currency `USD`. The file doesn't distinguish ETFs or give an
  exchange, so those stay unset.
- Search order: exact ticker, then ticker prefix, then case-insensitive title
  substring, each in file order, up to `limit`. Search results use the SEC's
  ticker spelling (`BRK-B`).

## Profile

From submissions: `sicDescription`/`sic`, headquarters = business address
`city, stateOrCountryDescription` (falling back to `stateOrCountry`), website
if non-empty, fiscal year end as `MM-DD` (from `fiscalYearEnd` `"0930"` →
`"09-30"`). The SEC publishes no business description, CEO, employee count,
founding or IPO date: those are `None`.

## Filings

- Built from the parallel arrays in `filings.recent`; ragged arrays are a
  `Parse` error.
- `forms` filter: case-insensitive **exact** match. `10-K` does not include
  `10-K/A`; ask for `10-K/A` explicitly.
- Sorted newest first (filing date, then acceptance time, then accession),
  truncated to `limit`. If the filtered recent list is shorter than `limit`,
  older pages from `filings.files` are fetched newest first. If an older page
  fails, the filings already loaded are returned and a warning is logged.
- `items` = the comma-separated 8-K `items` string split. `size_bytes` =
  `size`. `description` = `primaryDocDescription`.
- `accepted_at` parses `acceptanceDateTime` (e.g. `2025-10-31T06:01:26.000Z`)
  as written, i.e. UTC per its `Z`. Third-party developer reports (found by
  web search on 2026-10-05, not verified) say fresh filings can carry US
  Eastern wall-clock time labelled `Z` for several hours before the value is
  rewritten to true UTC. The SEC doesn't document the time zone and we can't
  check without live access, so treat `accepted_at` as possibly 4–5 hours
  early for very recent filings. `filed` (the filing date) is unaffected.

## Filing documents

- Only URLs under `<www.sec.gov>/Archives/edgar/data/` are fetched, so the
  declared contact is never sent to another host. Other URLs → `Unsupported`.
- Bodies over **25 MB** are refused (`Content-Length` and streamed size).
  `.htm/.html/.xhtml/.xml` are rendered with html2text 0.17 (plain, no
  decoration, raw table mode, width 120) after removing inline-XBRL
  `<ix:header>` blocks, which browsers hide but html2text would print. `.txt`
  is passed through. Anything else (PDF, images) → `Upstream` error.
- **Sections (10-K, 10-Q and variants).** A heading is a line starting with
  `Item <1–16>[A–C]` followed by punctuation, end of line, or a capitalized
  title. "Item 7 of this report" (lower-case continuation) is not a heading.
  If the heading line has no title, the next non-empty line is used. The table
  of contents repeats every heading, so the body starts at the occurrence,
  of the earliest item that appears more than once, that is followed by the
  most text. Headings before it belong to the cover/TOC. After it, each item
  uses its occurrence followed by the most text. 10-Q items repeat across
  Part I and Part II, so for 10-Qs the preceding `PART` line is part of the
  item's identity and appears in the title (`Part II, Item 1A. Risk
  Factors`). Text before the first item is a `Cover` section; a bare
  `PART …` line just before the next item is dropped from the end of the
  preceding section. Other forms, or periodic reports with no headings,
  return one `Document` section.

## Fundamentals

### Concept mapping

The first concept with a value for the period wins; the chosen concept is
recorded as `source_tag: Some("us-gaap:<Concept>")`. Reported values beat
derived ones (all concepts are tried as reported before any derivation).

| Code | Label | Statement | us-gaap concepts, in order |
|---|---|---|---|
| `revenue` | Revenue | Income | Revenues → RevenueFromContractWithCustomerExcludingAssessedTax → SalesRevenueNet → RevenueFromContractWithCustomerIncludingAssessedTax |
| `cost_of_revenue` | Cost of Revenue | Income | CostOfRevenue → CostOfGoodsAndServicesSold → CostOfGoodsSold |
| `gross_profit` | Gross Profit | Income | GrossProfit |
| `sga` | Selling, General & Administrative | Income | SellingGeneralAndAdministrativeExpense |
| `rnd` | Research & Development | Income | ResearchAndDevelopmentExpense |
| `operating_income` | Operating Income | Income | OperatingIncomeLoss |
| `interest_expense` | Interest Expense | Income | InterestExpense → InterestExpenseNonoperating |
| `pretax_income` | Pretax Income | Income | IncomeLossFromContinuingOperationsBeforeIncomeTaxesExtraordinaryItemsNoncontrollingInterest → IncomeLossFromContinuingOperationsBeforeIncomeTaxesMinorityInterestAndIncomeLossFromEquityMethodInvestments |
| `income_tax` | Income Tax | Income | IncomeTaxExpenseBenefit |
| `net_income` | Net Income | Income | NetIncomeLoss |
| `eps_basic` | EPS (Basic) | Income | EarningsPerShareBasic (unit `<cur>/shares`) |
| `eps_diluted` | EPS (Diluted) | Income | EarningsPerShareDiluted (unit `<cur>/shares`) |
| `shares_diluted` | Diluted Shares (Weighted Avg) | Income | WeightedAverageNumberOfDilutedSharesOutstanding (unit `shares`) |
| `ebitda` | EBITDA | Income | derived: `operating_income + d_and_a`, tag `derived:operating_income+D&A` |
| `cash` | Cash & Equivalents | Balance | CashAndCashEquivalentsAtCarryingValue |
| `short_term_investments` | Short-Term Investments | Balance | ShortTermInvestments → MarketableSecuritiesCurrent → AvailableForSaleSecuritiesDebtSecuritiesCurrent |
| `receivables` | Receivables | Balance | AccountsReceivableNetCurrent |
| `inventory` | Inventory | Balance | InventoryNet |
| `total_current_assets` | Total Current Assets | Balance | AssetsCurrent |
| `ppe_net` | Property, Plant & Equipment (Net) | Balance | PropertyPlantAndEquipmentNet |
| `goodwill` | Goodwill | Balance | Goodwill |
| `total_assets` | Total Assets | Balance | Assets |
| `accounts_payable` | Accounts Payable | Balance | AccountsPayableCurrent |
| `short_term_debt` | Short-Term Debt | Balance | DebtCurrent (total current debt) → LongTermDebtCurrent (current maturities only) → ShortTermBorrowings |
| `total_current_liabilities` | Total Current Liabilities | Balance | LiabilitiesCurrent |
| `long_term_debt` | Long-Term Debt | Balance | LongTermDebtNoncurrent → LongTermDebt (note: includes current maturities) |
| `total_liabilities` | Total Liabilities | Balance | Liabilities |
| `total_equity` | Total Equity | Balance | StockholdersEquity → StockholdersEquityIncludingPortionAttributableToNoncontrollingInterest |
| `cfo` | Cash from Operations | Cash flow | NetCashProvidedByUsedInOperatingActivities |
| `d_and_a` | Depreciation & Amortization | Cash flow | DepreciationDepreciationAndAmortization → DepreciationAndAmortization |
| `capex` | Capital Expenditures | Cash flow | PaymentsToAcquirePropertyPlantAndEquipment, **sign as reported** (a positive payment) |
| `fcf` | Free Cash Flow | Cash flow | derived: `cfo − capex`, tag `derived:cfo-capex` |
| `cfi` | Cash from Investing | Cash flow | NetCashProvidedByUsedInInvestingActivities |
| `dividends_paid` | Dividends Paid | Cash flow | PaymentsOfDividends → PaymentsOfDividendsCommonStock (positive payment, as reported) |
| `buybacks` | Share Repurchases | Cash flow | PaymentsForRepurchaseOfCommonStock (positive payment, as reported) |
| `cff` | Cash from Financing | Cash flow | NetCashProvidedByUsedInFinancingActivities |
| `net_change_cash` | Net Change in Cash | Cash flow | CashCashEquivalentsRestrictedCashAndRestrictedCashEquivalentsPeriodIncreaseDecreaseIncludingExchangeRateEffect → CashAndCashEquivalentsPeriodIncreaseDecrease |

Depth 0 = totals and subtotals, 1 = components.

### Facts and periods

- Only facts from **10-K, 10-K/A, 10-Q, 10-Q/A** count. For each
  (concept, start, end) the **most recently filed** value wins (ties: larger
  accession number), so restatements and recasts replace originals.
- Currency: `USD` if any mapped monetary concept reports in USD; otherwise
  the most used three-letter unit (a us-gaap filer reporting only in another
  currency). Other currency units are ignored.
- Durations of **350–380 days** are fiscal years, **80–100 days** quarters;
  facts without `start` are balance-sheet instants.
- **`fy`/`fp` describe the filing, not the fact.** A FY2025 10-K carries
  FY2024 and FY2023 comparatives tagged `fy: 2025`. So periods are classified
  by their own dates against a fiscal calendar:
  - Fiscal years = the distinct annual (10-K, `fp: FY`) periods.
  - A year's label is the `fy` of its **original** 10-K (the earliest one
    filed within 300 days of the year end). Years seen only as comparatives
    are labelled from the nearest labelled year by whole-year distance. If
    labels conflict, all are recomputed from the latest labelled year.
  - Years are extrapolated forward (the year in progress) and backward to
    cover every fact; gaps between reported years are filled.
  - A date within 7 days of a fiscal year end is Q4; otherwise the quarter is
    `round(days since the prior year end / 91.31)`. Each quarter's end date is
    the date most facts end on.
  - A company with no 10-K yet falls back to the submissions `fiscalYearEnd`,
    labelling each fiscal year by the calendar year it ends in.
- **Annual**: annual durations ending on the fiscal year end (preferring the
  one starting on the fiscal year start) and instants at the year end.
- **Quarterly**: 80–100-day durations ending on the quarter end, instants at
  the quarter end.
- **TTM**: one column per run of four consecutive quarters. Additive flows are
  the sum of the four quarterly values (all four required), tag
  `derived:TTM`. Balance lines are the last quarter's reported value (keeps
  its `us-gaap:` tag). EPS and share counts are omitted. `fiscal_period` is
  `"TTM"`, `fiscal_year` and `period_end` are the last quarter's.

### Derivations (never invented)

- **Q4** of an additive flow = FY − 9-month YTD, or FY − (Q1 + Q2 + Q3) when
  there is no 9-month fact; tag `derived:FY-9M`. Uses one concept for all
  terms.
- **Q2/Q3** of an additive flow when only year-to-date values are reported
  (10-Q cash-flow statements are usually YTD only) = YTD − previous YTD; tag
  `derived:YTD-diff`.
- **Never derived**: EPS, weighted-average share counts (not additive), and
  balance-sheet values. They are `None` when not reported.
- EBITDA and FCF are computed only when both inputs exist for the period.

### Output

Statements for the most recent `periods` columns with data (`periods: 0` is
treated as 1), newest first, grouped Income → Balance → Cash flow; one
`Statement` per (kind, period), skipped when that kind has no value in that
period. Each statement of a kind lists the same lines: every code that has a
value in at least one returned column, in display order, with `value: None`
where a column lacks it.

### Not covered

- IFRS filers (20-F/40-F, `ifrs-full` taxonomy) and company-specific
  extension concepts are not mapped; those companies get `NotFound` or sparse
  statements.
- As-reported only: no standardization beyond the mapping above, no
  point-in-time history (restated values replace the originals).

## Dividends: not implemented

`Dividend` needs an `ex_date`. Company facts give numeric values (`val`) per
*reporting period*, e.g. `CommonStockDividendsPerShareDeclared` for a quarter
or year, not per dividend event, and we found no ex-dividend date in that
data. US-GAAP date elements such as `DividendsPayableDateDeclaredDayMonthYear`
or `DividendsPayableDateOfRecordDayMonthYear` are declaration/record/pay
dates, not ex-dates, and we could not confirm that company facts include
date-typed values at all. Using a period end as an ex-date would be
misleading, so `Capability::Dividends` is not declared and the method
returns `Unsupported`.

## Tests

`cargo test -p meridian-provider-edgar` runs unit tests and end-to-end tests
against a local HTTP server that serves the hand-built fixtures in
`tests/fixtures/` (provenance in `tests/fixtures/SOURCES.md`). They cover CIK
padding, ticker normalization, the missing-contact path (no request sent), the
declared User-Agent, the per-request token bucket, filings filtering/sorting/
paging/URLs, profile mapping, section splitting with a table of contents,
annual/quarterly/TTM selection, Q4 and YTD derivations, EPS not derived, and
restatements.

`tests/live.rs` holds `#[ignore]`d tests against the real SEC endpoints. They
read your contact from `MERIDIAN_SEC_CONTACT` and skip when it's unset. They
were **not run** while building this crate, because the SEC requires a real
contact in the User-Agent and we don't invent one. Run them once with your
own contact to confirm the field names above:

```text
MERIDIAN_SEC_CONTACT="Jane Doe jane@example.com" \
  cargo test --manifest-path core/Cargo.toml -p meridian-provider-edgar --test live -- --ignored
```

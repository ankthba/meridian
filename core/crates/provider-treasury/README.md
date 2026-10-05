# meridian-provider-treasury

US Treasury Daily Par Yield Curve Rates. Provider id: `treasury`.

## Built against (checked 2026-10-05)

| What | URL |
|---|---|
| XML feed documentation | https://home.treasury.gov/treasury-daily-interest-rate-xml-feed |
| Yield curve methodology | https://home.treasury.gov/policy-issues/financing-the-government/interest-rate-statistics/treasury-yield-curve-methodology |
| Copyright status of US government works | https://www.usa.gov/government-copyright (17 U.S.C. § 105) |
| Treasury credit request ("we request that we be given appropriate credit") | https://www.treasurydirect.gov/terms.htm |

- **Auth:** none.
- **Rate limits:** none documented. We declare `RateLimit::per_minute(60)`. A curve call makes 1–2 requests. A series call makes one request per calendar year in range (each about 300 KB).
- **Format choice:** the XML feed (Atom + OData properties) is the documented machine interface. Each rate is a named element (`BC_1MONTH`, `BC_1_5MONTH`, `BC_10YEAR`, …). The CSV download on the rates page is undocumented. Tenors are parsed from element names, not a fixed list, so additions and removals need no code change. Verified examples:
  - 1990: 3 Mo–30 Yr, no 1 Mo or 20 Yr.
  - 2004: no 30 Yr (Treasury suspended it). The feed carries a `BC_30YEARDISPLAY` of `0.00` there, which we ignore.
  - 2025-02-18: 1.5 Mo first appears.
- **Publication:** "usually available ... by 6:00 PM Eastern Time each trading day." Inputs are "indicative, bid-side market price quotations (not actual transactions) for the most recently auctioned securities obtained by the Federal Reserve Bank of New York at or near 3:30 PM".

## Requests

| Use | URL |
|---|---|
| Curve for a month | `.../interest-rates/pages/xml?data=daily_treasury_yield_curve&field_tdr_date_value_month=YYYYMM` |
| Series for a year | `.../interest-rates/pages/xml?data=daily_treasury_yield_curve&field_tdr_date_value=YYYY` |

A month or year with no data returns a valid feed with no `<entry>`.

## Behavior

- **`yield_curve`**: name `UST` (case-insensitive). Other names return `NotFound`.
  - `date: None` gives the latest curve.
  - `date: Some(d)` gives the curve for `d`. If none was published that day (weekend, holiday, not yet published), it returns the latest earlier curve, checking `d`'s month and then the month before. `YieldCurve.date` is always the actual curve date.
  - Points are sorted by maturity and include only tenors published that day. Labels follow Treasury's rate tables: `1 Mo`, `1.5 Month`, `2 Mo`, …, `1 Yr`, …, `30 Yr`. `years` uses months/12 (1.5 Mo = 0.125).
- **`economic_series`**: ID `UST:<tenor>`.
  - Accepted spellings: `UST:10 Yr`, `UST:10Y`, `UST:1.5 Mo`, `UST:1.5 Month`, `UST:3M`, … (case-insensitive). The returned `id` uses the published label, e.g. `UST:1.5 Month`.
  - `from`/`to` are inclusive. Defaults: `to` = today, `from` = `to` − 365 days. Clamped to 1990 onward.
  - Days on which the tenor wasn't published produce no observation. No values are interpolated or carried forward. If the range has no observation at all, it returns `NotFound`.
  - Units are `Percent`, frequency `Daily`. `last_updated` is the feed's `<updated>` stamp.
- **Provenance:**
  - `delay`: `EndOfDay`. `source`: `Official`.
  - `as_of`: the feed's `<updated>` stamp.
  - `source_ref`: the request URL (for multi-year series, the first and last URL).
  - `attribution`: "Source: U.S. Department of the Treasury, Daily Treasury Par Yield Curve Rates."
- **Not implemented:** FiscalData auction results (optional in the plan; no trait method needs them yet).

## Terms

- US federal government works are not subject to copyright (17 U.S.C. § 105; usa.gov: such works are in the public domain domestically). Treasury requests appropriate credit, so the attribution above is set even though it isn't legally required.
- **Cache:** `Unrestricted`. **AI policy:** `Allowed`.

## Tests

- Unit tests parse fixtures captured from the live feed (`tests/fixtures/`, see `SOURCES.md`). They cover tenor-set changes (2004 without the 30-year, 2025 introduction of 1.5 Mo), an empty month, OData null elements, and malformed responses.
- Live tests (ignored by default):
  `cargo test -p meridian-provider-treasury --test live -- --ignored --nocapture`

  Results on 2026-10-05:
  - Latest curve is 2026-10-02 with 14 tenors: 1 Mo 4.04 … 10 Yr 5.28, 20 Yr 5.67, 30 Yr 5.63.
  - A request for 2025-07-04 (holiday) returned 2025-07-03.
  - 2004-01-02 has 10 tenors and no 30 Yr.
  - `UST:10 Yr` since 2025-01-01: 439 observations, latest 2026-10-02 = 5.28.
  - `UST:1.5 Mo` in 2025: first observation 2025-02-18.

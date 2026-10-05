# Fixture sources

All captured from the live public Treasury XML feed (no key) on 2026-10-05 with `curl -A "Meridian/0.1 (personal use)"`. Base URL: `https://home.treasury.gov/resource-center/data-chart-center/interest-rates/pages/xml`.

| File | Request | Edits |
|---|---|---|
| `par_yield_202610.xml` | `?data=daily_treasury_yield_curve&field_tdr_date_value_month=202610` | None (2 entries: 2026-10-01, 2026-10-02) |
| `par_yield_empty_month.xml` | `?data=daily_treasury_yield_curve&field_tdr_date_value_month=202612` | None (future month: feed with no entries) |
| `par_yield_200401_trimmed.xml` | `?data=daily_treasury_yield_curve&field_tdr_date_value_month=200401` | Trimmed to the first 3 of 20 entries. Entries are unmodified |
| `par_yield_2025_trimmed.xml` | `?data=daily_treasury_yield_curve&field_tdr_date_value=2025` | Trimmed to the first 32 of 249 entries (2025-01-02 to 2025-02-18, the first day with the 1.5-month tenor). Entries are unmodified |

Trimming kept the feed header and whole `<entry>` blocks only.

The OData null-element case in `feed.rs` tests is a hand-made inline snippet, labeled as such in the test.

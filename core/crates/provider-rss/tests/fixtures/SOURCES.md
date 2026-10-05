# Fixture sources

Captured 2026-10-05 with `curl` from public, keyless feeds. Each capture is trimmed: the
channel header and footer are kept verbatim, and only the listed items are kept, byte for
byte (only the whitespace between kept items changed).

| File | Origin URL | Captured (UTC) | User-Agent | Kept |
|---|---|---|---|---|
| `globenewswire_public_companies.xml` | https://www.globenewswire.com/RssFeed/orgclass/1/feedTitle/GlobeNewswire%20-%20News%20about%20Public%20Companies | 2026-10-05 18:41 | `MeridianBot/0.1 (personal use; RSS reader)` | 5 of 20 items (release ids 3374933, 3374916, 3374915, 3374912, 3374869) |
| `prnewswire_news_releases.xml` | https://www.prnewswire.com/rss/news-releases-list.rss | 2026-10-05 18:37 | `Meridian/0.1 (personal use)` | 5 of 20 items (positions 2, 3, 7, 10, 15) |
| `businesswire_all_news.xml` | https://feed.businesswire.com/mrss/home/?rss=G1QFDERJXkJcFVJYWQ== (the robots.txt-permitted `/mrss/` path) | 2026-10-05 18:55 | `MeridianBot/0.1 (personal use; RSS reader)` | 6 of 76 items (guids 20261005363276en, 20261005602857en, 20260930550483fr, 20261001519699en, 20261001581919en, 20260929663019en) |

The PR Newswire capture was made before the crate switched to its current User-Agent. The
feed serves the same content to either agent.

## Hand-made fixtures

These were written by hand for edge cases. They are not publisher data, and each file
says so in an XML comment.

| File | Covers |
|---|---|
| `handmade_atom.xml` | Atom: `rel="alternate"` vs `rel="self"` links, escaped-HTML title and summary, xhtml content, `updated` fallback, `category` term/label, an undated entry (skipped) |
| `handmade_cdata_html.xml` | RSS: CDATA HTML descriptions, HTML and numeric entities, links and tables, `content:encoded`, a description over the summary limit (the test fills `LONGTEXT_PLACEHOLDER` at runtime), guid without link, `javascript:` link, markup-only description |
| `handmade_bad_dates.xml` | RFC 2822 variants that are kept (no seconds, `UT`, wrong weekday, `+02:00`, ISO 8601) and items that are skipped (unknown zone abbreviation, garbage date, no date, no zone, blank title, no guid or link) |

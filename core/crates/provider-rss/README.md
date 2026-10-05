# meridian-provider-rss

Press releases from the big US newswires, read from their public RSS feeds. Provider id
`rss`. It serves the `News` capability for `PressReleases` and `Company` scopes. Items are
headlines and summaries only (`body` is always `None`), and each links to the publisher's
page.

It is keyless, and the default config needs no setup:

```rust
let provider = RssProvider::new(RssConfig::default())?;
```

`RssConfig { feeds: Vec<RssFeed { name, url }> }` replaces the list. `name` is the publisher
label shown as each item's `source`. Construction fails with `ProviderError::Parse`
(naming the feed) if the list is empty, a name is blank, or a URL is not `http(s)` with a
host.

## Feeds (verified 2026-10-05)

| Publisher | Feed | URL | Status with our UA | Depth |
|---|---|---|---|---|
| GlobeNewswire | News about Public Companies | `https://www.globenewswire.com/RssFeed/orgclass/1/feedTitle/GlobeNewswire%20-%20News%20about%20Public%20Companies` | 200, RSS 2.0 | last 20 releases |
| GlobeNewswire | Earnings Releases and Operating Results | `https://www.globenewswire.com/RssFeed/subjectcode/13-Earnings%20Releases%20And%20Operating%20Results/feedTitle/GlobeNewswire%20-%20Earnings%20Releases%20And%20Operating%20Results` | 200, RSS 2.0 | last 20 releases |
| PR Newswire | All News Releases | `https://www.prnewswire.com/rss/news-releases-list.rss` | 200, RSS 2.0 (see the redirect note below) | last 20 releases |
| Business Wire | All news, media-RSS path | `https://feed.businesswire.com/mrss/home/?rss=G1QFDERJXkJcFVJYWQ==` | 200, RSS 2.0, about 100 KB | releases with multimedia only: about 76 items over the last week |

Feed lists the URLs came from:
- GlobeNewswire: https://www.globenewswire.com/rss/list. It has hundreds of feeds by
  industry, subject, country, and exchange, in both `RssFeed/…` and `AtomFeed/…` forms.
- PR Newswire: https://www.prnewswire.com/rss/. It has the all-releases feed plus topic
  feeds such as `rss/energy-latest-news/energy-latest-news-list.rss`.
- Business Wire: its own help pages (`businesswire.com/help/feed-options`) return 403 to
  automated clients. The `/mrss/` URL above is listed as a Sitemap in
  `feed.businesswire.com/robots.txt`.
  - The same channel code under `/rss/home/` is the full all-news feed: about 4,000 items,
    4.7 MB, 7 days. Its robots.txt disallows that path, so we do not use it (see below).
  - Two other commonly cited `/rss/` codes were also broken on 2026-10-05:
    `?rss=G1QFDERJXkpaGVlYXg==` returns an "unavailable due to an error" channel, and
    `?rss=G1QFDERJXkJeEFtRXA==` returns an empty channel.

### robots.txt (checked 2026-10-05)

- **GlobeNewswire** (`www.globenewswire.com/robots.txt`): `User-agent: *` disallows
  `/SubscribeToRss/`, `/newsroom/rss/`, and the `JSWidget…` paths. `/RssFeed/` is allowed.
- **PR Newswire** (`www.prnewswire.com/robots.txt`): `/rss/` is not disallowed.
- **Business Wire** (`feed.businesswire.com/robots.txt`): `User-agent: *` has
  `Disallow: /rss/`, with exceptions only for two named agents. `/mrss/` is not disallowed,
  and `http://feed.businesswire.com/mrss/home/?rss=G1QFDERJXkJcFVJYWQ==` is listed as a
  Sitemap. The file also sets `Crawl-delay: 10` for many named bots (not for `*`). Our
  60 s per-feed cache keeps us well under that.

The robots.txt choice costs Business Wire coverage. `/mrss/` carries only releases with
multimedia attachments. On 2026-10-05 its 76 items were all present in the full `/rss/`
feed, and its newest item was about 3 hours older than the full feed's newest.

### User-Agent: differs from the other provider crates

This crate sends `MeridianBot/0.1 (personal use; RSS reader)`. Every other provider crate
sends `Meridian/0.1 (personal use)`, so this is a deliberate deviation from the project's
specified UA. It is the only line of `src/http.rs` that differs from the shared copy.

Why:
- GlobeNewswire's edge resets the HTTP/2 stream for any agent it does not recognise,
  including `Meridian/0.1 (personal use)`, `Mozilla/5.0`, and `reqwest/0.13`. It serves
  agents that identify as bots or as common tools.
- `MeridianBot/…` honestly identifies an automated feed reader. It does not imitate a
  browser or another tool, and robots.txt allows the paths we fetch.
- PR Newswire and Business Wire serve both agents.

### PR Newswire redirect fault

On 2026-10-05, about one request in three to the PR Newswire feed returned
`301 → …/news-releases-list.rss/` (trailing slash), and that URL returns 404. The next
request normally succeeds. Feed downloads therefore retry `404`, `5xx`, and network errors
up to 3 attempts in total, waiting 250 ms and then 500 ms. `429` is never retried here
because the router owns backoff.

## Publisher terms

All use is personal and non-commercial reading. Items are displayed, linked back to the
publisher, and never redistributed.

- **PR Newswire**: [Terms of Use](https://www.prnewswire.com/terms-of-use/), last updated
  2023-09-01.
  - Site materials are for personal, noncommercial use.
  - Reproduction, redistribution, framing, mirroring, and scraping are forbidden.
  - Robots, spiders, and data mining are forbidden.
  - Electronic redistribution and database storage are forbidden without written consent.
  - Using site content to train AI or ML systems is forbidden.
  - There is no RSS-specific clause. The feeds are published for RSS readers on
    https://www.prnewswire.com/rss/.
  - We fetch only the published feed, at most once a minute.
- **Business Wire**: [Terms of Use](https://www.businesswire.com/terms-of-use), effective
  2024-01-01. The page returns 403 to automated clients, so this summary is from a
  search-result excerpt (snippet).
  - Permitted uses include retrieving RSS feeds and reading releases.
  - Storing, aggregating, reproducing, or distributing site information is prohibited, as
    are commercial activities without written consent.
  - The aggregation clause is the main risk. Showing Business Wire items next to other
    wires in a personal reader is ordinary RSS-reader use, but anyone uncomfortable with it
    can drop the feed from `RssConfig`.
- **GlobeNewswire**: no reader-facing RSS terms were found.
  - The RSS list page states none.
  - `www.globenewswire.com/en/legal/terms-of-use` returns 404, and the `w.globenewswire.com`
    legal pages return 403 to automated clients.
  - The [Terms and Conditions](https://portal.notified.com/terms-conditions/en) are a
    customer (issuer) agreement with Intrado/Notified and do not address feed readers.
  - Reader terms are UNVERIFIED, so we apply the same personal-use limits.

How the terms map to `Capabilities`:
- `cache_policy: NoStore`, because of the Business Wire storing and aggregation clause and
  the PR Newswire database-storage clause. The provider keeps each feed response in memory
  for 60 s (below) and persists nothing.
- `attribution: None`. No publisher requires a credit line. Every item carries `source`
  (the publisher) and its `url`.
- `display_allowed: true`, `requires_credentials: false`.
- `ai_policy: Unreviewed`, which is treated as forbidden. PR Newswire forbids AI training.
  Sending items to the ASK model has not been reviewed against any of the three terms.
- `docs_url`: https://www.globenewswire.com/rss/list.

## Rate limit and caching

- `rate_limit: RateLimit::per_minute(30)` applies to `news()` calls, enforced by the
  router's token bucket. Each call reads every configured feed concurrently (`JoinSet`).
- Each feed's parsed response is reused for `FEED_CACHE_TTL` = 60 s, in memory only. Its
  mutex is held during the download, so concurrent callers share one fetch. Upstream load
  is therefore at most one download per feed per minute, plus up to two retries on
  transient errors.
- The provider does not depend on publisher caching, but it is compatible with it. Business
  Wire's `/mrss/` feed sends `Cache-Control: max-age=208`.
- Feeds are parsed on the blocking pool.

## Behaviour

- **Scopes**:
  - `PressReleases` returns all items.
  - `Company` keeps items whose tickers include one of the query's US equity symbols.
    Eligible keys have sector `Equity` and either no exchange or one of
    `US UN UW UQ UR UA UP UF UV`. `BRK-B`, `BRK/B`, and `BRK.B` compare equal. If no key is
    eligible, the result is empty and nothing is fetched.
  - `Top` and `Market` return `Unsupported { capability: News }`.
- **Filters**:
  - `text` is a case-insensitive substring match on the headline and summary.
  - `from` is inclusive and `to` is exclusive, both on `published_at`.
- **Ordering and limits**: newest first, with ties broken by id. Items are de-duplicated by
  URL, then by (headline, source), then truncated to `limit`. `next` is always `None`.
  - De-duplicating by headline collapses releases that share a title. For example, the full
    Business Wire `/rss/` feed (not used) carried 325 "Net Asset Value(s)" notices in one
    capture; only the newest would be kept.
- **Errors**:
  - If some feeds fail, the items of the others are returned and the failures are logged at
    `warn`.
  - If all feeds fail, the first configured feed's error is returned, with the feed name
    prefixed to the message.
  - A body that is not RSS 1.0/2.0 or Atom (for example an HTML block page) is
    `Parse { context }`.
  - If XML breaks after complete items, those items are kept and the break is logged.
- **Item mapping** (RSS 2.0, RSS 1.0/RDF, Atom; quick-xml streaming reader; elements matched
  by prefix as written, e.g. `dc:subject`):
  - `id`: `"{publisher}:{guid | atom:id | link}"`.
  - `headline`: the title, with HTML stripped.
  - `summary`: description, then summary, then `content:encoded`/content. Converted from
    HTML to text with html2text, whitespace collapsed, cut to 1,000 chars. `None` if empty.
  - `url`: the RSS link or the Atom `rel="alternate"` href, kept only for `http(s)`.
  - `published_at`: the feed's date, from `pubDate`, `dc:date`, `published`, or `updated`.
    `received_at` and `provenance.as_of` are the fetch time.
  - `provenance`: `rss`, not synthetic, `RealTime`, `Aggregated`. `source_ref` is the item
    link, or the feed URL if the item has none. Attribution is `None`.
  - `topics`: plain `<category>` (and Atom term/label), `dc:subject`, `dc:keyword`,
    `prn:subject`, and `prn:industry`. Pure numbers and codes of up to 3 upper-case letters
    (PR Newswire's `FIN`, `PDT`) are dropped.
  - Items with no title, no parseable date, or no id are skipped and logged at `debug`.
- **Dates**:
  - RFC 2822 is parsed with chrono, with or without seconds, with `GMT`, `UT`, `UTC`, `Z`,
    or US zone names, `±hhmm` or `±hh:mm` offsets, and a wrong or long weekday tolerated.
  - RFC 3339 / ISO 8601 must include an offset.
  - Dates without a zone, and other zone abbreviations (e.g. `CEST`), are rejected rather
    than guessed.

## Ticker extraction

Sources are the headline, the full description text, and categories.

- **Free text**: a recognised exchange label, a colon, then one or more symbols separated by
  `,`, `&`, `/`, or `and`. Labels match case-insensitively; symbols are case-sensitive
  upper-case. Examples:
  `(NASDAQ: AAPL)`, `(NYSE: IBM)`, `NYSE:IBM`, `(NYSE American: XYZ)`, `(Nasdaq GS: ABC)`,
  `(NasdaqGS: NAVN)`, `(NASDAQ Global Select Market: PGC)`, `(NASDAQ: AAPL, AAPLW)`,
  `(NYSE: BRK.A, BRK.B)`, `(OTCQB: ABCD)`, `(NASDAQ: ABC; TSX: ABC)`,
  `(TSX: DSV, OTCQX: DSVSF)`. A token followed by a colon starts the next listing.
- **GlobeNewswire categories**: `<category domain=".../rss/stock">Nasdaq:PGC</category>`
  is used directly. ISIN categories are ignored.
- **Symbols**: 1 to 6 upper-case letters plus an optional `.X`/`.XX` class suffix, with
  `-` and `/` normalised to `.`. Anything else is dropped, such as `NHY01`, `LBANK CBI 22`,
  or numeric codes.
- **US venues**: NYSE, NYSE American/MKT/Amex/Arca/Texas, AMEX, Nasdaq and its tiers,
  Cboe/BZX, BATS, and OTC tiers (OTC, Other OTC, OTC Markets, OTCQB, OTCQX, OTCMKTS,
  OTC Pink/OTCPK, OTCID, OTCBB, OTC US). These give the plain symbol, e.g. `AAPL`.
- **Other venues** give `SYMBOL:VENUE`, e.g. `DSV:TSX`, `MMY:TSXV`, `ABC:LSE`,
  `ALXYZ:EURONEXT`, `BVI:PARIS`. In text these are TSX, TSXV/TSX-V/TSX Venture, CSE,
  NEO/Cboe Canada, LSE/LN, AIM, ASX, SIX, XETRA, and Euronext (with or without Growth and a
  city). Category labels not in the table keep their own name, e.g. `NHY:OSLO`. Company
  filtering never matches these.
- **Not extracted** (by design, to avoid guessing a venue or matching prose):
  - bare parentheses like `(PZZA)` or `(the "Company")`;
  - cashtags like `$NAGE`, which Business Wire puts in some datelines;
  - "Ticker: ABC";
  - labels not in the table, such as "Nasdaq Stockholm" or non-English exchange names;
  - prose like "Nasdaq Composite".

## Limitations

- Headlines and summaries only. There are no bodies, and no history beyond what each feed
  currently lists.
- PR Newswire descriptions are cut by the publisher at about 250 characters, so its items
  rarely carry an exchange label. In the 2026-10-05 live run, 0 of 20 PR Newswire items had
  tickers, compared with 13 of 76 Business Wire items and 39 of 40 GlobeNewswire items.
- Business Wire coverage is partial. To respect its robots.txt we read the `/mrss/` feed,
  which carries only releases with multimedia (about 76 a week). The full `/rss/` feed
  (about 4,000 a week) is disallowed. Company news for issuers that wire only through Business
  Wire without multimedia will be missing.
- `Top` and `Market` news are not offered. Press releases are not market news.
- **Policy**: the incumbent terminal vendor's own feeds are never ingested. This crate's
  defaults contain only the three wires above, and any custom feed must not point at that
  vendor's domains. By the project's naming rule, the vendor is not named in code, so there
  is no name-based guard. The policy is enforced by review and by this default list.

## Tests

```
# Unit tests (fixtures in tests/fixtures, see SOURCES.md; local HTTP server for news())
cargo test --manifest-path core/Cargo.toml -p meridian-provider-rss

# Live: fetches every default feed (asserts ≥1 item with a headline dated within 30 days),
# then a PressReleases call and a Company round trip
cargo test --manifest-path core/Cargo.toml -p meridian-provider-rss -- --ignored --nocapture
```

Live results on 2026-10-05:

| Feed | Items | Items with tickers |
|---|---|---|
| GlobeNewswire public companies | 20 | 20 |
| GlobeNewswire earnings | 20 | 19 |
| PR Newswire | 20 | 0 |
| Business Wire (`/mrss/`) | 76 | 13 |

`news(PressReleases)` merged 136 items: 40 GlobeNewswire, 20 PR Newswire, and 76 Business
Wire. A company query for `PGC` found its release.

# Research: filings, news, earnings-call transcripts

Researched 2026-10-05 from official pages. "(snippet)" means only a search-result excerpt of the official page was available because the page blocked fetching. "(3P)" means a third-party source. UNVERIFIED means not confirmed.

Blocked fetches: finnhub.io pricing (JS-rendered), financialmodelingprep.com, benzinga.com/apis, reddithelp.com, cnbc.com, marketwatch (403s).

## 1. SEC EDGAR (free, official)

| Endpoint | Returns | Limits / latency | Source |
|---|---|---|---|
| `data.sec.gov/submissions/CIK##########.json` | Filer metadata + recent filing history (≥1 yr or 1,000 filings; older in paged files) | No key. "less than a second" processing delay; longer at peaks. No CORS | https://www.sec.gov/search-filings/edgar-application-programming-interfaces |
| `…/api/xbrl/companyconcept/CIK…/us-gaap/{Concept}.json` | All XBRL facts, one company × one concept | XBRL "under a minute" | same |
| `…/api/xbrl/companyfacts/CIK….json` | All XBRL facts for a company | same | same |
| `…/api/xbrl/frames/us-gaap/{Concept}/{Unit}/{Period}.json` | One fact per filer per period (e.g. CY2019Q1I) | same | same |
| Bulk `submissions.zip`, `companyfacts.zip` | Full dumps | Republished nightly ~3:00 a.m. ET | same |
| `/Archives/edgar/daily-index/`, `/full-index/`, `/Feed/`, `/Oldloads/` | `.idx` indexes, daily tarballs | Built nightly from ~10 p.m. ET | https://www.sec.gov/search-filings/edgar-search-assistance/accessing-edgar-data |
| Latest filings Atom: `/cgi-bin/browse-edgar?action=getcurrent&…&output=atom` | Newest filings, filterable by form/CIK/owner | 10–100 entries per page; cadence undocumented. **Best free near-real-time source** | https://www.sec.gov/about/rss-feeds |
| XBRL RSS (`usgaap.rss.xml`, `xbrl-inline.rss.xml`, `xbrlrss.all.xml`) | XBRL filings | Every 10 min, Mon–Fri 6 a.m.–10 p.m. ET | https://www.sec.gov/structureddata/rss-feeds |
| Full-text search `efts.sec.gov/LATEST/search-index?q=…` | JSON hits across filings since 2001 | **Undocumented.** 3P reports a 10,000-hit cap, 403 without User-Agent | https://www.sec.gov/edgar/search/efts-faq.html ; (3P) dev.to article |
| `company_tickers.json`, `company_tickers_exchange.json` | Ticker ↔ CIK ↔ name | Updated periodically | https://www.sec.gov/file/company-tickers |
| PDS push feed | All filings, pushed | Paid agreement, own server; fee UNVERIFIED | https://www.sec.gov/files/edgar-pds-new-subscriber-document.pdf |

**Fair access:** 10 requests/second per user across all machines (https://www.sec.gov/about/developer-resources). A declared User-Agent is required, in the form `AppName contact@domain`. Filings are accepted Mon–Fri 6 a.m.–10 p.m. ET.

**sec-api.io** (https://sec-api.io/pricing):
- **Free:** 100 calls total, not per month.
- **Personal:** $49/mo billed yearly; ~$55 billed monthly (snippet). Query API at 20/s, extractors, XBRL converter, Stream API. No full-text search. License: "Personal".
- **Business Internal:** $199/mo yearly, ~$239 monthly. Adds full-text search.
- **Stream:** `wss://stream.sec-api.io`, "< 300 milliseconds" (https://sec-api.io/docs/stream-api). The fetches disagreed on whether Stream is included in Personal; confirm before buying.

## 2. News

| Provider | Price/mo | Latency | Full text | Streaming | Limits | History | Terms | Source |
|---|---|---|---|---|---|---|---|---|
| Benzinga via Massive | $99 add-on | Real-time | **Yes** (`body`) | REST only | — | Since 2009-04-27 | "Individual use only" | https://massive.com/pricing ; https://massive.com/docs/rest/partners/benzinga/news |
| Massive built-in Ticker News | Included (Basic $0) | **Hourly** | No | No | Basic 5/min | Basic 2y; paid since 2016 | Individual use | https://massive.com/docs/rest/stocks/news |
| Benzinga direct | Sales; free Basic tier (snippet) | Real-time | Tier-dependent | WS `wss://api.benzinga.com/api/v1/news/stream` | 100/page | Deep | License agreement | https://docs.benzinga.com/ |
| Finnhub | Free; paid UNVERIFIED | UNVERIFIED | No | 1 WS connection per key | 30/s hard cap; free 60/min (snippet) | Free: 1y company news (NA only) | Personal only; **delete data when subscription ends** | https://finnhub.io/terms-of-service |
| Tiingo News | Power $30 (individual) | Real-time (snippet) | No (long description) | No | 10k/hr, 100k/day | 3 months | Personal; no display/share | https://www.tiingo.com/about/pricing |
| Alpha Vantage NEWS_SENTIMENT | Free 25/day; $49.99–$249.99 | UNVERIFIED | No | No | — | UNVERIFIED | Personal, non-commercial | https://www.alphavantage.co/premium/ |
| Marketaux | Free 100/day; $29–$199 | — | No (snippet + link) | No | Daily caps | UNVERIFIED | Personal | https://www.marketaux.com/pricing |
| NewsAPI.org | Dev $0 (24h delay, **not allowed in production**); $449+ | — | No | No | — | — | Dev = testing only | https://newsapi.org/pricing |
| GDELT DOC 2.0 | Free | 15 min | No | No | 250/call; ~1 req/5s (3P) | Rolling 3 months | Free use, attribution | https://blog.gdeltproject.org/gdelt-doc-2-0-api-debuts/ |
| Stocktwits | — | — | — | — | — | — | **Not accepting registrations** | https://api.stocktwits.com/developers |
| Reddit | Free non-commercial | — | — | — | 100 QPM | — | Requires approval first (snippet) | reddithelp.com |
| GlobeNewswire / PR Newswire / Business Wire RSS | Free | Live (items dated 2026-10-05) | No (summary) | RSS | 20–50 items | — | Terms UNVERIFIED | https://www.globenewswire.com/rss/list ; https://www.prnewswire.com/rss/ |
| Publisher RSS (WSJ Markets, Yahoo per-ticker, Seeking Alpha per-ticker, Investing.com) | Free | Live | No | RSS | — | — | Personal use | feed URLs confirmed live 2026-10-04/05 |

## 3. Earnings-call transcripts

| Provider | Price/mo | Latency | Content | History | Terms | Source |
|---|---|---|---|---|---|---|
| EarningsCall.biz | Starter $60; Premium $69 (speakers, timestamps); Ultimate $129 (audio); Ultimate+ $155 (real-time notifications) | "~50% within 15 minutes" | Text, audio from Ultimate | Most companies since 2020 | Free AAPL/MSFT demo | https://earningscall.biz/api-pricing |
| API Ninjas | Developer $39–59; Business $99–149; Pro $199–299 | UNVERIFIED | Text; speaker split from Business | Since 2005 (Developer: last 5y) | Free = evaluation only | https://api-ninjas.com/pricing |
| FMP | Ultimate only (~$99–149, sources conflict) | UNVERIFIED | Text | "Full historical" | Personal non-business; display needs separate agreement | (snippet) |
| Alpha Vantage | Free 25/day or premium (UNVERIFIED) | UNVERIFIED | Text + LLM sentiment | Since 2010Q1 (snippet) | Personal | (snippet) |
| Finnhub | Sales | UNVERIFIED | Text + audio, live endpoint | Since 2000 | Personal | (snippet) |
| Quartr | Sales only | Live, near-zero | Audio, transcripts, slides | UNVERIFIED | Enterprise | https://quartr.com/products/quartr-api |
| Seeking Alpha, Motley Fool, Koyfin | No API. Scrapers violate ToS (Koyfin: data providers forbid API access) | | | | | |

## 4. Gotchas

- EDGAR: User-Agent required; 10 req/s is global, so throttle prefetch too; `efts` is undocumented; poll the getcurrent Atom feed for speed.
- Finnhub requires deleting cached data when a subscription ends. Storage must support purge-by-provider.
- Individual licenses (Massive, Tiingo, Marketaux, Finnhub) forbid displaying data to others.
- Don't plan on social sources (Stocktwits closed, Reddit gated) or scraper wrappers.
- RSS carries headlines and summaries only. Several publishers block non-browser fetchers.
- **Project rule:** we will not ingest the incumbent terminal vendor's own RSS feed, to stay clear of its brand and terms.

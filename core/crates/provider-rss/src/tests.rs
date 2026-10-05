//! Unit tests: parsing (captured and hand-made fixtures), dates, HTML
//! stripping, query selection, and `news()` against a local HTTP server.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, Utc};
use meridian_provider::{
    AiPolicy, CachePolicy, Capability, NewsQuery, NewsScope, Provider, ProviderError,
};
use meridian_types::{
    AssetClass, DataDelay, FeedSource, FixedClock, MarketSector, NewsItem, Provenance, ProviderId,
    SecurityKey, UnixNanos, datetime_to_nanos,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::parse::{
    FeedKind, SUMMARY_MAX_CHARS, parse_date, parse_feed, parse_items, to_plain_text, truncate_chars,
};
use crate::{RssConfig, RssFeed, RssProvider, company_symbols, merge_feed_results, select_items};

const GNW: &str = include_str!("../tests/fixtures/globenewswire_public_companies.xml");
const PRN: &str = include_str!("../tests/fixtures/prnewswire_news_releases.xml");
const BW: &str = include_str!("../tests/fixtures/businesswire_all_news.xml");
const ATOM: &str = include_str!("../tests/fixtures/handmade_atom.xml");
const CDATA: &str = include_str!("../tests/fixtures/handmade_cdata_html.xml");
const DATES: &str = include_str!("../tests/fixtures/handmade_bad_dates.xml");

const FETCHED: UnixNanos = 1_791_225_600_000_000_000; // 2026-10-05T18:40:00Z

fn ts(rfc3339: &str) -> UnixNanos {
    datetime_to_nanos(
        DateTime::parse_from_rfc3339(rfc3339)
            .unwrap()
            .with_timezone(&Utc),
    )
}

fn feed(name: &str) -> RssFeed {
    RssFeed::new(name, "https://example.com/feed.xml")
}

fn items(xml: &str, name: &str) -> Vec<NewsItem> {
    parse_items(xml, &feed(name), FETCHED).unwrap()
}

fn find<'a>(items: &'a [NewsItem], headline_prefix: &str) -> &'a NewsItem {
    items
        .iter()
        .find(|i| i.headline.starts_with(headline_prefix))
        .unwrap_or_else(|| panic!("no item {headline_prefix:?}"))
}

// ---------------------------------------------------------------- parsing

#[test]
fn parses_globenewswire_fixture() {
    let items = items(GNW, "GlobeNewswire");
    assert_eq!(items.len(), 5);

    let pgc = find(&items, "Peapack Private Appoints");
    assert_eq!(pgc.source, "GlobeNewswire");
    assert_eq!(pgc.published_at, ts("2026-10-05T18:15:00Z"));
    assert_eq!(pgc.received_at, FETCHED);
    assert_eq!(pgc.tickers, vec!["PGC"]);
    assert_eq!(pgc.topics, vec!["Company Announcement"]);
    assert_eq!(pgc.body, None);
    let url = "https://www.globenewswire.com/news-release/2026/10/05/3374933/0/en/peapack-private-appoints-ana-parra-as-senior-managing-director-group-director.html";
    assert_eq!(pgc.url.as_deref(), Some(url));
    assert_eq!(pgc.id, format!("GlobeNewswire:{url}"));
    let summary = pgc.summary.as_deref().unwrap();
    assert!(
        summary
            .starts_with("BEDMINSTER, N.J., Oct. 05, 2026 (GLOBE NEWSWIRE) -- Peapack-Gladstone"),
        "{summary}"
    );
    assert!(
        summary.contains("Peapack Private Bank & Trust"),
        "{summary}"
    );
    assert!(!summary.contains('<'));
    assert_eq!(
        pgc.provenance,
        Provenance {
            provider: ProviderId::new("rss"),
            synthetic: false,
            delay: DataDelay::RealTime,
            source: FeedSource::Aggregated,
            as_of: FETCHED,
            source_ref: Some(url.to_owned()),
            attribution: None,
        }
    );

    let dsv = find(&items, "Discovery Announces Receipt");
    assert_eq!(dsv.tickers, vec!["DSV:TSX", "DSVSF"]);
    assert_eq!(
        dsv.topics,
        vec![
            "Product / Services Announcement",
            "Discovery Mining Ltd.",
            "Cordero",
            "permit"
        ]
    );

    let worx = find(&items, "SCWorx Corp.");
    assert_eq!(worx.tickers, vec!["WORX", "WORXD"]);

    // Category-only tickers; digit-bearing bond symbols are dropped.
    let nhy = find(&items, "Norsk Hydro");
    assert_eq!(nhy.tickers, vec!["IRSH:IRISH", "NHYDY", "NHY:OSLO"]);
    assert_eq!(
        nhy.summary.as_deref(),
        Some(
            "Hydro provides an update on the natural gas supply situation at the Alunorte alumina refinery."
        )
    );

    let mbws = find(&items, "Marie Brizard Wine & Spirits Monthly");
    assert_eq!(mbws.tickers, vec!["MBWS:PARIS"]);
    assert_eq!(
        mbws.topics,
        vec!["Law & Legal Issues", "European Regulatory News"]
    );
}

#[test]
fn parses_prnewswire_fixture() {
    let items = items(PRN, "PR Newswire");
    assert_eq!(items.len(), 5);
    let first = find(&items, "Nasdaq Hits Fresh Highs");
    assert_eq!(first.published_at, ts("2026-10-05T18:28:00Z"));
    assert!(
        first
            .summary
            .as_deref()
            .unwrap()
            .starts_with("NEW YORK, Oct. 5, 2026 /PRNewswire/ -- Stock Preachers")
    );
    // Readable topics kept; three-letter subject/industry codes dropped.
    assert_eq!(
        first.topics,
        vec![
            "Banking & Financial Services",
            "Investments Opinions",
            "Market Research Reports"
        ]
    );
    // "Nasdaq Composite" is not a listing.
    assert!(first.tickers.is_empty());
    // A bare "(PZZA)" without an exchange label is not extracted.
    let pzza = find(&items, "Papa John's International, Inc. (PZZA)");
    assert!(pzza.tickers.is_empty());
    assert!(pzza.topics.is_empty());
    assert!(items.iter().all(|i| {
        i.url
            .as_deref()
            .is_some_and(|u| u.starts_with("https://www.prnewswire.com/news-releases/"))
    }));
}

#[test]
fn parses_businesswire_fixture() {
    let items = items(BW, "Business Wire");
    assert_eq!(items.len(), 6);
    let schw = find(&items, "Schwab Initiates Secondary Listing");
    assert_eq!(schw.id, "Business Wire:20261001581919en");
    assert_eq!(schw.published_at, ts("2026-10-01T12:30:00Z"));
    assert_eq!(schw.tickers, vec!["SCHW"]);
    assert!(schw.topics.is_empty());
    assert!(
        schw.url
            .as_deref()
            .unwrap()
            .starts_with("http://www.businesswire.com/news/home/20261001581919/en/")
    );
    assert!(schw.summary.as_deref().unwrap().starts_with(
        "WESTLAKE, Texas--(BUSINESS WIRE)--The Charles Schwab Corporation (NYSE: SCHW)"
    ));

    // Double-escaped entities in the feed are decoded once more by the HTML pass.
    let az = find(&items, "AstraZeneca opens");
    assert_eq!(
        az.headline,
        "AstraZeneca opens new global strategic R&D center in Kendall Square, Cambridge, Massachusetts"
    );
    assert!(az.tickers.is_empty());
    let nage = find(&items, "Niagen Bioscience");
    assert!(
        nage.headline.contains("Behind \"Don't Get Old."),
        "{}",
        nage.headline
    );
    // Cashtag `$NAGE` is ignored; the exchange citation is used.
    assert_eq!(nage.tickers, vec!["NAGE"]);
    let tangent = find(&items, "Tangent Robotics");
    assert!(tangent.headline.contains("d'euros"), "{}", tangent.headline);
    assert_eq!(tangent.id, "Business Wire:20260930550483fr");

    // `(NYSE: MX, “Magnachip”)`: the quoted short name is not a symbol.
    assert_eq!(find(&items, "Magnachip Launches").tickers, vec!["MX"]);
    // Non-US listing.
    assert_eq!(find(&items, "EADV 2026").tickers, vec!["GALD:SIX"]);
}

#[test]
fn parses_handmade_atom() {
    let raw = parse_feed(ATOM, "atom").unwrap();
    assert_eq!(raw.kind, FeedKind::Atom);
    assert_eq!(raw.entries.len(), 3);

    let items = items(ATOM, "Example");
    assert_eq!(items.len(), 2, "undated entry is skipped");
    let acme = &items[0];
    assert_eq!(acme.headline, "Acme Reports Third Quarter Results");
    assert_eq!(
        acme.url.as_deref(),
        Some("https://example.com/news/acme-q3"),
        "rel=alternate wins over rel=self"
    );
    assert_eq!(acme.id, "Example:urn:example:acme-q3");
    assert_eq!(
        acme.published_at,
        ts("2026-10-05T12:30:00Z"),
        "published preferred over updated"
    );
    assert_eq!(
        acme.summary.as_deref(),
        Some("Acme Corp. (NYSE: ACME) reported revenue of $1.2 billion.")
    );
    assert_eq!(acme.tickers, vec!["ACME"]);
    assert_eq!(acme.topics, vec!["Earnings"]);

    let beta = &items[1];
    assert_eq!(
        beta.url.as_deref(),
        Some("https://example.com/news/beta-dividend")
    );
    assert_eq!(
        beta.published_at,
        ts("2026-10-04T20:15:00Z"),
        "falls back to updated"
    );
    assert_eq!(
        beta.summary.as_deref(),
        Some("Beta Inc. (Nasdaq: BETA) declared a dividend.")
    );
    assert_eq!(beta.tickers, vec!["BETA"]);
}

#[test]
fn parses_handmade_cdata_html() {
    let long = "word ".repeat(400);
    let xml = CDATA.replace("LONGTEXT_PLACEHOLDER", &long);
    let items = items(&xml, "Hand");
    assert_eq!(items.len(), 4);

    let gamma = find(&items, "Gamma");
    assert_eq!(gamma.headline, "Gamma & Co. Announces Buyback");
    assert_eq!(
        gamma.summary.as_deref(),
        Some(
            "NEW YORK – Gamma & Co. (NYSE: GAMA) today said its board approved a $500 million buyback. Shares 10m"
        )
    );
    assert_eq!(gamma.tickers, vec!["GAMA"]);
    assert_eq!(gamma.topics, vec!["Share Buybacks", "Corporate Action"]);
    assert_eq!(gamma.published_at, ts("2026-10-05T18:00:00Z"));

    let delta = find(&items, "Delta Ltd.");
    assert_eq!(
        delta.headline,
        "Delta Ltd. Files Annual Report — Plain Text"
    );
    assert_eq!(delta.url, None);
    assert_eq!(delta.id, "Hand:delta-annual-2026");
    assert_eq!(
        delta.provenance.source_ref.as_deref(),
        Some("https://example.com/feed.xml")
    );
    assert_eq!(
        delta.summary.as_deref(),
        Some("Delta Ltd. (OTCQX: DLTAF) filed its annual report. Revenue rose 4% year over year.")
    );
    assert_eq!(delta.tickers, vec!["DLTAF"]);

    let eps = find(&items, "Epsilon");
    let summary = eps.summary.as_deref().unwrap();
    assert!(summary.chars().count() <= SUMMARY_MAX_CHARS);
    assert!(summary.starts_with("word word"));
    assert_eq!(
        eps.id, "Hand:https://example.com/news/epsilon-long",
        "link used when there is no guid"
    );

    let zeta = find(&items, "Zeta");
    assert_eq!(zeta.url, None, "non-http links are dropped");
    assert_eq!(zeta.summary, None, "markup-only description has no text");
    assert_eq!(zeta.id, "Hand:zeta-1");
}

#[test]
fn skips_items_with_bad_dates_titles_or_ids() {
    let items = items(DATES, "Dates");
    let titles: Vec<&str> = items.iter().map(|i| i.headline.as_str()).collect();
    assert_eq!(
        titles,
        vec![
            "KEEP no seconds GMT",
            "KEEP UT zone single-digit day",
            "KEEP wrong weekday",
            "KEEP colon offset",
            "KEEP ISO date in pubDate",
        ]
    );
    assert_eq!(items[0].published_at, ts("2026-10-05T18:15:00Z"));
    assert_eq!(items[1].published_at, ts("2026-10-05T18:16:00Z"));
    assert_eq!(items[2].published_at, ts("2026-10-05T10:00:00Z"));
    assert_eq!(items[3].published_at, ts("2026-10-05T10:00:00Z"));
    assert_eq!(items[4].published_at, ts("2026-10-05T10:00:00Z"));
}

#[test]
fn date_variants() {
    let ok = [
        ("Mon, 05 Oct 2026 18:15:00 GMT", "2026-10-05T18:15:00Z"),
        ("Mon, 05 Oct 2026 18:15 GMT", "2026-10-05T18:15:00Z"),
        ("Mon, 5 Oct 2026 18:16:00 UT", "2026-10-05T18:16:00Z"),
        ("Mon, 05 Oct 2026 14:00:00 -0400", "2026-10-05T18:00:00Z"),
        ("Mon, 05 Oct 2026 14:00:00 EDT", "2026-10-05T18:00:00Z"),
        ("Mon, 05 Oct 2026 18:00:00 +0000", "2026-10-05T18:00:00Z"),
        ("Mon, 05 Oct 2026 18:00:00 UTC", "2026-10-05T18:00:00Z"),
        ("Mon, 05 Oct 2026 20:00:00 +02:00", "2026-10-05T18:00:00Z"),
        ("05 Oct 2026 18:00:00 GMT", "2026-10-05T18:00:00Z"),
        ("Monday, 05 Oct 2026 18:00:00 GMT", "2026-10-05T18:00:00Z"),
        (
            "  Mon,  05 Oct 2026\n 18:00:00 GMT ",
            "2026-10-05T18:00:00Z",
        ),
        ("2026-10-05T14:00:00-04:00", "2026-10-05T18:00:00Z"),
        ("2026-10-05T18:00:00.250Z", "2026-10-05T18:00:00.250Z"),
        ("2026-10-05T18:00:00+0000", "2026-10-05T18:00:00Z"),
    ];
    for (input, want) in ok {
        assert_eq!(parse_date(input), Some(ts(want)), "{input:?}");
    }
    for bad in [
        "",
        "   ",
        "yesterday",
        "Mon, 05 Oct 2026 18:00:00 CEST",
        "2026-10-05",
        "2026-10-05T18:00:00",
        "Mon, 32 Oct 2026 18:00:00 GMT",
    ] {
        assert_eq!(parse_date(bad), None, "{bad:?}");
    }
}

#[test]
fn html_to_text() {
    assert_eq!(
        to_plain_text("plain   text\n here").as_deref(),
        Some("plain text here")
    );
    assert_eq!(
        to_plain_text("<p>One</p><p>Two&nbsp;three</p>").as_deref(),
        Some("One Two three")
    );
    assert_eq!(
        to_plain_text("A &amp; B &lt;tag&gt;").as_deref(),
        Some("A & B <tag>")
    );
    assert_eq!(
        to_plain_text("<a href=\"https://x\">link</a> text").as_deref(),
        Some("link text")
    );
    assert_eq!(
        to_plain_text("<ul><li>a</li><li>b</li></ul>").as_deref(),
        Some("a b")
    );
    assert_eq!(to_plain_text("AT&T Inc.").as_deref(), Some("AT&T Inc."));
    assert_eq!(to_plain_text("<img src=\"x.png\">").as_deref(), Some(""));
}

#[test]
fn truncates_at_char_boundary() {
    assert_eq!(truncate_chars("héllo wörld", 5), "héllo");
    assert_eq!(truncate_chars("abc", 10), "abc");
    assert_eq!(truncate_chars("ab cd", 3), "ab");
}

#[test]
fn rejects_non_feeds() {
    for (doc, what) in [
        (
            "<!DOCTYPE html><html><body>Access Denied</body></html>",
            "html page",
        ),
        ("", "empty body"),
        ("not xml at all", "plain text"),
        (
            "<rss><channel><item><title>x</titl",
            "broken before any item",
        ),
    ] {
        let err = parse_items(doc, &feed("X"), FETCHED).unwrap_err();
        assert!(
            matches!(err, ProviderError::Parse { .. }),
            "{what}: {err:?}"
        );
    }
}

#[test]
fn keeps_complete_items_before_xml_breakage() {
    let doc = "<rss><channel><item><title>Good</title><link>https://e.com/1</link>\
               <pubDate>Mon, 05 Oct 2026 10:00:00 GMT</pubDate></item><item><title>Bad &</titl";
    let items = parse_items(doc, &feed("X"), FETCHED).unwrap();
    assert_eq!(
        items
            .iter()
            .map(|i| i.headline.as_str())
            .collect::<Vec<_>>(),
        vec!["Good"]
    );
}

#[test]
fn empty_channel_is_empty_not_error() {
    let doc = r#"<?xml version="1.0"?><rss version="2.0"><channel><title>BW</title>
        <description>The channel you requested is unavailable.</description></channel></rss>"#;
    assert!(parse_items(doc, &feed("X"), FETCHED).unwrap().is_empty());
}

#[test]
fn parses_rss1_rdf() {
    let doc = r#"<?xml version="1.0"?>
        <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns="http://purl.org/rss/1.0/"
                 xmlns:dc="http://purl.org/dc/elements/1.1/">
          <channel rdf:about="https://e.com/"><title>RDF</title></channel>
          <item rdf:about="https://e.com/1"><title>RDF item (NYSE: RDF)</title><link>https://e.com/1</link>
            <dc:date>2026-10-05T10:00:00Z</dc:date></item>
        </rdf:RDF>"#;
    let raw = parse_feed(doc, "rdf").unwrap();
    assert_eq!(raw.kind, FeedKind::Rdf);
    let items = parse_items(doc, &feed("X"), FETCHED).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].tickers, vec!["RDF"]);
    assert_eq!(items[0].published_at, ts("2026-10-05T10:00:00Z"));
}

// ---------------------------------------------------------------- selection

fn item(
    id: &str,
    source: &str,
    headline: &str,
    url: Option<&str>,
    published: &str,
    tickers: &[&str],
) -> NewsItem {
    NewsItem {
        id: id.into(),
        source: source.into(),
        headline: headline.into(),
        summary: Some(format!("summary of {headline}")),
        body: None,
        url: url.map(Into::into),
        published_at: ts(published),
        received_at: FETCHED,
        tickers: tickers.iter().map(|s| (*s).to_owned()).collect(),
        topics: vec![],
        provenance: Provenance {
            provider: ProviderId::new("rss"),
            synthetic: false,
            delay: DataDelay::RealTime,
            source: FeedSource::Aggregated,
            as_of: FETCHED,
            source_ref: None,
            attribution: None,
        },
    }
}

fn query(scope: NewsScope, keys: Vec<SecurityKey>) -> NewsQuery {
    NewsQuery {
        scope,
        keys,
        text: None,
        from: None,
        to: None,
        limit: 100,
    }
}

fn sample() -> Vec<NewsItem> {
    vec![
        item(
            "a",
            "GlobeNewswire",
            "Apple update",
            Some("https://e.com/a"),
            "2026-10-05T10:00:00Z",
            &["AAPL"],
        ),
        item(
            "b",
            "Business Wire",
            "Berkshire class B",
            Some("https://e.com/b"),
            "2026-10-05T12:00:00Z",
            &["BRK.B"],
        ),
        item(
            "c",
            "PR Newswire",
            "Canadian listing",
            Some("https://e.com/c"),
            "2026-10-05T11:00:00Z",
            &["ABC:TSX"],
        ),
        item(
            "d",
            "PR Newswire",
            "No tickers here",
            Some("https://e.com/d"),
            "2026-10-04T09:00:00Z",
            &[],
        ),
    ]
}

#[test]
fn company_scope_matches_us_symbols_only() {
    let keys = vec![
        SecurityKey::equity("aapl"),
        SecurityKey::equity("BRK-B"),
        SecurityKey::equity("ABC"),
    ];
    let symbols = company_symbols(&keys);
    assert_eq!(
        symbols,
        HashSet::from(["AAPL".to_owned(), "BRK.B".to_owned(), "ABC".to_owned()])
    );
    let q = query(NewsScope::Company, keys);
    let got = select_items(sample(), &q, Some(&symbols));
    // Newest first; "ABC:TSX" does not match the US key "ABC".
    assert_eq!(
        got.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
        vec!["b", "a"]
    );

    // Non-equity and non-US keys can't be matched.
    let other = vec![
        SecurityKey::index("SPX"),
        SecurityKey::currency("EURUSD"),
        SecurityKey::new("RY", Some("CN"), MarketSector::Equity),
    ];
    assert!(company_symbols(&other).is_empty());
    assert_eq!(
        company_symbols(&[SecurityKey::new("IBM", Some("UN"), MarketSector::Equity)]).len(),
        1
    );
    assert_eq!(
        company_symbols(&[SecurityKey::new("IBM", None, MarketSector::Equity)]).len(),
        1
    );
}

#[test]
fn text_and_time_filters() {
    let mut q = query(NewsScope::PressReleases, vec![]);
    q.text = Some("  BERKSHIRE ".into());
    assert_eq!(
        select_items(sample(), &q, None)
            .iter()
            .map(|i| i.id.as_str())
            .collect::<Vec<_>>(),
        vec!["b"]
    );

    // Matches the summary too.
    q.text = Some("summary of no tickers".into());
    assert_eq!(
        select_items(sample(), &q, None)
            .iter()
            .map(|i| i.id.as_str())
            .collect::<Vec<_>>(),
        vec!["d"]
    );

    // Empty text is no filter; from inclusive, to exclusive.
    q.text = Some(String::new());
    q.from = Some(ts("2026-10-05T10:00:00Z"));
    q.to = Some(ts("2026-10-05T12:00:00Z"));
    assert_eq!(
        select_items(sample(), &q, None)
            .iter()
            .map(|i| i.id.as_str())
            .collect::<Vec<_>>(),
        vec!["c", "a"]
    );
}

#[test]
fn sorts_dedups_and_limits() {
    let mut items = sample();
    // Same URL as "a" (another feed carrying the same release).
    items.push(item(
        "a2",
        "GlobeNewswire",
        "Apple update (copy)",
        Some("https://e.com/a"),
        "2026-10-05T10:00:00Z",
        &[],
    ));
    // Same headline and source as "b", different URL.
    items.push(item(
        "b2",
        "Business Wire",
        "Berkshire class B",
        Some("https://e.com/b2"),
        "2026-10-05T09:00:00Z",
        &[],
    ));
    // Same headline, different source: kept.
    items.push(item(
        "b3",
        "PR Newswire",
        "Berkshire class B",
        Some("https://e.com/b3"),
        "2026-10-05T08:00:00Z",
        &[],
    ));
    let mut q = query(NewsScope::PressReleases, vec![]);
    let got = select_items(items.clone(), &q, None);
    assert_eq!(
        got.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
        vec!["b", "c", "a", "b3", "d"]
    );

    q.limit = 2;
    assert_eq!(
        select_items(items, &q, None)
            .iter()
            .map(|i| i.id.as_str())
            .collect::<Vec<_>>(),
        vec!["b", "c"]
    );
}

#[test]
fn merge_returns_partial_results_or_first_error() {
    let ok = Arc::new(vec![item("a", "S", "h", None, "2026-10-05T10:00:00Z", &[])]);
    let err1 = ProviderError::Network("first: down".into());
    let err2 = ProviderError::Http {
        status: 500,
        body_snippet: "second".into(),
    };

    let got =
        merge_feed_results(vec![("f1", Err(err1.clone())), ("f2", Ok(Arc::clone(&ok)))]).unwrap();
    assert_eq!(got.len(), 1);

    let got = merge_feed_results(vec![("f1", Err(err1.clone())), ("f2", Err(err2))]).unwrap_err();
    assert_eq!(got, err1);

    // A feed that worked but had no items is a success.
    assert!(
        merge_feed_results(vec![("f1", Ok(Arc::new(vec![]))), ("f2", Err(err1))])
            .unwrap()
            .is_empty()
    );
}

// ---------------------------------------------------------------- provider

#[test]
fn config_validation() {
    assert_eq!(RssConfig::default().feeds.len(), 4);
    assert!(RssProvider::new(RssConfig::default()).is_ok());
    for feeds in [
        vec![],
        vec![RssFeed::new("", "https://example.com/rss")],
        vec![RssFeed::new("X", "not a url")],
        vec![RssFeed::new("X", "ftp://example.com/rss")],
        vec![RssFeed::new("X", "file:///etc/passwd")],
    ] {
        let err = RssProvider::new(RssConfig {
            feeds: feeds.clone(),
        })
        .unwrap_err();
        assert!(
            matches!(err, ProviderError::Parse { .. }),
            "{feeds:?}: {err:?}"
        );
    }
}

#[test]
fn capabilities_declare_news_only() {
    let p = RssProvider::new(RssConfig::default()).unwrap();
    assert_eq!(p.id(), ProviderId::new("rss"));
    let caps = p.capabilities();
    assert!(caps.supports(Capability::News, Some(AssetClass::Equity)));
    assert!(caps.supports(Capability::News, Some(AssetClass::Etf)));
    assert!(!caps.supports(Capability::Quotes, None));
    assert_eq!(caps.cache_policy, CachePolicy::NoStore);
    assert_eq!(caps.ai_policy, AiPolicy::Unreviewed);
    assert!(caps.display_allowed);
    assert!(!caps.requires_credentials);
    assert_eq!(caps.entries.len(), 1);
}

#[tokio::test]
async fn top_and_market_are_unsupported() {
    let p = RssProvider::new(RssConfig::default()).unwrap();
    for scope in [NewsScope::Top, NewsScope::Market] {
        let err = p.news(&query(scope, vec![])).await.unwrap_err();
        assert_eq!(
            err,
            ProviderError::Unsupported {
                capability: Capability::News
            }
        );
    }
}

/// Minimal HTTP/1.1 server. `routes` maps a path to (status, body); a path
/// listed several times answers its n-th request with its n-th entry (the
/// last entry repeats). Unknown paths get 404.
async fn serve(routes: Vec<(&'static str, u16, String)>) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let hits2 = Arc::clone(&hits);
    let routes = Arc::new(routes);
    let per_path: Arc<std::sync::Mutex<HashMap<String, usize>>> = Arc::default();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let (routes, hits, per_path) = (
                Arc::clone(&routes),
                Arc::clone(&hits2),
                Arc::clone(&per_path),
            );
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                hits.fetch_add(1, Ordering::SeqCst);
                let head = String::from_utf8_lossy(&buf);
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let nth = {
                    let mut map = per_path.lock().unwrap();
                    let n = map.entry(path.clone()).or_default();
                    *n += 1;
                    *n - 1
                };
                let matching: Vec<_> = routes.iter().filter(|(p, _, _)| *p == path).collect();
                let (status, body) = matching
                    .get(nth.min(matching.len().saturating_sub(1)))
                    .map_or((404, "not found".to_owned()), |(_, s, b)| (*s, b.clone()));
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/rss+xml; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (base, hits)
}

fn local_provider(base: &str, paths: &[(&str, &str)]) -> RssProvider {
    let feeds = paths
        .iter()
        .map(|(name, path)| RssFeed::new(*name, format!("{base}{path}")))
        .collect();
    RssProvider::new(RssConfig { feeds })
        .unwrap()
        .with_clock(Arc::new(FixedClock(FETCHED)))
}

#[tokio::test]
async fn news_press_releases_merges_feeds_and_caches() {
    let (base, hits) = serve(vec![
        ("/gnw", 200, GNW.to_owned()),
        ("/prn", 200, PRN.to_owned()),
        ("/bw", 200, BW.to_owned()),
    ])
    .await;
    let p = local_provider(
        &base,
        &[
            ("GlobeNewswire", "/gnw"),
            ("PR Newswire", "/prn"),
            ("Business Wire", "/bw"),
        ],
    );
    let page = p
        .news(&query(NewsScope::PressReleases, vec![]))
        .await
        .unwrap();
    assert_eq!(page.items.len(), 16);
    assert_eq!(page.next, None);
    assert!(
        page.items
            .windows(2)
            .all(|w| w[0].published_at >= w[1].published_at),
        "newest first"
    );
    assert_eq!(
        page.items[0].headline,
        "Nasdaq Hits Fresh Highs While Dow Slips and Small Caps Struggle for Direction"
    );
    assert!(
        page.items
            .iter()
            .all(|i| i.received_at == FETCHED && !i.provenance.synthetic)
    );
    assert_eq!(hits.load(Ordering::SeqCst), 3);

    // Within the cache TTL no feed is fetched again.
    let page = p
        .news(&query(NewsScope::PressReleases, vec![]))
        .await
        .unwrap();
    assert_eq!(page.items.len(), 16);
    assert_eq!(hits.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn news_company_scope_filters_by_ticker() {
    let (base, hits) = serve(vec![
        ("/gnw", 200, GNW.to_owned()),
        ("/bw", 200, BW.to_owned()),
    ])
    .await;
    let p = local_provider(
        &base,
        &[("GlobeNewswire", "/gnw"), ("Business Wire", "/bw")],
    );
    let page = p
        .news(&query(
            NewsScope::Company,
            vec![SecurityKey::equity("SCHW"), SecurityKey::equity("PGC")],
        ))
        .await
        .unwrap();
    let heads: Vec<&str> = page.items.iter().map(|i| i.headline.as_str()).collect();
    assert_eq!(heads.len(), 2, "{heads:?}");
    assert!(heads[0].starts_with("Peapack Private Appoints"));
    assert!(heads[1].starts_with("Schwab Initiates Secondary Listing"));

    // A foreign listing never matches a US key.
    for symbol in ["DSV", "GALD"] {
        let page = p
            .news(&query(
                NewsScope::Company,
                vec![SecurityKey::equity(symbol)],
            ))
            .await
            .unwrap();
        assert!(page.items.is_empty(), "{symbol}");
    }
    assert_eq!(hits.load(Ordering::SeqCst), 2);

    // No US equity keys: nothing to match, nothing fetched.
    let (base2, hits2) = serve(vec![]).await;
    let p2 = local_provider(&base2, &[("X", "/x")]);
    let page = p2
        .news(&query(NewsScope::Company, vec![SecurityKey::index("SPX")]))
        .await
        .unwrap();
    assert!(page.items.is_empty());
    assert_eq!(hits2.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn news_tolerates_some_failed_feeds() {
    let (base, _) = serve(vec![
        ("/bw", 200, BW.to_owned()),
        ("/html", 200, "<html><body>blocked</body></html>".into()),
    ])
    .await;
    let p = local_provider(
        &base,
        &[
            ("Missing", "/missing"),
            ("Business Wire", "/bw"),
            ("Html", "/html"),
        ],
    );
    let page = p
        .news(&query(NewsScope::PressReleases, vec![]))
        .await
        .unwrap();
    assert_eq!(page.items.len(), 6);
    assert!(page.items.iter().all(|i| i.source == "Business Wire"));
}

#[tokio::test]
async fn news_fails_when_every_feed_fails() {
    let (base, _) = serve(vec![
        ("/a", 500, "upstream broke".into()),
        ("/b", 503, "maintenance".into()),
    ])
    .await;
    let p = local_provider(&base, &[("Feed A", "/a"), ("Feed B", "/b")]);
    let err = p
        .news(&query(NewsScope::PressReleases, vec![]))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        ProviderError::Http {
            status: 500,
            body_snippet: "Feed A: upstream broke".into()
        }
    );

    let (base, _) = serve(vec![("/html", 200, "<!DOCTYPE html><html></html>".into())]).await;
    let p = local_provider(&base, &[("Html", "/html")]);
    let err = p
        .news(&query(NewsScope::PressReleases, vec![]))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ProviderError::Parse { context } if context.starts_with("Html:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn news_retries_transient_feed_failures() {
    // First request 404 (as PR Newswire's flaky redirect produces), then OK.
    let (base, hits) = serve(vec![
        ("/prn", 404, "gone".into()),
        ("/prn", 200, PRN.to_owned()),
    ])
    .await;
    let p = local_provider(&base, &[("PR Newswire", "/prn")]);
    let page = p
        .news(&query(NewsScope::PressReleases, vec![]))
        .await
        .unwrap();
    assert_eq!(page.items.len(), 5);
    assert_eq!(hits.load(Ordering::SeqCst), 2);

    // Persistent failure: three attempts, then the error.
    let (base, hits) = serve(vec![("/down", 502, "bad gateway".into())]).await;
    let p = local_provider(&base, &[("Down", "/down")]);
    let err = p
        .news(&query(NewsScope::PressReleases, vec![]))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        ProviderError::Http {
            status: 502,
            body_snippet: "Down: bad gateway".into()
        }
    );
    assert_eq!(hits.load(Ordering::SeqCst), 3);

    // Rate limiting is not retried here (the router owns backoff).
    let (base, hits) = serve(vec![("/busy", 429, "slow down".into())]).await;
    let p = local_provider(&base, &[("Busy", "/busy")]);
    let err = p
        .news(&query(NewsScope::PressReleases, vec![]))
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::RateLimited { .. }), "{err:?}");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

//! RSS 2.0 / RSS 1.0 (RDF) / Atom parsing with quick-xml's streaming reader,
//! and the mapping from feed entries to [`NewsItem`].
//!
//! Element matching is by qualified name as written in the document
//! (`dc:subject`, `prn:industry`), not by namespace URI: publishers bind the
//! usual prefixes to non-standard URIs (GlobeNewswire binds `dc:` to
//! `http://dublincore.org/documents/dcmi-namespace/`).

use std::borrow::Cow;

use chrono::{DateTime, Utc};
use html2text::render::TrivialDecorator;
use meridian_provider::ProviderError;
use meridian_types::{
    DataDelay, FeedSource, NewsItem, Provenance, ProviderId, UnixNanos, datetime_to_nanos,
};
use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesRef, BytesStart, Event};

use crate::RssFeed;
use crate::tickers;

/// Summaries longer than this are cut at a character boundary.
pub(crate) const SUMMARY_MAX_CHARS: usize = 1_000;
/// Wrap width handed to html2text; large enough that it never wraps.
const HTML_WIDTH: usize = 10_000;
/// Topics longer than this are dropped (they are prose, not labels).
const TOPIC_MAX_CHARS: usize = 120;

/// One `<item>` / `<entry>` before normalisation. Strings are as decoded from
/// XML (entities resolved, CDATA unwrapped) but may still contain HTML.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct RawEntry {
    pub title: Option<String>,
    /// RSS `<link>` text.
    pub link: Option<String>,
    /// Atom `<link rel="alternate" href>` (or a link without `rel`).
    pub alt_link: Option<String>,
    /// RSS `<description>` / Atom `<summary>`.
    pub summary: Option<String>,
    /// RSS `<content:encoded>` / Atom `<content>`.
    pub content: Option<String>,
    /// RSS `<pubDate>` / `<dc:date>` / Atom `<published>`.
    pub published: Option<String>,
    /// Atom `<updated>`.
    pub updated: Option<String>,
    /// RSS `<guid>` / Atom `<id>`.
    pub id: Option<String>,
    pub categories: Vec<RawCategory>,
    /// `dc:subject`, `dc:keyword`, `prn:subject`, `prn:industry`.
    pub subjects: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RawCategory {
    pub domain: Option<String>,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FeedKind {
    Rss,
    Rdf,
    Atom,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RawFeed {
    pub kind: FeedKind,
    pub entries: Vec<RawEntry>,
}

/// Which entry field a text capture fills.
#[derive(Debug, Clone, PartialEq)]
enum Field {
    Title,
    Link,
    Summary,
    Content,
    Published,
    Updated,
    Id,
    Category { domain: Option<String> },
    Subject,
}

struct Capture {
    field: Field,
    /// Element depth of the captured child.
    depth: usize,
    text: String,
}

fn attr(e: &BytesStart<'_>, name: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == name)
        .map(|a| {
            a.normalized_value(XmlVersion::Implicit1_0)
                .map_or_else(|_| a.value.to_string(), Cow::into_owned)
        })
}

fn local_name(qname: &str) -> &str {
    qname.rsplit(':').next().unwrap_or(qname)
}

/// Maps a direct child of an item/entry to the field it fills.
fn field_for(qname: &str, e: &BytesStart<'_>) -> Option<Field> {
    Some(match qname {
        "title" | "atom:title" => Field::Title,
        "link" | "atom:link" => Field::Link,
        "description" | "summary" | "atom:summary" => Field::Summary,
        "content:encoded" | "content" | "atom:content" => Field::Content,
        "pubDate" | "published" | "atom:published" | "dc:date" => Field::Published,
        "updated" | "atom:updated" => Field::Updated,
        "guid" | "id" | "atom:id" => Field::Id,
        "category" | "atom:category" => Field::Category {
            domain: attr(e, "domain"),
        },
        "dc:subject" | "dc:keyword" | "prn:subject" | "prn:industry" => Field::Subject,
        _ => return None,
    })
}

fn set_once(slot: &mut Option<String>, value: String) {
    if slot.is_none() && !value.trim().is_empty() {
        *slot = Some(value);
    }
}

fn store(entry: &mut RawEntry, field: Field, text: String) {
    match field {
        Field::Title => set_once(&mut entry.title, text),
        Field::Link => set_once(&mut entry.link, text.trim().to_owned()),
        Field::Summary => set_once(&mut entry.summary, text),
        Field::Content => set_once(&mut entry.content, text),
        Field::Published => set_once(&mut entry.published, text),
        Field::Updated => set_once(&mut entry.updated, text),
        Field::Id => set_once(&mut entry.id, text.trim().to_owned()),
        Field::Category { domain } => {
            if !text.trim().is_empty() {
                entry.categories.push(RawCategory {
                    domain,
                    value: text.trim().to_owned(),
                });
            }
        }
        Field::Subject => {
            if !text.trim().is_empty() {
                entry.subjects.push(text.trim().to_owned());
            }
        }
    }
}

/// Handles attribute-only children (Atom `<link href>` and `<category term>`),
/// which may be written as empty or as start tags.
fn handle_attr_child(entry: &mut RawEntry, qname: &str, e: &BytesStart<'_>) -> bool {
    match qname {
        "link" | "atom:link" => {
            let Some(href) = attr(e, "href") else {
                return false;
            };
            let rel = attr(e, "rel");
            if entry.alt_link.is_none()
                && rel.as_deref().is_none_or(|r| r == "alternate")
                && !href.trim().is_empty()
            {
                entry.alt_link = Some(href.trim().to_owned());
            }
            true
        }
        "category" | "atom:category" => {
            let Some(term) = attr(e, "label").or_else(|| attr(e, "term")) else {
                return false;
            };
            if !term.trim().is_empty() {
                entry.categories.push(RawCategory {
                    domain: attr(e, "scheme"),
                    value: term.trim().to_owned(),
                });
            }
            true
        }
        _ => false,
    }
}

fn push_ref(text: &mut String, r: &BytesRef<'_>) {
    if r.is_char_ref() {
        if let Ok(Some(c)) = r.resolve_char_ref() {
            text.push(c);
        }
    } else if let Some(s) = quick_xml::escape::resolve_xml_entity(r) {
        text.push_str(s);
    } else {
        // Not an XML entity (e.g. `&nbsp;` in a sloppy feed). Keep it as
        // written so the HTML pass can decode it.
        text.push('&');
        text.push_str(r);
        text.push(';');
    }
}

/// Parses an RSS 2.0, RSS 1.0 (RDF) or Atom document into raw entries.
///
/// A document that is not a feed, or that breaks before any complete entry,
/// is a `Parse` error. If the XML breaks after some entries were complete,
/// those entries are returned and the breakage is logged.
pub(crate) fn parse_feed(xml: &str, context: &str) -> Result<RawFeed, ProviderError> {
    let mut reader = Reader::from_str(xml.trim_start_matches('\u{feff}'));
    {
        let cfg = reader.config_mut();
        cfg.allow_dangling_amp = true;
        cfg.check_end_names = false;
    }

    let mut kind: Option<FeedKind> = None;
    let mut entries = Vec::new();
    let mut depth = 0usize;
    let mut entry: Option<(RawEntry, usize)> = None;
    let mut capture: Option<Capture> = None;

    loop {
        let event = match reader.read_event() {
            Ok(ev) => ev,
            Err(e) => {
                let pos = reader.error_position();
                if entries.is_empty() {
                    return Err(ProviderError::parse(format!(
                        "{context}: invalid XML at byte {pos}: {e}"
                    )));
                }
                tracing::warn!(feed = context, position = pos, error = %e, "feed XML broken; keeping complete items");
                break;
            }
        };
        match event {
            Event::Start(e) => {
                depth += 1;
                let name = e.name();
                let qname: &str = name.as_ref();
                if kind.is_none() {
                    kind = Some(root_kind(qname, context)?);
                    continue;
                }
                if let Some(cap) = capture.as_mut() {
                    // Nested markup inside a captured field (Atom xhtml):
                    // keep words apart.
                    cap.text.push(' ');
                    continue;
                }
                match entry.as_mut() {
                    None => {
                        if matches!(local_name(qname), "item" | "entry") {
                            entry = Some((RawEntry::default(), depth));
                        }
                    }
                    // Attribute-only children (Atom link/category) need no
                    // text capture; anything inside them is ignored.
                    Some((raw, entry_depth)) if depth == *entry_depth + 1 => {
                        if !handle_attr_child(raw, qname, &e)
                            && let Some(field) = field_for(qname, &e)
                        {
                            capture = Some(Capture {
                                field,
                                depth,
                                text: String::new(),
                            });
                        }
                    }
                    Some(_) => {}
                }
            }
            Event::Empty(e) => {
                let name = e.name();
                let qname: &str = name.as_ref();
                if kind.is_none() {
                    // A self-closing root has no entries.
                    kind = Some(root_kind(qname, context)?);
                    break;
                }
                if let Some(cap) = capture.as_mut() {
                    cap.text.push(' ');
                    continue;
                }
                if let Some((raw, entry_depth)) = entry.as_mut()
                    && depth == *entry_depth
                {
                    handle_attr_child(raw, qname, &e);
                }
            }
            Event::End(_) => {
                match capture.as_mut() {
                    // Closing nested markup inside a captured field.
                    Some(cap) if cap.depth != depth => cap.text.push(' '),
                    Some(_) => {
                        if let (Some(cap), Some((raw, _))) = (capture.take(), entry.as_mut()) {
                            store(raw, cap.field, cap.text);
                        }
                    }
                    None => {
                        if entry.as_ref().is_some_and(|(_, d)| *d == depth)
                            && let Some((raw, _)) = entry.take()
                        {
                            entries.push(raw);
                        }
                    }
                }
                depth = depth.saturating_sub(1);
            }
            Event::Text(t) => {
                if let Some(cap) = capture.as_mut() {
                    cap.text.push_str(&t.xml10_content());
                }
            }
            Event::CData(t) => {
                if let Some(cap) = capture.as_mut() {
                    cap.text.push_str(&t.xml10_content());
                }
            }
            Event::GeneralRef(r) => {
                if let Some(cap) = capture.as_mut() {
                    push_ref(&mut cap.text, &r);
                }
            }
            Event::Eof => break,
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) | Event::DocType(_) => {}
        }
    }

    let Some(kind) = kind else {
        return Err(ProviderError::parse(format!("{context}: empty document")));
    };
    Ok(RawFeed { kind, entries })
}

fn root_kind(qname: &str, context: &str) -> Result<FeedKind, ProviderError> {
    match local_name(qname) {
        "rss" => Ok(FeedKind::Rss),
        "RDF" => Ok(FeedKind::Rdf),
        "feed" => Ok(FeedKind::Atom),
        other => Err(ProviderError::parse(format!(
            "{context}: not an RSS or Atom document (root element <{other}>)"
        ))),
    }
}

/// Collapses all whitespace runs (including NBSP) to single spaces.
pub(crate) fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Converts possibly-HTML feed text to plain text with collapsed whitespace.
/// Text without markup or entities skips the HTML pass. `None` if html2text
/// fails (it should not with width overflow allowed).
pub(crate) fn to_plain_text(raw: &str) -> Option<String> {
    if !raw.contains(['<', '&']) {
        return Some(collapse_ws(raw));
    }
    match html2text::config::with_decorator(TrivialDecorator::new())
        .raw_mode(true)
        .allow_width_overflow()
        .string_from_read(raw.as_bytes(), HTML_WIDTH)
    {
        Ok(text) => Some(collapse_ws(&text)),
        Err(e) => {
            tracing::debug!(error = %e, "html2text failed");
            None
        }
    }
}

/// Truncates to at most `max` characters at a char boundary.
pub(crate) fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((idx, _)) => s[..idx].trim_end().to_owned(),
        None => s.to_owned(),
    }
}

/// Zone names accepted at the end of an RFC 2822 date. chrono maps any other
/// alphabetic zone to UTC (RFC 2822's "unknown"), which would shift e.g. a
/// `CEST` time by two hours, so those dates are rejected instead.
const RFC2822_ZONES: [&str; 12] = [
    "UT", "UTC", "GMT", "Z", "EST", "EDT", "CST", "CDT", "MST", "MDT", "PST", "PDT",
];

/// Parses a feed date (RFC 2822 for RSS, RFC 3339 for Atom; either is tried
/// for both). Dates without a time zone are rejected: we never guess one.
pub(crate) fn parse_date(raw: &str) -> Option<UnixNanos> {
    let s = collapse_ws(raw);
    if s.is_empty() {
        return None;
    }
    let to_nanos = |dt: DateTime<chrono::FixedOffset>| datetime_to_nanos(dt.with_timezone(&Utc));

    if let Ok(dt) = DateTime::parse_from_rfc3339(&s) {
        return Some(to_nanos(dt));
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f%z", "%Y-%m-%dT%H:%M%z"] {
        if let Ok(dt) = DateTime::parse_from_str(&s, fmt) {
            return Some(to_nanos(dt));
        }
    }

    // RFC 2822 family.
    let zone = s.rsplit(' ').next().unwrap_or_default();
    if zone.chars().all(|c| c.is_ascii_alphabetic())
        && !RFC2822_ZONES.iter().any(|z| z.eq_ignore_ascii_case(zone))
    {
        return None;
    }
    let head = &s[..s.len() - zone.len()];
    let s = match zone.as_bytes() {
        // `+05:30` → `+0530`.
        [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2]
            if [h1, h2, m1, m2].iter().all(|b| b.is_ascii_digit()) =>
        {
            format!(
                "{head}{}{}{}{}{}",
                *sign as char, *h1 as char, *h2 as char, *m1 as char, *m2 as char
            )
        }
        // chrono's RFC 2822 parser doesn't take these names; both mean UTC.
        _ if zone.eq_ignore_ascii_case("UTC") || zone.eq_ignore_ascii_case("Z") => {
            format!("{head}+0000")
        }
        _ => s.clone(),
    };
    if let Ok(dt) = DateTime::parse_from_rfc2822(&s) {
        return Some(to_nanos(dt));
    }
    // Some feeds print a weekday that doesn't match the date, or a long
    // weekday name; chrono rejects both. The date itself is authoritative.
    if let Some((_, rest)) = s.split_once(',')
        && let Ok(dt) = DateTime::parse_from_rfc2822(rest.trim())
    {
        return Some(to_nanos(dt));
    }
    None
}

fn is_http_url(s: &str) -> bool {
    let lower = s.get(..8).unwrap_or(s).to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// Short upper-case codes (`FIN`, `PDT`) and pure numbers are classification
/// codes, not readable topics.
fn is_code_like(s: &str) -> bool {
    s.chars().all(|c| c.is_ascii_digit())
        || (s.len() <= 3 && s.chars().all(|c| c.is_ascii_uppercase()))
}

fn push_topic(topics: &mut Vec<String>, raw: &str) {
    let Some(t) = to_plain_text(raw) else { return };
    if t.is_empty() || is_code_like(&t) || t.chars().count() > TOPIC_MAX_CHARS {
        return;
    }
    if !topics.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
        topics.push(t);
    }
}

fn domain_is(domain: Option<&str>, suffix: &str) -> bool {
    domain.is_some_and(|d| {
        d.trim_end_matches('/')
            .to_ascii_lowercase()
            .ends_with(suffix)
    })
}

/// Converts a raw entry to a [`NewsItem`]. Returns `None` (and logs why) for
/// entries without a headline, a parseable date, or any identifier.
pub(crate) fn to_news_item(
    raw: RawEntry,
    feed: &RssFeed,
    fetched_at: UnixNanos,
) -> Option<NewsItem> {
    let skip = |reason: &str| {
        tracing::debug!(feed = %feed.name, reason, title = raw.title.as_deref().unwrap_or(""), "skipping feed item");
    };

    let Some(headline) = raw
        .title
        .as_deref()
        .and_then(to_plain_text)
        .filter(|t| !t.is_empty())
    else {
        skip("missing title");
        return None;
    };
    let date_raw = raw.published.as_deref().or(raw.updated.as_deref());
    let Some(published_at) = date_raw.and_then(parse_date) else {
        skip("missing or unparseable date");
        return None;
    };
    let url = raw
        .link
        .clone()
        .or_else(|| raw.alt_link.clone())
        .filter(|u| is_http_url(u));
    let Some(id_basis) = raw.id.clone().or_else(|| url.clone()) else {
        skip("no guid/id or link");
        return None;
    };

    let full_summary = raw
        .summary
        .as_deref()
        .or(raw.content.as_deref())
        .and_then(to_plain_text)
        .filter(|t| !t.is_empty());

    let mut tickers_out = Vec::new();
    let mut topics = Vec::new();
    tickers::extract_from_text(&headline, &mut tickers_out);
    if let Some(s) = &full_summary {
        tickers::extract_from_text(s, &mut tickers_out);
    }
    for cat in &raw.categories {
        let domain = cat.domain.as_deref();
        if domain_is(domain, "/rss/stock") {
            if let Some(t) = tickers::from_stock_category(&cat.value) {
                tickers::push_ticker(&mut tickers_out, t);
            }
        } else if domain_is(domain, "/rss/isin") {
            // ISINs are identifiers, not topics.
        } else {
            tickers::extract_from_text(&cat.value, &mut tickers_out);
            push_topic(&mut topics, &cat.value);
        }
    }
    for subject in &raw.subjects {
        push_topic(&mut topics, subject);
    }

    let summary = full_summary.map(|s| truncate_chars(&s, SUMMARY_MAX_CHARS));
    let source_ref = url.clone().unwrap_or_else(|| feed.url.clone());
    Some(NewsItem {
        id: format!("{}:{}", feed.name, id_basis),
        source: feed.name.clone(),
        headline,
        summary,
        body: None,
        url,
        published_at,
        received_at: fetched_at,
        tickers: tickers_out,
        topics,
        provenance: Provenance {
            provider: ProviderId::new(crate::PROVIDER_ID),
            synthetic: false,
            delay: DataDelay::RealTime,
            source: FeedSource::Aggregated,
            as_of: fetched_at,
            source_ref: Some(source_ref),
            attribution: None,
        },
    })
}

/// Parses a whole feed body into news items, skipping malformed entries.
pub(crate) fn parse_items(
    body: &str,
    feed: &RssFeed,
    fetched_at: UnixNanos,
) -> Result<Vec<NewsItem>, ProviderError> {
    let raw = parse_feed(body, &feed.name)?;
    let total = raw.entries.len();
    let items: Vec<NewsItem> = raw
        .entries
        .into_iter()
        .filter_map(|e| to_news_item(e, feed, fetched_at))
        .collect();
    tracing::debug!(feed = %feed.name, kind = ?raw.kind, total, kept = items.len(), "parsed feed");
    Ok(items)
}

//! Plain-text rendering of a filing's primary document and, for periodic
//! reports (10-K, 10-Q and their variants), splitting into `Item` sections.
//!
//! Section heuristic: a heading is a line starting with `Item <n>[A-C]`
//! followed by punctuation, end of line, or a capitalized title (so a wrapped
//! cross-reference such as "Item 7 of this report" is not a heading). The
//! table of contents repeats every heading, so the body is taken to start at
//! the occurrence, of the earliest item that appears more than once, that is
//! followed by the most text. Headings before that line belong to the cover
//! and table of contents; after it, each item uses its occurrence followed by
//! the most text. 10-Q item numbers repeat across Part I and Part II, so for
//! 10-Qs the most recent `PART` line is part of the item's identity.

use std::collections::HashMap;

use meridian_provider::{ProviderError, ProviderResult};
use meridian_types::FilingSection;

/// Largest primary document we download (10-Ks run to 10+ MB).
pub(crate) const MAX_DOCUMENT_BYTES: usize = 25 * 1024 * 1024;
/// Column width of the rendered text.
const TEXT_WIDTH: usize = 120;
/// Longest line accepted as a heading title.
const MAX_TITLE_CHARS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocKind {
    Html,
    Text,
}

/// How to render a primary document, from its file name. `None` for formats
/// we can't render as text (PDF, images, …).
pub(crate) fn doc_kind(name: &str) -> Option<DocKind> {
    let path = name.split(['?', '#']).next().unwrap_or(name);
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("htm" | "html" | "xhtml" | "xml") => Some(DocKind::Html),
        Some("txt") => Some(DocKind::Text),
        _ => None,
    }
}

/// Renders document bytes as plain text. Invalid UTF-8 is replaced, not
/// rejected (older EDGAR documents are often Windows-1252).
pub(crate) fn render_text(bytes: &[u8], kind: DocKind) -> ProviderResult<String> {
    let raw = String::from_utf8_lossy(bytes);
    match kind {
        DocKind::Text => Ok(raw.into_owned()),
        DocKind::Html => {
            let cleaned = strip_ix_header(&raw);
            html2text::config::plain_no_decorate()
                .raw_mode(true)
                .allow_width_overflow()
                .string_from_read(cleaned.as_bytes(), TEXT_WIDTH)
                .map_err(|e| ProviderError::parse(format!("filing document HTML: {e}")))
        }
    }
}

/// Removes inline-XBRL `<ix:header>` blocks. Browsers hide them with CSS, but
/// html2text (built without CSS support) would print their hidden facts and
/// contexts as text.
fn strip_ix_header(html: &str) -> String {
    const OPEN: &str = "<ix:header";
    const CLOSE: &str = "</ix:header>";
    // ASCII lower-casing keeps byte offsets identical to `html`.
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut pos = 0;
    while let Some(start) = lower[pos..].find(OPEN).map(|i| i + pos) {
        let Some(end) = lower[start..].find(CLOSE).map(|i| i + start + CLOSE.len()) else {
            break;
        };
        out.push_str(&html[pos..start]);
        pos = end;
    }
    out.push_str(&html[pos..]);
    out
}

/// Whether `form` is a periodic report with `Item` sections.
fn is_periodic(form: &str) -> bool {
    let f = form.trim().to_ascii_uppercase();
    f.starts_with("10-K") || f.starts_with("10-Q")
}

/// Splits rendered text into sections: "Cover" plus one per item for
/// periodic reports, otherwise a single "Document" section.
pub(crate) fn split_sections(text: &str, form: &str) -> Vec<FilingSection> {
    let lines: Vec<&str> = text.lines().collect();
    let whole = || {
        vec![FilingSection {
            title: "Document".into(),
            text: clean_text(&lines),
        }]
    };
    if !is_periodic(form) {
        return whole();
    }
    let use_parts = form.trim().to_ascii_uppercase().starts_with("10-Q");
    let mut cands = find_headings(&lines, use_parts);
    if cands.is_empty() {
        return whole();
    }
    fill_titles(&mut cands, &lines);
    let lens = content_lengths(&cands, &lines);

    // Body start: the most-text occurrence of the earliest repeated item.
    let mut by_key: HashMap<(u8, u8, u8), Vec<usize>> = HashMap::new();
    for (i, c) in cands.iter().enumerate() {
        by_key.entry(c.ordinal()).or_default().push(i);
    }
    let best = |idxs: &[usize], from_line: usize| -> Option<usize> {
        idxs.iter()
            .copied()
            .filter(|i| cands[*i].line >= from_line)
            .max_by(|a, b| lens[*a].cmp(&lens[*b]).then(a.cmp(b)))
    };
    let body_start = by_key
        .iter()
        .filter(|(_, v)| v.len() > 1)
        .min_by_key(|(k, _)| **k)
        .and_then(|(_, v)| best(v, 0))
        .map_or(0, |i| cands[i].line);

    let mut chosen: Vec<usize> = by_key
        .values()
        .filter_map(|v| best(v, body_start))
        .collect();
    chosen.sort_by_key(|i| cands[*i].line);
    if chosen.is_empty() {
        return whole();
    }

    let mut sections = Vec::with_capacity(chosen.len() + 1);
    let cover = clean_text(&lines[..drop_trailing_part(&lines, 0, cands[chosen[0]].line)]);
    if !cover.is_empty() {
        sections.push(FilingSection {
            title: "Cover".into(),
            text: cover,
        });
    }
    for (n, &ci) in chosen.iter().enumerate() {
        let c = &cands[ci];
        let start = c.title_line.unwrap_or(c.line) + 1;
        let end = chosen.get(n + 1).map_or(lines.len(), |next| {
            drop_trailing_part(&lines, start, cands[*next].line)
        });
        let body = if start < end {
            clean_text(&lines[start..end])
        } else {
            String::new()
        };
        sections.push(FilingSection {
            title: c.display_title(use_parts),
            text: body,
        });
    }
    sections
}

#[derive(Debug, Clone)]
struct Heading {
    line: usize,
    part: u8,
    number: u8,
    letter: Option<char>,
    title: String,
    /// Line the title was taken from when the heading line had none.
    title_line: Option<usize>,
}

impl Heading {
    fn ordinal(&self) -> (u8, u8, u8) {
        (self.part, self.number, self.letter.map_or(0, |c| c as u8))
    }

    fn code(&self) -> String {
        match self.letter {
            Some(l) => format!("{}{l}", self.number),
            None => self.number.to_string(),
        }
    }

    fn display_title(&self, use_parts: bool) -> String {
        let item = if self.title.is_empty() {
            format!("Item {}", self.code())
        } else {
            format!("Item {}. {}", self.code(), self.title)
        };
        match (use_parts, roman(self.part)) {
            (true, Some(r)) => format!("Part {r}, {item}"),
            _ => item,
        }
    }
}

/// Moves `end` back over trailing blank lines and a bare `PART …` heading,
/// which introduce the next section rather than end this one.
fn drop_trailing_part(lines: &[&str], start: usize, mut end: usize) -> usize {
    let blank = |l: &str| l.trim().is_empty();
    let mut e = end;
    while e > start && blank(lines[e - 1]) {
        e -= 1;
    }
    if e > start
        && parse_part(lines[e - 1]).is_some_and(|(_, rest)| parse_item_heading(rest).is_none())
    {
        end = e - 1;
    }
    end
}

fn roman(part: u8) -> Option<&'static str> {
    match part {
        1 => Some("I"),
        2 => Some("II"),
        3 => Some("III"),
        4 => Some("IV"),
        _ => None,
    }
}

fn find_headings(lines: &[&str], use_parts: bool) -> Vec<Heading> {
    let mut out = Vec::new();
    let mut part = 0u8;
    for (i, line) in lines.iter().enumerate() {
        let mut candidate = *line;
        if let Some((p, rest)) = parse_part(line) {
            part = p;
            candidate = rest;
        }
        if let Some((number, letter, title)) = parse_item_heading(candidate) {
            out.push(Heading {
                line: i,
                part: if use_parts { part } else { 0 },
                number,
                letter,
                title,
                title_line: None,
            });
        }
    }
    out
}

/// When a heading line has no title ("Item 1A." alone, common when the
/// heading sits in table cells), uses the next non-empty line within three
/// lines, unless that line is itself a heading.
fn fill_titles(cands: &mut [Heading], lines: &[&str]) {
    let heading_lines: Vec<usize> = cands.iter().map(|c| c.line).collect();
    for c in cands.iter_mut().filter(|c| c.title.is_empty()) {
        for (j, line) in lines.iter().enumerate().skip(c.line + 1).take(3) {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if !heading_lines.contains(&j)
                && parse_part(t).is_none()
                && t.chars().count() <= MAX_TITLE_CHARS
            {
                c.title = clean_title(t);
                c.title_line = Some(j);
            }
            break;
        }
    }
}

/// Non-whitespace characters between each heading and the next one.
fn content_lengths(cands: &[Heading], lines: &[&str]) -> Vec<usize> {
    cands
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let start = c.title_line.unwrap_or(c.line) + 1;
            let end = cands.get(i + 1).map_or(lines.len(), |n| n.line);
            lines.get(start..end).map_or(0, |ls| {
                ls.iter()
                    .map(|l| l.chars().filter(|ch| !ch.is_whitespace()).count())
                    .sum()
            })
        })
        .collect()
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

/// Parses `PART I` … `PART IV` at line start. Returns the part number and the
/// rest of the line (which may hold an item heading). Lines that continue in
/// lower case ("Part II of this report …") are prose, not headings.
fn parse_part(line: &str) -> Option<(u8, &str)> {
    let t = line.trim();
    if t.chars().count() > 120 {
        return None;
    }
    let rest = strip_prefix_ci(t, "part")?;
    let after = rest.trim_start();
    if after.len() == rest.len() {
        return None;
    }
    let numeral_len = after
        .chars()
        .take_while(|c| matches!(c.to_ascii_uppercase(), 'I' | 'V'))
        .count();
    let numeral = after[..numeral_len].to_ascii_uppercase();
    let part = match numeral.as_str() {
        "I" => 1,
        "II" => 2,
        "III" => 3,
        "IV" => 4,
        _ => return None,
    };
    let tail = &after[numeral_len..];
    if tail.chars().next().is_some_and(char::is_alphanumeric) {
        return None;
    }
    let tail = tail.trim_start_matches(|c: char| {
        c.is_whitespace() || matches!(c, '.' | ',' | ':' | '-' | '\u{2013}' | '\u{2014}')
    });
    if tail.chars().next().is_some_and(char::is_lowercase) {
        return None;
    }
    Some((part, tail))
}

/// Parses an item heading at line start: `Item 1A. Risk Factors`,
/// `ITEM 7 — MD&A`, `Item 2` (title on the next line). Returns the item
/// number (1–16), its letter suffix, and the title (possibly empty).
fn parse_item_heading(line: &str) -> Option<(u8, Option<char>, String)> {
    let t = line.trim_start();
    let rest = strip_prefix_ci(t, "items").or_else(|| strip_prefix_ci(t, "item"))?;
    let after = rest.trim_start();
    if after.len() == rest.len() {
        return None;
    }
    let digits = after.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 2 {
        return None;
    }
    let number: u8 = after[..digits].parse().ok()?;
    if !(1..=16).contains(&number) {
        return None;
    }
    let mut tail = &after[digits..];
    let mut letter = None;
    let mut chars = tail.chars();
    if let Some(c) = chars.next()
        && matches!(c.to_ascii_uppercase(), 'A' | 'B' | 'C')
        && chars.next().is_none_or(|n| !n.is_alphanumeric())
    {
        letter = Some(c.to_ascii_uppercase());
        tail = &tail[1..];
    }
    if tail.chars().next().is_some_and(char::is_alphanumeric) {
        return None;
    }
    let tail = tail.trim_start();
    let title = if let Some(r) = tail.strip_prefix(['.', ':', '-', '\u{2013}', '\u{2014}']) {
        r
    } else if tail.is_empty() || tail.chars().next().is_some_and(char::is_uppercase) {
        tail
    } else {
        return None;
    };
    let title = clean_title(title);
    if title.chars().count() > MAX_TITLE_CHARS {
        return None;
    }
    Some((number, letter, title))
}

/// Trims whitespace, leading separators, and a trailing table-of-contents
/// page number ("Risk Factors ..... 12").
fn clean_title(s: &str) -> String {
    let t = s
        .trim()
        .trim_start_matches(['.', ':', '-', '\u{2013}', '\u{2014}'])
        .trim();
    let without_page = t.trim_end_matches(|c: char| c.is_ascii_digit());
    let t = if without_page.len() < t.len()
        && without_page.ends_with(|c: char| c.is_whitespace() || c == '.')
    {
        without_page
    } else {
        t
    };
    t.trim_end_matches(|c: char| c.is_whitespace() || c == '.')
        .to_owned()
}

/// Trims line ends and collapses runs of blank lines.
fn clean_text(lines: &[&str]) -> String {
    let mut out = String::new();
    let mut blank = false;
    for line in lines {
        let l = line.trim_end();
        if l.trim().is_empty() {
            blank = !out.is_empty();
            continue;
        }
        if blank {
            out.push('\n');
            blank = false;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(l);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenk_sections() -> Vec<FilingSection> {
        let html = include_bytes!("../tests/fixtures/example_10k.htm");
        let text = render_text(html, DocKind::Html).unwrap();
        split_sections(&text, "10-K")
    }

    #[test]
    fn parses_heading_variants() {
        assert_eq!(
            parse_item_heading("Item 1A. Risk Factors"),
            Some((1, Some('A'), "Risk Factors".into()))
        );
        assert_eq!(
            parse_item_heading("  ITEM 7 \u{2014} MANAGEMENT'S DISCUSSION"),
            Some((7, None, "MANAGEMENT'S DISCUSSION".into()))
        );
        assert_eq!(
            parse_item_heading("Item\u{a0}2."),
            Some((2, None, String::new()))
        );
        assert_eq!(
            parse_item_heading("Item 9C: Disclosure"),
            Some((9, Some('C'), "Disclosure".into()))
        );
        assert_eq!(
            parse_item_heading("Item 1B Unresolved Staff Comments"),
            Some((1, Some('B'), "Unresolved Staff Comments".into()))
        );
        assert_eq!(
            parse_item_heading("Item 1A. Risk Factors ........ 12"),
            Some((1, Some('A'), "Risk Factors".into()))
        );
        // Prose and non-headings.
        assert_eq!(parse_item_heading("Item 7 of this report describes"), None);
        assert_eq!(parse_item_heading("Items listed below"), None);
        assert_eq!(parse_item_heading("Item 17. Not an item"), None);
        assert_eq!(parse_item_heading("Item 1Business"), None);
        assert_eq!(parse_item_heading("Itemized deductions"), None);
    }

    #[test]
    fn parses_part_lines() {
        assert_eq!(parse_part("PART I"), Some((1, "")));
        assert_eq!(
            parse_part("Part II \u{2014} Other Information"),
            Some((2, "Other Information"))
        );
        assert_eq!(
            parse_part("PART II - ITEM 1. LEGAL PROCEEDINGS"),
            Some((2, "ITEM 1. LEGAL PROCEEDINGS"))
        );
        assert_eq!(parse_part("Part II of this report contains"), None);
        assert_eq!(parse_part("Partial results"), None);
        assert_eq!(parse_part("PART V"), None);
    }

    #[test]
    fn splits_10k_and_skips_table_of_contents() {
        let s = tenk_sections();
        let titles: Vec<&str> = s.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(
            titles,
            vec![
                "Cover",
                "Item 1. Business",
                "Item 1A. Risk Factors",
                "Item 2. Properties",
                "Item 7. Management's Discussion and Analysis",
                "Item 8. Financial Statements",
                "Item 16. Form 10-K Summary",
            ]
        );
        // The cover holds the cover page and the table of contents.
        assert!(s[0].text.contains("EXAMPLE CORP"));
        assert!(s[0].text.contains("Table of Contents"));
        assert!(s[0].text.contains("Forward-looking statements"));
        assert!(s[0].text.ends_with("table-of-contents row."));
        // Body text, not the TOC row, lands in each item.
        assert!(s[1].text.contains("Example Corp makes example widgets"));
        assert!(!s[1].text.contains("Risk Factors"));
        assert!(s[2].text.contains("Example risks are illustrative"));
        // The cross-reference wrapped to a line start stays inside Item 2.
        assert!(s[3].text.contains("Item 7 of this report"));
        assert!(s[3].text.ends_with("not a heading."));
        assert_eq!(s[6].text, "None.");
        // Hidden inline-XBRL header content is not rendered.
        assert!(!s.iter().any(|s| s.text.contains("hidden-dei-fact")));
    }

    #[test]
    fn splits_10q_by_part() {
        let text = "\
EXAMPLE CORP FORM 10-Q
PART I
Item 1. Financial Statements
Item 2. MD&A
PART II
Item 1. Legal Proceedings
Item 1A. Risk Factors

PART I - FINANCIAL INFORMATION
Item 1. Financial Statements
Balance sheet and income statement text for the quarter.
Item 2. MD&A
Discussion of the quarter in some detail.
PART II - OTHER INFORMATION
Item 1. Legal Proceedings
No material legal proceedings this quarter.
Item 1A. Risk Factors
No material changes to risk factors.
";
        let s = split_sections(text, "10-Q");
        let titles: Vec<&str> = s.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(
            titles,
            vec![
                "Cover",
                "Part I, Item 1. Financial Statements",
                "Part I, Item 2. MD&A",
                "Part II, Item 1. Legal Proceedings",
                "Part II, Item 1A. Risk Factors",
            ]
        );
        assert_eq!(s[3].text, "No material legal proceedings this quarter.");
        // The PART II heading opens the next section; it doesn't end this one.
        assert_eq!(s[2].text, "Discussion of the quarter in some detail.");
    }

    #[test]
    fn non_periodic_forms_are_one_section() {
        let s = split_sections("Item 2.02 Results of Operations\nText.", "8-K");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].title, "Document");
        assert_eq!(s[0].text, "Item 2.02 Results of Operations\nText.");
    }

    #[test]
    fn periodic_without_headings_is_one_section() {
        let s = split_sections("Just text.\n\n\n\nMore text.", "10-K/A");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].text, "Just text.\n\nMore text.");
    }

    #[test]
    fn classifies_document_kinds() {
        assert_eq!(doc_kind("exmp-20250930.htm"), Some(DocKind::Html));
        assert_eq!(doc_kind("xslF345X05/form4.xml"), Some(DocKind::Html));
        assert_eq!(doc_kind("0001193125-15-118890.txt"), Some(DocKind::Text));
        assert_eq!(doc_kind("scan.pdf"), None);
    }

    #[test]
    fn strips_inline_xbrl_header_case_insensitively() {
        let html = "<p>a</p><IX:HEADER><x>hidden</x></IX:HEADER><p>b</p><ix:header>open";
        assert_eq!(strip_ix_header(html), "<p>a</p><p>b</p><ix:header>open");
    }

    #[test]
    fn plain_text_documents_pass_through() {
        assert_eq!(
            render_text(b"line 1\nline 2", DocKind::Text).unwrap(),
            "line 1\nline 2"
        );
    }
}

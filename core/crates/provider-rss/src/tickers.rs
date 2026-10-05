//! Ticker extraction from press-release text and feed categories.
//!
//! Press releases cite their listing as `(NASDAQ: AAPL)`, `(NYSE American:
//! XYZ)`, `(TSX: ABC, OTCQX: ABCDF)` and so on. We only extract symbols that
//! follow a recognised exchange label and a colon; bare parentheses such as
//! `(PZZA)` or `(the "Company")` are never treated as tickers.
//!
//! Output format:
//! - US venues (NYSE family, Nasdaq tiers, Cboe/BATS, OTC tiers): the plain
//!   symbol, e.g. `AAPL`, `BRK.B`.
//! - Other venues: `SYMBOL:VENUE`, e.g. `DSV:TSX`. Company-scope filtering
//!   only ever matches plain (US) symbols.
//!
//! Symbols are upper-case, 1–6 letters plus an optional `.X`/`.XX` class
//! suffix; `-` and `/` class separators are normalised to `.`.

use std::sync::LazyLock;

use regex::Regex;

/// Exchange labels recognised in free text, longest variants first. Matched
/// case-insensitively and only when followed by a colon.
const TEXT_LABELS: &str = concat!(
    r"NYSE\s+American|NYSE\s+MKT|NYSE\s+Amex|NYSE\s+Arca|NYSE\s+Texas|NYSE\s+AM|NYSE|AMEX|",
    r"Nasdaq\s+Global\s+Select\s+Market|Nasdaq\s+Global\s+Market|Nasdaq\s+Capital\s+Market|",
    r"Nasdaq\s+Stock\s+Market|Nasdaq\s*GS|Nasdaq\s*GM|Nasdaq\s*CM|Nasdaq|",
    r"Cboe\s+Canada|Cboe\s+BZX|Cboe|BATS|",
    r"Other\s+OTC|OTC\s+Markets|OTC\s+Pink|OTC\s*BB|OTC\s+US|OTCQB|OTCQX|OTCMKTS|OTCPK|OTCID|OTC|",
    r"TSX\s+Venture\s+Exchange|TSX\s+Venture|TSX-V|TSXV|TSX|CSE|NEO|LSE|LN|AIM|ASX|SIX|XETRA|",
    r"Euronext(?:\s+Growth)?(?:\s+(?:Paris|Brussels|Amsterdam|Dublin|Lisbon|Milan|Oslo))?",
);

// The three patterns below are string literals checked by the unit tests, so
// `Regex::new` cannot fail at runtime.
static LABEL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"(?i)\b({TEXT_LABELS})\s*:\s*")).expect("valid label regex literal")
});

/// A symbol at the start of the remaining text, followed by a non-alphanumeric
/// character or the end. Case-sensitive on purpose: prose words are not
/// all-caps.
static SYMBOL_AT_START_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([A-Z]{1,6}(?:[.\-][A-Z]{1,2})?)(?:[^A-Za-z0-9]|$)")
        .expect("valid symbol regex literal")
});

/// Separator between symbols listed under one exchange: `,`, `&`, `/`, `and`.
static SEPARATOR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:,|&|/|and\s)\s*").expect("valid separator regex literal"));

/// A full-string symbol check used for structured (category) data.
static SYMBOL_EXACT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Z]{1,6}(?:[.\-/][A-Z]{1,2})?$").expect("valid symbol regex literal")
});

#[derive(Debug, Clone, PartialEq, Eq)]
enum Venue {
    Us,
    Foreign(String),
}

/// Maps an exchange label (as written in text or a feed category) to a venue.
/// Unknown labels are kept verbatim (upper-case, spaces as `-`) as a foreign
/// venue; they never match a US company query.
fn venue(label: &str) -> Venue {
    let norm = label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_uppercase();
    match norm.as_str() {
        "NYSE"
        | "NYSE AMERICAN"
        | "NYSE MKT"
        | "NYSE AMEX"
        | "NYSE AM"
        | "NYSE ARCA"
        | "NYSE TEXAS"
        | "AMEX"
        | "NASDAQ"
        | "NASDAQ GS"
        | "NASDAQ GM"
        | "NASDAQ CM"
        | "NASDAQGS"
        | "NASDAQGM"
        | "NASDAQCM"
        | "NASDAQ GLOBAL SELECT MARKET"
        | "NASDAQ GLOBAL MARKET"
        | "NASDAQ CAPITAL MARKET"
        | "NASDAQ STOCK MARKET"
        | "CBOE"
        | "CBOE BZX"
        | "BATS"
        | "OTC"
        | "OTHER OTC"
        | "OTC MARKETS"
        | "OTC PINK"
        | "OTCBB"
        | "OTC BB"
        | "OTC US"
        | "OTCQB"
        | "OTCQX"
        | "OTCMKTS"
        | "OTCPK"
        | "OTCID" => Venue::Us,
        "TSX VENTURE EXCHANGE" | "TSX VENTURE" | "TSX-V" | "TSXV" => Venue::Foreign("TSXV".into()),
        "TSX" | "TORONTO STOCK EXCHANGE" => Venue::Foreign("TSX".into()),
        "CBOE CANADA" | "NEO" => Venue::Foreign("NEO".into()),
        "LSE" | "LN" | "LONDON STOCK EXCHANGE" => Venue::Foreign("LSE".into()),
        n if n.starts_with("EURONEXT") => Venue::Foreign("EURONEXT".into()),
        _ => Venue::Foreign(norm.replace(' ', "-")),
    }
}

/// Normalises a symbol for comparison: upper-case, class separator `.`.
pub(crate) fn normalize_symbol(symbol: &str) -> String {
    symbol.trim().to_ascii_uppercase().replace(['-', '/'], ".")
}

fn format_ticker(symbol: &str, venue: &Venue) -> String {
    let sym = normalize_symbol(symbol);
    match venue {
        Venue::Us => sym,
        Venue::Foreign(code) => format!("{sym}:{code}"),
    }
}

/// Appends `ticker` unless already present (keeps first-seen order).
pub(crate) fn push_ticker(out: &mut Vec<String>, ticker: String) {
    if !out.contains(&ticker) {
        out.push(ticker);
    }
}

/// Appends every `EXCHANGE: SYMBOL[, SYMBOL…]` mention in `text` to `out`.
pub(crate) fn extract_from_text(text: &str, out: &mut Vec<String>) {
    for caps in LABEL_RE.captures_iter(text) {
        let (Some(whole), Some(label)) = (caps.get(0), caps.get(1)) else {
            continue;
        };
        let venue = venue(label.as_str());
        let mut rest = &text[whole.end()..];
        let mut first = true;
        loop {
            if !first {
                match SEPARATOR_RE.find(rest) {
                    Some(m) => rest = &rest[m.end()..],
                    None => break,
                }
            }
            let Some(sym) = SYMBOL_AT_START_RE.captures(rest).and_then(|c| c.get(1)) else {
                break;
            };
            let after = &rest[sym.end()..];
            // `(NASDAQ: ABC, TSX: ABC)`: the token before a colon is the next
            // exchange label, which the outer loop handles.
            if !first && after.trim_start().starts_with(':') {
                break;
            }
            push_ticker(out, format_ticker(sym.as_str(), &venue));
            rest = after;
            first = false;
        }
    }
}

/// Parses a structured `EXCHANGE:SYMBOL` category value (GlobeNewswire's
/// `<category domain=".../rss/stock">Nasdaq:PGC</category>`).
pub(crate) fn from_stock_category(value: &str) -> Option<String> {
    let (label, symbol) = value.split_once(':')?;
    let symbol = symbol.trim().to_ascii_uppercase();
    let label = label.trim();
    if label.is_empty() || !SYMBOL_EXACT_RE.is_match(&symbol) {
        return None;
    }
    Some(format_ticker(&symbol, &venue(label)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        extract_from_text(text, &mut out);
        out
    }

    #[test]
    fn regexes_compile() {
        LazyLock::force(&LABEL_RE);
        LazyLock::force(&SYMBOL_AT_START_RE);
        LazyLock::force(&SEPARATOR_RE);
        LazyLock::force(&SYMBOL_EXACT_RE);
    }

    #[test]
    fn text_extraction_table() {
        let cases: &[(&str, &[&str])] = &[
            ("Apple Inc. (NASDAQ: AAPL) today announced", &["AAPL"]),
            ("IBM (NYSE: IBM) reported", &["IBM"]),
            ("Acme (NYSE American: XYZ) said", &["XYZ"]),
            ("Acme (NYSE AMERICAN: XYZ) said", &["XYZ"]),
            ("Small Co (Nasdaq: ABCD) said", &["ABCD"]),
            ("Tiny Co (OTCQB: ABCD) said", &["ABCD"]),
            ("Tiny Co (OTC: ABCDF) said", &["ABCDF"]),
            ("Maple Co (TSX: MAPL) said", &["MAPL:TSX"]),
            ("listed as NYSE:IBM in the index", &["IBM"]),
            ("SPAC Corp (NASDAQ: AAPL, AAPLW) units", &["AAPL", "AAPLW"]),
            ("Berkshire (NYSE: BRK.A, BRK.B) said", &["BRK.A", "BRK.B"]),
            ("Berkshire (NYSE: BRK-B) said", &["BRK.B"]),
            ("Foo Inc (Nasdaq GS: ABC) said", &["ABC"]),
            (
                "Navan, Inc. (NasdaqGS: NAVN).Navan, headquartered",
                &["NAVN"],
            ),
            ("Peapack (NASDAQ Global Select Market: PGC) and", &["PGC"]),
            ("Dual (NASDAQ: ABC; TSX: ABC) listed", &["ABC", "ABC:TSX"]),
            (
                "Discovery Mining Ltd. (TSX: DSV, OTCQX: DSVSF) today",
                &["DSV:TSX", "DSVSF"],
            ),
            (
                "Venture (TSX-V: MMY) and (TSXV: MMZ)",
                &["MMY:TSXV", "MMZ:TSXV"],
            ),
            ("Corp (NYSE:DEI) and (OTCMKTS: APTL)", &["DEI", "APTL"]),
            (
                "Société (Euronext Growth Paris: ALXYZ) and (SIX: ROG)",
                &["ALXYZ:EURONEXT", "ROG:SIX"],
            ),
            ("Plc (LN: ABC) (XETRA: DEF)", &["ABC:LSE", "DEF:XETRA"]),
            ("Dual-listed (TSX and NYSE: XYZ)", &["XYZ"]),
            ("Agilent (NYSE: A) said", &["A"]),
            ("Two (NYSE: ABC and XYZ) classes", &["ABC", "XYZ"]),
        ];
        for (text, want) in cases {
            assert_eq!(
                extract(text),
                want.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
                "input: {text}"
            );
        }
    }

    #[test]
    fn text_extraction_negatives() {
        let none: &[&str] = &[
            r#"Acme Corp. (the "Company") announced"#,
            "Papa John's International, Inc. (PZZA) Shareholders",
            "Ticker: ABC is not an exchange label",
            "NYSE-listed company files report",
            "The Nasdaq Composite closed at 27,190.86",
            "listed on the NYSE: The company said",
            "Nasdaq Stockholm: ERIC B",
            "(NASDAQ: ABCDEFG) too long",
            "(NYSE: abc) lower case",
            "Visa (رمزها في بورصة نيويورك: V)",
            "Contact: John Smith, IR",
        ];
        for text in none {
            assert!(
                extract(text).is_empty(),
                "unexpected tickers in {text:?}: {:?}",
                extract(text)
            );
        }
    }

    #[test]
    fn deduplicates_and_keeps_order() {
        assert_eq!(
            extract("(NYSE: IBM) ... (NYSE: IBM) ... (NASDAQ: AAPL)"),
            vec!["IBM", "AAPL"]
        );
    }

    #[test]
    fn stock_categories() {
        assert_eq!(from_stock_category("Nasdaq:PGC").as_deref(), Some("PGC"));
        assert_eq!(from_stock_category("NYSE:LYB").as_deref(), Some("LYB"));
        assert_eq!(from_stock_category("Nasdaq: NBTX").as_deref(), Some("NBTX"));
        assert_eq!(
            from_stock_category("Other OTC:DSVSF").as_deref(),
            Some("DSVSF")
        );
        assert_eq!(
            from_stock_category("OTC Markets:WORXD").as_deref(),
            Some("WORXD")
        );
        assert_eq!(from_stock_category("TSX:DSV").as_deref(), Some("DSV:TSX"));
        assert_eq!(
            from_stock_category("TSX-V:MMY").as_deref(),
            Some("MMY:TSXV")
        );
        assert_eq!(
            from_stock_category("Paris:BVI").as_deref(),
            Some("BVI:PARIS")
        );
        assert_eq!(
            from_stock_category("Irish:IRSH").as_deref(),
            Some("IRSH:IRISH")
        );
        assert_eq!(from_stock_category("Oslo:NHY").as_deref(), Some("NHY:OSLO"));
        // Not sane symbols: digits, spaces, empty.
        assert_eq!(from_stock_category("Oslo:NHY01"), None);
        assert_eq!(from_stock_category("Iceland:LBANK CBI 22"), None);
        assert_eq!(from_stock_category("Frankfurt:D7Q1.F"), None);
        assert_eq!(from_stock_category("US7046991078"), None);
        assert_eq!(from_stock_category(":ABC"), None);
    }

    #[test]
    fn normalizes_class_separators() {
        assert_eq!(normalize_symbol("brk-b"), "BRK.B");
        assert_eq!(normalize_symbol("BRK/B"), "BRK.B");
        assert_eq!(normalize_symbol(" BRK.B "), "BRK.B");
    }
}

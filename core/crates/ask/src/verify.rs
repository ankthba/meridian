//! Number verifier.
//!
//! After an answer is final, every numeric token in it is checked against
//! the numbers in that question's tool results. A token is VERIFIED when
//! some result number matches it within the rounding tolerance implied by
//! how the token is displayed (`12.3` → ±0.05), after normalizing scale
//! suffixes (`1.2B` → 1.2e9) and allowing percent↔fraction (`12.3%` ↔
//! 0.123). Unmatched tokens are shown as UNVERIFIED rather than hidden.
//!
//! Not checked, by rule: standalone years (1900–2100), dates and times,
//! numbers that are part of identifiers (`Q3`, `FY2025`, `3Q25`, `10-K`,
//! `S&P 500`, `7203 JP Equity`, `1st`), hyphenated compounds (`30-day`),
//! lookback windows (`last 30 days`), citation markers (`[1]`), and list
//! markers (`1.` at the start of a line).
//!
//! Signs: a token written with an explicit sign (`-3.2%`, `+1.1`) must match
//! a result of the same sign. An unsigned token (`fell 3.2%`) or an
//! accounting negative (`(3.2)`) matches by magnitude, since the direction
//! is carried by words or layout rather than by the number.

use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::tools::ToolAudit;

/// The verdict for one numeric token in the answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NumberCheck {
    /// The token as displayed, e.g. `$1.2B`, `-3.2%`, `(4.1)`.
    pub text: String,
    /// Normalized value: scale applied (`1.2B` → 1.2e9), percent and
    /// percentage points as fractions (`12.3%` → 0.123), basis points as
    /// fractions (`25bp` → 0.0025), accounting parentheses negative.
    pub value: f64,
    /// Byte offsets of `text` in the answer, `[start, end)`.
    pub span: (usize, usize),
    pub verified: bool,
    /// `tool_use` id of the first tool call (in call order) whose result
    /// contains a matching number.
    pub matched_tool_call: Option<String>,
}

/// Checks every numeric token in `answer` against the numbers in the
/// successful tool calls. Error results are not evidence.
#[must_use]
pub fn verify_numbers(answer: &str, tool_calls: &[ToolAudit]) -> Vec<NumberCheck> {
    scan(answer)
        .into_iter()
        .map(|t| {
            let matched =
                tool_calls.iter().filter(|c| !c.is_error).find(|c| c.numbers.iter().any(|&n| t.matches(n)));
            NumberCheck {
                text: answer[t.span.clone()].to_owned(),
                value: t.value(),
                span: (t.span.start, t.span.end),
                verified: matched.is_some(),
                matched_tool_call: matched.map(|c| c.tool_use_id.clone()),
            }
        })
        .collect()
}

/// Tool-side extraction: pushes each token's displayed (signed) mantissa
/// and its normalized value.
pub(crate) fn numbers_in_text(s: &str, out: &mut Vec<f64>) {
    for t in scan(s) {
        let shown = t.signed(t.mantissa);
        out.push(shown);
        let v = t.value();
        if v != shown {
            out.push(v);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Plain,
    Percent,
    PercentagePoints,
    BasisPoints,
    Multiple,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sign {
    None,
    Neg,
    Pos,
}

#[derive(Debug, Clone)]
struct Token {
    span: Range<usize>,
    /// Absolute displayed value before scale.
    mantissa: f64,
    decimals: i32,
    sign: Sign,
    paren_negative: bool,
    scale: f64,
    unit: Unit,
}

impl Token {
    fn signed(&self, x: f64) -> f64 {
        if self.sign == Sign::Neg || self.paren_negative { -x } else { x }
    }

    fn value(&self) -> f64 {
        let base = match self.unit {
            Unit::Percent | Unit::PercentagePoints => self.mantissa / 100.0,
            Unit::BasisPoints => self.mantissa / 10_000.0,
            Unit::Plain | Unit::Multiple => self.mantissa * self.scale,
        };
        self.signed(base)
    }

    /// `(magnitude, absolute tolerance)` pairs the token may stand for.
    fn candidates(&self) -> Vec<(f64, f64)> {
        let m = self.mantissa;
        let half = 0.5 * 10f64.powi(-self.decimals);
        match self.unit {
            Unit::Plain if self.scale != 1.0 => {
                let s = self.scale;
                let mut c = vec![(m * s, half * s), (m, half)];
                // Statement data is often stored in thousands or millions.
                for d in [1e3, 1e6] {
                    if s / d > 1.0 {
                        c.push((m * s / d, half * s / d));
                    }
                }
                c
            }
            Unit::Plain | Unit::Multiple => vec![(m, half)],
            Unit::Percent | Unit::PercentagePoints => vec![(m, half), (m / 100.0, half / 100.0)],
            Unit::BasisPoints => vec![(m, half), (m / 100.0, half / 100.0), (m / 10_000.0, half / 10_000.0)],
        }
    }

    fn matches(&self, n: f64) -> bool {
        if !n.is_finite() {
            return false;
        }
        self.candidates().into_iter().any(|(c, tol)| {
            let tol = tol * (1.0 + 1e-9) + c.abs() * 1e-12;
            match self.sign {
                Sign::Neg => (n + c).abs() <= tol,
                Sign::Pos => (n - c).abs() <= tol,
                Sign::None => (n.abs() - c).abs() <= tol,
            }
        })
    }
}

// The patterns below are constants covered by the unit tests, so compiling
// them cannot fail at runtime.
fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("constant regex is valid")
}

static NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r"(?P<open>\()?",
        r"(?P<sign>[-+\x{2212}])?",
        r"(?P<cur>(?:US|C|A|HK|NZ|S)?[$\x{20AC}\x{00A3}\x{00A5}])?",
        r"(?P<sign2>[-+\x{2212}])?",
        r"(?P<num>[0-9]{1,3}(?:,[0-9]{3})+(?:\.[0-9]+)?|[0-9]+(?:\.[0-9]+)?|\.[0-9]+)",
        r"(?P<suffix>",
        r"\s?(?i:thousand|million|billion|trillion)\b",
        r"|\s?(?i:bps|bp)\b",
        r"|(?i:ppt|pp)\b",
        r"|%",
        r"|(?:MM|mm|Mn|mn|MN|bn|Bn|BN|tn|Tn|TN|tr|Tr|[KkMmBbTt])\b",
        r"|[xX]\b|\x{00D7}",
        r")?",
        r"(?P<close>\))?",
    ))
});

const MONTH: &str = r"(?:Jan(?:uary)?|Feb(?:ruary)?|Mar(?:ch)?|Apr(?:il)?|May|June?|July?|Aug(?:ust)?|Sep(?:t(?:ember)?)?|Oct(?:ober)?|Nov(?:ember)?|Dec(?:ember)?)";

/// Spans whose digits are never data.
static MASKS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // ISO dates and timestamps.
        re(r"\b[0-9]{4}-[0-9]{2}-[0-9]{2}(?:[T ][0-9]{2}:[0-9]{2}(?::[0-9]{2}(?:\.[0-9]+)?)?(?:Z|[+-][0-9]{2}:?[0-9]{2})?)?"),
        re(r"\b[0-9]{4}/[0-9]{1,2}/[0-9]{1,2}\b"),
        re(r"\b[0-9]{1,2}/[0-9]{1,2}(?:/[0-9]{2,4})?\b"),
        // Month-name dates: Oct 5, October 5th, 2026, 5 Oct 2026, Oct '26.
        re(&format!(r"(?i)\b{MONTH}\.?\s+[0-9]{{1,2}}(?:st|nd|rd|th)?\b(?:,?\s+[0-9]{{4}}\b)?")),
        re(&format!(r"(?i)\b[0-9]{{1,2}}(?:st|nd|rd|th)?\s+{MONTH}\b\.?(?:,?\s+[0-9]{{4}}\b)?")),
        re(&format!(r"(?i)\b{MONTH}\.?\s*['\x{{2019}}-]\s*[0-9]{{2}}\b")),
        // Clock times.
        re(r"\b[0-9]{1,2}:[0-9]{2}(?::[0-9]{2})?(?:\s?[AaPp]\.?[Mm]\.?)?"),
        // Citation markers [1], [1, 2], [1-3].
        re(r"\[[0-9]+(?:\s*[,\x{2013}-]\s*[0-9]+)*\]"),
        // List markers at the start of a line: `1.` / `2)`.
        re(r"(?m)^[ \t]*(?:[-*+][ \t]+)?[0-9]{1,3}[.)](?:[ \t]|$)"),
        // Index names.
        re(
            r"(?i)\b(?:S&P|Russell|Nasdaq|FTSE|Euro\s+Stoxx|Stoxx|Nikkei|DAX|CAC|Dow(?:\s+Jones)?|Hang\s+Seng|ASX|TSX|Wilshire)\s*[0-9]+\b",
        ),
        // Numeric security keys: `7203 JP Equity`.
        re(r"\b[0-9]{3,6}\s+[A-Z]{2}\s+(?:Equity|Index|Curncy|Comdty|Corp|Govt|Pfd|Muni|Mtge)\b"),
        // Lookback windows: `last 30 days`, `over the past 5 years`.
        re(
            r"(?i)\b(?:last|past|next|trailing|prior|previous|over|within)\s+(?:the\s+)?(?:last\s+|past\s+|next\s+|trailing\s+|prior\s+)?[0-9]{1,4}\s+(?:trading\s+|calendar\s+|business\s+)?(?:days?|weeks?|months?|quarters?|years?|sessions?|hours?|minutes?|yrs?|mos?)\b",
        ),
    ]
});

fn suffix_kind(s: &str) -> (f64, Unit) {
    let t = s.trim();
    match t {
        "%" => (1.0, Unit::Percent),
        "x" | "X" | "\u{00D7}" => (1.0, Unit::Multiple),
        "" => (1.0, Unit::Plain),
        _ => match t.to_ascii_lowercase().as_str() {
            "pp" | "ppt" => (1.0, Unit::PercentagePoints),
            "bp" | "bps" => (1.0, Unit::BasisPoints),
            "k" | "thousand" => (1e3, Unit::Plain),
            "m" | "mm" | "mn" | "million" => (1e6, Unit::Plain),
            "b" | "bn" | "billion" => (1e9, Unit::Plain),
            "t" | "tn" | "tr" | "trillion" => (1e12, Unit::Plain),
            _ => (1.0, Unit::Plain),
        },
    }
}

fn char_before(s: &str, i: usize) -> Option<char> {
    s[..i].chars().next_back()
}

fn chars_after(s: &str, i: usize) -> (Option<char>, Option<char>) {
    let mut it = s[i..].chars();
    (it.next(), it.next())
}

fn is_word(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

/// Finds checkable numeric tokens, applying every exclusion rule.
fn scan(text: &str) -> Vec<Token> {
    let masks: Vec<Range<usize>> =
        MASKS.iter().flat_map(|r| r.find_iter(text).map(|m| m.range()).collect::<Vec<_>>()).collect();
    let mut out = Vec::new();
    for caps in NUMBER.captures_iter(text) {
        let (Some(all), Some(num)) = (caps.get(0), caps.name("num")) else { continue };
        let open = caps.name("open");
        let close = caps.name("close");
        let cur = caps.name("cur");
        let suffix = caps.name("suffix");
        let mut start = all.start();
        let mut end = all.end();
        let paren_negative = open.is_some() && close.is_some();
        if let (Some(o), None) = (open, close) {
            start = o.end();
        }
        if let (None, Some(c)) = (open, close) {
            end = c.start();
        }

        let mut sign = match caps.name("sign").or_else(|| caps.name("sign2")).map(|m| m.as_str()) {
            Some("-" | "\u{2212}") => Sign::Neg,
            Some("+") => Sign::Pos,
            _ => Sign::None,
        };
        // A leading `-` glued to the previous word or number is a hyphen or
        // a range dash, not a sign.
        if let Some(sm) = caps.name("sign").filter(|m| m.start() == start) {
            match char_before(text, start) {
                Some(c) if is_word(c) => continue, // COVID-19, S-1, T+1
                Some(c) if c.is_ascii_digit() || c == '.' => {
                    start = sm.end();
                    sign = Sign::None;
                }
                _ => {}
            }
        }

        // Part of an identifier or a malformed/version-like string.
        let before = char_before(text, start);
        if cur.is_none() && before.is_some_and(is_word) {
            continue; // Q3, FY2025, H1, x2
        }
        if before.is_some_and(|c| c.is_ascii_digit() || c == '.' || c == ',') {
            continue;
        }
        match chars_after(text, end) {
            (Some(c), _) if is_word(c) => continue, // 3Q25, 1st, 10Y, 13F
            (Some('.' | ','), Some(d)) if d.is_ascii_digit() => continue, // 1.2.3
            (Some('.'), Some(d)) if d.is_ascii_uppercase() => continue, // 7203.T
            (Some('-' | '\u{2013}'), Some(d)) if is_word(d) => continue, // 30-day, 10-K
            _ => {}
        }
        let core = num.range();
        if masks.iter().any(|m| m.start < core.end && core.start < m.end) {
            continue;
        }

        let raw = num.as_str();
        let Ok(mantissa) = raw.replace(',', "").parse::<f64>() else { continue };
        let decimals = raw.split_once('.').map_or(0, |(_, frac)| frac.len() as i32);
        let (scale, unit) = suffix.map_or((1.0, Unit::Plain), |s| suffix_kind(s.as_str()));

        // Standalone year.
        let bare = sign == Sign::None
            && !paren_negative
            && cur.is_none()
            && suffix.is_none()
            && raw.len() == 4
            && raw.bytes().all(|b| b.is_ascii_digit());
        if bare && (1900.0..=2100.0).contains(&mantissa) {
            continue;
        }

        out.push(Token { span: start..end, mantissa, decimals, sign, paren_negative, scale, unit });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, numbers: &[f64]) -> ToolAudit {
        ToolAudit { tool_use_id: id.into(), numbers: numbers.to_vec(), ..ToolAudit::default() }
    }

    /// `(text, value)` of every checked token.
    fn tokens(s: &str) -> Vec<(String, f64)> {
        verify_numbers(s, &[]).into_iter().map(|c| (c.text, c.value)).collect()
    }

    fn texts(s: &str) -> Vec<String> {
        tokens(s).into_iter().map(|(t, _)| t).collect()
    }

    fn verified(s: &str, numbers: &[f64]) -> Vec<bool> {
        verify_numbers(s, &[call("toolu_1", numbers)]).into_iter().map(|c| c.verified).collect()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * b.abs().max(1.0)
    }

    #[test]
    fn spans_index_the_answer() {
        let s = "AAPL last traded at $227.52, down 1.3% [1]; market cap \u{2248} $3.4T.";
        for c in verify_numbers(s, &[]) {
            assert_eq!(&s[c.span.0..c.span.1], c.text);
        }
        assert_eq!(texts(s), ["$227.52", "1.3%", "$3.4T"]);
    }

    #[test]
    fn currency_and_grouping() {
        let t = tokens("$1,234.56 and \u{20AC}12 and US$5.2B and 1,234,567");
        assert_eq!(t[0].0, "$1,234.56");
        assert!(close(t[0].1, 1234.56));
        assert!(close(t[1].1, 12.0));
        assert_eq!(t[2].0, "US$5.2B");
        assert!(close(t[2].1, 5.2e9));
        assert!(close(t[3].1, 1_234_567.0));
    }

    #[test]
    fn scale_suffixes_normalize() {
        let t = tokens("1.2B, 4.5 million, 300K, $2.1bn, 3.4T, 750mm, 12 thousand, 1.1 trillion");
        let want = [1.2e9, 4.5e6, 3e5, 2.1e9, 3.4e12, 7.5e8, 1.2e4, 1.1e12];
        assert_eq!(t.len(), want.len(), "{t:?}");
        for ((_, v), w) in t.iter().zip(want) {
            assert!(close(*v, w), "{v} vs {w}");
        }
    }

    #[test]
    fn percent_bps_pp_and_multiples() {
        let t = tokens("12.3% vs 25 bps and 1.5pp; P/E 28.4x");
        assert!(close(t[0].1, 0.123));
        assert!(close(t[1].1, 0.0025));
        assert!(close(t[2].1, 0.015));
        assert!(close(t[3].1, 28.4));
        assert_eq!(t[3].0, "28.4x");
    }

    #[test]
    fn signs_and_parentheses() {
        let t = tokens("change -3.2%, \u{2212}1.5, +0.8 and (4.1) and ($2.5M)");
        assert!(close(t[0].1, -0.032));
        assert!(close(t[1].1, -1.5));
        assert!(close(t[2].1, 0.8));
        assert_eq!(t[3].0, "(4.1)");
        assert!(close(t[3].1, -4.1));
        assert!(close(t[4].1, -2.5e6));
    }

    #[test]
    fn ranges_are_two_unsigned_numbers() {
        let t = tokens("guidance 3.2-4.5% and 10\u{2013}12");
        assert_eq!(t.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["3.2", "4.5%", "10", "12"]);
        assert!(close(t[1].1, 0.045));
    }

    #[test]
    fn open_paren_without_close_is_not_negative() {
        let t = tokens("revenue rose (12.3% of sales) and margin 5.1%)");
        assert_eq!(t[0].0, "12.3%");
        assert!(close(t[0].1, 0.123));
        assert_eq!(t[1].0, "5.1%");
    }

    #[test]
    fn excludes_years_and_dates() {
        assert!(texts("In 2025 and 2026 revenue rose; FY2025 and Q3 2025 and 1999.").is_empty());
        assert!(texts("as of 2026-10-05, 2026-10-05T14:30:00Z and 2026/10/05").is_empty());
        assert!(texts("on Oct 5, on October 5, 2026, on 5 Oct 2026, on Sept. 30th, Oct '26").is_empty());
        assert!(texts("on 10/05/2026 and 10/5, at 9:30 am and 16:00").is_empty());
        // Not a year: has a currency, decimals, a sign, or is out of range.
        assert_eq!(texts("$2025, 2025.5, -2025, 2101"), ["$2025", "2025.5", "-2025", "2101"]);
    }

    #[test]
    fn excludes_identifiers_and_labels() {
        assert!(texts("Q3 FY26, 3Q25, 1H26, H1, FY2025, CY24").is_empty());
        assert!(texts("the 10-K and 8-K and 13F and S-1 and DEF 14A").is_empty());
        assert!(texts("the 30-day and 52-week and 200-day measures, 10Y and 2s10s").is_empty());
        assert!(texts("S&P 500, Russell 2000, Nasdaq 100, FTSE 100, 7203 JP Equity, 0700.HK").is_empty());
        assert!(texts("1st, 2nd, 3rd, 4th; COVID-19; T+1; version 1.2.3").is_empty());
        assert!(texts("over the last 30 days and the past 5 years and trailing 12 months").is_empty());
    }

    #[test]
    fn excludes_citations_and_list_markers() {
        let s = "1. AAPL closed at 227.52 [1]\n2) MSFT at 415.10 [2][3]\n  3. see [1, 2]\n- 4. nested";
        assert_eq!(texts(s), ["227.52", "415.10"]);
    }

    #[test]
    fn list_marker_rule_keeps_leading_decimals() {
        assert_eq!(texts("1.5% is the yield"), ["1.5%"]);
    }

    #[test]
    fn table_cells_are_checked() {
        let s = "| SYM | LAST | CHG% |\n|---|---|---|\n| AAPL | 227.52 | -1.30% |\n| MSFT | 415.10 | +0.42% |";
        assert_eq!(texts(s), ["227.52", "-1.30%", "415.10", "+0.42%"]);
    }

    #[test]
    fn rounding_tolerance_follows_displayed_precision() {
        assert_eq!(verified("$1,234.56", &[1234.555]), [true]);
        assert_eq!(verified("$1,235", &[1234.56]), [true]);
        assert_eq!(verified("$1,200", &[1234.0]), [false]);
        assert_eq!(verified("227.5", &[227.52]), [true]);
        assert_eq!(verified("227.50", &[227.52]), [false]);
        assert_eq!(verified("0.5", &[0.46]), [true]);
    }

    #[test]
    fn percent_fraction_equivalence() {
        assert_eq!(verified("12.3%", &[0.1234]), [true]);
        assert_eq!(verified("12.3%", &[12.31]), [true]);
        assert_eq!(verified("12.3%", &[0.125]), [false]);
        assert_eq!(verified("12.35%", &[0.12345]), [true]);
        assert_eq!(verified("25bp", &[0.0025]), [true]);
        assert_eq!(verified("25 bps", &[0.25]), [true]);
    }

    #[test]
    fn scale_equivalence() {
        assert_eq!(verified("1.2B", &[1_234_000_000.0]), [true]);
        assert_eq!(verified("1.2B", &[1_260_000_000.0]), [false]);
        assert_eq!(verified("$1.2 billion", &[1.2]), [true]);
        assert_eq!(verified("$1.2B", &[1_200.0]), [true]); // statement in millions
        assert_eq!(verified("4.5 million", &[4_500_000.0]), [true]);
        assert_eq!(verified("$3.4T", &[3.41e12]), [true]);
    }

    #[test]
    fn sign_rules() {
        assert_eq!(verified("-3.2%", &[-3.2]), [true]);
        assert_eq!(verified("-3.2%", &[3.2]), [false]);
        assert_eq!(verified("+3.2%", &[-0.032]), [false]);
        assert_eq!(verified("fell 3.2%", &[-0.032]), [true]);
        assert_eq!(verified("(3.2)", &[-3.2]), [true]);
        assert_eq!(verified("(3.2)", &[3.2]), [true]);
    }

    #[test]
    fn matched_call_is_first_in_call_order_and_errors_are_ignored() {
        let mut err = call("toolu_err", &[42.0]);
        err.is_error = true;
        let calls = [err, call("toolu_a", &[1.0]), call("toolu_b", &[42.0]), call("toolu_c", &[42.0])];
        let checks = verify_numbers("42 and 7", &calls);
        assert_eq!(checks[0].matched_tool_call.as_deref(), Some("toolu_b"));
        assert!(checks[0].verified);
        assert!(!checks[1].verified);
        assert_eq!(checks[1].matched_tool_call, None);
    }

    #[test]
    fn non_finite_tool_numbers_never_match() {
        assert_eq!(verified("5", &[f64::NAN, f64::INFINITY]), [false]);
    }

    #[test]
    fn realistic_answer() {
        let answer = "AAPL closed at $227.52 on Oct 2 [1], down 1.3% on the day and up 18.4% YTD [2]. \
                      Q3 FY2025 revenue was $94.9B (+6.1% y/y) [3]. Over the last 30 days the stock \
                      underperformed the S&P 500 by 2.1pp [4].";
        let calls = [
            call("toolu_q", &[227.52, -1.3, -0.013]),
            call("toolu_c", &[0.1843]),
            call("toolu_f", &[94_930_000_000.0, 0.061]),
            call("toolu_r", &[-0.0207]),
        ];
        let checks = verify_numbers(answer, &calls);
        let summary: Vec<(&str, bool)> = checks.iter().map(|c| (c.text.as_str(), c.verified)).collect();
        assert_eq!(
            summary,
            [("$227.52", true), ("1.3%", true), ("18.4%", true), ("$94.9B", true), ("+6.1%", true), ("2.1pp", true)]
        );
        assert_eq!(checks[3].matched_tool_call.as_deref(), Some("toolu_f"));
    }

    #[test]
    fn fabricated_number_is_unverified() {
        let checks = verify_numbers("The P/E is 31.7x.", &[call("toolu_q", &[227.52])]);
        assert_eq!(checks.len(), 1);
        assert!(!checks[0].verified);
    }

    #[test]
    fn tool_side_text_extraction() {
        let mut out = Vec::new();
        numbers_in_text("EPS $1.64 vs est. 1.60; shares -2.5%; filed 2026-08-01; FY2025", &mut out);
        for want in [1.64, 1.60, -2.5, -0.025] {
            assert!(out.iter().any(|x| close(*x, want)), "{want} missing from {out:?}");
        }
        assert!(!out.iter().any(|x| close(*x, 2026.0) || close(*x, 2025.0)));
    }
}

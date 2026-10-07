//! A small, lenient RFC 4180 reader.
//!
//! Broker exports are mostly well-formed, but they put line breaks inside
//! quoted fields (Robinhood descriptions), wrap the table in preamble and
//! footer lines with a different column count, and sometimes end rows with
//! a trailing comma. The reader keeps every record with the 1-based line
//! it starts on so warnings can point at the file.

/// One CSV record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// 1-based line number where the record starts.
    pub line: u32,
    pub fields: Vec<String>,
    /// The record ended inside an unterminated quoted field.
    pub unterminated: bool,
}

impl Record {
    /// Whether every field is empty after trimming.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.fields.iter().all(|f| f.trim().is_empty())
    }

    /// Field `i`, trimmed; empty when the record is shorter.
    #[must_use]
    pub fn get(&self, i: usize) -> &str {
        self.fields.get(i).map_or("", |s| s.trim())
    }

    /// Non-empty fields.
    #[must_use]
    pub fn filled(&self) -> usize {
        self.fields.iter().filter(|f| !f.trim().is_empty()).count()
    }

    /// The record re-joined for warning messages (truncated).
    #[must_use]
    pub fn excerpt(&self) -> String {
        let joined = self.fields.iter().map(|f| f.trim().replace('\n', " ")).collect::<Vec<_>>().join(",");
        let joined = joined.trim_end_matches(',').to_owned();
        if joined.chars().count() > 160 {
            let cut: String = joined.chars().take(157).collect();
            format!("{cut}...")
        } else {
            joined
        }
    }
}

/// Chooses the delimiter from the first non-empty line: comma unless that
/// line has none and has tabs or semicolons instead.
#[must_use]
pub fn sniff_delimiter(text: &str) -> char {
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    if first.contains(',') {
        ','
    } else if first.contains('\t') {
        '\t'
    } else if first.contains(';') {
        ';'
    } else {
        ','
    }
}

/// Parses `text` into records. A leading UTF-8 byte-order mark is skipped.
/// Line endings may be `\n`, `\r\n` or `\r`.
#[must_use]
pub fn read(text: &str, delimiter: char) -> Vec<Record> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out = Vec::new();
    let mut fields: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    // The current field began with a quote (so a later quote may close it).
    let mut quoted = false;
    let mut line: u32 = 1;
    let mut start_line: u32 = 1;
    let mut chars = text.chars().peekable();
    let mut any = false;

    while let Some(c) = chars.next() {
        any = true;
        if in_quotes {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => in_quotes = false,
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    line += 1;
                    field.push('\n');
                }
                '\n' => {
                    line += 1;
                    field.push('\n');
                }
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() && !quoted => {
                in_quotes = true;
                quoted = true;
            }
            c if c == delimiter => {
                fields.push(std::mem::take(&mut field));
                quoted = false;
            }
            '\r' | '\n' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                fields.push(std::mem::take(&mut field));
                out.push(Record { line: start_line, fields: std::mem::take(&mut fields), unterminated: false });
                quoted = false;
                line += 1;
                start_line = line;
                any = false;
            }
            // Text after a closing quote, or a stray quote inside an
            // unquoted field: keep it literally.
            _ => field.push(c),
        }
    }
    if any || in_quotes || !fields.is_empty() || !field.is_empty() {
        fields.push(field);
        out.push(Record { line: start_line, fields, unterminated: in_quotes });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(r: &Record) -> Vec<&str> {
        r.fields.iter().map(String::as_str).collect()
    }

    #[test]
    fn plain_rows_and_line_numbers() {
        let r = read("a,b,c\n1,2,3\n", ',');
        assert_eq!(r.len(), 2);
        assert_eq!(f(&r[0]), ["a", "b", "c"]);
        assert_eq!(r[1].line, 2);
    }

    #[test]
    fn quotes_commas_escapes_and_newlines() {
        let text = "\"x\",\"a, b\",\"say \"\"hi\"\"\"\r\n\"multi\nline\",2,3\r\nlast,row,here";
        let r = read(text, ',');
        assert_eq!(r.len(), 3);
        assert_eq!(f(&r[0]), ["x", "a, b", "say \"hi\""]);
        assert_eq!(f(&r[1]), ["multi\nline", "2", "3"]);
        assert_eq!(r[1].line, 2);
        // The quoted newline pushes the next record to line 4.
        assert_eq!(r[2].line, 4);
        assert_eq!(f(&r[2]), ["last", "row", "here"]);
    }

    #[test]
    fn bom_blank_lines_trailing_comma_and_cr_only() {
        let r = read("\u{feff}\r\rh1,h2,\rv1,v2,", ',');
        assert_eq!(r.len(), 4);
        assert!(r[0].is_blank() && r[1].is_blank());
        assert_eq!(f(&r[2]), ["h1", "h2", ""]);
        assert_eq!(r[3].line, 4);
    }

    #[test]
    fn unterminated_quote_is_flagged() {
        let r = read("a,\"open\nnever closed", ',');
        assert_eq!(r.len(), 1);
        assert!(r[0].unterminated);
        assert_eq!(r[0].fields[1], "open\nnever closed");
    }

    #[test]
    fn stray_quotes_are_literal() {
        let r = read("5\" pipe,\"ok\"x\n", ',');
        assert_eq!(f(&r[0]), ["5\" pipe", "okx"]);
    }

    #[test]
    fn delimiter_sniffing() {
        assert_eq!(sniff_delimiter("\n\na\tb\tc\n"), '\t');
        assert_eq!(sniff_delimiter("a;b;c"), ';');
        assert_eq!(sniff_delimiter("a,b;c"), ',');
        assert_eq!(read("a\tb\n", '\t')[0].fields, ["a", "b"]);
    }

    #[test]
    fn excerpt_is_trimmed_and_bounded() {
        let r = read(&format!("x,{},,\n", "y".repeat(300)), ',');
        let e = r[0].excerpt();
        assert!(e.starts_with("x,yyy") && e.ends_with("...") && e.chars().count() == 160);
    }
}

//! Incremental Server-Sent Events parser.
//!
//! Implements the line/field rules of the WHATWG `text/event-stream` format:
//! lines end in LF, CRLF, or a lone CR; a line starting with `:` is a
//! comment; `field:value` strips one leading space from the value; a blank
//! line dispatches the pending event if it has data. Bytes may arrive split
//! at any position (including inside a multi-byte UTF-8 sequence or between
//! the CR and LF of a CRLF), so the parser buffers until it has a full line.

/// One dispatched event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// The `event:` field, if the event had one.
    pub event: Option<String>,
    /// All `data:` lines joined with `\n`.
    pub data: String,
}

#[derive(Debug, Default)]
pub struct SseParser {
    buf: Vec<u8>,
    /// The previous chunk ended in CR; a leading LF in the next chunk belongs
    /// to that line terminator.
    pending_cr: bool,
    event: Option<String>,
    data: String,
    has_data: bool,
}

impl SseParser {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds a chunk and returns every event it completes, in order.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        let mut out = Vec::new();
        let mut bytes = chunk;
        if self.pending_cr && !bytes.is_empty() {
            self.pending_cr = false;
            if let Some((&b'\n', rest)) = bytes.split_first() {
                bytes = rest;
            }
        }
        let mut start = 0;
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\n' => {
                    self.line(&bytes[start..i], &mut out);
                    i += 1;
                    start = i;
                }
                b'\r' => {
                    self.line(&bytes[start..i], &mut out);
                    i += 1;
                    if i == bytes.len() {
                        self.pending_cr = true;
                    } else if bytes[i] == b'\n' {
                        i += 1;
                    }
                    start = i;
                }
                _ => i += 1,
            }
        }
        self.buf.extend_from_slice(&bytes[start..]);
        out
    }

    /// End of stream. Per the format, an event without its terminating blank
    /// line is discarded; this returns nothing and resets the parser.
    pub fn finish(&mut self) {
        *self = Self::default();
    }

    fn line(&mut self, tail: &[u8], out: &mut Vec<SseEvent>) {
        // Assemble the full line from any buffered prefix.
        let owned;
        let line: &[u8] = if self.buf.is_empty() {
            tail
        } else {
            self.buf.extend_from_slice(tail);
            owned = std::mem::take(&mut self.buf);
            &owned
        };
        if line.is_empty() {
            self.dispatch(out);
            return;
        }
        if line[0] == b':' {
            return; // comment
        }
        let (field, value) = match line.iter().position(|&b| b == b':') {
            Some(p) => {
                let v = &line[p + 1..];
                (&line[..p], v.strip_prefix(b" ").unwrap_or(v))
            }
            None => (line, &[][..]),
        };
        match field {
            b"event" => self.event = Some(String::from_utf8_lossy(value).into_owned()),
            b"data" => {
                if self.has_data {
                    self.data.push('\n');
                }
                self.data.push_str(&String::from_utf8_lossy(value));
                self.has_data = true;
            }
            // `id` and `retry` are irrelevant to the Messages API; unknown
            // fields are ignored per the format.
            _ => {}
        }
    }

    fn dispatch(&mut self, out: &mut Vec<SseEvent>) {
        let event = self.event.take();
        if !self.has_data {
            return;
        }
        self.has_data = false;
        out.push(SseEvent { event, data: std::mem::take(&mut self.data) });
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use proptest::prelude::*;

    use super::*;

    fn feed_all(chunks: &[&[u8]]) -> Vec<SseEvent> {
        let mut p = SseParser::new();
        let mut out = Vec::new();
        for c in chunks {
            out.extend(p.push(c));
        }
        out
    }

    fn ev(event: Option<&str>, data: &str) -> SseEvent {
        SseEvent { event: event.map(str::to_owned), data: data.to_owned() }
    }

    #[test]
    fn parses_basic_events() {
        let text = "event: message_start\ndata: {\"a\":1}\n\nevent: ping\ndata: {}\n\n";
        assert_eq!(
            feed_all(&[text.as_bytes()]),
            vec![ev(Some("message_start"), "{\"a\":1}"), ev(Some("ping"), "{}")]
        );
    }

    #[test]
    fn multi_line_data_comments_and_no_space() {
        let text = ": keepalive\nevent:x\ndata:line1\ndata: line2\ndata\n\n";
        assert_eq!(feed_all(&[text.as_bytes()]), vec![ev(Some("x"), "line1\nline2\n")]);
    }

    #[test]
    fn event_without_data_is_not_dispatched() {
        let text = "event: lonely\n\ndata: x\n\n";
        assert_eq!(feed_all(&[text.as_bytes()]), vec![ev(None, "x")]);
    }

    #[test]
    fn crlf_and_cr_line_endings() {
        let text = "event: a\r\ndata: 1\r\n\r\nevent: b\rdata: 2\r\r";
        assert_eq!(feed_all(&[text.as_bytes()]), vec![ev(Some("a"), "1"), ev(Some("b"), "2")]);
    }

    #[test]
    fn crlf_split_across_chunks_is_one_terminator() {
        let out = feed_all(&[b"data: 1\r", b"\n\r", b"\n"]);
        assert_eq!(out, vec![ev(None, "1")]);
    }

    #[test]
    fn utf8_split_inside_code_point() {
        let s = "data: caf\u{e9} \u{1F4C8}\n\n";
        let b = s.as_bytes();
        // Split inside the two-byte é and the four-byte emoji.
        let e = s.find('\u{e9}').unwrap() + 1;
        let f = s.find('\u{1F4C8}').unwrap() + 2;
        let out = feed_all(&[&b[..e], &b[e..f], &b[f..]]);
        assert_eq!(out, vec![ev(None, "caf\u{e9} \u{1F4C8}")]);
    }

    #[test]
    fn incomplete_trailing_event_is_held() {
        let mut p = SseParser::new();
        assert!(p.push(b"data: partial").is_empty());
        assert!(p.push(b"\n").is_empty());
        assert_eq!(p.push(b"\n"), vec![ev(None, "partial")]);
    }

    fn arb_event() -> impl Strategy<Value = SseEvent> {
        (
            proptest::option::of("[a-z_]{1,16}"),
            // Data lines: anything except CR/LF, including non-ASCII.
            proptest::collection::vec("[^\r\n]{0,24}", 1..4),
        )
            .prop_map(|(event, lines)| SseEvent { event, data: lines.join("\n") })
    }

    fn serialize(events: &[SseEvent], eol: &str) -> String {
        let mut s = String::new();
        for e in events {
            if let Some(name) = &e.event {
                let _ = write!(s, "event: {name}{eol}");
            }
            for line in e.data.split('\n') {
                let _ = write!(s, "data: {line}{eol}");
            }
            s.push_str(eol);
        }
        s
    }

    proptest! {
        #[test]
        fn chunk_boundaries_do_not_matter(
            events in proptest::collection::vec(arb_event(), 0..8),
            eol_idx in 0usize..3,
            cuts in proptest::collection::vec(any::<prop::sample::Index>(), 0..12),
        ) {
            let eol = ["\n", "\r\n", "\r"][eol_idx];
            let text = serialize(&events, eol);
            let bytes = text.as_bytes();
            let mut points: Vec<usize> = cuts.iter().map(|ix| ix.index(bytes.len() + 1)).collect();
            points.push(0);
            points.push(bytes.len());
            points.sort_unstable();
            points.dedup();
            let mut p = SseParser::new();
            let mut out = Vec::new();
            for w in points.windows(2) {
                out.extend(p.push(&bytes[w[0]..w[1]]));
            }
            // A data line with a leading space loses exactly one space on
            // parse, matching `data: <line>` serialization.
            prop_assert_eq!(out, events);
        }
    }
}

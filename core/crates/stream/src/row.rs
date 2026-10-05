//! Packed quote row layout, version 1.
//!
//! Little-endian, 128 bytes per row, no header. Missing values are NaN.
//! The Swift decoder (`Bridge/HotRows.swift`) asserts these offsets at
//! startup against [`LAYOUT`], so the two sides cannot drift silently.

/// Bytes per row.
pub const ROW_SIZE: usize = 128;
pub const LAYOUT_VERSION: u32 = 1;

pub const OFF_INSTRUMENT: usize = 0; // u32
pub const OFF_CHANGED: usize = 4; // u32 bitmask, see `changed`
pub const OFF_TS_EVENT: usize = 8; // i64 unix nanos
pub const OFF_BID: usize = 16; // f64
pub const OFF_ASK: usize = 24;
pub const OFF_LAST: usize = 32;
pub const OFF_OPEN: usize = 40;
pub const OFF_HIGH: usize = 48;
pub const OFF_LOW: usize = 56;
pub const OFF_PREV_CLOSE: usize = 64;
pub const OFF_VOLUME: usize = 72;
pub const OFF_BID_SIZE: usize = 80;
pub const OFF_ASK_SIZE: usize = 88;
pub const OFF_LAST_SIZE: usize = 96;
pub const OFF_NET_CHANGE: usize = 104;
pub const OFF_PCT_CHANGE: usize = 112;
pub const OFF_FLAGS: usize = 120; // u32, `meridian_types::quote_flags`
pub const OFF_RESERVED: usize = 124; // u32

/// Bits in the `changed` field: which values changed since the poll's
/// `since` sequence. Used to flash updated cells.
pub mod changed {
    pub const BID: u32 = 1 << 0;
    pub const ASK: u32 = 1 << 1;
    pub const LAST: u32 = 1 << 2;
    pub const BID_SIZE: u32 = 1 << 3;
    pub const ASK_SIZE: u32 = 1 << 4;
    pub const VOLUME: u32 = 1 << 5;
    pub const RANGE: u32 = 1 << 6; // open/high/low/prev_close
    pub const FLAGS: u32 = 1 << 7;
}

/// Field name → byte offset, exported over the FFI for the layout check.
pub const LAYOUT: &[(&str, usize)] = &[
    ("row_size", ROW_SIZE),
    ("instrument", OFF_INSTRUMENT),
    ("changed", OFF_CHANGED),
    ("ts_event", OFF_TS_EVENT),
    ("bid", OFF_BID),
    ("ask", OFF_ASK),
    ("last", OFF_LAST),
    ("open", OFF_OPEN),
    ("high", OFF_HIGH),
    ("low", OFF_LOW),
    ("prev_close", OFF_PREV_CLOSE),
    ("volume", OFF_VOLUME),
    ("bid_size", OFF_BID_SIZE),
    ("ask_size", OFF_ASK_SIZE),
    ("last_size", OFF_LAST_SIZE),
    ("net_change", OFF_NET_CHANGE),
    ("pct_change", OFF_PCT_CHANGE),
    ("flags", OFF_FLAGS),
];

/// One decoded row (used by tests and Rust-side consumers).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuoteRow {
    pub instrument: u32,
    pub changed: u32,
    pub ts_event: i64,
    pub bid: f64,
    pub ask: f64,
    pub last: f64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub prev_close: f64,
    pub volume: f64,
    pub bid_size: f64,
    pub ask_size: f64,
    pub last_size: f64,
    pub net_change: f64,
    pub pct_change: f64,
    pub flags: u32,
}

impl QuoteRow {
    pub fn write_to(&self, out: &mut Vec<u8>) {
        let start = out.len();
        out.resize(start + ROW_SIZE, 0);
        let b = &mut out[start..start + ROW_SIZE];
        b[OFF_INSTRUMENT..OFF_INSTRUMENT + 4].copy_from_slice(&self.instrument.to_le_bytes());
        b[OFF_CHANGED..OFF_CHANGED + 4].copy_from_slice(&self.changed.to_le_bytes());
        b[OFF_TS_EVENT..OFF_TS_EVENT + 8].copy_from_slice(&self.ts_event.to_le_bytes());
        for (off, v) in [
            (OFF_BID, self.bid),
            (OFF_ASK, self.ask),
            (OFF_LAST, self.last),
            (OFF_OPEN, self.open),
            (OFF_HIGH, self.high),
            (OFF_LOW, self.low),
            (OFF_PREV_CLOSE, self.prev_close),
            (OFF_VOLUME, self.volume),
            (OFF_BID_SIZE, self.bid_size),
            (OFF_ASK_SIZE, self.ask_size),
            (OFF_LAST_SIZE, self.last_size),
            (OFF_NET_CHANGE, self.net_change),
            (OFF_PCT_CHANGE, self.pct_change),
        ] {
            b[off..off + 8].copy_from_slice(&v.to_le_bytes());
        }
        b[OFF_FLAGS..OFF_FLAGS + 4].copy_from_slice(&self.flags.to_le_bytes());
    }

    #[must_use]
    pub fn read_from(b: &[u8]) -> Option<Self> {
        if b.len() < ROW_SIZE {
            return None;
        }
        let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let i64_at = |o: usize| {
            let mut a = [0u8; 8];
            a.copy_from_slice(&b[o..o + 8]);
            i64::from_le_bytes(a)
        };
        let f64_at = |o: usize| {
            let mut a = [0u8; 8];
            a.copy_from_slice(&b[o..o + 8]);
            f64::from_le_bytes(a)
        };
        Some(Self {
            instrument: u32_at(OFF_INSTRUMENT),
            changed: u32_at(OFF_CHANGED),
            ts_event: i64_at(OFF_TS_EVENT),
            bid: f64_at(OFF_BID),
            ask: f64_at(OFF_ASK),
            last: f64_at(OFF_LAST),
            open: f64_at(OFF_OPEN),
            high: f64_at(OFF_HIGH),
            low: f64_at(OFF_LOW),
            prev_close: f64_at(OFF_PREV_CLOSE),
            volume: f64_at(OFF_VOLUME),
            bid_size: f64_at(OFF_BID_SIZE),
            ask_size: f64_at(OFF_ASK_SIZE),
            last_size: f64_at(OFF_LAST_SIZE),
            net_change: f64_at(OFF_NET_CHANGE),
            pct_change: f64_at(OFF_PCT_CHANGE),
            flags: u32_at(OFF_FLAGS),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_are_dense_and_aligned() {
        assert_eq!(OFF_RESERVED + 4, ROW_SIZE);
        for (name, off) in LAYOUT.iter().skip(1) {
            let width = match *name {
                "instrument" | "changed" | "flags" => 4,
                _ => 8,
            };
            assert_eq!(off % width, 0, "{name} misaligned");
            assert!(off + width <= ROW_SIZE, "{name} out of bounds");
        }
    }

    #[test]
    fn round_trip() {
        let r = QuoteRow {
            instrument: 7,
            changed: changed::LAST | changed::VOLUME,
            ts_event: 1_700_000_000_000_000_000,
            bid: 1.0,
            ask: 1.1,
            last: 1.05,
            open: 1.0,
            high: 1.2,
            low: 0.9,
            prev_close: 1.01,
            volume: 1e6,
            bid_size: 100.0,
            ask_size: 200.0,
            last_size: 5.0,
            net_change: 0.04,
            pct_change: 3.96,
            flags: 3,
        };
        let mut buf = Vec::new();
        r.write_to(&mut buf);
        assert_eq!(buf.len(), ROW_SIZE);
        assert_eq!(QuoteRow::read_from(&buf), Some(r));
    }
}

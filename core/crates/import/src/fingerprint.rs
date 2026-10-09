//! Stable row fingerprints for de-duplication on re-import.
//!
//! A fingerprint must not change between app versions, so it uses FNV-1a
//! (128-bit; parameters from the FNV reference,
//! <http://www.isthe.com/chongo/tech/comp/fnv/index.html>) rather than
//! `std`'s hasher, whose algorithm is unspecified. The input is the
//! format family plus the row's own values (date, broker action text,
//! symbol, quantity, price, amount), not Meridian's classification, so
//! improving a code table later does not re-import old rows. Identical rows
//! in one file (two equal buys on one day) are told apart by their
//! occurrence number, which is stable because broker exports cover whole
//! days.
//!
//! The account is not an input: the same trade appears with no account in
//! a single-account download and with an account name or number in an
//! all-accounts one (and Fidelity moved from names to numbers in 2025), so
//! including it made overlapping downloads import twice. The cost is that
//! two single-account files with an identical trade on the same day
//! de-duplicate against each other; the import result counts it among the
//! duplicates.

use std::collections::HashMap;

const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

fn fnv1a(bytes: &[u8]) -> u128 {
    let mut h = OFFSET;
    for b in bytes {
        h ^= u128::from(*b);
        h = h.wrapping_mul(PRIME);
    }
    h
}

/// Canonical text for an optional number: fixed 6 decimals, `-0` folded.
fn num(v: Option<f64>) -> String {
    match v {
        Some(x) if x.is_finite() => {
            let r = (x * 1e6).round() / 1e6;
            let r = if r == 0.0 { 0.0 } else { r };
            format!("{r:.6}")
        }
        _ => String::new(),
    }
}

/// The values a fingerprint is computed from.
#[derive(Debug, Clone, Default)]
pub struct Parts<'a> {
    pub family: &'a str,
    pub date: &'a str,
    pub action: &'a str,
    pub symbol: &'a str,
    pub quantity: Option<f64>,
    pub price: Option<f64>,
    pub amount: Option<f64>,
    pub extra: Option<f64>,
}

/// Assigns fingerprints, numbering repeats of identical content.
#[derive(Debug, Default)]
pub struct Fingerprinter {
    seen: HashMap<u128, u32>,
}

impl Fingerprinter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 32 hex digits of the content hash, `-`, and the occurrence number.
    pub fn next(&mut self, p: &Parts<'_>) -> String {
        let action = p.action.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        let canonical = [
            p.family,
            // Formerly the account. Kept as an empty field so rows that never
            // had an account keep the fingerprints they had in 1.1.
            "",
            p.date,
            action.as_str(),
            p.symbol,
            &num(p.quantity),
            &num(p.price),
            &num(p.amount),
            &num(p.extra),
        ]
        .join("\u{1f}");
        let h = fnv1a(canonical.as_bytes());
        let n = self.seen.entry(h).or_insert(0);
        let out = format!("{h:032x}-{n}");
        *n += 1;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_reference_vectors() {
        // Vectors from Zig's std.hash.Fnv1a_128 tests
        // (lib/std/hash/fnv.zig) and the php-fnv1a README.
        assert_eq!(fnv1a(b""), 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d);
        assert_eq!(fnv1a(b"a"), 0xd228_cb69_6f1a_8caf_7891_2b70_4e4a_8964);
        assert_eq!(fnv1a(b"foobar"), 0x343e_1662_793c_64bf_6f0d_3597_ba44_6f18);
    }

    #[test]
    fn repeats_get_occurrence_numbers_and_values_are_canonical() {
        let mut f = Fingerprinter::new();
        let p = Parts { family: "robinhood", date: "2026-01-02", action: "Buy", symbol: "AAPL", quantity: Some(1.0), price: Some(10.0), amount: Some(-10.0), ..Default::default() };
        let a = f.next(&p);
        let b = f.next(&p);
        assert_ne!(a, b);
        assert!(a.ends_with("-0") && b.ends_with("-1"));
        // Same values written differently hash the same.
        let mut g = Fingerprinter::new();
        let q = Parts { action: "  buy ", quantity: Some(1.000_000_04), amount: Some(-10.0), ..p.clone() };
        assert_eq!(g.next(&q), a);
        // A different family never collides with the same row.
        let mut h = Fingerprinter::new();
        assert_ne!(h.next(&Parts { family: "schwab", ..p }), a);
    }
}

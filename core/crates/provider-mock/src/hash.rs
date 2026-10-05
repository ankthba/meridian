//! Stable hashing and seed derivation.
//!
//! Every generated series is a function of the configured seed and a stable
//! hash of the security key, so it never depends on `std`'s randomized
//! hasher, process state, or request order.

use meridian_types::SecurityKey;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a, 64-bit.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Fnv1a(u64);

impl Fnv1a {
    pub(crate) fn new() -> Self {
        Self(FNV_OFFSET)
    }

    pub(crate) fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(FNV_PRIME);
        }
    }

    pub(crate) fn finish(self) -> u64 {
        self.0
    }
}

pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h = Fnv1a::new();
    h.write(bytes);
    h.finish()
}

/// Stable hash of a security key (`symbol|exchange|sector`).
pub(crate) fn key_hash(key: &SecurityKey) -> u64 {
    let mut h = Fnv1a::new();
    h.write(key.symbol.as_bytes());
    h.write(b"|");
    if let Some(ex) = &key.exchange {
        h.write(ex.as_bytes());
    }
    h.write(b"|");
    h.write(key.sector.key_label().as_bytes());
    h.finish()
}

/// SplitMix64 finalizer: a strong 64-bit mixer.
pub(crate) fn splitmix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Combines a list of words into one well-mixed 64-bit value.
pub(crate) fn mix(parts: &[u64]) -> u64 {
    let mut h = 0x6a09_e667_f3bc_c909_u64;
    for p in parts {
        h = splitmix(h ^ splitmix(*p));
    }
    h
}

/// Tag words so different uses of the same key never share a stream.
pub(crate) fn tag(s: &str) -> u64 {
    fnv1a(s.as_bytes())
}

/// ChaCha8 stream for shared, one-off series (factors, macro, the stream task).
pub(crate) fn rng(parts: &[u64]) -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(mix(parts))
}

/// xoshiro256++ for hot per-symbol paths (daily and intraday). Implemented
/// here so its output can never change with a dependency upgrade, and so
/// debug builds stay fast.
#[derive(Debug, Clone)]
pub(crate) struct Fast {
    s: [u64; 4],
}

impl Fast {
    pub(crate) fn new(parts: &[u64]) -> Self {
        let mut z = mix(parts);
        let mut s = [0u64; 4];
        for x in &mut s {
            z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
            *x = splitmix(z);
        }
        if s == [0; 4] {
            s[0] = 1;
        }
        Self { s }
    }
}

impl rand::RngCore for Fast {
    fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    fn next_u64(&mut self) -> u64 {
        let s = &mut self.s;
        let result = s[0].wrapping_add(s[3]).rotate_left(23).wrapping_add(s[0]);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        result
    }

    fn fill_bytes(&mut self, dst: &mut [u8]) {
        for chunk in dst.chunks_mut(8) {
            let v = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&v[..chunk.len()]);
        }
    }
}

/// Uniform in `[0, 1)` from a hash.
pub(crate) fn unit(h: u64) -> f64 {
    (h >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

/// Small counter-based generator for places that need a handful of draws
/// tied to one (key, day) cell without the cost of seeding ChaCha.
#[derive(Debug, Clone)]
pub(crate) struct Cell {
    state: u64,
}

impl Cell {
    pub(crate) fn new(parts: &[u64]) -> Self {
        Self { state: mix(parts) }
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        splitmix(self.state)
    }

    /// Uniform in `[0, 1)`.
    pub(crate) fn u01(&mut self) -> f64 {
        unit(self.next_u64())
    }

    /// Uniform in `[lo, hi)`.
    pub(crate) fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.u01()
    }

    /// Uniform integer in `[lo, hi]`.
    pub(crate) fn int(&mut self, lo: i64, hi: i64) -> i64 {
        if hi <= lo {
            return lo;
        }
        let span = (hi - lo + 1) as u64;
        lo + (self.next_u64() % span) as i64
    }

    /// Standard normal (Box-Muller; one value per call).
    pub(crate) fn normal(&mut self) -> f64 {
        let u1 = self.u01().max(1e-300);
        let u2 = self.u01();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }

    /// Picks an element.
    pub(crate) fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        let i = (self.next_u64() % items.len() as u64) as usize;
        &items[i]
    }

    pub(crate) fn chance(&mut self, p: f64) -> bool {
        self.u01() < p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_reference_vectors() {
        // Published FNV-1a 64-bit test vectors.
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn key_hash_is_stable_and_distinct() {
        let a = key_hash(&SecurityKey::equity("AAPL"));
        assert_eq!(a, key_hash(&SecurityKey::equity("aapl")));
        assert_ne!(a, key_hash(&SecurityKey::equity("MSFT")));
        assert_ne!(a, key_hash(&SecurityKey::index("AAPL")));
    }

    #[test]
    fn cell_is_deterministic() {
        let mut a = Cell::new(&[1, 2, 3]);
        let mut b = Cell::new(&[1, 2, 3]);
        for _ in 0..10 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        let u = a.u01();
        assert!((0.0..1.0).contains(&u));
    }
}

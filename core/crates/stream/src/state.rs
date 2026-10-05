use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use meridian_types::{Quote, QuoteUpdate, UnixNanos, quote_flags};
use parking_lot::{Mutex, RwLock};

use crate::registry::InstrumentId;
use crate::row::{QuoteRow, changed};

/// Index of the provider that last wrote a cell (for stale marking).
pub type FeedIndex = u16;
pub const NO_FEED: FeedIndex = u16::MAX;

const F_BID: usize = 0;
const F_ASK: usize = 1;
const F_LAST: usize = 2;
const F_BID_SIZE: usize = 3;
const F_ASK_SIZE: usize = 4;
const F_VOLUME: usize = 5;
const F_RANGE: usize = 6;
const F_FLAGS: usize = 7;
const N_FIELDS: usize = 8;

#[derive(Debug, Clone)]
struct CellData {
    ts_event: UnixNanos,
    bid: f64,
    ask: f64,
    last: f64,
    open: f64,
    high: f64,
    low: f64,
    prev_close: f64,
    volume: f64,
    bid_size: f64,
    ask_size: f64,
    last_size: f64,
    flags: u32,
    field_versions: [u64; N_FIELDS],
    feed: FeedIndex,
}

impl Default for CellData {
    fn default() -> Self {
        Self {
            ts_event: 0,
            bid: f64::NAN,
            ask: f64::NAN,
            last: f64::NAN,
            open: f64::NAN,
            high: f64::NAN,
            low: f64::NAN,
            prev_close: f64::NAN,
            volume: f64::NAN,
            bid_size: f64::NAN,
            ask_size: f64::NAN,
            last_size: f64::NAN,
            flags: 0,
            field_versions: [0; N_FIELDS],
            feed: NO_FEED,
        }
    }
}

/// Bit-exact comparison that treats NaN == NaN (so re-sending a missing value
/// isn't a change).
fn same(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits()
}

fn set(slot: &mut f64, v: Option<f64>, versions: &mut [u64; N_FIELDS], field: usize, seq: u64) -> bool {
    match v {
        Some(v) if !same(*slot, v) => {
            *slot = v;
            versions[field] = seq;
            true
        }
        _ => false,
    }
}

#[derive(Debug, Default)]
struct QuoteCell {
    version: AtomicU64,
    data: Mutex<CellData>,
}

/// Latest quote state for every interned instrument.
#[derive(Debug, Default)]
pub struct MarketState {
    cells: RwLock<Vec<Arc<QuoteCell>>>,
    seq: AtomicU64,
}

impl MarketState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn cell(&self, id: InstrumentId) -> Arc<QuoteCell> {
        let idx = id as usize;
        if let Some(c) = self.cells.read().get(idx) {
            return c.clone();
        }
        let mut w = self.cells.write();
        while w.len() <= idx {
            w.push(Arc::new(QuoteCell::default()));
        }
        w[idx].clone()
    }

    /// Current global sequence number.
    #[must_use]
    pub fn seq(&self) -> u64 {
        self.seq.load(Ordering::Acquire)
    }

    /// Merges a partial update. Returns whether anything changed.
    pub fn apply(&self, id: InstrumentId, u: &QuoteUpdate, feed: FeedIndex) -> bool {
        let cell = self.cell(id);
        let mut d = cell.data.lock();
        // Reserve the sequence number under the cell lock so versions are
        // monotonic per cell.
        let seq = self.seq.fetch_add(1, Ordering::AcqRel) + 1;
        let mut any = false;
        let prev_last = d.last;
        let v = &mut d.field_versions.clone();
        any |= set(&mut d.bid, u.bid, v, F_BID, seq);
        any |= set(&mut d.ask, u.ask, v, F_ASK, seq);
        any |= set(&mut d.bid_size, u.bid_size, v, F_BID_SIZE, seq);
        any |= set(&mut d.ask_size, u.ask_size, v, F_ASK_SIZE, seq);
        let last_changed = set(&mut d.last, u.last, v, F_LAST, seq);
        any |= last_changed;
        if let Some(sz) = u.last_size {
            d.last_size = sz;
        }
        any |= set(&mut d.open, u.open, v, F_RANGE, seq);
        any |= set(&mut d.high, u.high, v, F_RANGE, seq);
        any |= set(&mut d.low, u.low, v, F_RANGE, seq);
        any |= set(&mut d.prev_close, u.prev_close, v, F_RANGE, seq);
        any |= set(&mut d.volume, u.volume, v, F_VOLUME, seq);
        if let Some(inc) = u.volume_increment
            && inc != 0.0
        {
            d.volume = if d.volume.is_nan() { inc } else { d.volume + inc };
            v[F_VOLUME] = seq;
            any = true;
        }
        if last_changed {
            let last = d.last;
            // Extend the session range with the new print when the feed
            // doesn't send high/low itself.
            if u.high.is_none() && !d.high.is_nan() && last > d.high {
                d.high = last;
                v[F_RANGE] = seq;
            }
            if u.low.is_none() && !d.low.is_nan() && last < d.low {
                d.low = last;
                v[F_RANGE] = seq;
            }
            let mut flags = d.flags & !(quote_flags::TICK_UP | quote_flags::TICK_DOWN);
            if !prev_last.is_nan() {
                if last > prev_last {
                    flags |= quote_flags::TICK_UP;
                } else if last < prev_last {
                    flags |= quote_flags::TICK_DOWN;
                } else {
                    flags |= d.flags & (quote_flags::TICK_UP | quote_flags::TICK_DOWN);
                }
            }
            d.flags = flags;
        }
        if d.flags & quote_flags::STALE != 0 {
            d.flags &= !quote_flags::STALE;
            v[F_FLAGS] = seq;
            any = true;
        }
        d.field_versions = *v;
        if u.ts_event > d.ts_event {
            d.ts_event = u.ts_event;
        }
        d.feed = feed;
        if any {
            cell.version.store(seq, Ordering::Release);
        }
        any
    }

    /// Replaces the cell with a full snapshot (e.g. from a REST quote).
    pub fn apply_snapshot(&self, id: InstrumentId, q: &Quote, feed: FeedIndex) {
        let u = QuoteUpdate {
            bid: q.bid,
            ask: q.ask,
            bid_size: q.bid_size,
            ask_size: q.ask_size,
            last: q.last,
            last_size: q.last_size,
            open: q.open,
            high: q.high,
            low: q.low,
            prev_close: q.prev_close,
            volume: q.volume,
            volume_increment: None,
            vwap: q.vwap,
            ts_event: q.ts_event,
        };
        self.apply(id, &u, feed);
        let extra = q.flags & (quote_flags::DELAYED | quote_flags::SYNTHETIC | quote_flags::HALTED | quote_flags::MARKET_CLOSED);
        self.set_flags(id, extra, quote_flags::DELAYED | quote_flags::SYNTHETIC | quote_flags::HALTED | quote_flags::MARKET_CLOSED);
    }

    /// Sets `flags & mask` on the cell, bumping its version if it changed.
    pub fn set_flags(&self, id: InstrumentId, flags: u32, mask: u32) {
        let cell = self.cell(id);
        let mut d = cell.data.lock();
        let new = (d.flags & !mask) | (flags & mask);
        if new != d.flags {
            let seq = self.seq.fetch_add(1, Ordering::AcqRel) + 1;
            d.flags = new;
            d.field_versions[F_FLAGS] = seq;
            cell.version.store(seq, Ordering::Release);
        }
    }

    /// Marks every cell last written by `feed` as stale (feed disconnected).
    pub fn mark_feed_stale(&self, feed: FeedIndex) {
        let cells: Vec<Arc<QuoteCell>> = self.cells.read().clone();
        for cell in cells {
            let mut d = cell.data.lock();
            if d.feed == feed && d.flags & quote_flags::STALE == 0 {
                let seq = self.seq.fetch_add(1, Ordering::AcqRel) + 1;
                d.flags |= quote_flags::STALE;
                d.field_versions[F_FLAGS] = seq;
                cell.version.store(seq, Ordering::Release);
            }
        }
    }

    fn row(id: InstrumentId, d: &CellData, since: u64) -> QuoteRow {
        let mut changed_mask = 0;
        for (field, bit) in [
            (F_BID, changed::BID),
            (F_ASK, changed::ASK),
            (F_LAST, changed::LAST),
            (F_BID_SIZE, changed::BID_SIZE),
            (F_ASK_SIZE, changed::ASK_SIZE),
            (F_VOLUME, changed::VOLUME),
            (F_RANGE, changed::RANGE),
            (F_FLAGS, changed::FLAGS),
        ] {
            if d.field_versions[field] > since {
                changed_mask |= bit;
            }
        }
        let net = d.last - d.prev_close;
        let pct = if d.prev_close.is_nan() || d.prev_close == 0.0 { f64::NAN } else { net / d.prev_close * 100.0 };
        QuoteRow {
            instrument: id,
            changed: changed_mask,
            ts_event: d.ts_event,
            bid: d.bid,
            ask: d.ask,
            last: d.last,
            open: d.open,
            high: d.high,
            low: d.low,
            prev_close: d.prev_close,
            volume: d.volume,
            bid_size: d.bid_size,
            ask_size: d.ask_size,
            last_size: d.last_size,
            net_change: net,
            pct_change: pct,
            flags: d.flags,
        }
    }

    /// Latest row for one instrument, if it has ever been written.
    #[must_use]
    pub fn snapshot(&self, id: InstrumentId) -> Option<QuoteRow> {
        let cell = self.cells.read().get(id as usize)?.clone();
        if cell.version.load(Ordering::Acquire) == 0 {
            return None;
        }
        let d = cell.data.lock();
        Some(Self::row(id, &d, u64::MAX))
    }

    /// Appends packed rows for every id whose cell changed after `since` and
    /// returns the sequence number to pass as `since` next time.
    pub fn poll_into(&self, ids: &[InstrumentId], since: u64, out: &mut Vec<u8>) -> u64 {
        let current = self.seq();
        let cells = self.cells.read();
        for &id in ids {
            let Some(cell) = cells.get(id as usize) else { continue };
            let v = cell.version.load(Ordering::Acquire);
            if v == 0 || v <= since {
                continue;
            }
            let d = cell.data.lock();
            Self::row(id, &d, since).write_to(out);
        }
        current
    }
}

#[cfg(test)]
mod tests {
    use meridian_types::{Provenance, SecurityKey};

    use super::*;
    use crate::row::{QuoteRow, ROW_SIZE};

    fn upd(last: f64) -> QuoteUpdate {
        QuoteUpdate { last: Some(last), ts_event: 1, ..Default::default() }
    }

    fn rows(buf: &[u8]) -> Vec<QuoteRow> {
        buf.chunks(ROW_SIZE).filter_map(QuoteRow::read_from).collect()
    }

    #[test]
    fn poll_returns_only_changed_cells() {
        let s = MarketState::new();
        s.apply(0, &upd(10.0), 0);
        s.apply(1, &upd(20.0), 0);
        let mut buf = Vec::new();
        let seq = s.poll_into(&[0, 1], 0, &mut buf);
        assert_eq!(rows(&buf).len(), 2);

        buf.clear();
        s.apply(1, &upd(21.0), 0);
        let seq2 = s.poll_into(&[0, 1], seq, &mut buf);
        let r = rows(&buf);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].instrument, 1);
        assert_eq!(r[0].last, 21.0);
        assert!(r[0].changed & changed::LAST != 0);
        assert!(r[0].flags & quote_flags::TICK_UP != 0);

        buf.clear();
        s.poll_into(&[0, 1], seq2, &mut buf);
        assert!(buf.is_empty());
    }

    #[test]
    fn identical_update_is_not_a_change() {
        let s = MarketState::new();
        assert!(s.apply(0, &upd(10.0), 0));
        assert!(!s.apply(0, &upd(10.0), 0));
    }

    #[test]
    fn volume_increments_accumulate() {
        let s = MarketState::new();
        let u = QuoteUpdate { volume_increment: Some(5.0), ..Default::default() };
        s.apply(0, &u, 0);
        s.apply(0, &u, 0);
        assert_eq!(s.snapshot(0).unwrap().volume, 10.0);
    }

    #[test]
    fn stale_marking_and_clearing() {
        let s = MarketState::new();
        s.apply(0, &upd(10.0), 3);
        s.mark_feed_stale(3);
        assert!(s.snapshot(0).unwrap().flags & quote_flags::STALE != 0);
        s.apply(0, &upd(11.0), 3);
        assert!(s.snapshot(0).unwrap().flags & quote_flags::STALE == 0);
    }

    #[test]
    fn net_and_pct_change_computed() {
        let s = MarketState::new();
        let mut q = Quote::empty(SecurityKey::equity("X"), Provenance::synthetic(0));
        q.last = Some(110.0);
        q.prev_close = Some(100.0);
        q.flags = quote_flags::SYNTHETIC;
        s.apply_snapshot(0, &q, 0);
        let r = s.snapshot(0).unwrap();
        assert_eq!(r.net_change, 10.0);
        assert!((r.pct_change - 10.0).abs() < 1e-12);
        assert!(r.flags & quote_flags::SYNTHETIC != 0);
    }

    #[test]
    fn range_extends_with_prints() {
        let s = MarketState::new();
        s.apply(0, &QuoteUpdate { last: Some(10.0), high: Some(10.0), low: Some(10.0), ..Default::default() }, 0);
        s.apply(0, &upd(12.0), 0);
        s.apply(0, &upd(9.0), 0);
        let r = s.snapshot(0).unwrap();
        assert_eq!((r.high, r.low), (12.0, 9.0));
    }
}

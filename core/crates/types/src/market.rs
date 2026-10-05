//! Quotes, trades, and bars.
//!
//! Prices are `f64`. Every source we use delivers binary floats or short
//! decimal strings that `f64` represents exactly to display precision;
//! display rounds to the instrument's `price_decimals`.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::key::SecurityKey;
use crate::provenance::Provenance;
use crate::time::UnixNanos;

/// Bit flags carried on quotes.
pub mod quote_flags {
    pub const STALE: u32 = 1 << 0;
    pub const HALTED: u32 = 1 << 1;
    pub const DELAYED: u32 = 1 << 2;
    pub const SYNTHETIC: u32 = 1 << 3;
    pub const TICK_UP: u32 = 1 << 4;
    pub const TICK_DOWN: u32 = 1 << 5;
    pub const MARKET_CLOSED: u32 = 1 << 6;
}

/// Latest full quote snapshot for an instrument.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quote {
    pub key: SecurityKey,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub bid_size: Option<f64>,
    pub ask_size: Option<f64>,
    pub last: Option<f64>,
    pub last_size: Option<f64>,
    pub open: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub prev_close: Option<f64>,
    pub volume: Option<f64>,
    pub vwap: Option<f64>,
    pub ts_event: UnixNanos,
    pub ts_recv: UnixNanos,
    pub flags: u32,
    pub provenance: Provenance,
}

impl Quote {
    #[must_use]
    pub fn empty(key: SecurityKey, provenance: Provenance) -> Self {
        Self {
            key,
            bid: None,
            ask: None,
            bid_size: None,
            ask_size: None,
            last: None,
            last_size: None,
            open: None,
            high: None,
            low: None,
            prev_close: None,
            volume: None,
            vwap: None,
            ts_event: 0,
            ts_recv: 0,
            flags: 0,
            provenance,
        }
    }

    #[must_use]
    pub fn net_change(&self) -> Option<f64> {
        Some(self.last? - self.prev_close?)
    }

    #[must_use]
    pub fn pct_change(&self) -> Option<f64> {
        let pc = self.prev_close?;
        if pc == 0.0 {
            return None;
        }
        Some((self.last? - pc) / pc * 100.0)
    }

    #[must_use]
    pub fn mid(&self) -> Option<f64> {
        Some((self.bid? + self.ask?) / 2.0)
    }
}

/// Partial update from a stream. `None` fields are unchanged.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct QuoteUpdate {
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub bid_size: Option<f64>,
    pub ask_size: Option<f64>,
    pub last: Option<f64>,
    pub last_size: Option<f64>,
    pub open: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub prev_close: Option<f64>,
    /// Cumulative session volume, if the feed provides it.
    pub volume: Option<f64>,
    /// Incremental traded size to add to session volume, if the feed only
    /// provides trade sizes.
    pub volume_increment: Option<f64>,
    pub vwap: Option<f64>,
    pub ts_event: UnixNanos,
}

/// Events emitted by streaming providers, already normalized.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StreamEvent {
    Quote { key: SecurityKey, update: QuoteUpdate },
    /// Provider connection state changed.
    Status { connected: bool, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BarInterval {
    Minute(u32),
    Hour(u32),
    Day,
    Week,
    Month,
}

impl BarInterval {
    /// Nominal length in seconds (months use 30 days; for bucketing only).
    #[must_use]
    pub fn seconds(self) -> i64 {
        match self {
            BarInterval::Minute(n) => 60 * i64::from(n),
            BarInterval::Hour(n) => 3_600 * i64::from(n),
            BarInterval::Day => 86_400,
            BarInterval::Week => 7 * 86_400,
            BarInterval::Month => 30 * 86_400,
        }
    }

    #[must_use]
    pub fn is_intraday(self) -> bool {
        matches!(self, BarInterval::Minute(_) | BarInterval::Hour(_))
    }

    /// Compact code used in storage and the FFI, e.g. `1m`, `4h`, `1d`.
    #[must_use]
    pub fn code(self) -> String {
        match self {
            BarInterval::Minute(n) => format!("{n}m"),
            BarInterval::Hour(n) => format!("{n}h"),
            BarInterval::Day => "1d".into(),
            BarInterval::Week => "1w".into(),
            BarInterval::Month => "1mo".into(),
        }
    }

    #[must_use]
    pub fn from_code(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        match s.as_str() {
            "1d" | "d" => return Some(BarInterval::Day),
            "1w" | "w" => return Some(BarInterval::Week),
            "1mo" | "mo" => return Some(BarInterval::Month),
            _ => {}
        }
        if let Some(n) = s.strip_suffix('m') {
            return n.parse().ok().filter(|n| *n > 0).map(BarInterval::Minute);
        }
        if let Some(n) = s.strip_suffix('h') {
            return n.parse().ok().filter(|n| *n > 0).map(BarInterval::Hour);
        }
        None
    }
}

impl fmt::Display for BarInterval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.code())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Adjustment {
    None,
    Splits,
    SplitsAndDividends,
}

/// One OHLCV bar, row form. Bulk paths use [`BarSeries`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bar {
    pub ts: UnixNanos,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

/// Columnar bar series. All columns have equal length and `ts` is strictly
/// increasing (enforced by [`BarSeries::validate`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BarSeries {
    pub key: SecurityKey,
    pub interval: BarInterval,
    pub adjustment: Adjustment,
    pub ts: Vec<UnixNanos>,
    pub open: Vec<f64>,
    pub high: Vec<f64>,
    pub low: Vec<f64>,
    pub close: Vec<f64>,
    pub volume: Vec<f64>,
    pub provenance: Provenance,
}

impl BarSeries {
    #[must_use]
    pub fn new(key: SecurityKey, interval: BarInterval, adjustment: Adjustment, provenance: Provenance) -> Self {
        Self {
            key,
            interval,
            adjustment,
            ts: Vec::new(),
            open: Vec::new(),
            high: Vec::new(),
            low: Vec::new(),
            close: Vec::new(),
            volume: Vec::new(),
            provenance,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.ts.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ts.is_empty()
    }

    pub fn push(&mut self, bar: Bar) {
        self.ts.push(bar.ts);
        self.open.push(bar.open);
        self.high.push(bar.high);
        self.low.push(bar.low);
        self.close.push(bar.close);
        self.volume.push(bar.volume);
    }

    #[must_use]
    pub fn bar(&self, i: usize) -> Bar {
        Bar {
            ts: self.ts[i],
            open: self.open[i],
            high: self.high[i],
            low: self.low[i],
            close: self.close[i],
            volume: self.volume[i],
        }
    }

    /// Sorts by time and drops duplicate timestamps (keeping the last).
    pub fn normalize(&mut self) {
        let mut rows: Vec<Bar> = (0..self.len()).map(|i| self.bar(i)).collect();
        rows.sort_by_key(|b| b.ts);
        rows.dedup_by(|later, earlier| {
            if later.ts == earlier.ts {
                *earlier = *later;
                true
            } else {
                false
            }
        });
        self.ts.clear();
        self.open.clear();
        self.high.clear();
        self.low.clear();
        self.close.clear();
        self.volume.clear();
        for b in rows {
            self.push(b);
        }
    }

    /// Checks column lengths, ordering, and OHLC consistency.
    pub fn validate(&self) -> Result<(), String> {
        let n = self.ts.len();
        for (name, len) in [
            ("open", self.open.len()),
            ("high", self.high.len()),
            ("low", self.low.len()),
            ("close", self.close.len()),
            ("volume", self.volume.len()),
        ] {
            if len != n {
                return Err(format!("column {name} has {len} rows, expected {n}"));
            }
        }
        for i in 1..n {
            if self.ts[i] <= self.ts[i - 1] {
                return Err(format!("timestamps not strictly increasing at row {i}"));
            }
        }
        for i in 0..n {
            let (o, h, l, c) = (self.open[i], self.high[i], self.low[i], self.close[i]);
            if !(h >= l && h >= o && h >= c && l <= o && l <= c) {
                return Err(format!("inconsistent OHLC at row {i}: o={o} h={h} l={l} c={c}"));
            }
        }
        Ok(())
    }

    /// Bars with `from <= ts < to`.
    #[must_use]
    pub fn slice_time(&self, from: UnixNanos, to: UnixNanos) -> BarSeries {
        let start = self.ts.partition_point(|t| *t < from);
        let end = self.ts.partition_point(|t| *t < to);
        BarSeries {
            key: self.key.clone(),
            interval: self.interval,
            adjustment: self.adjustment,
            ts: self.ts[start..end].to_vec(),
            open: self.open[start..end].to_vec(),
            high: self.high[start..end].to_vec(),
            low: self.low[start..end].to_vec(),
            close: self.close[start..end].to_vec(),
            volume: self.volume[start..end].to_vec(),
            provenance: self.provenance.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::Provenance;

    fn series(ts: &[i64]) -> BarSeries {
        let mut s = BarSeries::new(
            SecurityKey::equity("X"),
            BarInterval::Day,
            Adjustment::None,
            Provenance::synthetic(0),
        );
        for t in ts {
            s.push(Bar { ts: *t, open: 1.0, high: 2.0, low: 0.5, close: 1.5, volume: 10.0 });
        }
        s
    }

    #[test]
    fn interval_codes_round_trip() {
        for iv in [BarInterval::Minute(1), BarInterval::Minute(5), BarInterval::Hour(4), BarInterval::Day, BarInterval::Week, BarInterval::Month] {
            assert_eq!(BarInterval::from_code(&iv.code()), Some(iv));
        }
        assert_eq!(BarInterval::from_code("0m"), None);
    }

    #[test]
    fn normalize_sorts_and_dedups() {
        let mut s = series(&[3, 1, 2, 2]);
        s.normalize();
        assert_eq!(s.ts, vec![1, 2, 3]);
        assert!(s.validate().is_ok());
    }

    #[test]
    fn validate_rejects_bad_ohlc() {
        let mut s = series(&[1]);
        s.high[0] = 0.1;
        assert!(s.validate().is_err());
    }

    #[test]
    fn slice_time_is_half_open() {
        let s = series(&[1, 2, 3, 4]);
        assert_eq!(s.slice_time(2, 4).ts, vec![2, 3]);
    }

    #[test]
    fn quote_changes() {
        let mut q = Quote::empty(SecurityKey::equity("X"), Provenance::synthetic(0));
        q.last = Some(110.0);
        q.prev_close = Some(100.0);
        assert_eq!(q.net_change(), Some(10.0));
        assert!((q.pct_change().unwrap() - 10.0).abs() < 1e-12);
    }
}

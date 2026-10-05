//! Synthetic streaming quotes.
//!
//! One tokio task serves every subscription: a 10 ms tick walks shared
//! factor and currency Brownian motions, then scans preallocated per-symbol
//! slots for due Poisson arrivals and emits quote or trade updates. New
//! subscriptions start from the snapshot quote and are initialized under a
//! per-tick time budget so the tick never stalls. FX pairs are priced as
//! ratios of the walked currency levels, so crosses triangulate.
//!
//! The feed ticks around the clock (also when the synthetic market is
//! closed) so UI work can happen at any hour.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use meridian_provider::{EventSink, StreamHandle};
use meridian_types::{NANOS_PER_SEC, ProviderId, QuoteUpdate, SecurityKey, StreamEvent};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rand_distr::StandardNormal;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::Inner;
use crate::hash::{mix, tag};
use crate::market::round_tick;
use crate::universe::{CCYS, Kind, Model, N_FACTORS};

pub(crate) enum Cmd {
    Subscribe(Vec<SecurityKey>),
    Unsubscribe(Vec<SecurityKey>),
    Close,
}

pub(crate) struct MockStreamHandle {
    tx: UnboundedSender<Cmd>,
    closed: Arc<AtomicBool>,
}

impl StreamHandle for MockStreamHandle {
    fn subscribe(&self, keys: &[SecurityKey]) {
        let _ = self.tx.send(Cmd::Subscribe(keys.to_vec()));
    }

    fn unsubscribe(&self, keys: &[SecurityKey]) {
        let _ = self.tx.send(Cmd::Unsubscribe(keys.to_vec()));
    }

    fn close(&self) {
        if !self.closed.swap(true, Ordering::SeqCst) {
            let _ = self.tx.send(Cmd::Close);
        }
    }
}

impl Drop for MockStreamHandle {
    fn drop(&mut self) {
        self.close();
    }
}

pub(crate) fn open(inner: Arc<Inner>, sink: Arc<dyn EventSink>) -> Result<MockStreamHandle, String> {
    let rt = tokio::runtime::Handle::try_current().map_err(|e| format!("mock stream needs a tokio runtime: {e}"))?;
    let (tx, rx) = unbounded_channel();
    let closed = Arc::new(AtomicBool::new(false));
    let id = ProviderId::new("mock");
    sink.send(&id, StreamEvent::Status { connected: true, message: "MOCK stream connected (synthetic data)".into() });
    rt.spawn(run(inner, sink, rx, closed.clone()));
    Ok(MockStreamHandle { tx, closed })
}

#[derive(Debug, Clone, Copy)]
enum Price {
    /// log mid = ln(base) + sigma * (rho * (W_f - w0) + sqrt(1-rho^2) * idio)
    Factor { factor: usize, rho: f64, w0: f64, idio: f64 },
    /// mid = k * X_base / X_quote
    Fx { base: usize, quote: usize, k: f64 },
}

struct Slot {
    key: SecurityKey,
    active: bool,
    price: Price,
    base: f64,
    /// Volatility per sqrt(second).
    sigma: f64,
    tick: f64,
    spread_ticks: f64,
    lot: f64,
    vol_decimals: i32,
    has_book: bool,
    equity_like: bool,
    last: f64,
    bid: f64,
    ask: f64,
    high: f64,
    low: f64,
    volume: f64,
    pv: f64,
    last_t: f64,
    next_at: f64,
}

struct State {
    slots: Vec<Slot>,
    index: HashMap<SecurityKey, usize>,
    free: Vec<usize>,
    pending: VecDeque<SecurityKey>,
    /// Factor Brownian motions (unit variance per second).
    w: [f64; N_FACTORS],
    /// Log USD value of each currency, walked continuously.
    ccy: Vec<f64>,
    ccy_ready: bool,
    rate: f64,
}

fn seconds_per_year(kind: Kind) -> f64 {
    match kind {
        Kind::Crypto => 365.0 * 86_400.0,
        Kind::Fx => 260.0 * 86_400.0,
        _ => 252.0 * 6.5 * 3600.0,
    }
}

fn exp_draw(r: &mut ChaCha8Rng, rate: f64) -> f64 {
    let u: f64 = r.random();
    -(1.0 - u).ln() / rate.max(1e-6)
}

impl State {
    fn new(rate: f64) -> State {
        State {
            slots: Vec::with_capacity(2048),
            index: HashMap::with_capacity(2048),
            free: Vec::new(),
            pending: VecDeque::new(),
            w: [0.0; N_FACTORS],
            ccy: vec![0.0; CCYS.len()],
            ccy_ready: false,
            rate,
        }
    }

    fn unsubscribe(&mut self, keys: &[SecurityKey]) {
        for k in keys {
            self.pending.retain(|p| p != k);
            if let Some(i) = self.index.remove(k) {
                self.slots[i].active = false;
                self.free.push(i);
            }
        }
    }

    /// Initializes one pending subscription from the snapshot quote.
    /// Returns the full snapshot update to emit.
    fn init(&mut self, inner: &Inner, key: SecurityKey, t: f64, now_ns: i64, r: &mut ChaCha8Rng) -> Option<(SecurityKey, QuoteUpdate)> {
        if self.index.contains_key(&key) {
            return None;
        }
        let s = inner.resolve(&key).ok()?;
        let q = inner.quote(&s, now_ns)?;
        let last = q.last?;
        let price = if let Model::FxPair { base, quote } = s.p.model {
            {
                if !self.ccy_ready {
                    for (i, v) in self.ccy.iter_mut().enumerate() {
                        *v = inner.currency_level(i, now_ns).ln();
                    }
                    self.ccy_ready = true;
                }
                let ratio = (self.ccy[base] - self.ccy[quote]).exp();
                Price::Fx { base, quote, k: last / ratio }
            }
        } else {
            let factor = s.p.factor as usize;
            Price::Factor { factor, rho: s.p.rho.clamp(0.0, 1.0), w0: self.w[factor], idio: 0.0 }
        };
        let sigma = s.p.sigma / seconds_per_year(s.p.kind).sqrt();
        let tick = s.tick();
        let slot = Slot {
            key: key.clone(),
            active: true,
            price,
            base: last,
            sigma,
            tick,
            spread_ticks: ((last * s.p.spread_bps / 1e4) / tick).round().max(1.0),
            lot: s.p.lot,
            vol_decimals: s.p.vol_decimals,
            has_book: s.p.has_book,
            equity_like: matches!(s.p.kind, Kind::Equity | Kind::Etf),
            last,
            bid: q.bid.unwrap_or(last),
            ask: q.ask.unwrap_or(last),
            high: q.high.unwrap_or(last),
            low: q.low.unwrap_or(last),
            volume: q.volume.unwrap_or(0.0),
            pv: q.vwap.unwrap_or(last) * q.volume.unwrap_or(0.0),
            last_t: t,
            next_at: t + exp_draw(r, self.rate),
        };
        let i = if let Some(i) = self.free.pop() {
            self.slots[i] = slot;
            i
        } else {
            self.slots.push(slot);
            self.slots.len() - 1
        };
        self.index.insert(key.clone(), i);
        let update = QuoteUpdate {
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
            ts_event: now_ns,
        };
        Some((key, update))
    }

    fn walk(&mut self, dt: f64, r: &mut ChaCha8Rng) {
        let sq = dt.max(0.0).sqrt();
        for w in &mut self.w {
            let z: f64 = r.sample(StandardNormal);
            *w += sq * z;
        }
        if self.ccy_ready {
            let usd: f64 = r.sample(StandardNormal);
            for (i, c) in CCYS.iter().enumerate() {
                if c.vol == 0.0 {
                    continue;
                }
                let z: f64 = r.sample(StandardNormal);
                let s = c.vol / seconds_per_year(Kind::Fx).sqrt();
                self.ccy[i] += s * sq * (c.rho * usd + (1.0 - c.rho * c.rho).sqrt() * z);
            }
        }
    }

    fn mid(&mut self, i: usize, t: f64, r: &mut ChaCha8Rng) -> f64 {
        let w = self.w;
        let ccy = &self.ccy;
        let s = &mut self.slots[i];
        match &mut s.price {
            Price::Factor { factor, rho, w0, idio } => {
                let dt = (t - s.last_t).max(0.0);
                let z: f64 = r.sample(StandardNormal);
                *idio += dt.sqrt() * z;
                let x = *rho * (w[*factor] - *w0) + (1.0 - *rho * *rho).sqrt() * *idio;
                s.base * (s.sigma * x).exp()
            }
            Price::Fx { base, quote, k } => *k * (ccy[*base] - ccy[*quote]).exp(),
        }
    }

    fn step(&mut self, t: f64, now_ns: i64, r: &mut ChaCha8Rng, emit: &mut impl FnMut(&SecurityKey, QuoteUpdate)) {
        for i in 0..self.slots.len() {
            if !self.slots[i].active {
                continue;
            }
            let mut arrivals = 0;
            while self.slots[i].next_at <= t && arrivals < 5 {
                arrivals += 1;
                let at = self.slots[i].next_at;
                let mid = self.mid(i, at, r);
                let rate = self.rate;
                let s = &mut self.slots[i];
                s.last_t = at;
                s.next_at = at + exp_draw(r, rate);
                let ts = now_ns + ((at - t) * NANOS_PER_SEC as f64) as i64;
                let mut u = QuoteUpdate { ts_event: ts, ..QuoteUpdate::default() };
                if s.has_book {
                    let bid = round_tick(mid - s.spread_ticks * s.tick * 0.5, s.tick);
                    let ask = round_tick(bid + s.spread_ticks * s.tick, s.tick);
                    s.bid = bid;
                    s.ask = ask;
                    u.bid = Some(bid);
                    u.ask = Some(ask);
                    let n: i64 = r.random_range(1..=30);
                    let m: i64 = r.random_range(1..=30);
                    u.bid_size = Some(s.lot * n as f64);
                    u.ask_size = Some(s.lot * m as f64);
                }
                let trade = !s.has_book || r.random::<f64>() < 0.4;
                if trade {
                    let px = if s.has_book {
                        if r.random::<bool>() { s.ask } else { s.bid }
                    } else {
                        round_tick(mid, s.tick)
                    };
                    let size = if s.has_book {
                        if s.equity_like && r.random::<f64>() < 0.3 {
                            f64::from(r.random_range(1u32..100))
                        } else {
                            s.lot * f64::from(r.random_range(1u32..=10))
                        }
                    } else {
                        0.0
                    };
                    s.last = px;
                    u.last = Some(px);
                    if px > s.high {
                        s.high = px;
                        u.high = Some(px);
                    }
                    if px < s.low {
                        s.low = px;
                        u.low = Some(px);
                    }
                    if size > 0.0 {
                        let k = 10f64.powi(s.vol_decimals);
                        s.volume = ((s.volume + size) * k).round() / k;
                        s.pv += px * size;
                        u.last_size = Some(size);
                        u.volume = Some(s.volume);
                        if s.volume > 0.0 {
                            u.vwap = Some(round_tick(s.pv / s.volume, s.tick / 100.0));
                        }
                    }
                }
                emit(&self.slots[i].key, u);
            }
            if arrivals == 5 && self.slots[i].next_at <= t {
                // Fell behind (stall); resume from now instead of bursting.
                self.slots[i].next_at = t + exp_draw(r, self.rate);
            }
        }
    }
}

async fn run(inner: Arc<Inner>, sink: Arc<dyn EventSink>, mut rx: UnboundedReceiver<Cmd>, closed: Arc<AtomicBool>) {
    let id = ProviderId::new("mock");
    let start = tokio::time::Instant::now();
    let base_ns = inner.clock.now();
    let mut r = ChaCha8Rng::seed_from_u64(mix(&[inner.seed, tag("stream"), base_ns as u64]));
    let mut st = State::new(inner.stream_rate);
    let mut interval = tokio::time::interval(Duration::from_millis(10));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_t = 0.0;
    let mut emitted: u64 = 0;
    'outer: loop {
        tokio::select! {
            _ = interval.tick() => {}
            cmd = rx.recv() => match cmd {
                Some(Cmd::Subscribe(keys)) => st.pending.extend(keys),
                Some(Cmd::Unsubscribe(keys)) => st.unsubscribe(&keys),
                Some(Cmd::Close) | None => break 'outer,
            },
        }
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                Cmd::Subscribe(keys) => st.pending.extend(keys),
                Cmd::Unsubscribe(keys) => st.unsubscribe(&keys),
                Cmd::Close => break 'outer,
            }
        }
        if closed.load(Ordering::SeqCst) {
            break;
        }
        let t = start.elapsed().as_secs_f64();
        let now_ns = base_ns + (t * NANOS_PER_SEC as f64) as i64;
        st.walk(t - last_t, &mut r);
        last_t = t;
        // Initialize new subscriptions within a time budget.
        let budget = std::time::Instant::now();
        while let Some(key) = st.pending.pop_front() {
            if let Some((k, u)) = st.init(&inner, key, t, now_ns, &mut r) {
                sink.send(&id, StreamEvent::Quote { key: k, update: u });
                emitted += 1;
            }
            if budget.elapsed() > Duration::from_millis(4) {
                break;
            }
        }
        st.step(t, now_ns, &mut r, &mut |key, update| {
            sink.send(&id, StreamEvent::Quote { key: key.clone(), update });
            emitted += 1;
        });
    }
    tracing::debug!(events = emitted, "mock stream closed");
    sink.send(&id, StreamEvent::Status { connected: false, message: "MOCK stream closed".into() });
}

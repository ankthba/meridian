//! Bars and quote snapshots built from the daily and intraday generators.

use std::borrow::Cow;
use std::sync::Arc;

use chrono::{Datelike, Duration, NaiveDate};
use meridian_provider::{BarsRequest, ProviderError, ProviderResult};
use meridian_types::{
    Adjustment, Bar, BarInterval, BarSeries, Provenance, Quote, UnixNanos, date_to_nanos, quote_flags,
};

use crate::Inner;
use crate::cal::{SessionState, add_months, ymd};
use crate::cal::{Calendar, Session};
use crate::daily::{
    BarStyle, DayBar, GenSpec, Path, currency_path, day_bar_styled, day_bars_from_path, day_num, path_epoch,
    symbol_path, tracker_bars,
};
use crate::hash::{Cell, tag};
use crate::intraday::{aggregate, day_minutes, round_volume, summarize};
use crate::universe::{CCYS, Kind, Model, SPLITS, Sym};

/// Rounds to a multiple of `tick` (monotone, so OHLC ordering survives).
pub(crate) fn round_tick(x: f64, tick: f64) -> f64 {
    if tick <= 0.0 {
        return x;
    }
    let inv = (1.0 / tick).round();
    if (inv * tick - 1.0).abs() < 1e-9 {
        ((x * inv).round() / inv).max(tick)
    } else {
        ((x / tick).round() * tick).max(tick)
    }
}

/// Splits for a symbol, ascending by date.
pub(crate) fn splits_of(s: &Sym) -> Vec<(NaiveDate, f64)> {
    if s.p.kind != Kind::Equity || s.p.filler {
        return Vec::new();
    }
    SPLITS
        .iter()
        .filter(|(t, _, _)| *t == s.ticker())
        .map(|(_, (y, m, d), r)| (ymd(*y, *m, *d), *r))
        .collect()
}

/// Product of split ratios strictly after `d`: multiply path prices by this
/// to get prices as quoted on `d`.
pub(crate) fn split_factor_after(splits: &[(NaiveDate, f64)], d: NaiveDate) -> f64 {
    splits.iter().filter(|(sd, _)| *sd > d).map(|(_, r)| r).product()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Open,
    Closed,
}

/// A symbol's market state at an instant, with the bars needed to quote it.
pub(crate) struct Snapshot {
    /// Partial or complete summary of `day`'s minutes (path terms).
    pub summary: crate::intraday::Summary,
    pub prev_close: Option<f64>,
    pub phase: Phase,
    /// Multiplier from path terms to prices as quoted today.
    pub price_k: f64,
    pub session_close_ns: UnixNanos,
}

impl Inner {
    pub(crate) fn resolve(&self, key: &meridian_types::SecurityKey) -> ProviderResult<Cow<'_, Sym>> {
        if let Some(i) = self.universe.index_of(key) {
            return Ok(Cow::Borrowed(&self.universe.syms[i]));
        }
        if key.sector == meridian_types::MarketSector::Curncy
            && key.exchange.is_none()
            && let Some(s) = crate::universe::fx_sym(&key.symbol)
        {
            return Ok(Cow::Owned(s));
        }
        Err(ProviderError::NotFound(format!("{key} is not in the mock universe")))
    }

    pub(crate) fn path(&self, s: &Sym, until: NaiveDate) -> Path {
        let f = self.factors(until);
        symbol_path(self.seed, &self.universe.syms, s, &f, until)
    }

    /// Full-day bars in path terms for trading days in `[from, to]`.
    pub(crate) fn full_day_bars(&self, s: &Sym, from: NaiveDate, to: NaiveDate) -> Vec<DayBar> {
        if to < from {
            return Vec::new();
        }
        match s.p.model {
            Model::Tracks { under, ratio } => {
                let u = &self.universe.syms[under];
                let ub = self.full_day_bars(u, from, to);
                tracker_bars(self.seed, s, u, &ub, ratio)
            }
            Model::FxPair { base, quote } => self.fx_day_bars(s, base, quote, from, to),
            _ => {
                let p = self.path(s, to);
                day_bars_from_path(self.seed, s, &p, from, to)
            }
        }
    }

    /// The random stream and session used for a symbol's intraday path.
    /// Trackers sharing their underlying's session reuse its stream so the
    /// intraday shapes match.
    pub(crate) fn intraday_source(&self, s: &Sym) -> u64 {
        if let Model::Tracks { under, .. } = s.p.model {
            let u = &self.universe.syms[under];
            if u.p.session == s.p.session {
                return self.intraday_source(u);
            }
        }
        s.hash
    }

    pub(crate) fn minutes(&self, s: &Sym, bar: &DayBar) -> Vec<Bar> {
        if let Model::FxPair { base, quote } = s.p.model {
            return self.fx_minutes(base, quote, bar.date);
        }
        day_minutes(self.seed, self.intraday_source(s), &s.p.session, bar.date, bar, s.p.vol_decimals)
    }

    /// Last few full-day bars up to `today` (inclusive when it trades),
    /// cached per symbol and local date.
    pub(crate) fn recent(&self, s: &Sym, today: NaiveDate) -> Arc<Vec<DayBar>> {
        let key = (s.hash, today);
        if let Some(v) = self.recent.lock().get(&key) {
            return v.clone();
        }
        let from = today - Duration::days(20);
        let v = Arc::new(self.full_day_bars(s, from, today));
        let mut cache = self.recent.lock();
        if cache.len() > 50_000 {
            cache.clear();
        }
        cache.insert(key, v.clone());
        v
    }

    pub(crate) fn snapshot(&self, s: &Sym, now: UnixNanos) -> Option<Snapshot> {
        let cal = s.p.calendar;
        let st = SessionState::at(cal, &s.p.session, now);
        if st.today < s.p.listed {
            return None;
        }
        let recent = self.recent(s, st.today);
        let (day_idx, phase) = if st.is_open() {
            (recent.iter().position(|b| b.date == st.today)?, Phase::Open)
        } else {
            let last = st.last_complete_day(cal);
            let i = recent.iter().rposition(|b| b.date <= last)?;
            let phase = if s.p.session.is_24h() && st.today_trading { Phase::Open } else { Phase::Closed };
            (i, phase)
        };
        let day = recent[day_idx];
        let mut mins = self.minutes(s, &day);
        if st.is_open() && day.date == st.today {
            mins.truncate(st.elapsed.max(1) as usize);
        }
        let summary = summarize(&mins)?;
        let prev_close = day_idx.checked_sub(1).map(|i| recent[i].c);
        Some(Snapshot {
            summary,
            prev_close,
            phase,
            price_k: split_factor_after(&splits_of(s), st.today),
            session_close_ns: s.p.session.close_utc(day.date),
        })
    }

    pub(crate) fn quote(&self, s: &Sym, now: UnixNanos) -> Option<Quote> {
        let snap = self.snapshot(s, now)?;
        let tick = s.tick();
        let k = snap.price_k;
        let px = |x: f64| round_tick(x * k, tick);
        let sm = &snap.summary;
        let last = px(sm.close);
        let mut q = Quote::empty(s.key().clone(), Provenance::synthetic(now));
        q.last = Some(last);
        q.open = Some(px(sm.open));
        q.high = Some(px(sm.high));
        q.low = Some(px(sm.low));
        q.prev_close = snap.prev_close.map(px);
        q.volume = Some(round_volume(sm.volume / k, s.p.vol_decimals));
        q.vwap = sm.vwap.map(|v| round_tick(v * k, tick / 100.0));
        let minute = (sm.last_ts / crate::cal::NANOS_PER_MIN) as u64;
        let mut cell = Cell::new(&[self.seed, s.hash, tag("quote"), minute]);
        if s.p.has_book {
            let spread_ticks = ((last * s.p.spread_bps / 1e4) / tick).round().max(1.0);
            let below = cell.int(0, spread_ticks as i64) as f64;
            let bid = round_tick(last - below * tick, tick);
            let ask = round_tick(bid + spread_ticks * tick, tick);
            let (lo, hi) = match s.p.kind {
                Kind::Fx => (1, 10),
                Kind::Crypto | Kind::Future => (1, 40),
                _ => (1, 30),
            };
            q.bid = Some(bid);
            q.ask = Some(ask);
            q.bid_size = Some(s.p.lot * cell.int(lo, hi) as f64);
            q.ask_size = Some(s.p.lot * cell.int(lo, hi) as f64);
            q.last_size = Some(s.p.lot * cell.int(1, 10) as f64);
        }
        let mut flags = quote_flags::SYNTHETIC;
        if snap.phase == Phase::Closed {
            flags |= quote_flags::MARKET_CLOSED;
        }
        if let Some(pm) = sm.prev_minute_close {
            let pm = px(pm);
            if last > pm {
                flags |= quote_flags::TICK_UP;
            } else if last < pm {
                flags |= quote_flags::TICK_DOWN;
            }
        }
        q.flags = flags;
        q.ts_event = match snap.phase {
            Phase::Open => (sm.last_ts + crate::cal::NANOS_PER_MIN).min(now),
            Phase::Closed => snap.session_close_ns.min(now),
        };
        q.ts_recv = now;
        Some(q)
    }

    pub(crate) fn bars(&self, req: &BarsRequest) -> ProviderResult<BarSeries> {
        let s = self.resolve(&req.key)?;
        let now = self.clock.now();
        let mut series = BarSeries::new(s.key().clone(), req.interval, req.adjustment, Provenance::synthetic(now));
        series.provenance.source_ref = Some(format!("mock seed={}", self.seed));
        match req.interval {
            BarInterval::Minute(n) => self.intraday_bars(&s, req, i64::from(n.max(1)), now, &mut series),
            BarInterval::Hour(n) => self.intraday_bars(&s, req, 60 * i64::from(n.max(1)), now, &mut series),
            BarInterval::Day | BarInterval::Week | BarInterval::Month => self.daily_bars(&s, req, now, &mut series),
        }
        Ok(series)
    }

    fn daily_bars(&self, s: &Sym, req: &BarsRequest, now: UnixNanos, out: &mut BarSeries) {
        let cal = s.p.calendar;
        let st = SessionState::at(cal, &s.p.session, now);
        let today = st.today;
        let mut from = add_months(today, -240).max(path_epoch()).max(s.p.listed);
        if let Some(f) = req.from {
            from = from.max(meridian_types::nanos_to_date(f));
        }
        let to_ns = req.to.unwrap_or(i64::MAX);
        let last_full = st.last_complete_day(cal);
        let mut days = self.full_day_bars(s, from, if st.is_open() { today } else { last_full });
        if st.is_open()
            && let Some(last) = days.last_mut()
            && last.date == today
        {
            // Replace today's full-day bar with the session so far.
            let mut mins = self.minutes(s, last);
            mins.truncate(st.elapsed.max(1) as usize);
            if let Some(sm) = summarize(&mins) {
                *last = DayBar { date: today, o: sm.open, h: sm.high, l: sm.low, c: sm.close, v: sm.volume };
            }
        }
        let factors = self.adjustment_factors(s, req.adjustment, today, &days);
        let tick = s.tick();
        let ptick = if req.adjustment == Adjustment::SplitsAndDividends { tick / 100.0 } else { tick };
        let rows: Vec<Bar> = days
            .iter()
            .zip(&factors)
            .filter_map(|(b, (pk, vk))| {
                let ts = date_to_nanos(b.date);
                if ts >= to_ns || req.from.is_some_and(|f| ts < f) {
                    return None;
                }
                Some(Bar {
                    ts,
                    open: round_tick(b.o * pk, ptick),
                    high: round_tick(b.h * pk, ptick),
                    low: round_tick(b.l * pk, ptick),
                    close: round_tick(b.c * pk, ptick),
                    volume: round_volume(b.v / vk, s.p.vol_decimals),
                })
            })
            .collect();
        match req.interval {
            BarInterval::Week => aggregate_calendar(&rows, |d| d.iso_week().year() * 100 + d.iso_week().week() as i32, out),
            BarInterval::Month => aggregate_calendar(&rows, |d| d.year() * 100 + d.month() as i32, out),
            _ => {
                for r in rows {
                    out.push(r);
                }
            }
        }
    }

    /// Per-bar (price multiplier, volume divisor) for the requested
    /// adjustment, as of `today`.
    fn adjustment_factors(&self, s: &Sym, adj: Adjustment, today: NaiveDate, days: &[DayBar]) -> Vec<(f64, f64)> {
        let splits = splits_of(s);
        let k_today = split_factor_after(&splits, today);
        match adj {
            Adjustment::None => days
                .iter()
                .map(|b| {
                    let k = split_factor_after(&splits, b.date);
                    (k, k)
                })
                .collect(),
            Adjustment::Splits => vec![(k_today, k_today); days.len()],
            Adjustment::SplitsAndDividends => {
                let divs = self.dividend_events(s, today);
                let mut out = vec![(k_today, k_today); days.len()];
                let mut cum = 1.0;
                let mut di = divs.len();
                // Walk backwards; apply each dividend's factor to bars before its ex-date.
                for i in (0..days.len()).rev() {
                    while di > 0 && divs[di - 1].ex > days[i].date {
                        let d = &divs[di - 1];
                        if d.ex <= today {
                            let prev_close = days[i].c;
                            if prev_close > d.amount_path {
                                cum *= 1.0 - d.amount_path / prev_close;
                            }
                        }
                        di -= 1;
                    }
                    out[i].0 = k_today * cum;
                }
                out
            }
        }
    }

    fn intraday_bars(&self, s: &Sym, req: &BarsRequest, bucket: i64, now: UnixNanos, out: &mut BarSeries) {
        let cal = s.p.calendar;
        let st = SessionState::at(cal, &s.p.session, now);
        let today = st.today;
        let cap: usize = match bucket {
            1 => 60,
            2..=5 => 120,
            6..=30 => 250,
            _ => 500,
        };
        let to_ns = req.to.map_or(now + 1, |t| t.min(now + 1));
        let to_date = s.p.session.tz.to_local(to_ns - 1).0.min(today);
        let from_date = match req.from {
            Some(f) => s.p.session.tz.to_local(f).0,
            None => to_date - Duration::days(cap as i64 * 2 + 10),
        }
        .max(s.p.listed);
        let mut days = cal.days(from_date, to_date);
        if days.len() > cap {
            days.drain(..days.len() - cap);
        }
        let (Some(first), Some(last)) = (days.first().copied(), days.last().copied()) else {
            return;
        };
        let full = self.full_day_bars(s, first, last);
        let splits = splits_of(s);
        let k_today = split_factor_after(&splits, today);
        let tick = s.tick();
        let from_ns = req.from.unwrap_or(i64::MIN);
        let mut agg = Vec::with_capacity(full.len() * (s.p.session.minutes() as usize / bucket as usize + 1));
        for b in &full {
            if b.date == today && !st.today_complete() && !st.is_open() {
                continue; // pre-open: nothing yet
            }
            let mut mins = self.minutes(s, b);
            if b.date == today && st.is_open() {
                mins.truncate(st.elapsed.max(1) as usize);
            }
            let k = match req.adjustment {
                Adjustment::None => split_factor_after(&splits, b.date),
                _ => k_today,
            };
            for m in &mut mins {
                m.open = round_tick(m.open * k, tick);
                m.high = round_tick(m.high * k, tick);
                m.low = round_tick(m.low * k, tick);
                m.close = round_tick(m.close * k, tick);
                m.volume = round_volume(m.volume / k, s.p.vol_decimals);
            }
            agg.clear();
            aggregate(&mins, s.p.session.open_utc(b.date), bucket, &mut agg);
            for a in &agg {
                if a.ts >= from_ns && a.ts < to_ns {
                    let mut a = *a;
                    a.volume = round_volume(a.volume, s.p.vol_decimals);
                    out.push(a);
                }
            }
        }
    }

    /// Latest close in quoted terms (for option chains, holders, …).
    pub(crate) fn last_price(&self, s: &Sym, now: UnixNanos) -> Option<f64> {
        self.quote(s, now).and_then(|q| q.last)
    }

    /// Closes in path terms (ascending) for trading days in `[from, to]`.
    pub(crate) fn closes(&self, s: &Sym, from: NaiveDate, to: NaiveDate) -> Vec<(NaiveDate, f64)> {
        self.full_day_bars(s, from, to).iter().map(|b| (b.date, b.c)).collect()
    }
}

impl Inner {
    fn currency_style(i: usize) -> BarStyle {
        BarStyle { hash: GenSpec::currency(i).hash, continuous: true, adv: 0.0 }
    }

    /// Daily bars of a currency's USD value (weekdays).
    pub(crate) fn currency_day_bars(&self, i: usize, from: NaiveDate, to: NaiveDate) -> Vec<DayBar> {
        if CCYS[i].vol == 0.0 {
            return Calendar::Weekdays
                .days(from, to)
                .into_iter()
                .map(|date| DayBar { date, o: 1.0, h: 1.0, l: 1.0, c: 1.0, v: 0.0 })
                .collect();
        }
        let f = self.factors(to);
        let p = currency_path(self.seed, i, &f, to);
        let start = p.days.partition_point(|d| *d < from);
        let end = p.days.partition_point(|d| *d <= to);
        (start..end).map(|k| day_bar_styled(self.seed, Self::currency_style(i), &p, k)).collect()
    }

    fn currency_minutes(&self, i: usize, bar: &DayBar) -> Vec<Bar> {
        if CCYS[i].vol == 0.0 {
            let open = Session::UTC_24H.open_utc(bar.date);
            return (0..1440)
                .map(|j| Bar {
                    ts: open + j * crate::cal::NANOS_PER_MIN,
                    open: 1.0,
                    high: 1.0,
                    low: 1.0,
                    close: 1.0,
                    volume: 0.0,
                })
                .collect();
        }
        day_minutes(self.seed, GenSpec::currency(i).hash, &Session::UTC_24H, bar.date, bar, 0)
    }

    /// FX pair bars as ratios of currency bars: opens and closes triangulate
    /// exactly across all pairs.
    fn fx_day_bars(&self, s: &Sym, base: usize, quote: usize, from: NaiveDate, to: NaiveDate) -> Vec<DayBar> {
        let a = self.currency_day_bars(base, from, to);
        let b = self.currency_day_bars(quote, from, to);
        let sd = s.p.sigma / 260f64.sqrt();
        a.iter()
            .zip(&b)
            .map(|(x, y)| {
                let o = x.o / y.o;
                let c = x.c / y.c;
                let mut cell = Cell::new(&[self.seed, s.hash, tag("fxhl"), day_num(x.date)]);
                let e1 = -(1.0 - cell.u01()).ln();
                let e2 = -(1.0 - cell.u01()).ln();
                DayBar {
                    date: x.date,
                    o,
                    h: o.max(c) * (0.45 * sd * e1).exp(),
                    l: o.min(c) * (-0.45 * sd * e2).exp(),
                    c,
                    v: 0.0,
                }
            })
            .collect()
    }

    fn fx_minutes(&self, base: usize, quote: usize, date: NaiveDate) -> Vec<Bar> {
        let (Some(ab), Some(bb)) = (
            self.currency_day_bars(base, date, date).first().copied(),
            self.currency_day_bars(quote, date, date).first().copied(),
        ) else {
            return Vec::new();
        };
        let ma = self.currency_minutes(base, &ab);
        let mb = self.currency_minutes(quote, &bb);
        ma.iter()
            .zip(&mb)
            .map(|(x, y)| {
                let o = x.open / y.open;
                let c = x.close / y.close;
                let up = (x.high / x.open.max(x.close)) * (y.open.min(y.close) / y.low);
                let dn = (x.open.min(x.close) / x.low) * (y.high / y.open.max(y.close));
                Bar { ts: x.ts, open: o, high: o.max(c) * up.sqrt(), low: o.min(c) / dn.sqrt(), close: c, volume: 0.0 }
            })
            .collect()
    }

    /// A currency's USD value at `now` (the current minute's close).
    pub(crate) fn currency_level(&self, i: usize, now: UnixNanos) -> f64 {
        if CCYS[i].vol == 0.0 {
            return 1.0;
        }
        let st = SessionState::at(Calendar::Weekdays, &Session::UTC_24H, now);
        let day = if st.today_trading { st.today } else { Calendar::Weekdays.before(st.today) };
        let Some(bar) = self.currency_day_bars(i, day, day).first().copied() else {
            return CCYS[i].usd_value;
        };
        if day != st.today {
            return bar.c;
        }
        let mins = self.currency_minutes(i, &bar);
        let k = (st.elapsed.max(1) as usize).min(mins.len()) - 1;
        mins[k].close
    }
}

fn aggregate_calendar(rows: &[Bar], bucket: impl Fn(NaiveDate) -> i32, out: &mut BarSeries) {
    let mut cur: Option<(i32, Bar)> = None;
    for r in rows {
        let k = bucket(meridian_types::nanos_to_date(r.ts));
        match cur.as_mut() {
            Some((ck, b)) if *ck == k => {
                b.high = b.high.max(r.high);
                b.low = b.low.min(r.low);
                b.close = r.close;
                b.volume += r.volume;
            }
            _ => {
                if let Some((_, b)) = cur.take() {
                    out.push(b);
                }
                cur = Some((k, *r));
            }
        }
    }
    if let Some((_, b)) = cur {
        out.push(b);
    }
}

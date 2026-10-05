//! Integration tests for the mock provider's public surface.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Datelike, NaiveDate, TimeZone, Utc, Weekday};
use meridian_provider::{
    BarsRequest, CalendarRequest, Capability, ChainRequest, CurveRequest, EventSink, FilingsRequest,
    FundamentalsRequest, InstrumentQuery, NewsQuery, NewsScope, Provider, SeriesRequest,
};
use meridian_provider_mock::{FILING_BANNER, MOCK_PREFIX, MockConfig, MockProvider};
use meridian_types::{
    Adjustment, BarInterval, DataDelay, FeedSource, FixedClock, MarketSector, PeriodType, ProviderId, SecurityKey,
    StatementKind, StreamEvent, UnixNanos, datetime_to_nanos, nanos_to_date, quote_flags,
};
use parking_lot::Mutex;

/// Monday 2026-10-05 18:00 UTC = 14:00 New York (market open).
fn t_open() -> UnixNanos {
    datetime_to_nanos(Utc.with_ymd_and_hms(2026, 10, 5, 18, 0, 0).unwrap())
}

fn provider_at(seed: u64, now: UnixNanos) -> MockProvider {
    MockProvider::new(MockConfig { seed, clock: Arc::new(FixedClock(now)), ..MockConfig::default() })
}

fn daily(key: SecurityKey) -> BarsRequest {
    BarsRequest { key, interval: BarInterval::Day, from: None, to: None, adjustment: Adjustment::Splits }
}

fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

#[tokio::test]
async fn deterministic_for_same_seed_and_clock() {
    let a = provider_at(11, t_open());
    let b = provider_at(11, t_open());
    let c = provider_at(12, t_open());
    let key = SecurityKey::equity("AAPL");
    let ba = a.bars(&daily(key.clone())).await.unwrap();
    let bb = b.bars(&daily(key.clone())).await.unwrap();
    assert_eq!(ba, bb);
    assert_ne!(ba.close, c.bars(&daily(key.clone())).await.unwrap().close);

    let m = BarsRequest { interval: BarInterval::Minute(5), ..daily(key.clone()) };
    assert_eq!(a.bars(&m).await.unwrap(), b.bars(&m).await.unwrap());
    assert_eq!(a.quotes(std::slice::from_ref(&key)).await.unwrap(), b.quotes(std::slice::from_ref(&key)).await.unwrap());
    let chain = ChainRequest { underlying: key.clone(), expiry: None };
    assert_eq!(a.option_chain(&chain).await.unwrap(), b.option_chain(&chain).await.unwrap());
    let f = FundamentalsRequest { key: key.clone(), period_type: PeriodType::Quarterly, periods: 0 };
    assert_eq!(a.fundamentals(&f).await.unwrap(), b.fundamentals(&f).await.unwrap());
    let n = NewsQuery { scope: NewsScope::Top, keys: vec![], text: None, from: None, to: None, limit: 50 };
    assert_eq!(a.news(&n).await.unwrap(), b.news(&n).await.unwrap());

    // History does not change when the clock moves forward.
    let later = provider_at(11, t_open() + 30 * 86_400 * 1_000_000_000);
    let bl = later.bars(&daily(key)).await.unwrap();
    let cut = ba.ts.len() - 1; // today's partial bar differs
    let mut overlap = 0;
    for i in 0..cut {
        if let Ok(j) = bl.ts.binary_search(&ba.ts[i]) {
            assert_eq!((bl.open[j], bl.high[j], bl.low[j], bl.close[j]), (ba.open[i], ba.high[i], ba.low[i], ba.close[i]));
            overlap += 1;
        }
    }
    assert!(overlap > 4000, "{overlap}");
}

#[tokio::test]
async fn everything_is_labeled_synthetic() {
    let p = provider_at(1, t_open());
    assert!(p.universe().iter().all(|i| i.is_synthetic));
    assert!(p.universe().len() > 200);
    let caps = p.capabilities();
    assert_eq!(caps.entries.len(), 20);
    assert!(caps.supports(Capability::Fundamentals, None));
    assert!(caps.entries.iter().all(|e| e.delay == DataDelay::Synthetic && e.source == FeedSource::Synthetic));
    assert!(!caps.requires_credentials);
    let keys: Vec<SecurityKey> = p.universe().iter().take(150).map(|i| i.key.clone()).collect();
    let quotes = p.quotes(&keys).await.unwrap();
    assert!(quotes.len() > 140, "{}", quotes.len());
    for q in &quotes {
        assert!(q.flags & quote_flags::SYNTHETIC != 0);
        assert!(q.provenance.synthetic);
        assert_eq!(q.provenance.provider, ProviderId::new("mock"));
        let (last, low, high) = (q.last.unwrap(), q.low.unwrap(), q.high.unwrap());
        assert!(low <= last && last <= high, "{}: {low} {last} {high}", q.key);
        if let (Some(b), Some(a)) = (q.bid, q.ask) {
            assert!(b < a, "{}", q.key);
        }
    }
    // US equities are open at 14:00 New York on a Monday.
    let aapl = quotes.iter().find(|q| q.key.symbol == "AAPL").unwrap();
    assert_eq!(aapl.flags & quote_flags::MARKET_CLOSED, 0);
}

#[tokio::test]
async fn bars_are_valid_and_follow_calendars() {
    let p = provider_at(3, t_open());
    let today = nanos_to_date(t_open());
    for inst in p.universe() {
        let s = p.bars(&daily(inst.key.clone())).await.unwrap();
        s.validate().unwrap_or_else(|e| panic!("{}: {e}", inst.key));
        assert!(s.provenance.synthetic);
        assert!(s.len() > 1000, "{} has {} bars", inst.key, s.len());
        let last = nanos_to_date(*s.ts.last().unwrap());
        assert!(last <= today && (today - last).num_days() <= 4, "{}: last bar {last}", inst.key);
        let dates: Vec<NaiveDate> = s.ts.iter().map(|t| nanos_to_date(*t)).collect();
        match inst.key.sector {
            MarketSector::Equity => {
                assert!(dates.iter().all(|d| !matches!(d.weekday(), Weekday::Sat | Weekday::Sun)), "{}", inst.key);
                for h in [ymd(2025, 12, 25), ymd(2026, 7, 3), ymd(2026, 4, 3), ymd(2025, 1, 9), ymd(2024, 11, 28)] {
                    assert!(!dates.contains(&h), "{} trades on holiday {h}", inst.key);
                }
                assert_eq!(last, today, "{}", inst.key);
            }
            MarketSector::Curncy if inst.asset_class == meridian_types::AssetClass::Crypto => {
                assert!(dates.iter().any(|d| d.weekday() == Weekday::Sun));
            }
            MarketSector::Curncy => {
                assert!(dates.iter().all(|d| !matches!(d.weekday(), Weekday::Sat | Weekday::Sun)));
            }
            _ => {}
        }
        // About 20 years at most.
        assert!((today - dates[0]).num_days() <= 20 * 366);
    }
}

#[tokio::test]
async fn intraday_bars_respect_sessions_and_dst() {
    let p = provider_at(5, t_open());
    let key = SecurityKey::equity("MSFT");
    let req = BarsRequest { interval: BarInterval::Minute(1), ..daily(key.clone()) };
    let s = p.bars(&req).await.unwrap();
    s.validate().unwrap();
    let days: HashSet<NaiveDate> = s.ts.iter().map(|t| nanos_to_date(*t)).collect();
    assert!(days.len() <= 60 && days.len() >= 55, "{} days", days.len());
    assert!(*s.ts.last().unwrap() < t_open());
    // Summer: 09:30 New York = 13:30 UTC. Winter: 14:30 UTC.
    let first_on = |d: NaiveDate| s.ts.iter().copied().find(|t| nanos_to_date(*t) == d);
    let summer = first_on(ymd(2026, 9, 1)).unwrap();
    assert_eq!(Utc.timestamp_nanos(summer).format("%H:%M").to_string(), "13:30");
    let w = BarsRequest {
        interval: BarInterval::Minute(30),
        from: Some(datetime_to_nanos(Utc.with_ymd_and_hms(2026, 1, 5, 0, 0, 0).unwrap())),
        to: Some(datetime_to_nanos(Utc.with_ymd_and_hms(2026, 1, 6, 0, 0, 0).unwrap())),
        ..daily(key.clone())
    };
    let ws = p.bars(&w).await.unwrap();
    assert_eq!(ws.len(), 13);
    assert_eq!(Utc.timestamp_nanos(ws.ts[0]).format("%H:%M").to_string(), "14:30");
    // Intraday aggregates back to the daily bar.
    let d = p.bars(&daily(key.clone())).await.unwrap();
    let i = d.ts.iter().position(|t| nanos_to_date(*t) == ymd(2026, 1, 5)).unwrap();
    let hi = ws.high.iter().copied().fold(f64::MIN, f64::max);
    let lo = ws.low.iter().copied().fold(f64::MAX, f64::min);
    assert!((hi - d.high[i]).abs() < 1e-6 && (lo - d.low[i]).abs() < 1e-6);
    assert!((ws.open[0] - d.open[i]).abs() < 1e-6 && (ws.close[12] - d.close[i]).abs() < 1e-6);
    assert!((ws.volume.iter().sum::<f64>() - d.volume[i]).abs() < 1.0);
    // Hourly bars, crypto 24/7.
    let h = p.bars(&BarsRequest { interval: BarInterval::Hour(1), ..daily(SecurityKey::currency("BTCUSD")) }).await.unwrap();
    h.validate().unwrap();
    assert!(h.ts.iter().any(|t| nanos_to_date(*t).weekday() == Weekday::Sat));
}

#[tokio::test]
async fn splits_and_adjustments() {
    let p = provider_at(9, t_open());
    let key = SecurityKey::equity("NVDA");
    let adj = p.bars(&daily(key.clone())).await.unwrap();
    let raw = p.bars(&BarsRequest { adjustment: Adjustment::None, ..daily(key.clone()) }).await.unwrap();
    let tr = p.bars(&BarsRequest { adjustment: Adjustment::SplitsAndDividends, ..daily(key.clone()) }).await.unwrap();
    raw.validate().unwrap();
    tr.validate().unwrap();
    let before = raw.ts.iter().position(|t| nanos_to_date(*t) == ymd(2024, 6, 7)).unwrap();
    let ratio = raw.close[before] / adj.close[before];
    assert!((ratio - 10.0).abs() < 0.01, "{ratio}");
    let last = adj.len() - 1;
    assert_eq!(raw.close[last], adj.close[last]);
    assert!(tr.close[100] <= adj.close[100]);
    let divs = p.dividends(&key).await.unwrap();
    assert!(divs.dividends.iter().any(|d| d.kind == meridian_types::DividendKind::Split && d.ex_date == ymd(2024, 6, 10)));
    let weekly = p.bars(&BarsRequest { interval: BarInterval::Week, ..daily(key.clone()) }).await.unwrap();
    weekly.validate().unwrap();
    assert!(weekly.len() > 900 && weekly.len() < 1100);
}

#[tokio::test]
async fn accounting_identities_hold_exactly() {
    let p = provider_at(4, t_open());
    for t in ["AAPL", "JPM", "XOM", "WMT", "NVDA", "NEE", "PLTR", "ZQ0001"] {
        let p = if t == "ZQ0001" {
            MockProvider::new(MockConfig { seed: 4, clock: Arc::new(FixedClock(t_open())), extra_symbols: 2, ..MockConfig::default() })
        } else {
            provider_at(4, t_open())
        };
        for pt in [PeriodType::Annual, PeriodType::Quarterly] {
            let f = p.fundamentals(&FundamentalsRequest { key: SecurityKey::equity(t), period_type: pt, periods: 0 }).await.unwrap();
            let n_bal = f.statements.iter().filter(|s| s.kind == StatementKind::Balance).count();
            assert_eq!(n_bal, if pt == PeriodType::Annual { 10 } else { 40 }, "{t}");
            for s in &f.statements {
                let v = |c: &str| s.value(c).unwrap_or_else(|| panic!("{t} missing {c}"));
                match s.kind {
                    StatementKind::Balance => {
                        assert_eq!(v("total_assets"), v("total_liabilities") + v("total_equity"), "{t} {}", s.period_end);
                        assert_eq!(
                            v("total_current_assets"),
                            v("cash") + v("short_term_investments") + v("receivables") + v("inventory") + v("other_current_assets")
                        );
                        assert_eq!(v("total_current_liabilities"), v("accounts_payable") + v("short_term_debt") + v("other_current_liabilities"));
                    }
                    StatementKind::Income => {
                        assert_eq!(v("gross_profit"), v("revenue") - v("cost_of_revenue"));
                        assert_eq!(v("operating_income"), v("gross_profit") - v("sga") - v("rnd"));
                        assert_eq!(v("net_income"), v("pretax_income") - v("income_tax"));
                        assert!(v("revenue") > 0.0 && v("shares_diluted") > 0.0);
                    }
                    StatementKind::CashFlow => {
                        assert_eq!(v("net_change_cash"), v("cfo") + v("cfi") + v("cff"));
                        assert_eq!(v("fcf"), v("cfo") + v("capex"));
                        assert!(v("capex") <= 0.0);
                    }
                }
            }
        }
    }
    // Cash roll-forward links the statements.
    let f = p.fundamentals(&FundamentalsRequest { key: SecurityKey::equity("MSFT"), period_type: PeriodType::Quarterly, periods: 8 }).await.unwrap();
    let bal: Vec<_> = f.statements.iter().filter(|s| s.kind == StatementKind::Balance).collect();
    let cf: Vec<_> = f.statements.iter().filter(|s| s.kind == StatementKind::CashFlow).collect();
    for i in 1..bal.len() {
        assert_eq!(bal[i].value("cash").unwrap() - bal[i - 1].value("cash").unwrap(), cf[i].value("net_change_cash").unwrap());
    }
}

#[tokio::test]
async fn quarterly_flows_sum_to_annual() {
    let p = provider_at(8, t_open());
    for t in ["AAPL", "MSFT", "NVDA", "KO"] {
        let key = SecurityKey::equity(t);
        let a = p.fundamentals(&FundamentalsRequest { key: key.clone(), period_type: PeriodType::Annual, periods: 0 }).await.unwrap();
        let q = p.fundamentals(&FundamentalsRequest { key, period_type: PeriodType::Quarterly, periods: 0 }).await.unwrap();
        let annual: Vec<_> = a.statements.iter().filter(|s| s.kind == StatementKind::Income).collect();
        let mut checked = 0;
        for fy in annual {
            let qs: Vec<_> = q.statements.iter().filter(|s| s.kind == StatementKind::Income && s.fiscal_year == fy.fiscal_year).collect();
            if qs.len() != 4 {
                continue;
            }
            for code in ["revenue", "cost_of_revenue", "operating_income", "net_income", "ebitda"] {
                let sum: f64 = qs.iter().map(|s| s.value(code).unwrap()).sum();
                let ann = fy.value(code).unwrap();
                assert!((sum - ann).abs() <= ann.abs() * 1e-9 + 1.0, "{t} FY{} {code}: {sum} vs {ann}", fy.fiscal_year);
            }
            checked += 1;
        }
        assert!(checked >= 8, "{t}: {checked}");
    }
    // Apple's fiscal year ends in September.
    let a = p.fundamentals(&FundamentalsRequest { key: SecurityKey::equity("AAPL"), period_type: PeriodType::Annual, periods: 1 }).await.unwrap();
    assert_eq!(a.statements[0].period_end.month(), 9);
}

#[tokio::test]
async fn estimates_earnings_and_company_data() {
    let p = provider_at(6, t_open());
    let key = SecurityKey::equity("AAPL");
    let e = p.estimates(&key).await.unwrap();
    assert_eq!(e.estimates.len(), 18);
    let ern = p.earnings(&key).await.unwrap();
    assert_eq!(ern.records.iter().filter(|r| r.eps_actual.is_some()).count(), 12);
    assert!(ern.records.iter().all(|r| r.announce_date.unwrap() > r.period_end));
    let last_reported = ern.records.iter().filter(|r| r.eps_actual.is_some()).map(|r| r.period_end).max().unwrap();
    assert!(ern.records.iter().filter(|r| r.eps_actual.is_some()).all(|r| r.announce_date.unwrap() <= nanos_to_date(t_open())));
    for x in &e.estimates {
        let (lo, m, hi) = (x.low.unwrap(), x.mean.unwrap(), x.high.unwrap());
        assert!(lo <= m && m <= hi, "{x:?}");
        assert!(x.period_end > last_reported, "{x:?}");
        assert!(x.count.unwrap() >= 1 && x.actual.is_none());
    }
    let recs = p.recommendations(&key).await.unwrap();
    assert!(recs.total() >= 15 && (10..=25).contains(&recs.ratings.len()));
    let holders = p.holders(&key).await.unwrap();
    let inst_pct: f64 = holders.holders.iter().filter(|h| h.kind != meridian_types::HolderKind::Insider).filter_map(|h| h.pct_outstanding).sum();
    assert!(inst_pct < 80.0 && inst_pct > 20.0, "{inst_pct}");
    assert_eq!(holders.holders.iter().filter(|h| h.kind == meridian_types::HolderKind::Insider).count(), 10);
    let tr = p.transcripts(&key).await.unwrap();
    assert_eq!(tr.len(), 4);
    assert!(tr.iter().all(|t| t.segments.len() >= 10 && t.segments[0].text.contains("MOCK")));
    let prof = p.profile(&key).await.unwrap();
    assert!(prof.description.unwrap().starts_with("[MOCK]"));
    assert!(p.fundamentals(&FundamentalsRequest { key: SecurityKey::currency("EURUSD"), period_type: PeriodType::Annual, periods: 1 }).await.is_err());
}

#[tokio::test]
async fn option_chain_sanity() {
    let p = provider_at(2, t_open());
    for key in [SecurityKey::equity("AAPL"), SecurityKey::index("SPX"), SecurityKey::equity("SPY")] {
        let chain = p.option_chain(&ChainRequest { underlying: key.clone(), expiry: None }).await.unwrap();
        let spot = chain.underlying_price.unwrap();
        let exps = chain.expiries();
        assert!(exps.len() >= 15, "{key}: {} expiries", exps.len());
        for e in &exps {
            let mut calls: Vec<_> = chain.contracts.iter().filter(|c| c.expiry == *e && c.right == meridian_types::OptionRight::Call).collect();
            let mut puts: Vec<_> = chain.contracts.iter().filter(|c| c.expiry == *e && c.right == meridian_types::OptionRight::Put).collect();
            calls.sort_by(|a, b| a.strike.total_cmp(&b.strike));
            puts.sort_by(|a, b| a.strike.total_cmp(&b.strike));
            assert!(calls.first().unwrap().strike <= spot * 0.65 && calls.last().unwrap().strike >= spot * 1.35);
            for w in calls.windows(2) {
                assert!(w[1].mid().unwrap() <= w[0].mid().unwrap() + 1e-9, "{key} {e} call {} > {}", w[1].strike, w[0].strike);
                let d = w[1].greeks.as_ref().unwrap().delta.unwrap();
                assert!((0.0..=1.0).contains(&d));
            }
            for w in puts.windows(2) {
                assert!(w[1].mid().unwrap() + 1e-9 >= w[0].mid().unwrap(), "{key} {e} put");
            }
            // Put-call parity against the pricing inputs (r, q, T) is checked
            // in the crate's unit tests.
            for (c, pp) in calls.iter().zip(&puts) {
                assert_eq!(c.strike, pp.strike);
                assert!(c.bid.unwrap() <= c.ask.unwrap() && c.bid.unwrap() >= 0.0);
                let g = c.greeks.as_ref().unwrap();
                assert!(g.iv.unwrap() > 0.0 && g.gamma.unwrap() >= 0.0);
                assert!(matches!(g.source, meridian_types::GreeksSource::Computed { ref model } if model == "mock-bs"));
            }
        }
        assert!(chain.provenance.synthetic);
    }
    // Unknown expiry is a clear error.
    let bad = p.option_chain(&ChainRequest { underlying: SecurityKey::equity("AAPL"), expiry: Some(ymd(2026, 10, 7)) }).await;
    assert!(bad.is_err());
}

#[tokio::test]
async fn filings_and_documents_are_marked() {
    let p = provider_at(10, t_open());
    let key = SecurityKey::equity("AAPL");
    let page = p.filings(&FilingsRequest { key: Some(key.clone()), forms: vec![], limit: 0 }).await.unwrap();
    assert!(page.filings.len() >= 30 && page.filings.len() <= 60, "{}", page.filings.len());
    let forms: HashSet<&str> = page.filings.iter().map(|f| f.form.as_str()).collect();
    for f in ["10-K", "10-Q", "8-K", "DEF 14A", "4"] {
        assert!(forms.contains(f), "missing {f}");
    }
    assert!(page.filings.windows(2).all(|w| w[0].filed >= w[1].filed));
    let tenks: Vec<_> = page.filings.iter().filter(|f| f.form == "10-K").collect();
    assert!(tenks.len() >= 2);
    let d0 = p.filing_document(tenks[0]).await.unwrap();
    let d1 = p.filing_document(tenks[1]).await.unwrap();
    assert!(d0.full_text().starts_with(FILING_BANNER));
    assert!(d0.sections.iter().any(|s| s.title.starts_with("Item 1A")));
    assert!(d0.sections.iter().any(|s| s.title.starts_with("Item 7.")));
    assert!(d0.sections.iter().any(|s| s.title.starts_with("Item 8")));
    assert_ne!(d0.full_text(), d1.full_text());
    let risks = |d: &meridian_types::FilingDocument| d.sections.iter().find(|s| s.title.starts_with("Item 1A")).unwrap().text.clone();
    let (r0, r1) = (risks(&d0), risks(&d1));
    let p0: HashSet<&str> = r0.split("\n\n").collect();
    let p1: HashSet<&str> = r1.split("\n\n").collect();
    assert!(p0.intersection(&p1).count() >= 3, "consecutive 10-Ks should share paragraphs");
    assert!(p0.symmetric_difference(&p1).count() >= 2, "and differ in some");
    for f in page.filings.iter().take(12) {
        let d = p.filing_document(f).await.unwrap();
        assert_eq!(d.sections[0].title, FILING_BANNER);
    }
    let latest = p.filings(&FilingsRequest { key: None, forms: vec!["8-K".into()], limit: 20 }).await.unwrap();
    assert!(!latest.filings.is_empty() && latest.filings.iter().all(|f| f.form == "8-K"));
}

#[tokio::test]
async fn news_is_marked_and_scoped() {
    let p = provider_at(13, t_open());
    for scope in [NewsScope::Top, NewsScope::Market, NewsScope::Company, NewsScope::PressReleases] {
        let q = NewsQuery { scope, keys: vec![SecurityKey::equity("AAPL"), SecurityKey::equity("JPM")], text: None, from: None, to: None, limit: 100 };
        let page = p.news(&q).await.unwrap();
        assert!(!page.items.is_empty(), "{scope:?}");
        for it in &page.items {
            assert!(it.headline.starts_with(MOCK_PREFIX), "{}", it.headline);
            assert!(it.published_at <= t_open());
            assert!(it.provenance.synthetic);
            let body = it.body.as_ref().unwrap();
            assert!((2..=6).contains(&body.split("\n\n").count()));
        }
        assert!(page.items.windows(2).all(|w| w[0].published_at >= w[1].published_at));
        if scope == NewsScope::Company {
            assert!(page.items.iter().all(|i| i.tickers.iter().any(|t| t == "AAPL" || t == "JPM")));
        }
    }
    let filtered = p
        .news(&NewsQuery { scope: NewsScope::Market, keys: vec![], text: Some("treasury".into()), from: None, to: None, limit: 10 })
        .await
        .unwrap();
    assert!(filtered.items.iter().all(|i| i.headline.to_lowercase().contains("treasury") || i.summary.as_ref().unwrap().to_lowercase().contains("treasury")));
}

#[tokio::test]
async fn macro_series_calendar_and_curve_agree() {
    let p = provider_at(14, t_open());
    for id in p.economic_series_ids() {
        let s = p.economic_series(&SeriesRequest { id: id.into(), from: None, to: None }).await.unwrap();
        assert!(s.observations.len() > 100, "{id}");
        assert!(s.observations.first().unwrap().date.year() <= 2000, "{id}");
        assert!(s.notes.as_ref().unwrap().contains("SYNTHETIC"));
        let latest = s.latest().unwrap().date;
        // Quarterly GDP lags the most (advance estimate ~4 weeks after the quarter).
        assert!(latest <= nanos_to_date(t_open()) && (nanos_to_date(t_open()) - latest).num_days() < 200, "{id}: {latest}");
    }
    let get = |id: &'static str| {
        let p = &p;
        async move { p.economic_series(&SeriesRequest { id: id.into(), from: Some(ymd(2026, 9, 1)), to: None }).await.unwrap() }
    };
    let (d2, d10, d30, spread) = (get("DGS2").await, get("DGS10").await, get("DGS30").await, get("T10Y2Y").await);
    let last = d10.observations.iter().rev().find(|o| o.value.is_some()).unwrap();
    let curve = p.yield_curve(&CurveRequest { name: "UST".into(), date: None }).await.unwrap();
    assert_eq!(curve.date, last.date);
    assert_eq!(curve.points.len(), 13);
    let pt = |t: &str| curve.points.iter().find(|x| x.tenor == t).unwrap().yield_pct.unwrap();
    let at = |s: &meridian_types::EconomicSeries| s.observations.iter().find(|o| o.date == curve.date).unwrap().value.unwrap();
    assert_eq!(pt("2 Yr"), at(&d2));
    assert_eq!(pt("10 Yr"), at(&d10));
    assert_eq!(pt("30 Yr"), at(&d30));
    assert!((at(&spread) - (at(&d10) - at(&d2))).abs() < 1e-9);
    // Bond-market holiday shows as a missing value.
    let col = p.economic_series(&SeriesRequest { id: "DGS10".into(), from: Some(ymd(2025, 10, 13)), to: Some(ymd(2025, 10, 13)) }).await.unwrap();
    assert_eq!(col.observations.len(), 1);
    assert!(col.observations[0].value.is_none());

    let cal = p.economic_calendar(&CalendarRequest { from: ymd(2026, 9, 1), to: ymd(2026, 10, 31), countries: vec![] }).await.unwrap();
    assert!(cal.len() > 40);
    let names: HashSet<&str> = cal.iter().map(|e| e.event.as_str()).collect();
    for n in ["CPI MoM", "Change in Nonfarm Payrolls", "FOMC Rate Decision (Upper Bound)", "Initial Jobless Claims", "ECB Deposit Facility Rate"] {
        assert!(names.contains(n), "missing {n}");
    }
    for e in &cal {
        assert_eq!(e.actual.is_some(), e.release_time <= t_open(), "{}", e.event);
        assert!(e.provenance.synthetic);
    }
    let us_only = p.economic_calendar(&CalendarRequest { from: ymd(2026, 9, 1), to: ymd(2026, 9, 30), countries: vec!["US".into()] }).await.unwrap();
    assert!(us_only.iter().all(|e| e.country == "US"));
    // The CPI release for August matches the series.
    let cpi = p.economic_series(&SeriesRequest { id: "CPIAUCSL".into(), from: Some(ymd(2026, 7, 1)), to: Some(ymd(2026, 8, 1)) }).await.unwrap();
    let mom = ((cpi.observations[1].value.unwrap() / cpi.observations[0].value.unwrap() - 1.0) * 1000.0).round() / 10.0;
    let ev = cal.iter().find(|e| e.event == "CPI MoM" && e.period.as_deref() == Some("Aug")).unwrap();
    assert_eq!(ev.actual, Some(mom));
}

#[tokio::test]
async fn fx_crosses_triangulate_and_search_works() {
    let p = provider_at(15, t_open());
    let q = p
        .quotes(&[SecurityKey::currency("EURUSD"), SecurityKey::currency("USDJPY"), SecurityKey::currency("EURJPY"), SecurityKey::currency("GBPNZD")])
        .await
        .unwrap();
    assert_eq!(q.len(), 4);
    let (eu, uj, ej) = (q[0].last.unwrap(), q[1].last.unwrap(), q[2].last.unwrap());
    assert!(((eu * uj) / ej - 1.0).abs() < 2e-4, "{eu} * {uj} vs {ej}");
    let hits = p.search(&InstrumentQuery { text: "app".into(), sector: None, limit: 5 }).await.unwrap();
    assert_eq!(hits[0].key.symbol, "AAPL");
    let hits = p.search(&InstrumentQuery { text: "Microsoft".into(), sector: None, limit: 5 }).await.unwrap();
    assert_eq!(hits[0].key.symbol, "MSFT");
    let hits = p.search(&InstrumentQuery { text: "SPX".into(), sector: Some(MarketSector::Index), limit: 5 }).await.unwrap();
    assert_eq!(hits[0].key, SecurityKey::index("SPX"));
    assert_eq!(p.index_region(&SecurityKey::index("NKY")), Some("APAC"));
    assert_eq!(p.index_region(&SecurityKey::index("DAX")), Some("EMEA"));
}

struct Collect {
    events: Mutex<Vec<(Instant, StreamEvent)>>,
}

impl EventSink for Collect {
    fn send(&self, _provider: &ProviderId, event: StreamEvent) {
        self.events.lock().push((Instant::now(), event));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_emits_for_subscribed_keys() {
    let p = provider_at(16, t_open());
    // Warm shared caches so the timing below measures the stream itself.
    let keys: Vec<SecurityKey> = ["AAPL", "MSFT", "NVDA", "AMZN", "GOOGL", "META", "TSLA", "JPM", "XOM", "KO"]
        .iter()
        .map(|t| SecurityKey::equity(t))
        .collect();
    let _ = p.quotes(&keys[..1]).await.unwrap();
    let sink = Arc::new(Collect { events: Mutex::new(Vec::new()) });
    let handle = p.streaming().unwrap().connect(sink.clone()).await.unwrap();
    assert!(matches!(sink.events.lock()[0].1, StreamEvent::Status { connected: true, .. }));
    let start = Instant::now();
    handle.subscribe(&keys);
    let mut first: Option<Duration> = None;
    while start.elapsed() < Duration::from_secs(5) {
        tokio::time::sleep(Duration::from_millis(5)).await;
        let ev = sink.events.lock();
        if first.is_none() && ev.iter().any(|(_, e)| matches!(e, StreamEvent::Quote { .. })) {
            first = Some(start.elapsed());
        }
        let seen: HashSet<&SecurityKey> = ev.iter().filter_map(|(_, e)| if let StreamEvent::Quote { key, .. } = e { Some(key) } else { None }).collect();
        if seen.len() == keys.len() {
            break;
        }
    }
    let first = first.expect("no quote events");
    assert!(first < Duration::from_millis(200), "first event after {first:?}");
    // Updates keep arriving and stay in each symbol's ballpark.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    handle.unsubscribe(&keys[5..]);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let n_before = sink.events.lock().len();
    tokio::time::sleep(Duration::from_millis(500)).await;
    {
        let ev = sink.events.lock();
        let seen: HashSet<&SecurityKey> = ev.iter().filter_map(|(_, e)| if let StreamEvent::Quote { key, .. } = e { Some(key) } else { None }).collect();
        assert_eq!(seen.len(), keys.len());
        let late: HashSet<&SecurityKey> = ev[n_before..].iter().filter_map(|(_, e)| if let StreamEvent::Quote { key, .. } = e { Some(key) } else { None }).collect();
        assert!(late.iter().all(|k| keys[..5].contains(k)), "unsubscribed keys still streaming");
        for (_, e) in ev.iter() {
            if let StreamEvent::Quote { update, .. } = e
                && let (Some(b), Some(a)) = (update.bid, update.ask)
            {
                assert!(b < a);
            }
        }
    }
    handle.close();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(sink.events.lock().iter().any(|(_, e)| matches!(e, StreamEvent::Status { connected: false, .. })));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_load_2000_symbols() {
    let p = MockProvider::new(MockConfig {
        seed: 17,
        clock: Arc::new(FixedClock(t_open())),
        extra_symbols: 2000,
        updates_per_symbol_per_sec: 3.0,
    });
    let keys: Vec<SecurityKey> = p.universe().iter().filter(|i| i.key.symbol.starts_with("ZQ")).map(|i| i.key.clone()).collect();
    assert_eq!(keys.len(), 2000);
    let sink = Arc::new(Collect { events: Mutex::new(Vec::with_capacity(200_000)) });
    let handle = p.streaming().unwrap().connect(sink.clone()).await.unwrap();
    let t0 = Instant::now();
    handle.subscribe(&keys);
    // Wait for every subscription to initialize.
    loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let seen = {
            let ev = sink.events.lock();
            ev.iter().filter_map(|(_, e)| if let StreamEvent::Quote { key, .. } = e { Some(key.clone()) } else { None }).collect::<HashSet<_>>().len()
        };
        if seen == keys.len() || t0.elapsed() > Duration::from_secs(120) {
            break;
        }
    }
    let init = t0.elapsed();
    let n0 = sink.events.lock().len();
    let t1 = Instant::now();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let n1 = sink.events.lock().len();
    let rate = (n1 - n0) as f64 / t1.elapsed().as_secs_f64();
    println!("2,000-symbol mock stream: init {init:?}, {rate:.0} events/s (target ~6,000 at 3/s/symbol)");
    assert!(rate > 1000.0, "{rate}");
    handle.close();
}

#[tokio::test]
async fn unknown_and_inapplicable_requests_fail_clearly() {
    let p = provider_at(18, t_open());
    assert!(p.instrument(&SecurityKey::equity("NOPE")).await.is_err());
    assert!(p.bars(&daily(SecurityKey::equity("NOPE"))).await.is_err());
    assert!(p.economic_series(&SeriesRequest { id: "NOPE".into(), from: None, to: None }).await.is_err());
    assert!(p.yield_curve(&CurveRequest { name: "BUND".into(), date: None }).await.is_err());
    assert!(p.option_chain(&ChainRequest { underlying: SecurityKey::currency("EURUSD"), expiry: None }).await.is_err());
    assert!(p.quotes(&[SecurityKey::equity("NOPE")]).await.unwrap().is_empty());
}

//! Template-generated news and earnings-call transcripts.
//!
//! Stories are tied to the synthetic data: price-move stories quote the
//! day's synthetic move, earnings stories quote the synthetic results on the
//! synthetic announcement date, macro stories quote synthetic releases.
//! Every headline starts with `[MOCK] ` and every body ends with a notice
//! that the story is not real.

use chrono::{Duration, NaiveDate};
use meridian_provider::{CalendarRequest, NewsQuery, NewsScope};
use meridian_types::{
    NANOS_PER_DAY, NewsItem, NewsPage, PeriodType, Provenance, SecurityKey, Transcript, TranscriptSegment, UnixNanos,
    date_to_nanos, nanos_to_date,
};

use crate::Inner;
use crate::cal::NANOS_PER_MIN;
use crate::corp::{FIRST_NAMES, LAST_NAMES, brokers, person};
use crate::filings::{millions, ordinal, pct_change};
use crate::fundamentals::Period;
use crate::hash::{Cell, fnv1a, tag};
use crate::universe::{Kind, Sym};

pub const MOCK_PREFIX: &str = "[MOCK] ";
const DISCLAIMER: &str =
    "This story is synthetic mock data generated locally by Meridian for interface development. It does not describe real events.";

struct Draft {
    source: &'static str,
    headline: String,
    summary: String,
    paragraphs: Vec<String>,
    tickers: Vec<String>,
    topics: Vec<String>,
    at: UnixNanos,
    weight: f64,
}

impl Draft {
    fn finish(self, seed: u64, now: UnixNanos) -> NewsItem {
        let h = fnv1a(format!("{}|{}|{}", self.headline, self.at, seed).as_bytes());
        let mut body = self.paragraphs.join("\n\n");
        body.push_str("\n\n");
        body.push_str(DISCLAIMER);
        let mut provenance = Provenance::synthetic(now);
        provenance.source_ref = Some(format!("mock-news-{h:016x}"));
        NewsItem {
            id: format!("mock-{h:016x}"),
            source: self.source.into(),
            headline: format!("{MOCK_PREFIX}{}", self.headline),
            summary: Some(self.summary),
            body: Some(body),
            url: None,
            published_at: self.at,
            received_at: self.at + 350 * 1_000_000,
            tickers: self.tickers,
            topics: self.topics,
            provenance,
        }
    }
}

fn signed_pct(x: f64) -> String {
    format!("{}{:.2}%", if x >= 0.0 { "+" } else { "" }, x)
}

/// Minutes after 00:00 UTC, weighted to US market hours.
fn publish_minute(c: &mut Cell) -> i64 {
    if c.chance(0.7) { c.int(11 * 60, 22 * 60) } else { c.int(0, 24 * 60 - 1) }
}

impl Inner {
    /// Day move (percent, quoted terms) for a symbol on a date, if it traded.
    fn day_move(&self, s: &Sym, d: NaiveDate, now: UnixNanos) -> Option<(f64, f64)> {
        let today = s.p.session.tz.to_local(now).0;
        let recent = self.recent(s, today);
        let i = recent.iter().position(|b| b.date == d)?;
        let prev = recent.get(i.checked_sub(1)?)?.c;
        let close = if d == today {
            self.quote(s, now).and_then(|q| q.last.zip(q.prev_close)).map(|(l, p)| l / p * prev)?
        } else {
            recent[i].c
        };
        let k = crate::market::split_factor_after(&crate::market::splits_of(s), today);
        Some((pct_change(close, prev), crate::market::round_tick(close * k, s.tick())))
    }

    fn company_drafts(&self, s: &Sym, d: NaiveDate, now: UnixNanos, out: &mut Vec<Draft>) {
        let mut c = Cell::new(&[self.seed, s.hash, tag("news"), crate::daily::day_num(d)]);
        let day0 = date_to_nanos(d);
        let mcap = s.anchor_mcap();
        let quiet = !s.p.calendar.is_trading_day(d);
        let n = match s.p.kind {
            _ if quiet || (s.p.kind == Kind::Equity && s.p.filler) => c.int(0, 1),
            Kind::Equity if mcap > 1e12 => c.int(3, 6),
            Kind::Equity if mcap > 1e11 => c.int(1, 4),
            Kind::Crypto => c.int(1, 3),
            _ => c.int(0, 2),
        };
        let name = s.inst.name.clone();
        let short = short_name(&name);
        let t = s.ticker().to_string();
        let sector = s.inst.sector.clone().unwrap_or_default();
        let mv = self.day_move(s, d, now);
        for j in 0..n {
            let at = day0 + publish_minute(&mut c) * NANOS_PER_MIN + j;
            let (headline, summary, p1, topic) = match (j, mv) {
                (0, Some((pct, px))) if pct.abs() >= 2.5 => {
                    let reasons_up = ["Upbeat Demand Outlook", "an Analyst Upgrade", "a New Product Launch", "Strong Channel Checks", "a Larger Buyback"];
                    let reasons_dn = ["Margin Concerns", "Cautious Guidance", "Regulatory Scrutiny", "Supply-Chain Worries", "an Executive Departure"];
                    if pct > 0.0 {
                        let r = c.pick(&reasons_up);
                        (
                            format!("{short} Shares Jump {pct:.1}% on {r}"),
                            format!("{name} rose {pct:.1}% to ${px:.2} in synthetic trading."),
                            format!("Shares of {name} ({t}) climbed {} to ${px:.2}, outpacing the broader market, as investors reacted to {}.", signed_pct(pct), r.to_lowercase()),
                            "Equities",
                        )
                    } else {
                        let r = c.pick(&reasons_dn);
                        (
                            format!("{short} Slides {:.1}% on {r}", pct.abs()),
                            format!("{name} fell {:.1}% to ${px:.2} in synthetic trading.", pct.abs()),
                            format!("Shares of {name} ({t}) dropped {} to ${px:.2} as investors weighed {}.", signed_pct(pct), r.to_lowercase()),
                            "Equities",
                        )
                    }
                }
                (0, Some((pct, px))) => (
                    format!("{short} {} in {} Trading", if pct >= 0.0 { "Edges Higher" } else { "Edges Lower" }, if c.chance(0.5) { "Heavy" } else { "Light" }),
                    format!("{name} moved {} to ${px:.2}.", signed_pct(pct)),
                    format!("{name} ({t}) finished the session {} at ${px:.2}, with volume near its recent average.", signed_pct(pct)),
                    "Equities",
                ),
                _ => {
                    let k = c.int(0, 7);
                    let role = c.pick(&["Chief Financial Officer", "Chief Operating Officer", "Chief Technology Officer", "President of International Operations"]);
                    let x = c.int(1, 9);
                    match k {
                        0 => (
                            format!("{short} Expands Push in {} With New Partnership", s.inst.industry.clone().unwrap_or_else(|| sector.clone())),
                            format!("{name} announced a multi-year partnership (synthetic)."),
                            format!("{name} said it entered a multi-year partnership intended to broaden distribution of its {} offerings.", sector.to_lowercase()),
                            "Products",
                        ),
                        1 => (
                            format!("{short} Names New {role}"),
                            format!("{name} appointed {} as {role} (synthetic).", person(&mut c)),
                            format!("{name} appointed {} as {role}, effective next month. The company did not disclose terms.", person(&mut c)),
                            "Management",
                        ),
                        2 => (
                            format!("{short} Unveils Cost Program Targeting ${x} Billion in Savings"),
                            format!("{name} outlined ${x} billion of savings over three years (synthetic)."),
                            format!("{name} outlined a program to reduce annual costs by about ${x} billion over three years, citing automation and procurement savings."),
                            "Corporate Actions",
                        ),
                        3 => (
                            format!("Options Traders Position for a Big Move in {t}"),
                            format!("Synthetic options activity in {t} rose sharply."),
                            format!("Call and put volume in {name} ran well above its 20-day average, with activity concentrated in near-dated contracts."),
                            "Options",
                        ),
                        4 => (
                            format!("{short} to Present at Investor Conference"),
                            format!("{name} management will present at an industry conference (synthetic)."),
                            format!("{name} said members of its management team will present at an industry conference next week, with a webcast available to investors."),
                            "Corporate Events",
                        ),
                        5 => (
                            format!("{short} Faces Lawsuit Over {}", c.pick(&["Patent Claims", "Data Practices", "Contract Dispute", "Product Claims"])),
                            format!("A lawsuit was filed against {name} (synthetic)."),
                            format!("A complaint filed in federal court seeks unspecified damages from {name}. The company said it would defend itself vigorously."),
                            "Legal",
                        ),
                        6 => (
                            format!("{short} Said to Weigh Acquisition in {sector}"),
                            format!("{name} is reportedly exploring a deal (synthetic)."),
                            format!("{name} is exploring an acquisition of a smaller {} company, according to people familiar with the matter. No decision has been made.", sector.to_lowercase()),
                            "M&A",
                        ),
                        _ => (
                            format!("What to Watch in {t} Ahead of Earnings"),
                            format!("Previewing {name}'s next report (synthetic)."),
                            format!("Investors will focus on {name}'s margins, guidance, and capital returns when it reports results."),
                            "Earnings",
                        ),
                    }
                }
            };
            let p2 = format!(
                "{name} has a synthetic market value of about ${:.0} billion. Analysts' views and estimates on this screen are generated by the mock provider.",
                mcap / 1e9
            );
            out.push(Draft {
                source: if c.chance(0.5) { "Mock Wire" } else { "Synthetic Newswire" },
                headline,
                summary,
                paragraphs: vec![p1, p2],
                tickers: vec![t.clone()],
                topics: vec![topic.into(), sector.clone()].into_iter().filter(|x| !x.is_empty()).collect(),
                at,
                weight: (mcap / 1e11).max(0.1),
            });
        }
    }

    /// Routine company press releases (about every other trading day for
    /// large companies).
    fn press_release_drafts(&self, s: &Sym, d: NaiveDate, out: &mut Vec<Draft>) {
        if s.p.kind != Kind::Equity || !s.p.calendar.is_trading_day(d) {
            return;
        }
        let mut c = Cell::new(&[self.seed, s.hash, tag("press"), crate::daily::day_num(d)]);
        if !c.chance(if s.p.filler { 0.1 } else { 0.6 }) {
            return;
        }
        let name = s.inst.name.clone();
        let t = s.ticker().to_string();
        let industry = s.inst.industry.clone().unwrap_or_default();
        let x = c.int(1, 15);
        let (headline, p1, topic) = match c.int(0, 5) {
            0 => (
                format!("{name} Announces Date of Next Quarterly Results and Conference Call"),
                format!("{name} ({t}) will report quarterly results after the market close and host a conference call for investors."),
                "Earnings",
            ),
            1 => (
                format!("{name} to Participate in Upcoming Investor Conferences"),
                format!("Members of {name}'s management team will participate in fireside chats at two industry conferences this month."),
                "Corporate Events",
            ),
            2 => (
                format!("{name} Launches New {industry} Offering"),
                format!("{name} introduced a new offering for customers in the {} market, available beginning next quarter.", industry.to_lowercase()),
                "Products",
            ),
            3 => (
                format!("{name} Publishes Annual Sustainability Report"),
                format!("{name} published its annual sustainability report, describing emissions, workforce, and governance metrics."),
                "ESG",
            ),
            4 => (
                format!("{name} Prices ${x} Billion Senior Notes Offering"),
                format!("{name} priced an offering of ${x} billion aggregate principal amount of senior unsecured notes in several tranches."),
                "Debt",
            ),
            _ => (
                format!("{name} Expands Share Repurchase Authorization by ${x} Billion"),
                format!("{name}'s board of directors authorized the repurchase of up to an additional ${x} billion of common stock."),
                "Corporate Actions",
            ),
        };
        out.push(Draft {
            source: "Mock PR Wire",
            summary: format!("Press release from {name} (synthetic)."),
            headline,
            paragraphs: vec![p1, format!("About {name}: a company in the {} industry. This release and its contents are synthetic.", industry.to_lowercase())],
            tickers: vec![t],
            topics: vec!["Press Releases".into(), topic.into()],
            at: crate::cal::Tz::NewYork.to_utc(d, (c.int(6 * 60, 9 * 60)) as i32),
            weight: 2.0,
        });
    }

    fn earnings_and_rating_drafts(&self, s: &Sym, from: UnixNanos, to: UnixNanos, now: UnixNanos, out: &mut Vec<Draft>) {
        if s.p.kind != Kind::Equity {
            return;
        }
        let today = nanos_to_date(now);
        let short = short_name(&s.inst.name);
        let name = s.inst.name.clone();
        let t = s.ticker().to_string();
        if let Some(e) = self.earnings_for(s, now) {
            for r in e.records.iter().filter(|r| r.eps_actual.is_some()) {
                let Some(d) = r.announce_date else { continue };
                let at = crate::cal::Tz::NewYork.to_utc(d, 16 * 60 + 5);
                if at < from || at > to {
                    continue;
                }
                let (a, est) = (r.eps_actual.unwrap_or(0.0), r.eps_estimate.unwrap_or(0.0));
                let beat = if a >= est { "Beats" } else { "Misses" };
                let rev = r.revenue_actual.unwrap_or(0.0);
                out.push(Draft {
                    source: "Mock Wire",
                    headline: format!("{short} {} EPS ${a:.2} {beat} Estimate of ${est:.2}; Revenue {}", r.fiscal_label, billions(rev)),
                    summary: format!("{name} reported {} results (synthetic).", r.fiscal_label),
                    paragraphs: vec![
                        format!("{name} ({t}) reported {} diluted earnings of ${a:.2} per share versus a synthetic consensus of ${est:.2}, on revenue of {}.", r.fiscal_label, billions(rev)),
                        format!("Revenue compared with a synthetic estimate of {}. The company will discuss results on a conference call.", billions(r.revenue_estimate.unwrap_or(0.0))),
                    ],
                    tickers: vec![t.clone()],
                    topics: vec!["Earnings".into()],
                    at,
                    weight: 50.0,
                });
                out.push(Draft {
                    source: "Mock PR Wire",
                    headline: format!("{name} Reports {} Results", r.fiscal_label),
                    summary: format!("Press release: {name} {} results (synthetic).", r.fiscal_label),
                    paragraphs: vec![format!("{name} today announced financial results for {}: revenue of {} and diluted EPS of ${a:.2}.", r.fiscal_label, billions(rev))],
                    tickers: vec![t.clone()],
                    topics: vec!["Press Releases".into(), "Earnings".into()],
                    at: at - 5 * NANOS_PER_MIN,
                    weight: 5.0,
                });
            }
        }
        if let Some(recs) = self.recommendations_for(s, now) {
            for r in recs.ratings.iter().take(4) {
                let at = crate::cal::Tz::NewYork.to_utc(r.date, 7 * 60 + 15);
                if at < from || at > to || r.date > today {
                    continue;
                }
                let verb = match r.score {
                    Some(5 | 4) => "Upgrades",
                    Some(1 | 2) => "Downgrades",
                    _ => "Reiterates",
                };
                let pt = r.target_price.map_or(String::new(), |p| format!(", PT ${p:.0}"));
                out.push(Draft {
                    source: "Mock Markets Desk",
                    headline: format!("{} {verb} {short} to {}{pt}", r.firm, r.rating),
                    summary: format!("{} changed its view on {t} (synthetic).", r.firm),
                    paragraphs: vec![format!(
                        "{} (an invented research firm) rated {name} {}{pt}. The rating and the firm are synthetic.",
                        r.firm, r.rating
                    )],
                    tickers: vec![t.clone()],
                    topics: vec!["Analyst Ratings".into()],
                    at,
                    weight: 8.0,
                });
            }
        }
        if let Some(divs) = self.dividends_for(s, now) {
            for d in divs.dividends.iter().filter(|d| d.kind == meridian_types::DividendKind::Regular) {
                let Some(dd) = d.declared_date else { continue };
                let at = crate::cal::Tz::NewYork.to_utc(dd, 16 * 60 + 15);
                if at < from || at > to {
                    continue;
                }
                out.push(Draft {
                    source: "Mock PR Wire",
                    headline: format!("{name} Declares Dividend of ${:.4} per Share", d.amount),
                    summary: format!("{name} declared a dividend payable {} (synthetic).", d.pay_date.map_or(String::new(), |p| p.to_string())),
                    paragraphs: vec![format!(
                        "{name} declared a dividend of ${:.4} per share, payable on {} to shareholders of record on {}.",
                        d.amount,
                        d.pay_date.map_or(String::new(), |p| p.to_string()),
                        d.record_date.map_or(String::new(), |p| p.to_string())
                    )],
                    tickers: vec![t.clone()],
                    topics: vec!["Press Releases".into(), "Dividends".into()],
                    at,
                    weight: 3.0,
                });
            }
        }
    }

    fn market_drafts(&self, d: NaiveDate, now: UnixNanos, out: &mut Vec<Draft>) {
        let mut c = Cell::new(&[self.seed, tag("market-news"), crate::daily::day_num(d)]);
        let u = &self.universe;
        let get = |t: &str, sec| u.by_ticker(t, sec).map(|i| &u.syms[i]);
        use meridian_types::MarketSector::{Cmdty, Curncy, Index};
        let close_at = crate::cal::Session::US_EQUITY.close_utc(d) + 5 * NANOS_PER_MIN;
        if let (Some(spx), Some(ndx), Some(indu)) = (get("SPX", Index), get("CCMP", Index), get("INDU", Index))
            && let (Some((a, pa)), Some((b, _)), Some((cc, _))) = (self.day_move(spx, d, now), self.day_move(ndx, d, now), self.day_move(indu, d, now))
            && close_at <= now
        {
            let verb = if a >= 0.0 { "Rises" } else { "Falls" };
            let why = if a >= 0.0 {
                c.pick(&["as Tech Rallies", "on Rate-Cut Hopes", "After Upbeat Data", "as Earnings Impress"])
            } else {
                c.pick(&["as Yields Climb", "on Growth Worries", "as Tech Slips", "After Hawkish Fed Remarks"])
            };
            out.push(Draft {
                source: "Mock Markets Desk",
                headline: format!("S&P 500 {verb} {:.1}% {why}", a.abs()),
                summary: format!("Synthetic close: S&P 500 {}, Nasdaq {}, Dow {}.", signed_pct(a), signed_pct(b), signed_pct(cc)),
                paragraphs: vec![
                    format!("The S&P 500 closed {} at {pa:.2} in synthetic trading. The Nasdaq Composite moved {} and the Dow Jones Industrial Average {}.", signed_pct(a), signed_pct(b), signed_pct(cc)),
                    "Sector performance was mixed; breadth and volume figures are generated by the mock provider.".into(),
                ],
                tickers: vec!["SPX".into(), "CCMP".into(), "INDU".into()],
                topics: vec!["Markets".into(), "Equities".into()],
                at: close_at,
                weight: 100.0,
            });
        }
        // Treasuries.
        let y_now = self.ust_yield(d, 10.0);
        let y_prev = self.ust_yield(crate::cal::Calendar::UsBond.before(d), 10.0);
        let at = crate::cal::Tz::NewYork.to_utc(d, 15 * 60 + 30);
        if at <= now && crate::cal::Calendar::UsBond.is_trading_day(d) {
            let bp = ((y_now - y_prev) * 100.0).round();
            out.push(Draft {
                source: "Mock Markets Desk",
                headline: format!("10-Year Treasury Yield {} to {y_now:.2}%", if bp >= 0.0 { "Rises" } else { "Falls" }),
                summary: format!("The synthetic 10-year yield moved {bp:+.0} bp."),
                paragraphs: vec![format!("The 10-year Treasury yield moved {bp:+.0} basis points to {y_now:.2}% (synthetic curve). Two-year yields ended at {:.2}%.", self.ust_yield(d, 2.0))],
                tickers: vec!["DGS10".into()],
                topics: vec!["Rates".into(), "Fixed Income".into()],
                at,
                weight: 40.0,
            });
        }
        for (t, sec, label, topic) in [
            ("EURUSD", Curncy, "Euro", "FX"),
            ("CL1", Cmdty, "Oil", "Commodities"),
            ("GC1", Cmdty, "Gold", "Commodities"),
            ("BTCUSD", Curncy, "Bitcoin", "Crypto"),
        ] {
            let Some(s) = get(t, sec) else { continue };
            let Some((pct, px)) = self.day_move(s, d, now) else { continue };
            let at = d.and_hms_opt(0, 0, 0).map_or(0, |x| meridian_types::datetime_to_nanos(x.and_utc())) + (19 * 60 + c.int(0, 120)) * NANOS_PER_MIN;
            if at > now {
                continue;
            }
            let dp = usize::from(s.inst.price_decimals);
            out.push(Draft {
                source: "Mock Markets Desk",
                headline: format!("{label} {} {:.1}% to {px:.dp$}", if pct >= 0.0 { "Gains" } else { "Drops" }, pct.abs()),
                summary: format!("{label} moved {} (synthetic).", signed_pct(pct)),
                paragraphs: vec![format!("{} moved {} to {px:.dp$} in synthetic trading.", s.inst.name, signed_pct(pct))],
                tickers: vec![t.into()],
                topics: vec![topic.into(), "Markets".into()],
                at,
                weight: 20.0,
            });
        }
        // Economic releases of the day.
        let events = self.economic_calendar_for(&CalendarRequest { from: d, to: d, countries: Vec::new() });
        for e in events.into_iter().filter(|e| e.importance != meridian_types::Importance::Low) {
            let (Some(a), Some(cons)) = (e.actual, e.consensus) else { continue };
            let unit = e.unit.clone().unwrap_or_default();
            let cmp = if (a - cons).abs() < 1e-9 { "Matches" } else if a > cons { "Tops" } else { "Trails" };
            out.push(Draft {
                source: "Mock Wire",
                headline: format!("{} {} {a}{unit} {cmp} Estimates", country_label(&e.country), e.event),
                summary: format!("{} {} came in at {a}{unit} vs {cons}{unit} expected (synthetic).", e.country, e.event),
                paragraphs: vec![format!(
                    "{} for {} printed {a}{unit} against a synthetic consensus of {cons}{unit}; the prior reading was {}{unit}.",
                    e.event,
                    e.period.clone().unwrap_or_else(|| "the period".into()),
                    e.prior.map_or("n/a".into(), |p| p.to_string())
                )],
                tickers: e.series_id.clone().into_iter().collect(),
                topics: vec!["Economy".into(), e.country.clone()],
                at: e.release_time + 2 * NANOS_PER_MIN,
                weight: if e.importance == meridian_types::Importance::High { 90.0 } else { 30.0 },
            });
        }
    }

    pub(crate) fn news_for(&self, q: &NewsQuery) -> NewsPage {
        let now = self.clock.now();
        let to = q.to.map_or(now, |t| t.min(now));
        let lookback = if matches!(q.scope, NewsScope::Company | NewsScope::PressReleases) { 7 } else { 3 };
        let from = q.from.unwrap_or(to - lookback * NANOS_PER_DAY).max(to - 30 * NANOS_PER_DAY);
        if to <= from {
            return NewsPage { items: Vec::new(), next: None };
        }
        let days: Vec<NaiveDate> = {
            let mut v = Vec::new();
            let mut d = nanos_to_date(from);
            while d <= nanos_to_date(to) {
                v.push(d);
                d += Duration::days(1);
            }
            v
        };
        let mut drafts = Vec::new();
        let resolve_keys = |keys: &[SecurityKey]| -> Vec<&Sym> {
            keys.iter().filter_map(|k| self.universe.index_of(k).map(|i| &self.universe.syms[i])).collect()
        };
        let top_caps = || -> Vec<&Sym> {
            let mut v: Vec<&Sym> = self.universe.syms.iter().filter(|s| s.p.kind == Kind::Equity && !s.p.filler).collect();
            v.sort_by(|a, b| b.anchor_mcap().total_cmp(&a.anchor_mcap()));
            v.truncate(12);
            v
        };
        match q.scope {
            NewsScope::Company => {
                for s in resolve_keys(&q.keys) {
                    for d in &days {
                        self.company_drafts(s, *d, now, &mut drafts);
                        self.press_release_drafts(s, *d, &mut drafts);
                    }
                    self.earnings_and_rating_drafts(s, from, to, now, &mut drafts);
                }
            }
            NewsScope::PressReleases => {
                let syms = if q.keys.is_empty() { top_caps() } else { resolve_keys(&q.keys) };
                let mut all = Vec::new();
                for s in syms {
                    for d in &days {
                        self.press_release_drafts(s, *d, &mut all);
                    }
                    self.earnings_and_rating_drafts(s, from, to, now, &mut all);
                }
                drafts.extend(all.into_iter().filter(|d| d.source == "Mock PR Wire"));
            }
            NewsScope::Market | NewsScope::Top => {
                for d in &days {
                    self.market_drafts(*d, now, &mut drafts);
                }
                for s in top_caps() {
                    for d in &days {
                        self.company_drafts(s, *d, now, &mut drafts);
                    }
                    if q.scope == NewsScope::Top {
                        self.earnings_and_rating_drafts(s, from, to, now, &mut drafts);
                    }
                }
                if q.scope == NewsScope::Top {
                    // Top stories: the most important items of each day.
                    drafts.sort_by(|a, b| b.weight.total_cmp(&a.weight).then(b.at.cmp(&a.at)));
                    let keep = (days.len() * 8).max(10);
                    drafts.truncate(keep);
                }
            }
        }
        let text = q.text.as_ref().map(|t| t.to_lowercase());
        let mut items: Vec<NewsItem> = drafts
            .into_iter()
            .filter(|d| d.at >= from && d.at <= to)
            .filter(|d| {
                text.as_ref().is_none_or(|t| d.headline.to_lowercase().contains(t) || d.summary.to_lowercase().contains(t))
            })
            .map(|d| d.finish(self.seed, now))
            .collect();
        items.sort_by(|a, b| b.published_at.cmp(&a.published_at).then(a.id.cmp(&b.id)));
        items.dedup_by(|a, b| a.id == b.id);
        let limit = if q.limit == 0 { 50 } else { q.limit };
        items.truncate(limit);
        NewsPage { items, next: None }
    }

    pub(crate) fn transcripts_for(&self, s: &Sym, now: UnixNanos) -> Option<Vec<Transcript>> {
        if s.p.kind != Kind::Equity {
            return None;
        }
        let today = nanos_to_date(now);
        let model = self.company(s, today)?;
        let r = model.reported(today);
        let mut names = Cell::new(&[self.seed, s.hash, tag("execs")]);
        let ceo = person(&mut names);
        let cfo = person(&mut names);
        let ir = person(&mut names);
        let firms = brokers(self.seed, s, 6);
        let name = &s.inst.name;
        let mut out = Vec::new();
        for (qi, q) in model.quarters[..r].iter().enumerate().rev().take(4) {
            let p = Period::single(q, PeriodType::Quarterly, format!("Q{}", q.fq));
            let prev = (qi >= 4).then(|| Period::single(&model.quarters[qi - 4], PeriodType::Quarterly, String::new()));
            let g = prev.as_ref().map_or(0.0, |pp| pct_change(p.revenue, pp.revenue));
            let gm = p.gross / p.revenue * 100.0;
            let mut c = Cell::new(&[self.seed, s.hash, tag("transcript"), crate::daily::day_num(q.end)]);
            let seg = |speaker: &str, role: &str, text: String| TranscriptSegment { speaker: speaker.into(), role: Some(role.into()), text };
            let quarter = format!("{} quarter of fiscal {}", ordinal(q.fq), q.fy);
            let mut segs = vec![
                seg("Operator", "Operator", format!(
                    "[MOCK TRANSCRIPT — synthetic, not a real call] Good afternoon, and welcome to the {name} {quarter} earnings conference call. All participants are in listen-only mode."
                )),
                seg(&ir, "Vice President, Investor Relations", format!(
                    "Thank you. Joining me are {ceo}, our Chief Executive Officer, and {cfo}, our Chief Financial Officer. Today's remarks include forward-looking statements; actual results may differ. All names and figures in this transcript are synthetic."
                )),
                seg(&ceo, "Chief Executive Officer", format!(
                    "Thanks, everyone. Revenue for the {quarter} was {}, {} {:.1}% from a year ago. {}",
                    millions(p.revenue),
                    if g >= 0.0 { "up" } else { "down" },
                    g.abs(),
                    c.pick(&[
                        "Demand was healthy across our major regions, and we continued to invest for growth.",
                        "We executed well in a mixed environment and gained share in our core markets.",
                        "We made steady progress on our product roadmap and our productivity agenda.",
                    ])
                )),
                seg(&cfo, "Chief Financial Officer", format!(
                    "Gross margin was {gm:.1}%. Operating income was {} and diluted EPS was ${:.2}. We ended the quarter with {} in cash and returned {} to shareholders. For next quarter we expect revenue growth in the {}% to {}% range.",
                    millions(p.op),
                    p.eps_diluted(),
                    millions(p.bal.cash),
                    millions(p.div + p.buyback),
                    (g - 2.0).round(),
                    (g + 2.0).round()
                )),
                seg("Operator", "Operator", "We will now begin the question-and-answer session.".into()),
            ];
            for (k, topic) in ["demand trends", "gross margin drivers", "capital allocation"].iter().enumerate() {
                let analyst = format!("{} {}", c.pick(FIRST_NAMES), c.pick(LAST_NAMES));
                segs.push(seg(&analyst, &format!("Analyst, {}", firms[k % firms.len()]), format!("Thanks for taking my question. Could you talk about {topic} and how you're thinking about the next few quarters?")));
                let (who, role) = if k == 1 { (&cfo, "Chief Financial Officer") } else { (&ceo, "Chief Executive Officer") };
                segs.push(seg(who, role, format!(
                    "Sure. On {topic}, {}",
                    c.pick(&[
                        "we're seeing stable trends and we remain disciplined on costs.",
                        "the picture is improving, though we're planning conservatively.",
                        "we expect gradual improvement through the year as new products ramp.",
                    ])
                )));
            }
            segs.push(seg("Operator", "Operator", "This concludes today's call. Thank you for participating. [MOCK TRANSCRIPT]".into()));
            out.push(Transcript {
                key: s.key().clone(),
                fiscal_label: q.label(),
                date: q.announce,
                segments: segs,
                provenance: Provenance::synthetic(now),
            });
        }
        Some(out)
    }
}

fn billions(x: f64) -> String {
    format!("${:.2}B", x / 1e9)
}

fn country_label(c: &str) -> &str {
    match c {
        "US" => "U.S.",
        "EU" => "Eurozone",
        "GB" => "U.K.",
        "JP" => "Japan",
        "CN" => "China",
        "DE" => "Germany",
        other => other,
    }
}

/// Drops corporate suffixes for headlines ("Apple Inc." → "Apple").
fn short_name(name: &str) -> String {
    let mut n = name.trim_start_matches("The ").to_string();
    for suffix in [
        ", Inc. Class A", ", Inc. Class B", " Inc. Class A", " Class A", " Class B", ", Inc.", " Inc.", " Incorporated",
        " Corporation", " Company", " Corp.", " plc", " N.V.", " & Co.", ".com", " Holdings", " Group", ",",
    ] {
        if let Some(stripped) = n.strip_suffix(suffix) {
            n = stripped.to_string();
        }
    }
    n.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_names() {
        assert_eq!(short_name("Apple Inc."), "Apple");
        assert_eq!(short_name("Alphabet Inc. Class A"), "Alphabet");
        assert_eq!(short_name("The Walt Disney Company"), "Walt Disney");
        assert_eq!(short_name("Amazon.com, Inc."), "Amazon");
    }
}

//! CALENDAR — earnings releases, ex-dividend dates and macro releases in a
//! date window, for holdings and watchlists (or every security). TODAY's
//! "Coming up" section is built from the same rows.
//!
//! Each kind has its own source and degrades on its own: a missing source
//! leaves only that kind NOT AVAILABLE. Each also loads on its own (see
//! [`super::parts`]): a slow source shows "…: loading…" while the other
//! kinds' rows are already up.

use std::sync::Arc;

use chrono::{NaiveDate, Timelike};
use meridian_provider::{CalendarRequest, Capability, EventCalendarRequest, ProviderError};
use meridian_types::{
    DividendCalendar, DividendKind, EarningsCalendar, EarningsEvent, EarningsSession, EconomicEvent, Importance,
    Provenance, SecurityKey, UnixNanos, nanos_to_date,
};

use super::ScreenRequest;
use super::dividends::fmt_ratio;
use super::parts::{FILL_IN_MS, Part};
use super::scope::{RANGE_OPTIONS, Scope, Window, is_company_key, new_york_date, new_york_time};
use crate::cache::ttl;
use crate::core::Engine;
use crate::error::EngineError;
use crate::screen::{Action, Block, Cell, Column, Field, Input, InputKind, NoticeLevel, Row, Screen, Style, Table};

const TITLE: &str = "Calendar";

/// Which kinds of events to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Kinds {
    pub earnings: bool,
    pub dividends: bool,
    pub macro_releases: bool,
}

impl Kinds {
    pub const ALL: Kinds = Kinds { earnings: true, dividends: true, macro_releases: true };
    const OPTIONS: [&'static str; 4] = ["All", "Earnings", "Dividends", "Macro"];

    fn parse(arg: Option<&str>) -> (Self, &'static str) {
        match arg.map(|a| a.trim().to_ascii_lowercase()).as_deref() {
            Some("earnings") => (Kinds { earnings: true, dividends: false, macro_releases: false }, Self::OPTIONS[1]),
            Some("dividends" | "dividend") => {
                (Kinds { earnings: false, dividends: true, macro_releases: false }, Self::OPTIONS[2])
            }
            Some("macro" | "economic") => (Kinds { earnings: false, dividends: false, macro_releases: true }, Self::OPTIONS[3]),
            _ => (Kinds::ALL, Self::OPTIONS[0]),
        }
    }

    fn has(self, kind: Kind) -> bool {
        match kind {
            Kind::Earnings => self.earnings,
            Kind::Dividends => self.dividends,
            Kind::Macro => self.macro_releases,
        }
    }
}

/// One kind of event: each has its own source, cache entry and section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Earnings,
    Dividends,
    Macro,
}

impl Kind {
    /// The kind's section, named as notes and the Sources block show it.
    fn section(self, high_only: bool, status: SectionStatus) -> Section {
        let (name, note) = match self {
            Kind::Earnings => ("Earnings", "estimates are the source's consensus"),
            Kind::Dividends => ("Dividends", "ex-dates of announced dividends and splits"),
            Kind::Macro if high_only => ("Macro", "release calendar, high-importance releases"),
            Kind::Macro => ("Macro", "release calendar, all releases"),
        };
        Section { name, status, note }
    }

    fn capability(self) -> Capability {
        match self {
            Kind::Earnings => Capability::EarningsCalendar,
            Kind::Dividends => Capability::DividendCalendar,
            Kind::Macro => Capability::EconomicCalendar,
        }
    }
}

const MACRO_OPTIONS: [&str; 2] = ["High importance", "All"];

/// What a row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum EventKind {
    Macro,
    Earnings,
    ExDividend,
    Split,
}

impl EventKind {
    fn label(self) -> &'static str {
        match self {
            EventKind::Macro => "Macro",
            EventKind::Earnings => "Earnings",
            EventKind::ExDividend => "Ex-dividend",
            EventKind::Split => "Split",
        }
    }
}

/// One calendar line.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CalendarRow {
    pub date: NaiveDate,
    /// Minutes after midnight New York time used for ordering within a day;
    /// all-day and unknown times sort last.
    pub slot: u32,
    pub kind: EventKind,
    pub key: Option<SecurityKey>,
    /// Ticker, or the country for macro releases.
    pub symbol: String,
    /// Company or release name.
    pub name: String,
    /// Session or time of day, when known.
    pub time: String,
    pub detail: String,
    pub action: Action,
}

impl CalendarRow {
    pub(crate) fn to_row(&self) -> Row {
        let mut row = Row::new(vec![
            Cell::text(self.date.format("%a %m/%d").to_string()),
            Cell::text(&self.time).styled(Style::Muted),
            Cell::text(self.kind.label()),
            Cell::text(&self.symbol).styled(if self.key.is_some() { Style::Link } else { Style::Normal }),
            Cell::text(&self.name).styled(Style::Emphasis),
            Cell::text(&self.detail),
        ])
        .action(self.action.clone());
        if let Some(k) = &self.key {
            row = row.security(&k.to_string());
        }
        row
    }
}

/// Columns for [`CalendarRow::to_row`].
pub(crate) fn calendar_columns() -> Vec<Column> {
    vec![
        Column::text("Date", 10),
        Column::text("Time", 13),
        Column::text("Kind", 11),
        Column::text("Security", 9),
        Column::text("Name", 32),
        Column::text("Detail", 40),
    ]
}

/// How one kind's source fared.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SectionStatus {
    /// Loaded from `source` (display name).
    Loaded { source: String },
    /// Not shown because of the choices made (kind filter): no note.
    Off,
    /// Nothing to ask for, with the reason (e.g. no stocks in scope).
    Empty(String),
    /// The source is missing or failed: NOT AVAILABLE with the reason.
    Unavailable(String),
    /// Still being fetched (CALENDAR only; TODAY waits for the whole
    /// calendar).
    Loading,
}

#[derive(Debug, Clone)]
pub(crate) struct Section {
    pub name: &'static str,
    pub status: SectionStatus,
    /// What the source line says about the data (e.g. estimates are the
    /// source's consensus).
    pub note: &'static str,
}

/// Rows from every kind, plus how each kind's source fared.
#[derive(Debug, Clone)]
pub(crate) struct CalendarData {
    pub rows: Vec<CalendarRow>,
    pub sections: Vec<Section>,
    pub provenance: Vec<Provenance>,
    /// Fetch time of the oldest stale cache entry served, if any.
    pub stale_since: Option<UnixNanos>,
}

/// One kind's rows and how its source fared.
#[derive(Debug, Clone)]
struct KindData {
    rows: Vec<CalendarRow>,
    section: Section,
    provenance: Option<Provenance>,
    /// Fetch time of the stale cache entry served, if one was.
    stale_since: Option<UnixNanos>,
}

impl KindData {
    /// No rows, only a status (off, nothing to ask for, failed, loading).
    fn status(kind: Kind, high_only: bool, status: SectionStatus) -> Self {
        Self { rows: Vec::new(), section: kind.section(high_only, status), provenance: None, stale_since: None }
    }
}

impl CalendarData {
    /// Every kind's rows in date order, with each kind's section in the
    /// order given.
    fn from_kinds<'a>(kinds: impl IntoIterator<Item = &'a KindData>) -> Self {
        let mut data = CalendarData { rows: Vec::new(), sections: Vec::new(), provenance: Vec::new(), stale_since: None };
        for k in kinds {
            data.rows.extend(k.rows.iter().cloned());
            data.sections.push(k.section.clone());
            data.provenance.extend(k.provenance.iter().cloned());
            if let Some(at) = k.stale_since {
                data.stale_since = Some(data.stale_since.map_or(at, |s| s.min(at)));
            }
        }
        data.rows.sort_by(|a, b| {
            (a.date, a.slot, a.kind, &a.symbol, &a.name).cmp(&(b.date, b.slot, b.kind, &b.symbol, &b.name))
        });
        data
    }

    /// Some kind is still being fetched.
    fn loading(&self) -> bool {
        self.sections.iter().any(|s| s.status == SectionStatus::Loading)
    }

    /// NOT AVAILABLE, "nothing to show" and "loading" notes, one per kind.
    pub(crate) fn notices(&self) -> Vec<Block> {
        self.sections
            .iter()
            .filter_map(|s| match &s.status {
                SectionStatus::Unavailable(r) => {
                    Some(Block::Notice { level: NoticeLevel::Warning, text: format!("{}: NOT AVAILABLE — {r}", s.name) })
                }
                SectionStatus::Empty(r) => Some(Block::Notice { level: NoticeLevel::Info, text: format!("{}: {r}", s.name) }),
                SectionStatus::Loading => Some(Block::Notice { level: NoticeLevel::Info, text: format!("{}: loading…", s.name) }),
                SectionStatus::Loaded { .. } | SectionStatus::Off => None,
            })
            .collect()
    }

    /// "Sources" fields: one per kind that loaded.
    pub(crate) fn source_fields(&self) -> Option<Block> {
        let fields: Vec<Field> = self
            .sections
            .iter()
            .filter_map(|s| match &s.status {
                SectionStatus::Loaded { source } => {
                    let text = if s.note.is_empty() { source.clone() } else { format!("{source} · {}", s.note) };
                    Some(Field::text(s.name, text).styled(Style::Muted))
                }
                _ => None,
            })
            .collect();
        (!fields.is_empty()).then_some(Block::Fields { title: Some("Sources".into()), columns: 1, fields })
    }
}

/// Display name for a provider id.
pub(crate) fn source_name(id: &str) -> String {
    match id {
        "finnhub" => "Finnhub".into(),
        "alpaca" => "Alpaca".into(),
        "fred" => "FRED".into(),
        "edgar" => "SEC EDGAR".into(),
        "treasury" => "US Treasury".into(),
        "coinbase" => "Coinbase".into(),
        "kraken" => "Kraken".into(),
        "mock" => "Mock data".into(),
        other => other.to_owned(),
    }
}

/// Up to 4 decimals, at least 2 (`0.51`, `0.125`, `1.00`).
fn fmt_amount(x: f64) -> String {
    let s = format!("{x:.4}");
    let trimmed = s.trim_end_matches('0');
    let decimals = trimmed.split_once('.').map_or(0, |(_, d)| d.len());
    if decimals >= 2 { trimmed.to_owned() } else { format!("{x:.2}") }
}

fn earnings_detail(e: &EarningsEvent) -> String {
    let mut parts = Vec::new();
    if let (Some(q), Some(y)) = (e.fiscal_quarter, e.fiscal_year) {
        parts.push(format!("Q{q} {y}"));
    }
    match (e.eps_estimate, e.eps_actual) {
        (Some(est), Some(act)) => parts.push(format!("EPS {act:.2} vs est. {est:.2}")),
        (Some(est), None) => parts.push(format!("EPS est. {est:.2}")),
        (None, Some(act)) => parts.push(format!("EPS {act:.2}")),
        (None, None) => {}
    }
    parts.join(" · ")
}

fn session_slot(s: Option<EarningsSession>) -> (u32, &'static str) {
    match s {
        Some(EarningsSession::BeforeOpen) => (0, EarningsSession::BeforeOpen.label()),
        Some(EarningsSession::DuringMarket) => (600, EarningsSession::DuringMarket.label()),
        Some(EarningsSession::AfterClose) => (1_000, EarningsSession::AfterClose.label()),
        None => (ALL_DAY, ""),
    }
}

/// Sort slot for all-day or unknown times.
const ALL_DAY: u32 = 1_500;

fn earnings_rows(engine: &Engine, cal: &EarningsCalendar) -> Vec<CalendarRow> {
    cal.events
        .iter()
        .map(|e| {
            let (slot, time) = session_slot(e.session);
            let ks = e.key.to_string();
            CalendarRow {
                date: e.date,
                slot,
                kind: EventKind::Earnings,
                key: Some(e.key.clone()),
                symbol: e.key.symbol.clone(),
                name: engine.known_name(&e.key),
                time: time.to_owned(),
                detail: earnings_detail(e),
                action: Action::new("ERN", Some(&ks)),
            }
        })
        .collect()
}

fn dividend_rows(engine: &Engine, cal: &DividendCalendar) -> Vec<CalendarRow> {
    cal.events
        .iter()
        .map(|e| {
            let d = &e.dividend;
            let ks = e.key.to_string();
            let (kind, detail) = match d.kind {
                DividendKind::Split => (EventKind::Split, format!("{} split", fmt_ratio(d.amount))),
                DividendKind::Regular | DividendKind::Special => {
                    let mut s = fmt_amount(d.amount);
                    if !d.currency.is_empty() {
                        s = format!("{s} {}", d.currency);
                    }
                    if d.kind == DividendKind::Special {
                        s = format!("Special {s}");
                    }
                    if let Some(p) = d.pay_date {
                        s = format!("{s} · pays {}", p.format("%m/%d"));
                    }
                    (EventKind::ExDividend, s)
                }
            };
            CalendarRow {
                date: d.ex_date,
                slot: ALL_DAY,
                kind,
                key: Some(e.key.clone()),
                symbol: e.key.symbol.clone(),
                name: engine.known_name(&e.key),
                time: String::new(),
                detail,
                action: Action::new("DVD", Some(&ks)),
            }
        })
        .collect()
}

fn macro_rows(events: &[EconomicEvent], window: &Window, high_only: bool) -> Vec<CalendarRow> {
    events
        .iter()
        .filter(|e| !high_only || e.importance == Importance::High)
        .filter_map(|e| {
            // Release times without a time of day are midnight UTC of the date.
            let (date, slot, time) = if e.time_known {
                let t = new_york_time(e.release_time);
                (t.date(), t.hour() * 60 + t.minute(), format!("{} ET", t.format("%H:%M")))
            } else {
                (nanos_to_date(e.release_time), ALL_DAY, String::new())
            };
            if date < window.from || date > window.to {
                return None;
            }
            let importance = match e.importance {
                Importance::High => "High importance",
                Importance::Medium => "Medium importance",
                Importance::Low => "Low importance",
            };
            let detail = match &e.period {
                Some(p) => format!("{p} · {importance}"),
                None => importance.to_owned(),
            };
            let action = match &e.series_id {
                Some(id) => Action::new("ECO", None).arg("series", id.clone()),
                None => Action::new("ECO", None)
                    .arg("from", date.format("%m/%d/%Y").to_string())
                    .arg("to", date.format("%m/%d/%Y").to_string()),
            };
            Some(CalendarRow {
                date,
                slot,
                kind: EventKind::Macro,
                key: None,
                symbol: e.country.clone(),
                name: e.event.clone(),
                time,
                detail,
                action,
            })
        })
        .collect()
}

/// Cache-key part for a key set (`*` = no filter).
fn keys_part(keys: Option<&[SecurityKey]>) -> String {
    match keys {
        None => "*".into(),
        Some(k) => {
            let mut v: Vec<String> = k.iter().map(ToString::to_string).collect();
            v.sort();
            v.join(",")
        }
    }
}

/// Status for a failed fetch. "None of these securities is covered"
/// (`NotFound`) is an empty answer from the source, not a failure.
fn failed(e: &EngineError, engine: &Engine, cap: Capability) -> SectionStatus {
    match e {
        EngineError::Provider(ProviderError::NotFound(_)) => SectionStatus::Loaded {
            source: engine.router().providers_for(cap, None).first().map_or_else(String::new, |p| source_name(p.as_str())),
        },
        EngineError::Provider(ProviderError::Unsupported { .. }) => {
            SectionStatus::Unavailable(format!("no configured source provides a {} ({})", cap.label(), engine.unavailable_hint()))
        }
        e => SectionStatus::Unavailable(e.user_message()),
    }
}

impl Engine {
    /// Earnings releases, dividend ex-dates and macro releases in `window`.
    /// `keys` = `None` asks for every security; macro releases don't depend
    /// on keys. The three sources are fetched concurrently and cached.
    pub(crate) async fn calendar_data(
        self: &Arc<Self>,
        window: &Window,
        keys: Option<&[SecurityKey]>,
        kinds: Kinds,
        high_only: bool,
    ) -> CalendarData {
        let kind = |k: Kind| async move {
            if kinds.has(k) {
                self.calendar_kind(k, window, keys, high_only).await
            } else {
                KindData::status(k, high_only, SectionStatus::Off)
            }
        };
        let (earnings, dividends, releases) = tokio::join!(kind(Kind::Earnings), kind(Kind::Dividends), kind(Kind::Macro));
        CalendarData::from_kinds([&earnings, &dividends, &releases])
    }

    /// One kind's rows in `window`, from its source (cached). `keys` as for
    /// [`Engine::calendar_data`].
    async fn calendar_kind(self: &Arc<Self>, kind: Kind, window: &Window, keys: Option<&[SecurityKey]>, high_only: bool) -> KindData {
        let router = self.router();
        let fetched = match kind {
            Kind::Earnings | Kind::Dividends => {
                let company_keys: Option<Vec<SecurityKey>> =
                    keys.map(|k| k.iter().filter(|k| is_company_key(k)).cloned().collect());
                if company_keys.as_ref().is_some_and(Vec::is_empty) {
                    let why = if keys.is_some_and(<[SecurityKey]>::is_empty) { "no securities in scope" } else { "no stocks in scope" };
                    return KindData::status(kind, high_only, SectionStatus::Empty(why.into()));
                }
                let ck = format!("{}|{}|{}", window.from, window.to, keys_part(company_keys.as_deref()));
                let req = EventCalendarRequest { from: window.from, to: window.to, keys: company_keys.unwrap_or_default() };
                if kind == Kind::Earnings {
                    self.cached("earnings_cal", &ck, ttl::CALENDAR, async move { router.earnings_calendar(req).await }).await.map(|f| {
                        let p = f.value.provenance.clone();
                        (earnings_rows(self, &f.value), source_name(p.provider.as_str()), Some(p), f.stale.then_some(f.fetched_at))
                    })
                } else {
                    self.cached("dividend_cal", &ck, ttl::CALENDAR, async move { router.dividend_calendar(req).await }).await.map(|f| {
                        let p = f.value.provenance.clone();
                        (dividend_rows(self, &f.value), source_name(p.provider.as_str()), Some(p), f.stale.then_some(f.fetched_at))
                    })
                }
            }
            Kind::Macro => {
                // A day either side: timed releases are stored in UTC.
                let creq = CalendarRequest { from: window.from - chrono::Duration::days(1), to: window.to + chrono::Duration::days(1), countries: vec![] };
                let mck = format!("{}|{}", creq.from, creq.to);
                self.cached("calendar", &mck, ttl::CALENDAR, async move { router.economic_calendar(creq).await }).await.map(|f| {
                    let p = f.value.first().map(|e| e.provenance.clone());
                    (macro_rows(&f.value, window, high_only), source_name(&f.provider), p, f.stale.then_some(f.fetched_at))
                })
            }
        };
        match fetched {
            Ok((rows, source, provenance, stale_since)) => {
                KindData { rows, section: kind.section(high_only, SectionStatus::Loaded { source }), provenance, stale_since }
            }
            Err(e) => KindData::status(kind, high_only, failed(&e, self, kind.capability())),
        }
    }
}

/// CALENDAR's kinds, each loading in the background on its own and kept on
/// the engine between calls (see [`super::parts`]).
#[derive(Default)]
pub(crate) struct CalendarParts {
    earnings: Part<KindData>,
    dividends: Part<KindData>,
    macro_releases: Part<KindData>,
}

impl CalendarParts {
    fn part(&self, kind: Kind) -> &Part<KindData> {
        match kind {
            Kind::Earnings => &self.earnings,
            Kind::Dividends => &self.dividends,
            Kind::Macro => &self.macro_releases,
        }
    }
}

/// One kind for CALENDAR: off, loaded, or loading while it has never loaded
/// for these inputs.
async fn load_kind(
    engine: &Arc<Engine>,
    kind: Kind,
    kinds: Kinds,
    window: &Window,
    keys: Option<&[SecurityKey]>,
    high_only: bool,
) -> Arc<KindData> {
    if !kinds.has(kind) {
        return Arc::new(KindData::status(kind, high_only, SectionStatus::Off));
    }
    // The inputs that decide what this kind shows.
    let sig = match kind {
        Kind::Earnings | Kind::Dividends => format!("{}|{}|{}", window.from, window.to, keys_part(keys)),
        Kind::Macro => format!("{}|{}|{high_only}", window.from, window.to),
    };
    let (e, w, k) = (engine.clone(), window.clone(), keys.map(<[SecurityKey]>::to_vec));
    let got = engine
        .calendar
        .part(kind)
        .get(engine.handle(), engine.deterministic, &sig, move || async move { e.calendar_kind(kind, &w, k.as_deref(), high_only).await })
        .await;
    got.unwrap_or_else(|| Arc::new(KindData::status(kind, high_only, SectionStatus::Loading)))
}

pub(crate) async fn calendar(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    let today = new_york_date(engine.now());
    let window = Window::parse(req.arg("range"), today);
    let scope = Scope::parse(req.arg("scope"));
    let (kinds, kind_label) = Kinds::parse(req.arg("kind"));
    let high_only = !req.arg("importance").is_some_and(|i| i.trim().eq_ignore_ascii_case("all"));
    let keys = engine.scope_keys(scope, today);
    let keys = (scope != Scope::All).then_some(keys.as_slice());
    let (earnings, dividends, releases) = tokio::join!(
        load_kind(&engine, Kind::Earnings, kinds, &window, keys, high_only),
        load_kind(&engine, Kind::Dividends, kinds, &window, keys, high_only),
        load_kind(&engine, Kind::Macro, kinds, &window, keys, high_only),
    );
    let data = CalendarData::from_kinds([&*earnings, &*dividends, &*releases]);

    let mut s = Screen::new("CALENDAR", TITLE, None);
    if data.loading() {
        s.refresh_ms = Some(FILL_IN_MS);
    }
    for p in &data.provenance {
        s.source(p);
    }
    let choice = |id: &str, label: &str, value: &str, options: &[&str]| Input {
        id: id.into(),
        label: label.into(),
        value: value.into(),
        kind: InputKind::Choice,
        options: options.iter().map(|o| (*o).to_owned()).collect(),
    };
    s.push(Block::Inputs {
        title: None,
        inputs: vec![
            choice("range", "When", window.label, &RANGE_OPTIONS),
            choice("scope", "Securities", scope.label(), &Scope::OPTIONS),
            choice("kind", "Events", kind_label, &Kinds::OPTIONS),
            choice("importance", "Macro releases", if high_only { MACRO_OPTIONS[0] } else { MACRO_OPTIONS[1] }, &MACRO_OPTIONS),
        ],
    });
    if let Some(at) = data.stale_since {
        s.push(Block::Notice {
            level: NoticeLevel::Warning,
            text: format!("OFFLINE — showing cached data from {}", super::fmt_datetime(at)),
        });
    }
    for n in data.notices() {
        s.push(n);
    }
    let title = format!("{} · {}", window.label, window.describe());
    if data.rows.is_empty() {
        let loaded = data.sections.iter().any(|x| matches!(x.status, SectionStatus::Loaded { .. }));
        // Not "nothing scheduled" while another kind may still bring rows.
        if loaded && !data.loading() {
            s.push(Block::Notice { level: NoticeLevel::Info, text: format!("Nothing scheduled — {title}") });
        }
    } else {
        s.push(Block::Table(Table {
            title: Some(title),
            columns: calendar_columns(),
            rows: data.rows.iter().map(CalendarRow::to_row).collect(),
            page_size: Some(25),
            numbered: true,
        }));
    }
    if let Some(b) = data.source_fields() {
        s.push(b);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_keep_two_to_four_decimals() {
        assert_eq!(fmt_amount(0.51), "0.51");
        assert_eq!(fmt_amount(0.125), "0.125");
        assert_eq!(fmt_amount(0.2625), "0.2625");
        assert_eq!(fmt_amount(1.0), "1.00");
        assert_eq!(fmt_amount(0.00001), "0.00");
    }

    #[test]
    fn kinds_merge_in_date_order_and_a_loading_kind_says_so() {
        let d = |day| NaiveDate::from_ymd_opt(2026, 10, day).unwrap();
        let row = |date, kind, symbol: &str| CalendarRow {
            date,
            slot: ALL_DAY,
            kind,
            key: None,
            symbol: symbol.into(),
            name: symbol.into(),
            time: String::new(),
            detail: String::new(),
            action: Action::new("ECO", None),
        };
        let loaded = |kind: Kind, rows, stale_since| KindData {
            rows,
            section: kind.section(true, SectionStatus::Loaded { source: "Test source".into() }),
            provenance: None,
            stale_since,
        };
        let earnings = loaded(Kind::Earnings, vec![row(d(9), EventKind::Earnings, "B")], Some(20));
        let dividends = loaded(Kind::Dividends, vec![row(d(8), EventKind::ExDividend, "A")], Some(10));
        let releases = KindData::status(Kind::Macro, true, SectionStatus::Loading);
        let data = CalendarData::from_kinds([&earnings, &dividends, &releases]);
        assert_eq!(data.rows.iter().map(|r| r.symbol.as_str()).collect::<Vec<_>>(), ["A", "B"]);
        assert_eq!(data.stale_since, Some(10), "the oldest stale entry");
        assert!(data.loading());
        let notes: Vec<String> = data
            .notices()
            .into_iter()
            .filter_map(|b| match b {
                Block::Notice { level: NoticeLevel::Info, text } => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(notes, ["Macro: loading…"]);
        match data.source_fields() {
            Some(Block::Fields { fields, .. }) => {
                assert_eq!(fields.iter().map(|f| f.label.as_str()).collect::<Vec<_>>(), ["Earnings", "Dividends"]);
            }
            other => panic!("{other:?}"),
        }
        let off = KindData::status(Kind::Macro, true, SectionStatus::Off);
        assert!(!CalendarData::from_kinds([&earnings, &off]).loading());
    }

    #[test]
    fn kinds_parse() {
        assert_eq!(Kinds::parse(None).0, Kinds::ALL);
        assert_eq!(Kinds::parse(Some("Earnings")).1, "Earnings");
        assert!(!Kinds::parse(Some("dividends")).0.earnings);
        assert!(Kinds::parse(Some("macro")).0.macro_releases);
    }

    #[test]
    fn earnings_detail_reads_naturally() {
        let e = EarningsEvent {
            key: SecurityKey::equity("X"),
            date: NaiveDate::from_ymd_opt(2026, 10, 8).unwrap(),
            session: None,
            fiscal_year: Some(2026),
            fiscal_quarter: Some(3),
            eps_estimate: Some(1.2345),
            eps_actual: None,
            revenue_estimate: None,
            revenue_actual: None,
        };
        assert_eq!(earnings_detail(&e), "Q3 2026 · EPS est. 1.23");
        let reported = EarningsEvent { eps_actual: Some(1.3), ..e.clone() };
        assert_eq!(earnings_detail(&reported), "Q3 2026 · EPS 1.30 vs est. 1.23");
        let bare = EarningsEvent { fiscal_year: None, eps_estimate: None, ..e };
        assert_eq!(earnings_detail(&bare), "");
    }
}

//! FILINGS — an inbox of recent SEC filings for every stock in holdings and
//! watchlists, newest first, with read/unread state kept in SQLite.
//!
//! A row opens the filing in CF (which marks it read): periodic reports
//! open on "what changed versus the prior filing of the same form", other
//! forms on the document. No AI calls are made here; CF summarizes a
//! filing only when asked and an AI service is configured.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::{Duration, NaiveDate};
use meridian_provider::{Capability, ProviderError};
use meridian_types::{Filing, Provenance, SecurityKey, UnixNanos, date_to_nanos};

use super::ScreenRequest;
use super::calendar::source_name;
use super::scope::{is_company_key, new_york_date};
use crate::core::Engine;
use crate::error::EngineError;
use crate::screen::{Action, Block, Cell, Column, Format, Input, InputKind, NoticeLevel, Row, Screen, Style, Table};

const TITLE: &str = "Filings";
/// Filings fetched per company (shared with CF's list cache).
const PER_COMPANY: usize = 200;

/// Official item titles from Form 8-K (SEC 873 (02-25),
/// <https://www.sec.gov/files/form8-k.pdf>, read 2026-10-07).
const FORM_8K_ITEMS: &[(&str, &str)] = &[
    ("1.01", "Entry into a Material Definitive Agreement"),
    ("1.02", "Termination of a Material Definitive Agreement"),
    ("1.03", "Bankruptcy or Receivership"),
    ("1.04", "Mine Safety – Reporting of Shutdowns and Patterns of Violations"),
    ("1.05", "Material Cybersecurity Incidents"),
    ("2.01", "Completion of Acquisition or Disposition of Assets"),
    ("2.02", "Results of Operations and Financial Condition"),
    ("2.03", "Creation of a Direct Financial Obligation or an Obligation under an Off-Balance Sheet Arrangement of a Registrant"),
    ("2.04", "Triggering Events That Accelerate or Increase a Direct Financial Obligation or an Obligation under an Off-Balance Sheet Arrangement"),
    ("2.05", "Costs Associated with Exit or Disposal Activities"),
    ("2.06", "Material Impairments"),
    ("3.01", "Notice of Delisting or Failure to Satisfy a Continued Listing Rule or Standard; Transfer of Listing"),
    ("3.02", "Unregistered Sales of Equity Securities"),
    ("3.03", "Material Modification to Rights of Security Holders"),
    ("4.01", "Changes in Registrant’s Certifying Accountant"),
    ("4.02", "Non-Reliance on Previously Issued Financial Statements or a Related Audit Report or Completed Interim Review"),
    ("5.01", "Changes in Control of Registrant"),
    ("5.02", "Departure of Directors or Certain Officers; Election of Directors; Appointment of Certain Officers; Compensatory Arrangements of Certain Officers"),
    ("5.03", "Amendments to Articles of Incorporation or Bylaws; Change in Fiscal Year"),
    ("5.04", "Temporary Suspension of Trading Under Registrant’s Employee Benefit Plans"),
    ("5.05", "Amendments to the Registrant’s Code of Ethics, or Waiver of a Provision of the Code of Ethics"),
    ("5.06", "Change in Shell Company Status"),
    ("5.07", "Submission of Matters to a Vote of Security Holders"),
    ("5.08", "Shareholder Director Nominations"),
    ("6.01", "ABS Informational and Computational Material"),
    ("6.02", "Change of Servicer or Trustee"),
    ("6.03", "Change in Credit Enhancement or Other External Support"),
    ("6.04", "Failure to Make a Required Distribution"),
    ("6.05", "Securities Act Updating Disclosure"),
    ("6.06", "Static Pool"),
    ("7.01", "Regulation FD Disclosure"),
    ("8.01", "Other Events"),
    ("9.01", "Financial Statements and Exhibits"),
];

/// Plain names for common forms (amendments add "(amended)").
const FORMS: &[(&str, &str)] = &[
    ("10-K", "Annual report"),
    ("10-Q", "Quarterly report"),
    ("20-F", "Annual report (foreign issuer)"),
    ("40-F", "Annual report (Canadian issuer)"),
    ("6-K", "Current report (foreign issuer)"),
    ("8-K", "Current report"),
    ("3", "Initial insider holdings"),
    ("4", "Insider transaction"),
    ("5", "Annual insider holdings report"),
    ("144", "Notice of proposed insider sale"),
    ("DEF 14A", "Proxy statement"),
    ("DEFA14A", "Additional proxy materials"),
    ("PRE 14A", "Preliminary proxy statement"),
    ("DEF 14C", "Information statement"),
    ("SC 13D", "Ownership report, 5% or more (active)"),
    ("SCHEDULE 13D", "Ownership report, 5% or more (active)"),
    ("SC 13G", "Ownership report, 5% or more (passive)"),
    ("SCHEDULE 13G", "Ownership report, 5% or more (passive)"),
    ("13F-HR", "Institutional holdings report"),
    ("S-1", "Registration statement"),
    ("S-3", "Shelf registration statement"),
    ("S-3ASR", "Shelf registration statement"),
    ("S-4", "Registration statement (business combination)"),
    ("S-8", "Employee stock plan registration"),
    ("424B2", "Prospectus"),
    ("424B3", "Prospectus"),
    ("424B4", "Prospectus"),
    ("424B5", "Prospectus"),
    ("FWP", "Free writing prospectus"),
    ("11-K", "Employee savings plan annual report"),
    ("ARS", "Annual report to shareholders"),
    ("SD", "Specialized disclosure report"),
    ("NT 10-K", "Notice of late annual report"),
    ("NT 10-Q", "Notice of late quarterly report"),
    ("CORRESP", "Correspondence with the SEC"),
    ("UPLOAD", "SEC staff letter"),
    ("25-NSE", "Notice of delisting"),
    ("8-A12B", "Registration of securities on an exchange"),
];

/// Forms where "what changed versus the prior filing of the same form" is
/// meaningful: periodic reports and annual proxies.
const COMPARABLE_FORMS: &[&str] = &["10-K", "10-Q", "20-F", "40-F", "DEF 14A"];

/// One line for a filing: 8-K items by their official titles (Item 9.01,
/// exhibits, is left out when other items are present), a plain name for
/// other known forms, else EDGAR's own description or the form.
#[must_use]
pub fn plain_description(form: &str, items: &[String], edgar: Option<&str>) -> String {
    let (base, amended) = match form.strip_suffix("/A") {
        Some(b) => (b, true),
        None => (form, false),
    };
    let amend = |s: String| if amended { format!("{s} (amended)") } else { s };
    if base == "8-K" && !items.is_empty() {
        let mut titles: Vec<&str> = items
            .iter()
            .filter(|i| items.len() == 1 || i.as_str() != "9.01")
            .filter_map(|i| FORM_8K_ITEMS.iter().find(|(n, _)| *n == i.trim()).map(|(_, t)| *t))
            .collect();
        titles.dedup();
        if !titles.is_empty() {
            return amend(titles.join(" · "));
        }
    }
    if let Some((_, name)) = FORMS.iter().find(|(f, _)| f.eq_ignore_ascii_case(base)) {
        return amend((*name).to_owned());
    }
    match edgar.map(str::trim).filter(|d| !d.is_empty() && !d.eq_ignore_ascii_case(form)) {
        Some(d) => d.to_owned(),
        None => form.to_owned(),
    }
}

/// Sort time: acceptance time, else midnight of the filing date.
fn sort_time(f: &Filing) -> UnixNanos {
    f.accepted_at.unwrap_or_else(|| date_to_nanos(f.filed))
}

/// One filing in the inbox.
#[derive(Debug, Clone)]
pub(crate) struct InboxItem {
    pub key: SecurityKey,
    pub filing: Filing,
    pub unread: bool,
    /// An earlier filing of the same form is on file (CF can compare).
    pub has_prior: bool,
}

impl InboxItem {
    /// CF on this filing: "what changed" for comparable forms, else the
    /// document.
    pub(crate) fn action(&self) -> Action {
        let ks = self.key.to_string();
        let a = Action::new("CF", Some(&ks)).arg("doc", self.filing.accession.clone());
        let form = self.filing.form.as_str();
        if self.has_prior && COMPARABLE_FORMS.contains(&form) { a.arg("diff", "1") } else { a }
    }

    pub(crate) fn description(&self) -> String {
        plain_description(&self.filing.form, &self.filing.items, self.filing.description.as_deref())
    }

    /// Filed time (acceptance) as a DateTime cell, or the date as text.
    pub(crate) fn filed_cell(&self) -> Cell {
        match self.filing.accepted_at {
            Some(t) => Cell::num(Some(t as f64)),
            None => Cell::text(self.filing.filed.format("%m/%d/%Y").to_string()),
        }
    }
}

/// Recent filings across a set of securities.
#[derive(Debug, Clone, Default)]
pub(crate) struct Inbox {
    /// Newest first.
    pub items: Vec<InboxItem>,
    /// Set when no filing could be loaded at all: the reason.
    pub unavailable: Option<String>,
    /// Partial problems and "nothing to show" notes.
    pub notes: Vec<Block>,
    pub provenance: Option<Provenance>,
    pub stale_since: Option<UnixNanos>,
}

impl Inbox {
    pub(crate) fn unread(&self) -> usize {
        self.items.iter().filter(|i| i.unread).count()
    }
}

fn not_a_filer(e: &EngineError) -> bool {
    matches!(
        e,
        EngineError::NotFound(_)
            | EngineError::Provider(ProviderError::NotFound(_) | ProviderError::Unsupported { .. })
    )
}

impl Engine {
    /// SEC filings since `since` for the stocks among `keys`, newest first,
    /// with read state. Companies are fetched concurrently through the CF
    /// list cache.
    pub(crate) async fn filings_inbox(self: &Arc<Self>, keys: &[SecurityKey], since: NaiveDate) -> Inbox {
        let mut inbox = Inbox::default();
        let stocks: Vec<SecurityKey> = keys.iter().filter(|k| is_company_key(k)).cloned().collect();
        if stocks.is_empty() {
            let why = if keys.is_empty() { "your holdings and watchlists are empty" } else { "no stocks in your holdings or watchlists" };
            inbox.notes.push(Block::Notice { level: NoticeLevel::Info, text: format!("Filings: {why}") });
            return inbox;
        }
        let covered: Vec<SecurityKey> =
            stocks.iter().filter(|k| self.router().supports(Capability::Filings, Some(k))).cloned().collect();
        if covered.is_empty() {
            inbox.unavailable = Some(format!("no configured source provides SEC filings ({})", self.unavailable_hint()));
            return inbox;
        }
        let mut set = tokio::task::JoinSet::new();
        for (i, k) in covered.iter().enumerate() {
            let (e, k) = (self.clone(), k.clone());
            set.spawn(async move {
                let r = e.filings_list(&k, &[], PER_COMPANY).await;
                (i, k, r)
            });
        }
        let mut results = Vec::with_capacity(covered.len());
        while let Some(r) = set.join_next().await {
            if let Ok(x) = r {
                results.push(x);
            }
        }
        results.sort_by_key(|(i, _, _)| *i);

        let mut seen: HashSet<String> = HashSet::new();
        let mut no_filer: Vec<String> = Vec::new();
        let mut errors: Vec<(String, String)> = Vec::new();
        let mut any_ok = false;
        for (_, key, r) in results {
            let list = match r {
                Ok(l) => l,
                Err(e) if not_a_filer(&e) => {
                    no_filer.push(key.symbol.clone());
                    continue;
                }
                Err(e) => {
                    errors.push((key.symbol.clone(), e.user_message()));
                    continue;
                }
            };
            any_ok = true;
            if list.stale {
                inbox.stale_since = Some(inbox.stale_since.map_or(list.fetched_at, |s| s.min(list.fetched_at)));
            }
            for (idx, f) in list.value.iter().enumerate() {
                if f.filed < since || !seen.insert(f.accession.clone()) {
                    continue;
                }
                if inbox.provenance.is_none() {
                    inbox.provenance = Some(f.provenance.clone());
                }
                let has_prior = list.value.iter().skip(idx + 1).any(|p| p.form == f.form);
                inbox.items.push(InboxItem { key: key.clone(), filing: f.clone(), unread: true, has_prior });
            }
        }
        if !any_ok && !errors.is_empty() {
            let mut msgs: Vec<&str> = errors.iter().map(|(_, m)| m.as_str()).collect();
            msgs.dedup();
            inbox.unavailable = Some(msgs.join("; "));
            return inbox;
        }
        if !errors.is_empty() {
            let who: Vec<&str> = errors.iter().map(|(s, _)| s.as_str()).collect();
            inbox.notes.push(Block::Notice {
                level: NoticeLevel::Warning,
                text: format!("Filings for {}: NOT AVAILABLE — {}", who.join(", "), errors[0].1),
            });
        }
        if !no_filer.is_empty() {
            let source = self
                .router()
                .providers_for(Capability::Filings, covered.first())
                .first()
                .map_or_else(|| "the filings source".to_owned(), |p| source_name(p.as_str()));
            inbox.notes.push(Block::Notice {
                level: NoticeLevel::Info,
                text: format!("No SEC filings found for {} ({source})", no_filer.join(", ")),
            });
        }
        let accessions: Vec<String> = inbox.items.iter().map(|i| i.filing.accession.clone()).collect();
        let read = self.stores().app.read_filings(&accessions).unwrap_or_default();
        for it in &mut inbox.items {
            it.unread = !read.contains(&it.filing.accession);
        }
        inbox.items.sort_by(|a, b| {
            sort_time(&b.filing).cmp(&sort_time(&a.filing)).then_with(|| a.filing.accession.cmp(&b.filing.accession))
        });
        inbox
    }
}

const PERIODS: [(&str, i64); 3] = [("Last 7 days", 7), ("Last 30 days", 30), ("Last 90 days", 90)];
const SHOW: [&str; 2] = ["All", "Unread"];

fn period_of(arg: Option<&str>) -> (&'static str, i64) {
    let a = arg.map(|a| a.trim().to_ascii_lowercase());
    PERIODS
        .iter()
        .find(|(label, days)| a.as_deref().is_some_and(|a| a == label.to_ascii_lowercase() || a == days.to_string()))
        .copied()
        .unwrap_or(PERIODS[1])
}

pub(crate) fn inbox_columns() -> Vec<Column> {
    vec![
        Column::text("", 1),
        Column::text("Security", 9),
        Column::text("Form", 9),
        Column::num("Filed", Format::DateTime, 16),
        Column::text("Description", 70),
    ]
}

pub(crate) fn inbox_row(it: &InboxItem) -> Row {
    let (dot, text) = if it.unread { (Cell::text("●").styled(Style::Emphasis), Style::Emphasis) } else { (Cell::empty(), Style::Muted) };
    Row::new(vec![
        dot,
        Cell::text(&it.key.symbol).styled(Style::Link),
        Cell::text(&it.filing.form).styled(text),
        it.filed_cell(),
        Cell::text(it.description()).styled(text),
    ])
    .security(&it.key.to_string())
    .action(it.action())
}

pub(crate) async fn filings(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    let now = engine.now();
    let today = new_york_date(now);
    let (period, days) = period_of(req.arg("days"));
    let unread_only = req.arg("show").is_some_and(|s| s.trim().eq_ignore_ascii_case("unread"));
    let keys = engine.scope_keys(super::scope::Scope::Mine, today);
    let mut inbox = engine.filings_inbox(&keys, today - Duration::days(days)).await;

    // Mark read: everything listed up to a time (idempotent on reload, and
    // filings that arrive later stay unread), or mark one unread.
    let mut changed = false;
    if let Some(through) = req.arg("read_through").and_then(|t| t.trim().parse::<i64>().ok()) {
        let ids: Vec<String> =
            inbox.items.iter().filter(|i| i.unread && sort_time(&i.filing) <= through).map(|i| i.filing.accession.clone()).collect();
        if !ids.is_empty() {
            changed = engine.stores().app.mark_filings_read(&ids, now).is_ok();
        }
    }
    if let Some(acc) = req.arg("unread").map(str::trim).filter(|a| !a.is_empty()) {
        changed |= engine.stores().app.mark_filing_unread(acc).is_ok();
    }
    if changed {
        let accessions: Vec<String> = inbox.items.iter().map(|i| i.filing.accession.clone()).collect();
        let read = engine.stores().app.read_filings(&accessions).unwrap_or_default();
        for it in &mut inbox.items {
            it.unread = !read.contains(&it.filing.accession);
        }
    }

    if let Some(reason) = inbox.unavailable.take() {
        return Screen::not_available("FILINGS", TITLE, None, reason);
    }
    let mut s = Screen::new("FILINGS", TITLE, None);
    if let Some(p) = &inbox.provenance {
        s.source(p);
    }
    let base = Action::new("FILINGS", None).arg("days", days.to_string()).arg("show", if unread_only { "Unread" } else { "All" });
    if let Some(newest) = inbox.items.iter().filter(|i| i.unread).map(|i| sort_time(&i.filing)).max() {
        s.menu_item("Mark all read", base.clone().arg("read_through", newest.to_string()), false);
    }
    s.menu_item("Unread only", base.clone().arg("show", "Unread"), unread_only);
    s.menu_item("All filings", base.arg("show", "All"), !unread_only);
    s.push(Block::Inputs {
        title: None,
        inputs: vec![
            Input {
                id: "days".into(),
                label: "Period".into(),
                value: period.into(),
                kind: InputKind::Choice,
                options: PERIODS.iter().map(|(l, _)| (*l).to_owned()).collect(),
            },
            Input {
                id: "show".into(),
                label: "Show".into(),
                value: if unread_only { SHOW[1] } else { SHOW[0] }.into(),
                kind: InputKind::Choice,
                options: SHOW.iter().map(|o| (*o).to_owned()).collect(),
            },
        ],
    });
    if let Some(at) = inbox.stale_since {
        s.push(Block::Notice {
            level: NoticeLevel::Warning,
            text: format!("OFFLINE — showing cached filings from {}", super::fmt_datetime(at)),
        });
    }
    for n in &inbox.notes {
        s.push(n.clone());
    }
    let unread = inbox.unread();
    let total = inbox.items.len();
    let rows: Vec<Row> = inbox.items.iter().filter(|i| !unread_only || i.unread).map(inbox_row).collect();
    if rows.is_empty() {
        let text = if unread_only && total > 0 {
            format!("No unread filings in the {} ({total} read)", period.to_ascii_lowercase())
        } else {
            format!("No filings in the {} for your holdings and watchlists", period.to_ascii_lowercase())
        };
        s.push(Block::Notice { level: NoticeLevel::Info, text });
        return s;
    }
    s.push(Block::Table(Table {
        title: Some(format!("{period} · {unread} unread of {total}")),
        columns: inbox_columns(),
        rows,
        page_size: Some(25),
        numbered: true,
    }));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn eight_k_items_use_official_titles() {
        assert_eq!(
            plain_description("8-K", &items(&["2.02", "9.01"]), None),
            "Results of Operations and Financial Condition"
        );
        assert_eq!(
            plain_description("8-K", &items(&["5.02", "7.01"]), None),
            "Departure of Directors or Certain Officers; Election of Directors; Appointment of Certain Officers; \
             Compensatory Arrangements of Certain Officers · Regulation FD Disclosure"
        );
        assert_eq!(plain_description("8-K", &items(&["9.01"]), None), "Financial Statements and Exhibits");
        assert_eq!(plain_description("8-K/A", &items(&["1.01"]), None), "Entry into a Material Definitive Agreement (amended)");
        // Unknown items fall back to the form's plain name.
        assert_eq!(plain_description("8-K", &items(&["99.99"]), None), "Current report");
        assert_eq!(plain_description("8-K", &[], Some("8-K")), "Current report");
        assert_eq!(FORM_8K_ITEMS.len(), 33);
    }

    #[test]
    fn other_forms_read_plainly() {
        assert_eq!(plain_description("10-K", &[], Some("Annual report [Section 13 and 15(d)]")), "Annual report");
        assert_eq!(plain_description("10-Q", &[], None), "Quarterly report");
        assert_eq!(plain_description("4", &[], None), "Insider transaction");
        assert_eq!(plain_description("4/A", &[], None), "Insider transaction (amended)");
        assert_eq!(plain_description("DEF 14A", &[], None), "Proxy statement");
        assert_eq!(plain_description("N-CSR", &[], Some("Certified shareholder report")), "Certified shareholder report");
        assert_eq!(plain_description("N-CSR", &[], None), "N-CSR");
        assert_eq!(plain_description("X-1", &[], Some("X-1")), "X-1");
    }

    fn filing(form: &str) -> Filing {
        Filing {
            cik: 1,
            company: "Test Co".into(),
            accession: format!("0000000001-26-{form}"),
            form: form.into(),
            filed: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            accepted_at: None,
            period_of_report: None,
            primary_document: None,
            primary_doc_url: None,
            description: None,
            size_bytes: None,
            items: Vec::new(),
            provenance: Provenance::synthetic(0),
        }
    }

    #[test]
    fn periodic_reports_open_on_what_changed() {
        let item = |form: &str, has_prior| InboxItem { key: SecurityKey::equity("TST"), filing: filing(form), unread: true, has_prior };
        let args = |i: &InboxItem| i.action().args;
        assert!(args(&item("10-K", true)).contains(&("diff".into(), "1".into())));
        assert!(args(&item("10-Q", true)).contains(&("diff".into(), "1".into())));
        // Nothing earlier to compare with, or a form where comparing is noise.
        assert!(!args(&item("10-K", false)).iter().any(|(k, _)| k == "diff"));
        assert!(!args(&item("8-K", true)).iter().any(|(k, _)| k == "diff"));
        assert!(!args(&item("4", true)).iter().any(|(k, _)| k == "diff"));
        let a = item("8-K", true).action();
        assert_eq!((a.function.as_str(), a.security.as_deref()), ("CF", Some("TST US Equity")));
        assert_eq!(a.args[0], ("doc".into(), "0000000001-26-8-K".into()));
        // No acceptance time: the filing date as text.
        assert_eq!(item("4", false).filed_cell().text.as_deref(), Some("10/01/2026"));
    }

    #[test]
    fn periods_parse() {
        assert_eq!(period_of(None), ("Last 30 days", 30));
        assert_eq!(period_of(Some("7")), ("Last 7 days", 7));
        assert_eq!(period_of(Some("Last 90 days")), ("Last 90 days", 90));
        assert_eq!(period_of(Some("forever")), ("Last 30 days", 30));
    }
}

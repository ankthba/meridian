//! CF — company filings: list, document view, diff vs prior, AI summary.

use std::sync::Arc;

use meridian_provider::FilingsRequest;
use meridian_types::{Filing, FilingDocument, SecurityKey};
use similar::{ChangeTag, TextDiff};

use super::{ScreenRequest, error_screen, require_security, stale_notice};
use crate::cache::{Fetched, ttl};
use crate::core::Engine;
use crate::error::{EngineError, EngineResult};
use crate::screen::{
    Action, Block, Cell, Column, DiffKind, DiffLine, Field, Format, Input, InputKind, NoticeLevel, Row, Screen, Style, Table,
};

const TITLE: &str = "Company Filings";

const FORM_FILTERS: &[(&str, &[&str])] = &[
    ("All", &[]),
    ("10-K", &["10-K", "10-K/A", "20-F", "40-F"]),
    ("10-Q", &["10-Q", "10-Q/A"]),
    ("8-K", &["8-K", "8-K/A", "6-K"]),
    ("Proxy", &["DEF 14A", "DEFA14A", "PRE 14A"]),
    ("Ownership", &["3", "4", "5", "SC 13D", "SC 13G", "SC 13D/A", "SC 13G/A", "13F-HR"]),
];

impl Engine {
    pub async fn filings_list(&self, key: &SecurityKey, forms: &[&str], limit: usize) -> EngineResult<Fetched<Vec<Filing>>> {
        let ck = format!("{key}|{}|{limit}", forms.join(","));
        let req = FilingsRequest { key: Some(key.clone()), forms: forms.iter().map(|f| (*f).to_string()).collect(), limit };
        let router = self.router().clone();
        let fetched = self
            .cached("filings", &ck, ttl::FILINGS, async move {
                router.filings(req).await.map(|r| meridian_provider::Routed { value: r.value.filings, provider: r.provider })
            })
            .await?;
        if !fetched.from_cache {
            let _ = self.stores().market.put_filings(&fetched.value);
        }
        Ok(fetched)
    }

    pub async fn filing_document(&self, filing: &Filing) -> EngineResult<Fetched<FilingDocument>> {
        let router = self.router().clone();
        let f = filing.clone();
        self.cached("filing_doc", &filing.accession, ttl::FILING_DOC, async move { router.filing_document(&f).await }).await
    }
}

fn filter_forms(name: &str) -> &'static [&'static str] {
    FORM_FILTERS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map_or(&[], |(_, f)| f)
}

pub(crate) async fn cf(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    let key = match require_security("CF", TITLE, &req) {
        Ok(k) => k,
        Err(s) => return *s,
    };
    if let Some(acc) = req.arg("doc").map(str::to_owned) {
        return document(engine, &req, &key, &acc).await;
    }
    let filter = req.arg("form").unwrap_or("All").to_owned();
    let forms = filter_forms(&filter);
    let list = match engine.filings_list(&key, forms, 200).await {
        Ok(l) => l,
        Err(e) => return error_screen("CF", TITLE, Some(&key), &e),
    };
    let ks = key.to_string();
    let mut s = Screen::new("CF", format!("{ks} — {TITLE}"), Some(ks.clone()));
    if let Some(f) = list.value.first() {
        s.source(&f.provenance);
    }
    stale_notice(&mut s, &list);
    s.push(Block::Inputs {
        title: None,
        inputs: vec![Input {
            id: "form".into(),
            label: "Form".into(),
            value: filter.clone(),
            kind: InputKind::Choice,
            options: FORM_FILTERS.iter().map(|(n, _)| (*n).to_string()).collect(),
        }],
    });
    let rows: Vec<Row> = list
        .value
        .iter()
        .map(|f| {
            Row::new(vec![
                Cell::text(f.filed.format("%m/%d/%y").to_string()),
                Cell::text(&f.form).styled(Style::Emphasis),
                Cell::text(f.description.clone().unwrap_or_else(|| f.form.clone())),
                Cell::text(f.period_of_report.map(|d| d.format("%m/%d/%y").to_string()).unwrap_or_default()),
                Cell::num(f.size_bytes.map(|b| b as f64)),
            ])
            .action(Action::new("CF", Some(&ks)).arg("doc", f.accession.clone()))
        })
        .collect();
    if rows.is_empty() {
        s.push(Block::Notice { level: NoticeLevel::Info, text: format!("No {filter} filings found") });
        return s;
    }
    s.push(Block::Table(Table {
        title: None,
        columns: vec![
            Column::text("Filed", 9),
            Column::text("Form", 9),
            Column::text("Description", 60),
            Column::text("Period", 9),
            Column::num("Size", Format::Large { decimals: 1 }, 8),
        ],
        rows,
        page_size: Some(22),
        numbered: true,
    }));
    s
}

async fn document(engine: Arc<Engine>, req: &ScreenRequest, key: &SecurityKey, accession: &str) -> Screen {
    let ks = key.to_string();
    let list = match engine.filings_list(key, &[], 400).await {
        Ok(l) => l.value,
        Err(e) => return error_screen("CF", TITLE, Some(key), &e),
    };
    let Some(idx) = list.iter().position(|f| f.accession == accession) else {
        return Screen::not_available("CF", TITLE, Some(ks), format!("filing {accession} not found"));
    };
    let filing = list[idx].clone();
    let doc = match engine.filing_document(&filing).await {
        Ok(d) => d,
        Err(e) => return error_screen("CF", TITLE, Some(key), &e),
    };
    let mut s = Screen::new("CF", format!("{} {} — filed {}", filing.company, filing.form, filing.filed.format("%m/%d/%Y")), Some(ks.clone()));
    s.source(&filing.provenance);
    stale_notice(&mut s, &doc);
    let base = Action::new("CF", Some(&ks)).arg("doc", accession);
    s.menu_item("Sections", base.clone(), req.arg("section").is_none() && req.arg("diff").is_none() && req.arg("summary").is_none());
    s.menu_item("Diff vs Prior", base.clone().arg("diff", "1"), req.arg("diff").is_some());
    s.menu_item("AI Summary", base.clone().arg("summary", "1"), req.arg("summary").is_some());
    s.menu_item("Back to List", Action::new("CF", Some(&ks)), false);

    let mut fields = vec![
        Field::text("Form", &filing.form).styled(Style::Emphasis),
        Field::text("Accession", &filing.accession),
        Field::text("Filed", filing.filed.format("%m/%d/%Y").to_string()),
    ];
    if let Some(p) = filing.period_of_report {
        fields.push(Field::text("Period", p.format("%m/%d/%Y").to_string()));
    }
    if let Some(u) = &filing.primary_doc_url {
        fields.push(Field::text("Document", u).styled(Style::Link));
    }
    s.push(Block::Fields { title: None, columns: 2, fields });

    if req.arg("diff").is_some() {
        let prior = list.iter().skip(idx + 1).find(|f| f.form == filing.form).cloned();
        let Some(prior) = prior else {
            s.push(Block::Notice { level: NoticeLevel::Info, text: format!("No earlier {} on file to compare", filing.form) });
            return s;
        };
        match engine.filing_document(&prior).await {
            Ok(pdoc) => {
                let summary = diff_documents(&pdoc.value, &doc.value, &mut s);
                s.blocks.insert(
                    s.blocks.len() - summary,
                    Block::Notice {
                        level: NoticeLevel::Info,
                        text: format!("Changes vs {} filed {} ({})", prior.form, prior.filed.format("%m/%d/%Y"), prior.accession),
                    },
                );
            }
            Err(e) => s.push(Block::Notice { level: NoticeLevel::Error, text: e.user_message() }),
        }
        return s;
    }

    if req.arg("summary").is_some() {
        match engine.summarize_filing(&filing, &doc.value).await {
            Ok(text) => s.push(Block::Text { title: Some("AI Summary".into()), body: text }),
            Err(e) => s.push(Block::Notice {
                level: if e.is_unavailable() { NoticeLevel::Warning } else { NoticeLevel::Error },
                text: e.user_message(),
            }),
        }
        return s;
    }

    let sections = &doc.value.sections;
    match req.arg("section").and_then(|x| x.parse::<usize>().ok()) {
        Some(i) if i >= 1 && i <= sections.len() => {
            let sec = &sections[i - 1];
            s.push(Block::Text { title: Some(sec.title.clone()), body: sec.text.clone() });
        }
        _ => {
            let rows = sections
                .iter()
                .enumerate()
                .map(|(i, sec)| {
                    Row::new(vec![Cell::text(&sec.title).styled(Style::Emphasis), Cell::num(Some(sec.text.split_whitespace().count() as f64))])
                        .action(base.clone().arg("section", (i + 1).to_string()))
                })
                .collect();
            s.push(Block::Table(Table {
                title: Some("Sections".into()),
                columns: vec![Column::text("Section", 70), Column::num("Words", Format::Integer, 8)],
                rows,
                page_size: Some(22),
                numbered: true,
            }));
        }
    }
    s
}

/// Pushes one diff block per changed section; returns blocks pushed.
fn diff_documents(old: &FilingDocument, new: &FilingDocument, s: &mut Screen) -> usize {
    let mut pushed = 0;
    let norm = |t: &str| t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).take(6).collect::<Vec<_>>().join(" ").to_lowercase();
    for sec in &new.sections {
        let key = norm(&sec.title);
        let old_text = old.sections.iter().find(|o| norm(&o.title) == key).map_or("", |o| o.text.as_str());
        let lines = paragraph_diff(old_text, &sec.text);
        if lines.iter().any(|l| l.kind != DiffKind::Same) {
            s.push(Block::Diff { title: sec.title.clone(), lines });
            pushed += 1;
        }
    }
    for o in &old.sections {
        if !new.sections.iter().any(|n| norm(&n.title) == norm(&o.title)) {
            s.push(Block::Diff { title: format!("{} (removed)", o.title), lines: vec![DiffLine { kind: DiffKind::Removed, text: o.text.clone() }] });
            pushed += 1;
        }
    }
    if pushed == 0 {
        s.push(Block::Notice { level: NoticeLevel::Info, text: "No textual changes".into() });
        pushed = 1;
    }
    pushed
}

/// Paragraph-level diff with unchanged runs collapsed to one context
/// paragraph on each side.
fn paragraph_diff(old: &str, new: &str) -> Vec<DiffLine> {
    let paras = |t: &str| -> String {
        t.split("\n\n").map(|p| p.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n")
    };
    let (a, b) = (paras(old), paras(new));
    let diff = TextDiff::from_lines(&a, &b);
    let mut raw: Vec<DiffLine> = diff
        .iter_all_changes()
        .map(|c| DiffLine {
            kind: match c.tag() {
                ChangeTag::Equal => DiffKind::Same,
                ChangeTag::Insert => DiffKind::Added,
                ChangeTag::Delete => DiffKind::Removed,
            },
            text: c.value().trim_end().to_string(),
        })
        .collect();
    // Collapse long unchanged runs.
    let mut out = Vec::new();
    let n = raw.len();
    let near_change = |i: usize, raw: &[DiffLine]| {
        (i.saturating_sub(1)..=(i + 1).min(n.saturating_sub(1))).any(|j| raw[j].kind != DiffKind::Same)
    };
    let mut skipped = 0usize;
    for i in 0..n {
        if raw[i].kind != DiffKind::Same || near_change(i, &raw) {
            if skipped > 0 {
                out.push(DiffLine { kind: DiffKind::Same, text: format!("… {skipped} unchanged paragraph(s) …") });
                skipped = 0;
            }
            out.push(std::mem::replace(&mut raw[i], DiffLine { kind: DiffKind::Same, text: String::new() }));
        } else {
            skipped += 1;
        }
    }
    if skipped > 0 && !out.is_empty() {
        out.push(DiffLine { kind: DiffKind::Same, text: format!("… {skipped} unchanged paragraph(s) …") });
    }
    out
}

impl Engine {
    /// AI summary of a filing via the ASK model. Implemented by the ASK
    /// integration; until an API key is configured it reports why.
    pub async fn summarize_filing(&self, filing: &Filing, doc: &FilingDocument) -> EngineResult<String> {
        match self.ai() {
            Some(ai) => ai.summarize_filing(filing, doc).await,
            None => Err(EngineError::NotAvailable {
                what: "AI summary".into(),
                reason: "add an Anthropic API key in Settings to enable ASK features".into(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_collapses_unchanged() {
        let old = "A\n\nB\n\nC\n\nD\n\nE";
        let new = "A\n\nB\n\nC changed\n\nD\n\nE";
        let d = paragraph_diff(old, new);
        assert!(d.iter().any(|l| l.kind == DiffKind::Added && l.text == "C changed"));
        assert!(d.iter().any(|l| l.kind == DiffKind::Removed && l.text == "C"));
        assert!(d.iter().any(|l| l.text.contains("unchanged")));
    }

    #[test]
    fn identical_docs_have_no_changes() {
        let d = paragraph_diff("x\n\ny", "x\n\ny");
        assert!(d.iter().all(|l| l.kind == DiffKind::Same));
    }
}

//! Synthetic SEC-style filings and their documents.
//!
//! Filing dates follow the company's synthetic reporting calendar (8-K on
//! the earnings date, 10-Q/10-K shortly after). Documents are assembled from
//! templates and the company's synthetic fundamentals; risk factors and
//! MD&A vary by fiscal year so consecutive 10-Ks differ. Every document
//! opens with a section stating it is synthetic.

use std::fmt::Write as _;

use chrono::{Datelike, Duration, NaiveDate};
use meridian_provider::{FilingsRequest, ProviderError, ProviderResult};
use meridian_types::{Filing, FilingDocument, FilingSection, FilingsPage, PeriodType, Provenance, UnixNanos};

use crate::Inner;
use crate::cal::{Calendar, Tz};
use crate::corp::person;
use crate::fundamentals::{Period, Quarter};
use crate::hash::{Cell, fnv1a, tag};
use crate::universe::{Kind, Sym, filing_cik};

pub const FILING_BANNER: &str = "SYNTHETIC FILING — MOCK DATA. Not a real SEC document.";

/// Thousands separators for an integer-valued amount.
pub(crate) fn commas(x: f64) -> String {
    let neg = x < 0.0;
    let s = format!("{:.0}", x.abs());
    let mut out = String::with_capacity(s.len() + s.len() / 3 + 1);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if neg { format!("({out})") } else { out }
}

/// Amount in millions with separators, e.g. `$391,035 million`.
pub(crate) fn millions(x: f64) -> String {
    format!("${} million", commas(x / 1e6))
}

pub(crate) fn pct_change(a: f64, b: f64) -> f64 {
    if b.abs() < 1e-9 { 0.0 } else { (a / b - 1.0) * 100.0 }
}

#[derive(Debug, Clone)]
struct Planned {
    form: &'static str,
    filed: NaiveDate,
    period: Option<NaiveDate>,
    items: Vec<String>,
    salt: u64,
}

fn description(form: &str, items: &[String]) -> String {
    let base = match form {
        "10-K" => "Annual report [Section 13 and 15(d), not S-K Item 405]".to_string(),
        "10-Q" => "Quarterly report [Sections 13 or 15(d)]".to_string(),
        "8-K" => format!("Current report, items {}", items.join(", ")),
        "DEF 14A" => "Other definitive proxy statements".to_string(),
        "4" => "Statement of changes in beneficial ownership of securities".to_string(),
        _ => form.to_string(),
    };
    format!("{base} (SYNTHETIC — MOCK DATA)")
}

impl Inner {
    fn planned_filings(&self, s: &Sym, today: NaiveDate) -> Vec<(Planned, Option<Quarter>)> {
        let Some(model) = self.company(s, today) else { return Vec::new() };
        let since = today - Duration::days(3 * 365 + 30);
        let cal = Calendar::Nyse;
        let mut out: Vec<(Planned, Option<Quarter>)> = Vec::new();
        for q in &model.quarters {
            if q.announce < since || q.announce > today {
                continue;
            }
            let mut c = Cell::new(&[self.seed, s.hash, tag("filing-dates"), crate::daily::day_num(q.end)]);
            out.push((
                Planned {
                    form: "8-K",
                    filed: q.announce,
                    period: Some(q.announce),
                    items: vec!["2.02".into(), "9.01".into()],
                    salt: 1,
                },
                Some(q.clone()),
            ));
            let (form, lag) = if q.fq == 4 { ("10-K", c.int(5, 20)) } else { ("10-Q", c.int(0, 3)) };
            let filed = cal.on_or_after(q.announce + Duration::days(lag));
            if filed <= today {
                out.push((
                    Planned { form, filed, period: Some(q.end), items: Vec::new(), salt: 2 },
                    Some(q.clone()),
                ));
            }
            if q.fq == 4 {
                let proxy = cal.on_or_after(q.end + Duration::days(c.int(95, 120)));
                if proxy <= today && proxy >= since {
                    out.push((
                        Planned { form: "DEF 14A", filed: proxy, period: None, items: Vec::new(), salt: 3 },
                        Some(q.clone()),
                    ));
                }
            }
        }
        // Occasional other 8-Ks and Form 4s.
        let mut c = Cell::new(&[self.seed, s.hash, tag("filing-misc")]);
        let span = (today - since).num_days().max(1);
        for k in 0..6u64 {
            let d = cal.on_or_after(since + Duration::days(c.int(0, span)));
            if d <= today {
                let item = *c.pick(&["5.02", "8.01", "7.01", "5.07"]);
                out.push((
                    Planned { form: "8-K", filed: d, period: Some(d), items: vec![item.into()], salt: 10 + k },
                    None,
                ));
            }
        }
        for k in 0..12u64 {
            let d = cal.on_or_after(since + Duration::days(c.int(0, span)));
            if d <= today {
                out.push((
                    Planned { form: "4", filed: d, period: Some(d - Duration::days(2)), items: Vec::new(), salt: 100 + k },
                    None,
                ));
            }
        }
        out.sort_by(|a, b| b.0.filed.cmp(&a.0.filed).then(a.0.salt.cmp(&b.0.salt)));
        out
    }

    fn to_filing(&self, s: &Sym, p: &Planned, now: UnixNanos) -> Filing {
        let h = fnv1a(format!("{}|{}|{}|{}", s.ticker(), p.form, p.filed, p.salt).as_bytes()) ^ self.seed;
        let mut c = Cell::new(&[h, tag("filing")]);
        let accession = format!("0009999999-{:02}-{:06}", p.filed.year().rem_euclid(100), h % 1_000_000);
        let t = s.ticker().to_ascii_lowercase();
        let ymd = |d: NaiveDate| d.format("%Y%m%d").to_string();
        let primary = match p.form {
            "10-K" | "10-Q" => format!("{t}-{}.htm", ymd(p.period.unwrap_or(p.filed))),
            "8-K" => format!("{t}-8k_{}.htm", ymd(p.filed)),
            "DEF 14A" => format!("{t}-def14a_{}.htm", ymd(p.filed)),
            _ => format!("form4_{}.xml", h % 100_000),
        };
        let size = match p.form {
            "10-K" => c.int(1_500_000, 12_000_000),
            "10-Q" => c.int(500_000, 5_000_000),
            "8-K" => c.int(50_000, 500_000),
            "DEF 14A" => c.int(1_000_000, 4_000_000),
            _ => c.int(5_000, 15_000),
        } as u64;
        let accepted = Tz::NewYork.to_utc(p.filed, 16 * 60 + c.int(5, 90) as i32);
        let mut prov = Provenance::synthetic(now).with_ref(accession.clone());
        prov.attribution = None;
        Filing {
            cik: filing_cik(s),
            company: s.inst.name.clone(),
            accession,
            form: p.form.into(),
            filed: p.filed,
            accepted_at: Some(accepted),
            period_of_report: p.period,
            primary_document: Some(primary),
            primary_doc_url: None,
            description: Some(description(p.form, &p.items)),
            size_bytes: Some(size),
            items: p.items.clone(),
            provenance: prov,
        }
    }

    pub(crate) fn filings_for(&self, req: &FilingsRequest) -> ProviderResult<FilingsPage> {
        let now = self.clock.now();
        let today = meridian_types::nanos_to_date(now);
        let limit = if req.limit == 0 { 100 } else { req.limit };
        let form_ok = |f: &str| req.forms.is_empty() || req.forms.iter().any(|x| x.trim().eq_ignore_ascii_case(f));
        let mut filings = Vec::new();
        if let Some(key) = &req.key {
            {
                let s = self.resolve(key)?;
                if s.p.kind != Kind::Equity {
                    return Err(ProviderError::NotFound(format!("no filings for {key}: not an operating company")));
                }
                for (p, _) in self.planned_filings(&s, today) {
                    if form_ok(p.form) {
                        filings.push(self.to_filing(&s, &p, now));
                    }
                }
            }
        } else {
            {
                // Latest filings across the core universe (last 30 days).
                let since = today - Duration::days(30);
                for s in self.universe.syms.iter().filter(|s| s.p.kind == Kind::Equity && !s.p.filler) {
                    for (p, _) in self.planned_filings(s, today) {
                        if p.filed >= since && form_ok(p.form) {
                            filings.push(self.to_filing(s, &p, now));
                        }
                    }
                }
                filings.sort_by(|a, b| b.accepted_at.cmp(&a.accepted_at));
            }
        }
        filings.truncate(limit);
        Ok(FilingsPage { key: req.key.clone(), filings })
    }

    pub(crate) fn filing_document_for(&self, f: &Filing) -> ProviderResult<FilingDocument> {
        let now = self.clock.now();
        let today = meridian_types::nanos_to_date(now);
        let idx = self
            .universe
            .by_cik(f.cik)
            .ok_or_else(|| ProviderError::NotFound(format!("CIK {} is not in the mock universe", f.cik)))?;
        let s = &self.universe.syms[idx];
        let planned = self.planned_filings(s, today);
        let (p, q) = planned
            .iter()
            .find(|(p, _)| self.to_filing(s, p, now).accession == f.accession)
            .ok_or_else(|| ProviderError::NotFound(format!("accession {} is not a mock filing", f.accession)))?;
        let filing = self.to_filing(s, p, now);
        let mut sections = vec![FilingSection {
            title: FILING_BANNER.into(),
            text: format!(
                "This document was generated locally by Meridian's mock data provider for interface development. \
                 The company name and ticker are real; every figure, statement, name, and event below is synthetic \
                 and must not be relied on. Form {}, accession {}, filed {}.",
                filing.form, filing.accession, filing.filed
            ),
        }];
        let model = self.company(s, today);
        let doc = DocCtx { inner: self, s, filing: &filing, quarter: q.as_ref(), model: model.as_ref() };
        match p.form {
            "10-K" => doc.annual(&mut sections),
            "10-Q" => doc.quarterly(&mut sections),
            "8-K" => doc.current(&mut sections, &p.items),
            "DEF 14A" => doc.proxy(&mut sections),
            _ => doc.form4(&mut sections),
        }
        Ok(FilingDocument { filing, sections })
    }
}

struct DocCtx<'a> {
    inner: &'a Inner,
    s: &'a Sym,
    filing: &'a Filing,
    quarter: Option<&'a Quarter>,
    model: Option<&'a crate::fundamentals::CompanyModel>,
}

const RISKS: &[(&str, &str)] = &[
    ("Competition", "We face intense competition in the {industry} industry. Competitors may introduce products or services at lower prices or with features that reduce demand for ours, which could reduce our revenue and margins."),
    ("Macroeconomic conditions", "Our results depend on global economic conditions. Slower growth, inflation, changes in interest rates, or reduced consumer and business spending could adversely affect demand for our offerings."),
    ("Supply chain", "We rely on third parties for components, manufacturing, logistics, and services. Disruptions, capacity constraints, or price increases at these suppliers could delay shipments and increase our costs."),
    ("Cybersecurity", "Our systems and those of our partners are subject to cyberattacks and other security incidents. A significant breach could disrupt operations, expose sensitive information, and result in liability and reputational harm."),
    ("Regulation", "We are subject to complex and changing laws in the jurisdictions where we operate, including competition, privacy, trade, and tax rules. New regulations or enforcement actions could require changes to our business."),
    ("International operations", "A significant portion of our revenue is generated outside the United States, exposing us to currency fluctuations, tariffs, export controls, and political instability."),
    ("Key personnel", "Our success depends on retaining and attracting key executives and skilled employees. The loss of key personnel or failure to hire could impair our ability to execute our strategy."),
    ("Intellectual property", "We may be unable to adequately protect our intellectual property, and third parties may claim that we infringe theirs, which could result in costly litigation or licensing arrangements."),
    ("Acquisitions", "We have made and may continue to make acquisitions. Integrating acquired businesses involves risks, and expected benefits may not be realized, which could lead to impairment of goodwill."),
    ("Indebtedness", "Our outstanding debt and future borrowings could limit our flexibility and increase our exposure to rising interest rates. A downgrade of our credit ratings could increase our borrowing costs."),
    ("Climate and natural events", "Severe weather, natural disasters, pandemics, and other events could disrupt our operations, our suppliers, and our customers, and could increase our costs."),
    ("Technology change", "Rapid changes in technology, including artificial intelligence, could make our products less competitive or require significant investment to keep pace."),
    ("Customer concentration", "A limited number of customers and channel partners account for a meaningful share of our revenue. Changes in their purchasing could have a disproportionate effect on our results."),
    ("Litigation", "We are party to legal proceedings and investigations in the ordinary course of business. Unfavorable outcomes could result in significant damages, fines, or changes to business practices."),
    ("Tax", "Changes in tax laws or their interpretation, including global minimum tax rules, could increase our effective tax rate and affect our results."),
    ("Stock price volatility", "The market price of our common stock may fluctuate significantly in response to our results, guidance, analyst expectations, and broader market conditions."),
];

impl DocCtx<'_> {
    fn fill(&self, t: &str) -> String {
        t.replace("{industry}", &self.s.inst.industry.clone().unwrap_or_default().to_lowercase())
            .replace("{name}", &self.s.inst.name)
    }

    fn fy_period(&self, fy: i32) -> Option<Period> {
        let m = self.model?;
        let qs: Vec<Quarter> = m.quarters.iter().filter(|q| q.fy == fy).cloned().collect();
        (qs.len() == 4).then(|| Period::sum(&qs, PeriodType::Annual, "FY".into()))
    }

    fn statements_text(cur: &Period, prev: Option<&Period>, label_cur: &str, label_prev: &str) -> String {
        let row = |name: &str, a: f64, b: Option<f64>| match b {
            Some(b) => format!("{name:<38}{:>16}{:>16}\n", commas(a / 1e6), commas(b / 1e6)),
            None => format!("{name:<38}{:>16}\n", commas(a / 1e6)),
        };
        let mut t = format!("(in millions, except per-share amounts; SYNTHETIC)\n{:<38}{:>16}{:>16}\n", "", label_cur, label_prev);
        t.push_str(&row("Revenue", cur.revenue, prev.map(|p| p.revenue)));
        t.push_str(&row("Cost of revenue", cur.cogs, prev.map(|p| p.cogs)));
        t.push_str(&row("Gross profit", cur.gross, prev.map(|p| p.gross)));
        t.push_str(&row("Selling, general and administrative", cur.sga, prev.map(|p| p.sga)));
        t.push_str(&row("Research and development", cur.rnd, prev.map(|p| p.rnd)));
        t.push_str(&row("Operating income", cur.op, prev.map(|p| p.op)));
        t.push_str(&row("Interest expense", cur.interest, prev.map(|p| p.interest)));
        t.push_str(&row("Income before taxes", cur.pretax, prev.map(|p| p.pretax)));
        t.push_str(&row("Provision for income taxes", cur.tax, prev.map(|p| p.tax)));
        t.push_str(&row("Net income", cur.ni, prev.map(|p| p.ni)));
        let _ = writeln!(
            t,
            "{:<38}{:>16.2}{}",
            "Diluted earnings per share",
            cur.eps_diluted(),
            prev.map_or(String::new(), |p| format!("{:>16.2}", p.eps_diluted()))
        );
        let b = &cur.bal;
        t.push_str("\nBalance sheet data (end of period):\n");
        t.push_str(&row("Cash and cash equivalents", b.cash, None));
        t.push_str(&row("Total assets", b.ta(), None));
        t.push_str(&row("Total liabilities", b.tl(), None));
        t.push_str(&row("Total shareholders' equity", b.eq, None));
        t.push_str(&row("Total liabilities and equity", b.tl() + b.eq, None));
        t
    }

    fn mdna(&self, cur: &Period, prev: Option<&Period>, what: &str) -> String {
        let mut c = Cell::new(&[self.inner.seed, self.s.hash, tag("mdna"), crate::daily::day_num(cur.end)]);
        let name = &self.s.inst.name;
        let mut t = String::new();
        if let Some(p) = prev {
            let g = pct_change(cur.revenue, p.revenue);
            let dir = if g >= 0.0 { "increased" } else { "decreased" };
            let _ = write!(t, 
                "Revenue for {what} was {}, which {dir} {:.1}% compared with {} in the prior-year period. ",
                millions(cur.revenue),
                g.abs(),
                millions(p.revenue)
            );
            let gm = cur.gross / cur.revenue * 100.0;
            let gm_p = p.gross / p.revenue * 100.0;
            let _ = write!(t, "Gross margin was {gm:.1}%, compared with {gm_p:.1}% a year earlier. ");
        } else {
            let _ = write!(t, "Revenue for {what} was {}. ", millions(cur.revenue));
        }
        let _ = write!(t, 
            "Operating income was {} and net income was {}, or ${:.2} per diluted share.\n\n",
            millions(cur.op),
            millions(cur.ni),
            cur.eps_diluted()
        );
        let drivers = [
            "Results reflected higher volumes in our core offerings and continued pricing discipline.",
            "Results reflected a favorable product mix, partially offset by higher input and logistics costs.",
            "Results were affected by softer demand in certain end markets and unfavorable currency movements.",
            "We continued to invest in research, infrastructure, and go-to-market capacity to support long-term growth.",
            "Operating expenses grew more slowly than revenue as productivity initiatives took effect.",
        ];
        let _ = write!(t, "{} {}\n\n", c.pick(&drivers), c.pick(&drivers));
        let returned = cur.div + cur.buyback;
        let _ = write!(t, 
            "Liquidity and capital resources. {name} ended the period with {} of cash and cash equivalents and {} of total debt. \
             Cash from operations was {} and capital expenditures were {}. We returned {} to shareholders through dividends and share repurchases.",
            millions(cur.bal.cash),
            millions(cur.bal.std + cur.bal.ltd),
            millions(cur.cfo()),
            millions(cur.capex),
            millions(returned)
        );
        t
    }

    fn risk_factors(&self, fy: i32) -> String {
        let mut t = String::from(
            "Investing in our securities involves risk. The following synthetic risk factors are illustrative only.\n\n",
        );
        for (i, (title, body)) in RISKS.iter().enumerate() {
            let stable = Cell::new(&[self.inner.seed, self.s.hash, tag("risk"), i as u64]).u01();
            let yearly = Cell::new(&[self.inner.seed, self.s.hash, tag("risk-year"), i as u64, fy as u64]).u01();
            if stable < 0.55 || yearly < 0.3 {
                let _ = write!(t, "{title}. {}\n\n", self.fill(body));
            }
        }
        let _ = write!(t, 
            "Fiscal {fy} developments. During fiscal {fy}, {} updated its assessment of the risks above in light of conditions in the {} industry.",
            self.s.inst.name,
            self.s.inst.industry.clone().unwrap_or_default().to_lowercase()
        );
        t
    }

    fn annual(&self, out: &mut Vec<FilingSection>) {
        let Some(q) = self.quarter else { return };
        let fy = q.fy;
        let cur = self.fy_period(fy);
        let prev = self.fy_period(fy - 1);
        let name = &self.s.inst.name;
        let employees = self.inner.profile_for(self.s, self.inner.clock.now()).employees.unwrap_or(0);
        let mut c = Cell::new(&[self.inner.seed, self.s.hash, tag("10k"), fy as u64]);
        out.push(FilingSection {
            title: "Cover Page".into(),
            text: format!(
                "ANNUAL REPORT PURSUANT TO SECTION 13 OR 15(d) OF THE SECURITIES EXCHANGE ACT OF 1934 (SYNTHETIC)\nFor the fiscal year ended {}\n{name}\nCommission CIK {}",
                q.end.format("%B %-d, %Y"),
                self.filing.cik
            ),
        });
        let segs = ["Products", "Services", "International"];
        let a = c.range(0.45, 0.7);
        let b = c.range(0.15, 1.0 - a - 0.05);
        out.push(FilingSection {
            title: "Item 1. Business".into(),
            text: format!(
                "{name} (the \"Company\") operates in the {} industry within the {} sector. This description is synthetic.\n\n\
                 The Company reports three segments: {} ({:.0}% of revenue), {} ({:.0}%), and {} ({:.0}%).\n\n\
                 As of the end of fiscal {fy}, the Company had approximately {} full-time employees.",
                self.s.inst.industry.clone().unwrap_or_default().to_lowercase(),
                self.s.inst.sector.clone().unwrap_or_default(),
                segs[0],
                a * 100.0,
                segs[1],
                b * 100.0,
                segs[2],
                (1.0 - a - b) * 100.0,
                commas(employees as f64)
            ),
        });
        out.push(FilingSection { title: "Item 1A. Risk Factors".into(), text: self.risk_factors(fy) });
        out.push(FilingSection {
            title: "Item 2. Properties".into(),
            text: format!(
                "The Company's headquarters and principal facilities are owned or leased. As of the end of fiscal {fy}, it occupied approximately {} million square feet worldwide (synthetic).",
                c.int(2, 60)
            ),
        });
        if let Some(cur) = &cur {
            out.push(FilingSection {
                title: "Item 7. Management's Discussion and Analysis of Financial Condition and Results of Operations".into(),
                text: self.mdna(cur, prev.as_ref(), &format!("fiscal {fy}")),
            });
        }
        out.push(FilingSection {
            title: "Item 7A. Quantitative and Qualitative Disclosures About Market Risk".into(),
            text: format!(
                "The Company is exposed to interest rate and foreign currency risk. A hypothetical 100 basis point increase in interest rates would change annual interest expense by approximately {} (synthetic).",
                millions(cur.as_ref().map_or(0.0, |p| (p.bal.std + p.bal.ltd) * 0.01))
            ),
        });
        if let Some(cur) = &cur {
            out.push(FilingSection {
                title: "Item 8. Financial Statements and Supplementary Data".into(),
                text: Self::statements_text(cur, prev.as_ref(), &format!("FY{fy}"), &format!("FY{}", fy - 1)),
            });
        }
        out.push(FilingSection {
            title: "Item 9A. Controls and Procedures".into(),
            text: "Management concluded that the Company's disclosure controls and procedures were effective as of the end of the period (synthetic statement).".into(),
        });
    }

    fn quarterly(&self, out: &mut Vec<FilingSection>) {
        let Some(q) = self.quarter else { return };
        let Some(m) = self.model else { return };
        let cur = Period::single(q, PeriodType::Quarterly, format!("Q{}", q.fq));
        let prev = m
            .quarters
            .iter()
            .find(|x| x.fy == q.fy - 1 && x.fq == q.fq)
            .map(|x| Period::single(x, PeriodType::Quarterly, format!("Q{}", x.fq)));
        let what = format!("the {} quarter of fiscal {}", ordinal(q.fq), q.fy);
        out.push(FilingSection {
            title: "Part I — Item 1. Financial Statements".into(),
            text: Self::statements_text(&cur, prev.as_ref(), &q.label(), &format!("Q{} {:02}", q.fq, (q.fy - 1).rem_euclid(100))),
        });
        out.push(FilingSection {
            title: "Part I — Item 2. Management's Discussion and Analysis of Financial Condition and Results of Operations".into(),
            text: self.mdna(&cur, prev.as_ref(), &what),
        });
        out.push(FilingSection {
            title: "Part I — Item 3. Quantitative and Qualitative Disclosures About Market Risk".into(),
            text: "There were no material changes to the Company's market risk disclosures during the quarter (synthetic).".into(),
        });
        out.push(FilingSection {
            title: "Part I — Item 4. Controls and Procedures".into(),
            text: "There were no changes in internal control over financial reporting that materially affected it during the quarter (synthetic).".into(),
        });
        out.push(FilingSection { title: "Part II — Item 1A. Risk Factors".into(), text: self.risk_factors(q.fy) });
    }

    fn current(&self, out: &mut Vec<FilingSection>, items: &[String]) {
        let name = &self.s.inst.name;
        if items.iter().any(|i| i == "2.02") {
            if let Some(q) = self.quarter {
                let p = Period::single(q, PeriodType::Quarterly, format!("Q{}", q.fq));
                out.push(FilingSection {
                    title: "Item 2.02. Results of Operations and Financial Condition".into(),
                    text: format!(
                        "On {}, {name} issued a press release announcing its financial results for the {} quarter of fiscal {}. A copy is furnished as Exhibit 99.1.",
                        self.filing.filed.format("%B %-d, %Y"),
                        ordinal(q.fq),
                        q.fy
                    ),
                });
                out.push(FilingSection {
                    title: "Item 9.01. Financial Statements and Exhibits — Exhibit 99.1 (summary)".into(),
                    text: format!(
                        "{name} reports {} quarter fiscal {} results (SYNTHETIC): revenue of {}, operating income of {}, net income of {}, and diluted EPS of ${:.2}.",
                        ordinal(q.fq),
                        q.fy,
                        millions(p.revenue),
                        millions(p.op),
                        millions(p.ni),
                        p.eps_diluted()
                    ),
                });
            }
            return;
        }
        let mut c = Cell::new(&[self.inner.seed, self.s.hash, tag("8k"), crate::daily::day_num(self.filing.filed)]);
        for item in items {
            let (title, text) = match item.as_str() {
                "5.02" => (
                    "Item 5.02. Departure of Directors or Certain Officers; Election of Directors; Appointment of Certain Officers",
                    format!("On {}, the Board of Directors of {name} appointed {} as a director, effective immediately (synthetic event).", self.filing.filed.format("%B %-d, %Y"), person(&mut c)),
                ),
                "5.07" => (
                    "Item 5.07. Submission of Matters to a Vote of Security Holders",
                    format!("At the annual meeting, shareholders elected all director nominees and approved, on an advisory basis, executive compensation with {:.1}% of votes cast in favor (synthetic).", c.range(80.0, 97.0)),
                ),
                "7.01" => (
                    "Item 7.01. Regulation FD Disclosure",
                    format!("{name} will present at an investor conference; the presentation materials are furnished as Exhibit 99.1 (synthetic)."),
                ),
                _ => (
                    "Item 8.01. Other Events",
                    format!("{name} announced that its Board authorized an additional {} share repurchase program (synthetic).", millions((c.range(1.0, 20.0)).round() * 1e9)),
                ),
            };
            out.push(FilingSection { title: title.into(), text });
        }
    }

    fn proxy(&self, out: &mut Vec<FilingSection>) {
        let name = &self.s.inst.name;
        let mut c = Cell::new(&[self.inner.seed, self.s.hash, tag("proxy"), crate::daily::day_num(self.filing.filed)]);
        let meeting = Calendar::Nyse.on_or_after(self.filing.filed + Duration::days(c.int(35, 50)));
        out.push(FilingSection {
            title: "Notice of Annual Meeting of Shareholders".into(),
            text: format!("The annual meeting of shareholders of {name} will be held virtually on {} (synthetic).", meeting.format("%B %-d, %Y")),
        });
        let directors: Vec<String> = (0..c.int(8, 12)).map(|_| person(&mut c)).collect();
        out.push(FilingSection {
            title: "Proposal 1 — Election of Directors".into(),
            text: format!("The Board recommends a vote FOR each of the following synthetic nominees: {}.", directors.join(", ")),
        });
        out.push(FilingSection {
            title: "Proposal 2 — Ratification of Independent Registered Public Accounting Firm".into(),
            text: "The Board recommends a vote FOR ratification of the independent auditor for the coming fiscal year (synthetic).".into(),
        });
        out.push(FilingSection {
            title: "Proposal 3 — Advisory Vote to Approve Executive Compensation".into(),
            text: format!(
                "Total compensation of the Chief Executive Officer for the last fiscal year was ${} (synthetic), of which {:.0}% was performance-based.",
                commas((c.range(8.0, 45.0) * 1e6 / 1000.0).round() * 1000.0),
                c.range(80.0, 95.0)
            ),
        });
    }

    fn form4(&self, out: &mut Vec<FilingSection>) {
        let mut c = Cell::new(&[self.inner.seed, self.s.hash, tag("form4"), crate::daily::day_num(self.filing.filed), fnv1a(self.filing.accession.as_bytes())]);
        let who = person(&mut c);
        let role = *c.pick(&["Director", "Chief Financial Officer", "Chief Executive Officer", "General Counsel", "Chief Operating Officer"]);
        let price = self.inner.last_price(self.s, self.inner.clock.now()).unwrap_or(self.s.p.ref_price);
        let shares = (c.range(500.0, 40_000.0)).round();
        let code = if c.chance(0.75) { "S" } else { "M" };
        let px = (price * c.range(0.97, 1.03) * 100.0).round() / 100.0;
        out.push(FilingSection {
            title: "Table I — Non-Derivative Securities Acquired, Disposed of, or Beneficially Owned".into(),
            text: format!(
                "Reporting person: {who} ({role}) — synthetic\nTransaction date: {}\nTransaction code: {code}\nShares: {}\nPrice: ${px:.2}\nShares owned following transaction: {}",
                self.filing.period_of_report.unwrap_or(self.filing.filed),
                commas(shares),
                commas((shares * c.range(2.0, 30.0)).round())
            ),
        });
    }
}

pub(crate) fn ordinal(q: u32) -> &'static str {
    match q {
        1 => "first",
        2 => "second",
        3 => "third",
        _ => "fourth",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_formatting() {
        assert_eq!(commas(391_035.0), "391,035");
        assert_eq!(commas(-1_234_567.0), "(1,234,567)");
        assert_eq!(commas(12.0), "12");
        assert_eq!(millions(391_035e6), "$391,035 million");
    }
}

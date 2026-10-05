//! News, filings, transcripts.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::key::SecurityKey;
use crate::provenance::Provenance;
use crate::time::UnixNanos;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewsItem {
    /// Provider-scoped unique ID.
    pub id: String,
    /// Publisher / wire, e.g. `GlobeNewswire`.
    pub source: String,
    pub headline: String,
    pub summary: Option<String>,
    /// Full text, only when the provider licenses it.
    pub body: Option<String>,
    pub url: Option<String>,
    pub published_at: UnixNanos,
    pub received_at: UnixNanos,
    pub tickers: Vec<String>,
    pub topics: Vec<String>,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewsPage {
    pub items: Vec<NewsItem>,
    /// Opaque cursor for the next page, if any.
    pub next: Option<String>,
}

/// SEC filing metadata (CF).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Filing {
    pub cik: u64,
    pub company: String,
    /// e.g. `0000320193-25-000079`.
    pub accession: String,
    pub form: String,
    pub filed: NaiveDate,
    pub accepted_at: Option<UnixNanos>,
    pub period_of_report: Option<NaiveDate>,
    pub primary_document: Option<String>,
    pub primary_doc_url: Option<String>,
    pub description: Option<String>,
    pub size_bytes: Option<u64>,
    pub items: Vec<String>,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FilingsPage {
    pub key: Option<SecurityKey>,
    pub filings: Vec<Filing>,
}

/// One section of a filing document (e.g. `Item 1A. Risk Factors`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FilingSection {
    pub title: String,
    pub text: String,
}

/// Plain-text rendering of a filing's primary document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FilingDocument {
    pub filing: Filing,
    pub sections: Vec<FilingSection>,
}

impl FilingDocument {
    #[must_use]
    pub fn full_text(&self) -> String {
        let mut out = String::new();
        for s in &self.sections {
            out.push_str(&s.title);
            out.push('\n');
            out.push_str(&s.text);
            out.push_str("\n\n");
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub speaker: String,
    pub role: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    pub key: SecurityKey,
    pub fiscal_label: String,
    pub date: NaiveDate,
    pub segments: Vec<TranscriptSegment>,
    pub provenance: Provenance,
}

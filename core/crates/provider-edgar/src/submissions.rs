//! `https://data.sec.gov/submissions/CIK##########.json`: filer metadata and
//! filing history.
//!
//! `filings.recent` holds parallel arrays (one array per field, one index per
//! filing) covering at least one year or the 1,000 most recent filings;
//! `filings.files` names older pages, which use the same columnar layout at
//! their top level.

use chrono::{DateTime, NaiveDate, Utc};
use meridian_provider::{ProviderError, ProviderResult};
use meridian_types::{CompanyProfile, Filing, Provenance, UnixNanos, datetime_to_nanos};
use serde::{Deserialize, Deserializer};

use crate::tickers::de_cik;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SubmissionsDto {
    #[serde(deserialize_with = "de_cik")]
    pub cik: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "de_opt_text")]
    sic: Option<String>,
    #[serde(default)]
    sic_description: Option<String>,
    #[serde(default)]
    website: Option<String>,
    #[serde(default)]
    fiscal_year_end: Option<String>,
    #[serde(default)]
    addresses: Option<AddressesDto>,
    #[serde(default)]
    filings: Option<FilingsDto>,
}

#[derive(Debug, Deserialize)]
struct AddressesDto {
    #[serde(default)]
    business: Option<AddressDto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddressDto {
    #[serde(default)]
    city: Option<String>,
    #[serde(default)]
    state_or_country: Option<String>,
    #[serde(default)]
    state_or_country_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FilingsDto {
    #[serde(default)]
    recent: Option<FilingColumnsDto>,
    #[serde(default)]
    files: Vec<FileRefDto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileRefDto {
    name: String,
    #[serde(default)]
    filing_to: Option<String>,
}

/// Columnar filing list: `filings.recent`, or the top level of an older page.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FilingColumnsDto {
    accession_number: Vec<String>,
    filing_date: Vec<String>,
    form: Vec<String>,
    #[serde(default)]
    report_date: Vec<Option<String>>,
    #[serde(default)]
    acceptance_date_time: Vec<Option<String>>,
    #[serde(default)]
    items: Vec<Option<String>>,
    #[serde(default)]
    size: Vec<Option<u64>>,
    #[serde(default)]
    primary_document: Vec<Option<String>>,
    #[serde(default)]
    primary_doc_description: Vec<Option<String>>,
}

/// Accepts a string, a number, or null.
fn de_opt_text<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Text {
        Str(String),
        Num(serde_json::Number),
    }
    Ok(match Option::<Text>::deserialize(d)? {
        Some(Text::Str(s)) => Some(s),
        Some(Text::Num(n)) => Some(n.to_string()),
        None => None,
    })
}

fn non_empty(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

impl SubmissionsDto {
    pub(crate) fn company_name(&self) -> String {
        non_empty(self.name.as_deref()).unwrap_or_default()
    }

    pub(crate) fn recent(&self) -> Option<&FilingColumnsDto> {
        self.filings.as_ref().and_then(|f| f.recent.as_ref())
    }

    /// Older filing pages, newest first. Names that aren't plain file names
    /// are skipped so they can't alter the request path.
    pub(crate) fn older_pages(&self) -> Vec<String> {
        let Some(f) = &self.filings else {
            return Vec::new();
        };
        let mut files: Vec<&FileRefDto> = f
            .files
            .iter()
            .filter(|r| {
                !r.name.is_empty()
                    && !r.name.starts_with('.')
                    && r.name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            })
            .collect();
        files.sort_by(|a, b| b.filing_to.cmp(&a.filing_to));
        files.into_iter().map(|r| r.name.clone()).collect()
    }

    /// `fiscalYearEnd` (`"MMDD"`) as (month, day).
    pub(crate) fn fiscal_year_end_md(&self) -> Option<(u32, u32)> {
        let s = self.fiscal_year_end.as_deref()?.trim();
        if s.len() != 4 || !s.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let m: u32 = s[..2].parse().ok()?;
        let d: u32 = s[2..].parse().ok()?;
        ((1..=12).contains(&m) && (1..=31).contains(&d)).then_some((m, d))
    }

    /// DES fields the submissions API carries. The SEC publishes no business
    /// description, CEO, employee count, or IPO date, so those stay `None`.
    pub(crate) fn to_profile(&self) -> CompanyProfile {
        let business = self.addresses.as_ref().and_then(|a| a.business.as_ref());
        let headquarters = business.and_then(|b| {
            let city = non_empty(b.city.as_deref());
            let region = non_empty(b.state_or_country_description.as_deref())
                .or_else(|| non_empty(b.state_or_country.as_deref()));
            match (city, region) {
                (Some(c), Some(r)) => Some(format!("{c}, {r}")),
                (Some(c), None) => Some(c),
                (None, Some(r)) => Some(r),
                (None, None) => None,
            }
        });
        CompanyProfile {
            website: non_empty(self.website.as_deref()),
            headquarters,
            fiscal_year_end: self
                .fiscal_year_end_md()
                .map(|(m, d)| format!("{m:02}-{d:02}")),
            sic_code: non_empty(self.sic.as_deref()),
            sic_description: non_empty(self.sic_description.as_deref()),
            ..CompanyProfile::default()
        }
    }
}

/// `https://www.sec.gov/Archives/edgar/data/<cik>/<accession-no-dashes>/<doc>`.
pub(crate) fn primary_doc_url(www_base: &str, cik: u64, accession: &str, doc: &str) -> String {
    let acc: String = accession.chars().filter(|c| *c != '-').collect();
    format!("{www_base}/Archives/edgar/data/{cik}/{acc}/{doc}")
}

fn parse_date(s: &str, what: &str) -> ProviderResult<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
        .map_err(|_| ProviderError::parse(format!("submissions: invalid {what} {s:?}")))
}

fn parse_opt_date(s: Option<&str>, what: &str) -> ProviderResult<Option<NaiveDate>> {
    match s.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => parse_date(s, what).map(Some),
        None => Ok(None),
    }
}

/// `acceptanceDateTime`, e.g. `2025-10-31T06:01:26.000Z`, read as written
/// (UTC per its `Z` suffix; see the README for the reported caveat).
fn parse_accepted(s: Option<&str>) -> ProviderResult<Option<UnixNanos>> {
    match s.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => DateTime::parse_from_rfc3339(s)
            .map(|dt| Some(datetime_to_nanos(dt.with_timezone(&Utc))))
            .map_err(|_| {
                ProviderError::parse(format!("submissions: invalid acceptanceDateTime {s:?}"))
            }),
        None => Ok(None),
    }
}

/// Element `i` of an optional column; the column must be empty (absent) or
/// as long as the accession column.
fn col<T: Clone>(v: &[Option<T>], i: usize) -> Option<T> {
    v.get(i).cloned().flatten()
}

impl FilingColumnsDto {
    fn check_lengths(&self) -> ProviderResult<usize> {
        let n = self.accession_number.len();
        let required = [
            ("filingDate", self.filing_date.len()),
            ("form", self.form.len()),
        ];
        let optional = [
            ("reportDate", self.report_date.len()),
            ("acceptanceDateTime", self.acceptance_date_time.len()),
            ("items", self.items.len()),
            ("size", self.size.len()),
            ("primaryDocument", self.primary_document.len()),
            ("primaryDocDescription", self.primary_doc_description.len()),
        ];
        for (name, len) in required {
            if len != n {
                return Err(ProviderError::parse(format!(
                    "submissions: column {name} has {len} rows, expected {n}"
                )));
            }
        }
        for (name, len) in optional {
            if len != 0 && len != n {
                return Err(ProviderError::parse(format!(
                    "submissions: column {name} has {len} rows, expected {n}"
                )));
            }
        }
        Ok(n)
    }

    /// Normalizes every row into a [`Filing`]. `base` is cloned per filing
    /// with `source_ref` set to the accession number.
    pub(crate) fn to_filings(
        &self,
        cik: u64,
        company: &str,
        www_base: &str,
        base: &Provenance,
    ) -> ProviderResult<Vec<Filing>> {
        let n = self.check_lengths()?;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let accession = self.accession_number[i].trim().to_owned();
            let primary_document = non_empty(col(&self.primary_document, i).as_deref());
            let primary_doc_url = primary_document
                .as_deref()
                .map(|d| primary_doc_url(www_base, cik, &accession, d));
            let items = col(&self.items, i)
                .map(|s| {
                    s.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            out.push(Filing {
                cik,
                company: company.to_owned(),
                form: self.form[i].trim().to_owned(),
                filed: parse_date(&self.filing_date[i], "filingDate")?,
                accepted_at: parse_accepted(col(&self.acceptance_date_time, i).as_deref())?,
                period_of_report: parse_opt_date(
                    col(&self.report_date, i).as_deref(),
                    "reportDate",
                )?,
                primary_document,
                primary_doc_url,
                description: non_empty(col(&self.primary_doc_description, i).as_deref()),
                size_bytes: col(&self.size, i),
                items,
                provenance: base.clone().with_ref(accession.clone()),
                accession,
            });
        }
        Ok(out)
    }
}

/// Keeps filings whose form equals one of `forms` (case-insensitive, exact:
/// `10-K` does not include `10-K/A`), or all filings when `forms` is empty.
pub(crate) fn matches_forms(filing: &Filing, forms: &[String]) -> bool {
    forms.is_empty()
        || forms
            .iter()
            .any(|f| f.trim().eq_ignore_ascii_case(&filing.form))
}

/// Newest first: filing date, then acceptance time, then accession.
pub(crate) fn sort_newest_first(filings: &mut [Filing]) {
    filings.sort_by(|a, b| {
        b.filed
            .cmp(&a.filed)
            .then_with(|| b.accepted_at.cmp(&a.accepted_at))
            .then_with(|| b.accession.cmp(&a.accession))
    });
}

#[cfg(test)]
mod tests {
    use meridian_types::{DataDelay, FeedSource, ProviderId};

    use super::*;

    const WWW: &str = "https://www.sec.gov";

    fn base() -> Provenance {
        Provenance {
            provider: ProviderId::new("edgar"),
            synthetic: false,
            delay: DataDelay::RealTime,
            source: FeedSource::Official,
            as_of: 1,
            source_ref: None,
            attribution: None,
        }
    }

    fn fixture() -> SubmissionsDto {
        crate::http::parse_json(
            include_str!("../tests/fixtures/submissions_CIK0000000001.json"),
            "submissions",
        )
        .unwrap()
    }

    fn filings() -> Vec<Filing> {
        let s = fixture();
        s.recent()
            .unwrap()
            .to_filings(s.cik, &s.company_name(), WWW, &base())
            .unwrap()
    }

    #[test]
    fn maps_profile_fields() {
        let s = fixture();
        assert_eq!(s.cik, 1);
        assert_eq!(s.company_name(), "Example Corp");
        let p = s.to_profile();
        assert_eq!(p.headquarters.as_deref(), Some("ANYTOWN, CA"));
        assert_eq!(p.fiscal_year_end.as_deref(), Some("09-30"));
        assert_eq!(p.sic_code.as_deref(), Some("7372"));
        assert_eq!(
            p.sic_description.as_deref(),
            Some("Services-Prepackaged Software")
        );
        // Empty in the payload → None; never filled in.
        assert_eq!(p.website, None);
        assert_eq!(p.description, None);
        assert_eq!(p.employees, None);
        assert_eq!(s.fiscal_year_end_md(), Some((9, 30)));
    }

    #[test]
    fn builds_filings_from_parallel_arrays() {
        let f = filings();
        assert_eq!(f.len(), 7);
        let k = f
            .iter()
            .find(|f| f.accession == "0000000001-25-000010")
            .unwrap();
        assert_eq!(k.form, "10-K");
        assert_eq!(k.filed, NaiveDate::from_ymd_opt(2025, 11, 14).unwrap());
        assert_eq!(k.period_of_report, NaiveDate::from_ymd_opt(2025, 9, 30));
        assert_eq!(
            k.primary_doc_url.as_deref(),
            Some("https://www.sec.gov/Archives/edgar/data/1/000000000125000010/exmp-20250930.htm")
        );
        assert_eq!(k.description.as_deref(), Some("10-K"));
        assert_eq!(k.size_bytes, Some(123_456));
        let accepted = DateTime::parse_from_rfc3339("2025-11-14T16:05:00Z").unwrap();
        assert_eq!(
            k.accepted_at,
            Some(datetime_to_nanos(accepted.with_timezone(&Utc)))
        );
        assert_eq!(
            k.provenance.source_ref.as_deref(),
            Some("0000000001-25-000010")
        );
        assert!(k.items.is_empty());

        let eight_k = f.iter().find(|f| f.form == "8-K").unwrap();
        assert_eq!(eight_k.items, vec!["2.02", "9.01"]);
        assert_eq!(
            eight_k.period_of_report,
            NaiveDate::from_ymd_opt(2025, 11, 13)
        );

        // Missing report date and primary document stay None.
        let form4 = f.iter().find(|f| f.form == "4").unwrap();
        assert_eq!(form4.period_of_report, None);
        assert_eq!(form4.description, None);
    }

    #[test]
    fn filters_forms_exactly_and_sorts_newest_first() {
        let mut f: Vec<Filing> = filings()
            .into_iter()
            .filter(|f| matches_forms(f, &["10-k".into()]))
            .collect();
        sort_newest_first(&mut f);
        let acc: Vec<&str> = f.iter().map(|f| f.accession.as_str()).collect();
        // 10-K/A is not included by a 10-K filter.
        assert_eq!(acc, vec!["0000000001-25-000010", "0000000001-24-000010"]);

        let mut all = filings();
        sort_newest_first(&mut all);
        assert_eq!(all[0].form, "10-Q");
        assert_eq!(all[0].filed, NaiveDate::from_ymd_opt(2026, 2, 5).unwrap());
        assert!(all.windows(2).all(|w| w[0].filed >= w[1].filed));

        let amend: Vec<Filing> = filings()
            .into_iter()
            .filter(|f| matches_forms(f, &["10-K/A".into()]))
            .collect();
        assert_eq!(amend.len(), 1);
    }

    #[test]
    fn lists_older_pages_newest_first() {
        let s = fixture();
        assert_eq!(s.older_pages(), vec!["CIK0000000001-submissions-001.json"]);
    }

    #[test]
    fn rejects_ragged_columns() {
        let cols: FilingColumnsDto = serde_json::from_str(
            r#"{"accessionNumber":["a","b"],"filingDate":["2025-01-01"],"form":["8-K","8-K"]}"#,
        )
        .unwrap();
        let err = cols.to_filings(1, "X", WWW, &base()).unwrap_err();
        assert!(matches!(err, ProviderError::Parse { .. }));
    }

    #[test]
    fn builds_archive_url_without_dashes_or_padding() {
        assert_eq!(
            primary_doc_url(WWW, 320_193, "0000320193-25-000079", "aapl-20250927.htm"),
            "https://www.sec.gov/Archives/edgar/data/320193/000032019325000079/aapl-20250927.htm"
        );
    }
}

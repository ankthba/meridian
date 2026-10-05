//! Live tests against SEC EDGAR. Ignored by default and skipped unless
//! `MERIDIAN_SEC_CONTACT` holds your own name and email, because the SEC's
//! fair-access policy requires a real contact in the User-Agent:
//!
//! ```text
//! MERIDIAN_SEC_CONTACT="Jane Doe jane@example.com" \
//!   cargo test --manifest-path core/Cargo.toml -p meridian-provider-edgar --test live -- --ignored
//! ```

use meridian_provider::{FilingsRequest, FundamentalsRequest, Provider};
use meridian_provider_edgar::{EdgarConfig, EdgarProvider};
use meridian_types::{PeriodType, SecurityKey};

fn provider() -> Option<EdgarProvider> {
    let contact = std::env::var("MERIDIAN_SEC_CONTACT")
        .ok()
        .filter(|c| !c.trim().is_empty())?;
    let p = EdgarProvider::new(EdgarConfig { contact }).ok()?;
    p.is_configured().then_some(p)
}

#[tokio::test]
#[ignore = "live: calls SEC EDGAR; needs MERIDIAN_SEC_CONTACT"]
async fn live_profile_filings_and_document() {
    let Some(p) = provider() else {
        eprintln!("MERIDIAN_SEC_CONTACT not set; skipping");
        return;
    };
    let key = SecurityKey::equity("AAPL");
    let inst = p.instrument(&key).await.expect("instrument");
    assert_eq!(inst.cik, Some(320_193));
    let profile = p.profile(&key).await.expect("profile");
    assert!(profile.sic_code.is_some());
    let page = p
        .filings(&FilingsRequest {
            key: Some(key),
            forms: vec!["10-K".into()],
            limit: 2,
        })
        .await
        .expect("filings");
    assert!(!page.filings.is_empty());
    assert!(page.filings.iter().all(|f| f.form == "10-K"));
    let doc = p.filing_document(&page.filings[0]).await.expect("document");
    assert!(doc.sections.iter().any(|s| s.title.starts_with("Item 1A")));
}

#[tokio::test]
#[ignore = "live: calls SEC EDGAR; needs MERIDIAN_SEC_CONTACT"]
async fn live_fundamentals() {
    let Some(p) = provider() else {
        eprintln!("MERIDIAN_SEC_CONTACT not set; skipping");
        return;
    };
    for period_type in [PeriodType::Annual, PeriodType::Quarterly, PeriodType::Ttm] {
        let req = FundamentalsRequest {
            key: SecurityKey::equity("AAPL"),
            period_type,
            periods: 4,
        };
        let f = p.fundamentals(&req).await.expect("fundamentals");
        assert!(!f.statements.is_empty());
        assert!(f.statements.iter().any(|s| s.value("revenue").is_some()));
    }
}

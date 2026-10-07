//! Live smoke tests against Finnhub. Ignored by default and never run in
//! CI: they need a real key, which lives in the Keychain, not here.
//!
//! To run by hand (the key only in the shell's environment, never in a
//! file):
//!
//! ```text
//! FINNHUB_API_KEY=… cargo test -p meridian-provider-finnhub --test live -- --ignored --nocapture
//! ```
//!
//! Each test skips (passes without doing anything) when the variable is
//! unset.

use chrono::{Days, Utc};
use meridian_provider::{EventCalendarRequest, Provider};
use meridian_provider_finnhub::{FinnhubConfig, FinnhubProvider};
use meridian_types::SecurityKey;
use secrecy::SecretString;

fn live_provider() -> Option<FinnhubProvider> {
    let key = std::env::var("FINNHUB_API_KEY").ok().filter(|k| !k.trim().is_empty())?;
    Some(FinnhubProvider::new(FinnhubConfig { api_key: Some(SecretString::from(key)) }).expect("client"))
}

/// The next two weeks for the whole market: the free tier answers, rows are
/// in the window and well formed. Prints the row count so a cap on window
/// responses (undocumented) would show up.
#[tokio::test]
#[ignore = "needs FINNHUB_API_KEY and network"]
async fn live_earnings_calendar_market_window() {
    let Some(p) = live_provider() else { return };
    let today = Utc::now().date_naive();
    let req = EventCalendarRequest { from: today, to: today.checked_add_days(Days::new(14)).unwrap(), keys: vec![] };
    let cal = p.earnings_calendar(&req).await.unwrap();
    println!("{} releases in the next 14 days; source {:?}", cal.events.len(), cal.provenance.source_ref);
    assert!(!cal.events.is_empty(), "expected some US earnings releases in two weeks");
    for e in &cal.events {
        assert!(e.date >= req.from && e.date <= req.to, "{e:?}");
    }
    let with_session = cal.events.iter().filter(|e| e.session.is_some()).count();
    let with_estimate = cal.events.iter().filter(|e| e.eps_estimate.is_some()).count();
    println!("{with_session} with a session, {with_estimate} with an EPS estimate");
}

/// A per-symbol request agrees with the window request for the same
/// symbols (checks the client-side filter against Finnhub's own).
#[tokio::test]
#[ignore = "needs FINNHUB_API_KEY and network"]
async fn live_earnings_calendar_per_symbol_matches_window() {
    let Some(p) = live_provider() else { return };
    let today = Utc::now().date_naive();
    let to = today.checked_add_days(Days::new(30)).unwrap();
    let wide = p.earnings_calendar(&EventCalendarRequest { from: today, to, keys: vec![] }).await.unwrap();
    let picked: Vec<SecurityKey> = wide.events.iter().take(2).map(|e| e.key.clone()).collect();
    if picked.is_empty() {
        return;
    }
    let narrow = p.earnings_calendar(&EventCalendarRequest { from: today, to, keys: picked.clone() }).await.unwrap();
    for k in &picked {
        let a: Vec<_> = wide.events.iter().filter(|e| &e.key == k).map(|e| e.date).collect();
        let b: Vec<_> = narrow.events.iter().filter(|e| &e.key == k).map(|e| e.date).collect();
        assert_eq!(a, b, "{k}");
    }
}

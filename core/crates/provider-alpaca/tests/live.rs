//! Live smoke tests against Alpaca. Ignored by default and never run in CI:
//! they need real keys, which live in the Keychain, not here.
//!
//! To run by hand (keys only in the shell's environment, never in a file):
//!
//! ```text
//! ALPACA_KEY_ID=… ALPACA_SECRET_KEY=… cargo test -p meridian-provider-alpaca --test live -- --ignored
//! ```
//!
//! Set `ALPACA_FEED=sip` if the keys have Algo Trader Plus. Each test skips
//! (passes without doing anything) when the variables are unset.

use meridian_provider::{BarsRequest, ChainRequest, NewsQuery, NewsScope, Provider};
use meridian_provider_alpaca::{AlpacaConfig, AlpacaFeed, AlpacaProvider};
use meridian_types::{Adjustment, BarInterval, SecurityKey};
use secrecy::SecretString;

fn live_provider() -> Option<AlpacaProvider> {
    let key_id = std::env::var("ALPACA_KEY_ID").ok()?;
    let secret_key = std::env::var("ALPACA_SECRET_KEY").ok()?;
    let feed = match std::env::var("ALPACA_FEED").as_deref() {
        Ok("sip") => AlpacaFeed::Sip,
        _ => AlpacaFeed::Iex,
    };
    let config = AlpacaConfig {
        key_id: Some(SecretString::from(key_id)),
        secret_key: Some(SecretString::from(secret_key)),
        feed,
    };
    Some(AlpacaProvider::new(config).expect("client"))
}

#[tokio::test]
#[ignore = "needs ALPACA_KEY_ID / ALPACA_SECRET_KEY and network"]
async fn live_quotes() {
    let Some(p) = live_provider() else { return };
    let quotes = p.quotes(&[SecurityKey::equity("AAPL"), SecurityKey::equity("BRK/B")]).await.unwrap();
    assert!(!quotes.is_empty());
    for q in &quotes {
        assert!(q.last.is_some(), "{q:?}");
    }
}

#[tokio::test]
#[ignore = "needs ALPACA_KEY_ID / ALPACA_SECRET_KEY and network"]
async fn live_daily_bars() {
    let Some(p) = live_provider() else { return };
    let req = BarsRequest {
        key: SecurityKey::equity("AAPL"),
        interval: BarInterval::Day,
        from: None,
        to: None,
        adjustment: Adjustment::SplitsAndDividends,
    };
    let series = p.bars(&req).await.unwrap();
    assert!(series.len() > 2000, "expected about ten years of daily bars, got {}", series.len());
    series.validate().unwrap();
}

#[tokio::test]
#[ignore = "needs ALPACA_KEY_ID / ALPACA_SECRET_KEY and network"]
async fn live_option_chain() {
    let Some(p) = live_provider() else { return };
    let chain = p.option_chain(&ChainRequest { underlying: SecurityKey::equity("SPY"), expiry: None }).await.unwrap();
    assert!(!chain.contracts.is_empty());
}

#[tokio::test]
#[ignore = "needs ALPACA_KEY_ID / ALPACA_SECRET_KEY and network"]
async fn live_news() {
    let Some(p) = live_provider() else { return };
    let q = NewsQuery {
        scope: NewsScope::Company,
        keys: vec![SecurityKey::equity("AAPL")],
        text: None,
        from: None,
        to: None,
        limit: 5,
    };
    let page = p.news(&q).await.unwrap();
    assert!(page.items.len() <= 5);
}

/// Confirms the corporate actions endpoint works on the keys' plan (the docs
/// name no plan requirement) and that a future `end` date is accepted.
#[tokio::test]
#[ignore = "needs ALPACA_KEY_ID / ALPACA_SECRET_KEY and network"]
async fn live_corporate_actions() {
    let Some(p) = live_provider() else { return };
    // Coca-Cola pays quarterly dividends. How far back Alpaca's history goes
    // isn't documented, so only a few years are required.
    let d = p.dividends(&SecurityKey::equity("KO")).await.unwrap();
    let cash = d.dividends.iter().filter(|x| x.kind != meridian_types::DividendKind::Split).count();
    assert!(cash >= 12, "expected several years of quarterly dividends, got {cash}");
    assert!(d.dividends.windows(2).all(|w| w[0].ex_date >= w[1].ex_date));
}

/// The multi-symbol calendar request: the last 120 days for a few regular
/// payers should hold each one's latest quarterly ex-date, and the same
/// dates as the per-symbol path.
#[tokio::test]
#[ignore = "needs ALPACA_KEY_ID / ALPACA_SECRET_KEY and network"]
async fn live_dividend_calendar() {
    use meridian_provider::EventCalendarRequest;

    let Some(p) = live_provider() else { return };
    let today = chrono::Utc::now().date_naive();
    let keys: Vec<SecurityKey> = ["KO", "PG", "JNJ"].iter().map(|s| SecurityKey::equity(s)).collect();
    let req = EventCalendarRequest { from: today - chrono::Duration::days(120), to: today, keys: keys.clone() };
    let cal = p.dividend_calendar(&req).await.unwrap();
    for k in &keys {
        let mine: Vec<_> = cal.events.iter().filter(|e| &e.key == k).map(|e| e.dividend.ex_date).collect();
        assert!(!mine.is_empty(), "{k}: no ex-date in the last 120 days");
        let single = p.dividends(k).await.unwrap();
        let mut want: Vec<_> =
            single.dividends.iter().filter(|d| d.ex_date >= req.from && d.ex_date <= req.to).map(|d| d.ex_date).collect();
        want.sort();
        assert_eq!(mine, want, "{k}");
    }
}

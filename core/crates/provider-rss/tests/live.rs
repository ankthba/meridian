//! Live checks against the real feeds. Opt-in:
//!
//! ```text
//! cargo test --manifest-path core/Cargo.toml -p meridian-provider-rss -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;

use chrono::Utc;
use meridian_provider::{NewsQuery, NewsScope, Provider};
use meridian_provider_rss::{RssConfig, RssProvider};
use meridian_types::{NANOS_PER_DAY, SecurityKey, datetime_to_nanos};

fn query(scope: NewsScope, keys: Vec<SecurityKey>) -> NewsQuery {
    NewsQuery {
        scope,
        keys,
        text: None,
        from: None,
        to: None,
        limit: 100_000,
    }
}

#[tokio::test]
#[ignore = "live network: fetches each default feed"]
async fn each_default_feed_parses() {
    let now = datetime_to_nanos(Utc::now());
    let mut failures = Vec::new();
    for feed in RssConfig::default().feeds {
        let provider = RssProvider::new(RssConfig {
            feeds: vec![feed.clone()],
        })
        .expect("valid default feed");
        match provider
            .news(&query(NewsScope::PressReleases, vec![]))
            .await
        {
            Ok(page) => {
                let recent = page
                    .items
                    .iter()
                    .filter(|i| {
                        !i.headline.trim().is_empty() && i.published_at > now - 30 * NANOS_PER_DAY
                    })
                    .count();
                let with_tickers = page.items.iter().filter(|i| !i.tickers.is_empty()).count();
                let sample = page.items.first().map_or("-", |i| i.headline.as_str());
                println!(
                    "{} <{}>: {} items, {recent} recent with headline, {with_tickers} with tickers; newest: {sample:?}",
                    feed.name,
                    feed.url,
                    page.items.len()
                );
                if recent == 0 {
                    failures.push(format!("{}: no recent item with a headline", feed.url));
                }
            }
            Err(e) => {
                println!("{} <{}>: FAILED: {e}", feed.name, feed.url);
                failures.push(format!("{}: {e}", feed.url));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
#[ignore = "live network: fetches all default feeds"]
async fn press_releases_and_company_news() {
    let provider = RssProvider::new(RssConfig::default()).expect("default config");
    let page = provider
        .news(&query(NewsScope::PressReleases, vec![]))
        .await
        .expect("press releases");
    assert!(!page.items.is_empty());
    assert!(
        page.items
            .windows(2)
            .all(|w| w[0].published_at >= w[1].published_at)
    );

    let mut per_source: BTreeMap<&str, usize> = BTreeMap::new();
    for item in &page.items {
        *per_source.entry(item.source.as_str()).or_default() += 1;
    }
    println!("press releases: {} items {per_source:?}", page.items.len());
    for item in page.items.iter().take(5) {
        println!("  {} | {} | {:?}", item.source, item.headline, item.tickers);
    }

    // Company scope round trip: a US ticker seen in the feeds must find its item.
    let Some(symbol) = page
        .items
        .iter()
        .flat_map(|i| &i.tickers)
        .find(|t| !t.contains(':'))
        .cloned()
    else {
        println!("no US ticker in the current feed window; company check skipped");
        return;
    };
    let cn = provider
        .news(&query(
            NewsScope::Company,
            vec![SecurityKey::equity(&symbol)],
        ))
        .await
        .expect("company news");
    println!(
        "company news for {symbol}: {} items, first: {:?}",
        cn.items.len(),
        cn.items.first().map(|i| &i.headline)
    );
    assert!(!cn.items.is_empty());
    assert!(cn.items.iter().all(|i| i.tickers.contains(&symbol)));
}

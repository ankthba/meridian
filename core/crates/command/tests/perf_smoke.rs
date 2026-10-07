//! Lenient timing smoke test for autocomplete over 20,000 instruments.
//!
//! The real budgets (< 2 ms per call for mnemonic queries, ARCHITECTURE §12;
//! < 5 ms for plain-language ones) are measured by
//! `cargo bench -p meridian-command --bench suggest`. This test only catches
//! gross regressions, so it allows 20 ms in an unoptimized build; built with
//! optimizations (`cargo test --release`) it holds the 5 ms budget.

mod support;

use std::time::{Duration, Instant};

use meridian_command::{ParseContext, SuggestIndex};

/// 20 ms unoptimized; the 5 ms budget when optimized.
const LIMIT: Duration = if cfg!(debug_assertions) {
    Duration::from_millis(20)
} else {
    Duration::from_millis(5)
};

#[test]
fn suggest_is_fast_on_20k_instruments() {
    let index = SuggestIndex::new(support::universe(20_000));
    assert!(index.len() >= 20_000);
    let ctx = ParseContext {
        loaded: Some(meridian_types::SecurityKey::equity("AAPL")),
    };

    for query in support::QUERIES.iter().chain(&support::PLAIN_QUERIES) {
        // Best of three, to ride out scheduler noise.
        let best = (0..3)
            .map(|_| {
                let start = Instant::now();
                let out = index.suggest(query, &ctx, 12);
                let elapsed = start.elapsed();
                assert!(out.len() <= 12);
                elapsed
            })
            .min()
            .unwrap();
        assert!(best < LIMIT, "{query:?} took {best:?}");
    }
}

#[test]
fn known_targets_rank_first_in_a_large_universe() {
    let index = SuggestIndex::new(support::universe(20_000));
    let ctx = ParseContext::default();
    let top = |q: &str| {
        index
            .suggest(q, &ctx, 12)
            .into_iter()
            .next()
            .unwrap()
            .display
    };
    assert_eq!(top("AAPL"), "AAPL US Equity");
    assert_eq!(top("apple"), "AAPL US Equity");
    assert_eq!(top("mazon"), "AMZN US Equity");
    assert_eq!(top("bank of"), "BAC US Equity");
    assert_eq!(top("AAPL US <EQUITY> G"), "GP");

    // The row Return runs, for plain queries.
    let best = |q: &str| {
        index
            .suggest(q, &ctx, 12)
            .into_iter()
            .find(|s| s.best)
            .unwrap()
            .title
    };
    assert_eq!(best("aapl fil"), "Filings");
    assert_eq!(best("aapl 5y"), "5-year chart");
    assert_eq!(best("earn"), "Earnings this week");
    assert_eq!(best("appl"), "AAPL");
    assert_eq!(best("btc"), "BTCUSD");
    assert_eq!(best("aapl vs micro"), "MSFT");
}

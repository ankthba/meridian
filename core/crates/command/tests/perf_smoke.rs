//! Lenient timing smoke test for autocomplete over 20,000 instruments.
//!
//! The real budget (< 2 ms per call, ARCHITECTURE §12) is measured by
//! `cargo bench -p meridian-command --bench suggest`. This test only catches
//! gross regressions, so it allows 20 ms even in an unoptimized build.

mod support;

use std::time::{Duration, Instant};

use meridian_command::{ParseContext, SuggestIndex};

#[test]
fn suggest_is_fast_on_20k_instruments() {
    let index = SuggestIndex::new(support::universe(20_000));
    assert!(index.len() >= 20_000);
    let ctx = ParseContext::default();

    for query in support::QUERIES {
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
        assert!(best < Duration::from_millis(20), "{query:?} took {best:?}");
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
}

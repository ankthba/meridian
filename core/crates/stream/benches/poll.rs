//! Hot-path benchmarks: applying updates and polling 2,000 instruments.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use meridian_stream::MarketState;
use meridian_types::QuoteUpdate;

fn bench(c: &mut Criterion) {
    let state = MarketState::new();
    let ids: Vec<u32> = (0..2_000).collect();
    for &id in &ids {
        state.apply(id, &QuoteUpdate { last: Some(100.0), bid: Some(99.9), ask: Some(100.1), ..Default::default() }, 0);
    }

    let mut px = 100.0;
    c.bench_function("apply_update", |b| {
        b.iter(|| {
            px += 0.01;
            state.apply(black_box(17), &QuoteUpdate { last: Some(px), ..Default::default() }, 0)
        });
    });

    // Typical frame: ~10% of 2,000 instruments changed since last poll.
    c.bench_function("poll_2000_with_200_changed", |b| {
        let mut out = Vec::with_capacity(2_000 * 128);
        b.iter_batched(
            || {
                let since = state.seq();
                for id in (0..2_000).step_by(10) {
                    px += 0.01;
                    state.apply(id, &QuoteUpdate { last: Some(px), ..Default::default() }, 0);
                }
                since
            },
            |since| {
                out.clear();
                black_box(state.poll_into(&ids, since, &mut out))
            },
            criterion::BatchSize::SmallInput,
        );
    });

    c.bench_function("poll_2000_full", |b| {
        let mut out = Vec::with_capacity(2_000 * 128);
        b.iter(|| {
            out.clear();
            black_box(state.poll_into(&ids, 0, &mut out))
        });
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);

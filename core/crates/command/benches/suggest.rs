//! Autocomplete latency over 20,000 instruments, `limit` 12.
//! Budget: < 2 ms per call for mnemonic queries (ARCHITECTURE §12) and
//! < 5 ms for plain-language ones.

#[path = "../tests/support/mod.rs"]
mod support;

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use meridian_command::{ParseContext, SuggestIndex, parse};

fn bench_suggest(c: &mut Criterion) {
    let index = SuggestIndex::new(support::universe(20_000));
    let ctx = ParseContext::default();

    let mut group = c.benchmark_group("suggest_20k");
    for query in support::QUERIES {
        group.bench_with_input(BenchmarkId::from_parameter(query), query, |b, q| {
            b.iter(|| index.suggest(black_box(q), &ctx, 12));
        });
    }
    group.finish();

    let mut group = c.benchmark_group("suggest_plain_20k");
    for query in support::PLAIN_QUERIES {
        group.bench_with_input(BenchmarkId::from_parameter(query.trim()), query, |b, q| {
            b.iter(|| index.suggest(black_box(q), &ctx, 12));
        });
    }
    group.finish();

    c.bench_function("interpret_plain", |b| {
        b.iter(|| index.interpret(black_box("aapl vs msft 5y"), &ctx));
    });

    c.bench_function("build_index_20k", |b| {
        b.iter_batched(
            || support::universe(20_000),
            SuggestIndex::new,
            criterion::BatchSize::LargeInput,
        );
    });

    c.bench_function("parse", |b| {
        b.iter(|| parse(black_box("AAPL US <EQUITY> GP 1Y"), &ctx));
    });
}

criterion_group!(benches, bench_suggest);
criterion_main!(benches);

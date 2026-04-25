//! Page-turn micro-benchmark.
//!
//! PR1 ships a no-op placeholder so the criterion harness compiles and CI
//! can lock in the `harness = false` wiring. Real benches arrive in PR4
//! once the layout engine and reader view exist.

use criterion::{Criterion, criterion_group, criterion_main};

fn page_turn_placeholder(c: &mut Criterion) {
    c.bench_function("page_turn_noop", |b| b.iter(|| std::hint::black_box(0u64)));
}

criterion_group!(benches, page_turn_placeholder);
criterion_main!(benches);

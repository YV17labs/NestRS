//! What a unit pays for its trace context: a trace id and a span id minted,
//! the correlation a request opens and the child a message opens, alone and
//! with every core of a worker minting at once.
//!
//! `cargo bench -p nest-rs-core --bench trace_context`; the protocol and how a
//! run is reported are `bench/RUNBOOK.md`'s.

use std::hint::black_box;
use std::thread;
use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use nest_rs_core::{Correlation, SpanId, TraceId};

fn minting(c: &mut Criterion) {
    let mut group = c.benchmark_group("mint");
    group.bench_function("trace_id", |b| b.iter(|| black_box(TraceId::mint())));
    group.bench_function("span_id", |b| b.iter(|| black_box(SpanId::mint())));
    group.bench_function("correlation", |b| {
        b.iter(|| black_box(Correlation::minted(None)));
    });
    let parent = Correlation::minted(None);
    group.bench_function("child", |b| b.iter(|| black_box(parent.child())));
    group.finish();
}

/// `iters` mints of `mint` shared by `threads` threads started together: the
/// wall time a worker's cores pay when every request mints at once.
fn shared<T>(threads: u64, iters: u64, mint: fn() -> T) -> Duration {
    let each = iters.div_ceil(threads);
    let started = Instant::now();
    thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(move || {
                for _ in 0..each {
                    black_box(mint());
                }
            });
        }
    });
    started.elapsed()
}

fn contended(c: &mut Criterion) {
    let cores = thread::available_parallelism().map_or(4, |cores| cores.get() as u64);
    let mut group = c.benchmark_group("mint_on_every_core");
    for threads in [1, cores] {
        group.bench_with_input(
            BenchmarkId::new("trace_id", threads),
            &threads,
            |b, &threads| b.iter_custom(|iters| shared(threads, iters, TraceId::mint)),
        );
        group.bench_with_input(
            BenchmarkId::new("span_id", threads),
            &threads,
            |b, &threads| b.iter_custom(|iters| shared(threads, iters, SpanId::mint)),
        );
    }
    group.finish();
}

criterion_group!(benches, minting, contended);
criterion_main!(benches);

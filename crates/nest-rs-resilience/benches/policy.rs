//! What a call pays its policy, measured with no dependency behind it: a call
//! answering at once through `run`, `run_idempotent` and a decorated method
//! run once and idempotent, on a policy with no optional mechanism and with
//! each one set — the breaker, the bulkhead, the rate; an answer its method's
//! keys judge against one the defaults judge; a call an open breaker refuses
//! unsent, with a subscription to the policy's events open and with none; a
//! decorated method called on one policy from a thread per core at once; and
//! the token an attempt is handed, alone.
//!
//! `cargo bench -p nest-rs-resilience --bench policy` prints criterion's
//! estimate of each row. A figure is its machine's and its run's: quoted, it
//! names both, and two commits are compared back to back on one machine. An
//! attempt inside a transaction opens a savepoint, which needs a database:
//! what it allocates is held by `tests/integration/policy.rs`.
#![expect(
    clippy::expect_used,
    reason = "a benchmark that cannot set itself up has nothing to measure; it stops there"
)]

use std::future::Future;
use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use nest_rs_core::{App, Instance, instance, module};
use nest_rs_resilience::{
    CancellationToken, ResilienceConfig, ResilienceError, ResilienceModule, ResiliencePolicy,
    resilient,
};

/// No optional mechanism: an attempt's timeout and the retries alone.
#[instance]
struct Bare;

/// The breaker alone, its window counted.
#[instance]
struct Breaker;

/// The bulkhead alone.
#[instance]
struct Bulkhead;

/// The rate alone.
#[instance]
struct Rate;

/// A breaker opened by hand: every call refused unsent.
#[instance]
struct Refusing;

fn bare() -> ResilienceConfig {
    ResilienceConfig {
        breaker_window: None,
        ..ResilienceConfig::default()
    }
}

/// A bucket the measured calls never empty: a billion tokens, one back each
/// nanosecond.
fn rate() -> ResilienceConfig {
    ResilienceConfig {
        rate_limit: Some(1_000_000_000),
        rate_window: Duration::from_secs(1),
        ..bare()
    }
}

#[module(imports = [
    ResilienceModule::for_root((Bare, bare())),
    ResilienceModule::for_root((Breaker, ResilienceConfig::default())),
    ResilienceModule::for_root((Bulkhead, ResilienceConfig { concurrency: Some(8), ..bare() })),
    ResilienceModule::for_root((Rate, rate())),
    ResilienceModule::for_root((Refusing, ResilienceConfig::default())),
])]
struct BenchModule;

#[derive(Debug, thiserror::Error)]
#[error("the dependency is down")]
struct Down;

impl Down {
    fn is_transient(&self) -> bool {
        false
    }
}

type Answer = Result<u64, ResilienceError<Down>>;

/// The dependency answering at once.
async fn answering() -> Result<u64, Down> {
    Ok(7)
}

/// The dependency failing at once.
async fn failing() -> Result<u64, Down> {
    Err(Down)
}

/// A host built by hand over the policy of the instance `I`.
struct Dependency<I: Instance> {
    policy: Arc<ResiliencePolicy<I>>,
}

impl<I: Instance> Dependency<I> {
    #[resilient(policy)]
    async fn once(&self) -> Answer {
        answering().await
    }

    #[resilient(policy, idempotent)]
    async fn again(&self) -> Answer {
        answering().await
    }

    /// An `Ok` its `retry_ok_if` reads.
    #[resilient(
        policy,
        idempotent,
        retry_if = Down::is_transient,
        retry_ok_if = |answer| *answer == 0,
        record_if = |error| !error.is_transient()
    )]
    async fn again_judged(&self) -> Answer {
        answering().await
    }

    /// An `Ok` under a fallback, which answers for none.
    #[resilient(policy, idempotent, fallback = stale)]
    async fn again_or_stale(&self) -> Answer {
        answering().await
    }

    async fn stale(&self, _error: ResilienceError<Down>) -> Answer {
        Ok(0)
    }

    /// An `Err` the default reads: a failure. Run once, so none is waited
    /// out before a retry.
    #[resilient(policy)]
    async fn failed(&self) -> Answer {
        failing().await
    }

    /// An `Err` its `record_if` reads and refuses.
    #[resilient(policy, record_if = Down::is_transient)]
    async fn failed_judged(&self) -> Answer {
        failing().await
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime")
}

fn dependency<I: Instance>(app: &App) -> Dependency<I> {
    Dependency {
        policy: app
            .container()
            .get::<ResiliencePolicy<I>>()
            .expect("the module built the instance's policy"),
    }
}

/// `iters` calls of `call`, one after the other, timed on `rt`.
fn timed<F, Fut>(rt: &tokio::runtime::Runtime, iters: u64, call: F) -> Duration
where
    F: Fn() -> Fut,
    Fut: Future,
{
    rt.block_on(async {
        let started = Instant::now();
        for _ in 0..iters {
            black_box(call().await);
        }
        started.elapsed()
    })
}

/// A call answering at once on the policy of `I`, through each of the four
/// paths an app calls it by.
fn answering_at_once<I: Instance>(
    c: &mut Criterion,
    rt: &tokio::runtime::Runtime,
    app: &App,
    mechanism: &str,
) {
    let dependency = dependency::<I>(app);
    let policy = &dependency.policy;
    c.benchmark_group("run").bench_function(mechanism, |b| {
        b.iter_custom(|iters| timed(rt, iters, || policy.run(answering)));
    });
    c.benchmark_group("run_idempotent")
        .bench_function(mechanism, |b| {
            b.iter_custom(|iters| timed(rt, iters, || policy.run_idempotent(answering)));
        });
    c.benchmark_group("resilient")
        .bench_function(mechanism, |b| {
            b.iter_custom(|iters| timed(rt, iters, || dependency.once()));
        });
    c.benchmark_group("resilient_idempotent")
        .bench_function(mechanism, |b| {
            b.iter_custom(|iters| timed(rt, iters, || dependency.again()));
        });
}

fn answering_by_mechanism(c: &mut Criterion, rt: &tokio::runtime::Runtime, app: &App) {
    answering_at_once::<Bare>(c, rt, app, "bare");
    answering_at_once::<Breaker>(c, rt, app, "breaker");
    answering_at_once::<Bulkhead>(c, rt, app, "bulkhead");
    answering_at_once::<Rate>(c, rt, app, "rate");
}

/// A decorated method's answer read by the keys it writes against the
/// defaults, on the policy of `I`: an `Ok` of an idempotent one, an `Err` of
/// one run once.
fn judged_on<I: Instance>(
    c: &mut Criterion,
    rt: &tokio::runtime::Runtime,
    app: &App,
    mechanism: &str,
) {
    let dependency = dependency::<I>(app);
    let mut group = c.benchmark_group(format!("judged/{mechanism}"));
    group.bench_function("ok_by_default", |b| {
        b.iter_custom(|iters| timed(rt, iters, || dependency.again()));
    });
    group.bench_function("ok_by_keys", |b| {
        b.iter_custom(|iters| timed(rt, iters, || dependency.again_judged()));
    });
    group.bench_function("ok_under_fallback", |b| {
        b.iter_custom(|iters| timed(rt, iters, || dependency.again_or_stale()));
    });
    group.bench_function("err_by_keys", |b| {
        b.iter_custom(|iters| timed(rt, iters, || dependency.failed_judged()));
    });
    group.finish();
}

fn judged(c: &mut Criterion, rt: &tokio::runtime::Runtime, app: &App) {
    judged_on::<Bare>(c, rt, app, "bare");
    judged_on::<Breaker>(c, rt, app, "breaker");
    // On no breaker alone: by default an `Err` is a failure, and a breaker
    // counting them opens, so there no two calls take the same path.
    let dependency = dependency::<Bare>(app);
    c.benchmark_group("judged/bare")
        .bench_function("err_by_default", |b| {
            b.iter_custom(|iters| timed(rt, iters, || dependency.failed()));
        });
}

fn refused(c: &mut Criterion, rt: &tokio::runtime::Runtime, app: &App) {
    let dependency = dependency::<Refusing>(app);
    let policy = &dependency.policy;
    policy
        .open_breaker(Duration::from_secs(60 * 60))
        .expect("an open time the breaker takes");
    let mut group = c.benchmark_group("refused");
    group.bench_function("run/no_subscription", |b| {
        b.iter_custom(|iters| timed(rt, iters, || policy.run(answering)));
    });
    group.bench_function("resilient/no_subscription", |b| {
        b.iter_custom(|iters| timed(rt, iters, || dependency.once()));
    });
    // Unread: past what it holds, each event told takes the oldest one's place.
    let events = policy.events();
    group.bench_function("run/subscription", |b| {
        b.iter_custom(|iters| timed(rt, iters, || policy.run(answering)));
    });
    group.bench_function("resilient/subscription", |b| {
        b.iter_custom(|iters| timed(rt, iters, || dependency.once()));
    });
    drop(events);
    group.finish();
}

/// A decorated method run once on the policy of `I`, called by a thread per
/// core at once, each on a runtime of its own: a row is one call's wall time on
/// one thread, while every other thread calls too.
fn contended_on<I: Instance>(c: &mut Criterion, app: &App, mechanism: &str) {
    let dependency = dependency::<I>(app);
    let threads = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    c.benchmark_group("contended")
        .bench_function(mechanism, |b| {
            b.iter_custom(|iters| {
                let together = std::sync::Barrier::new(threads + 1);
                std::thread::scope(|scope| {
                    for _ in 0..threads {
                        scope.spawn(|| {
                            let rt = runtime();
                            together.wait();
                            rt.block_on(async {
                                for _ in 0..iters {
                                    let _answer = black_box(dependency.once().await);
                                }
                            });
                            together.wait();
                        });
                    }
                    together.wait();
                    let started = Instant::now();
                    together.wait();
                    started.elapsed()
                })
            });
        });
}

/// On a policy with no optional mechanism, on one with its breaker, and on
/// that breaker opened by hand, refusing every call.
fn contended(c: &mut Criterion, app: &App) {
    contended_on::<Bare>(c, app, "bare");
    contended_on::<Breaker>(c, app, "breaker");
    contended_on::<Refusing>(c, app, "refused");
}

fn token(c: &mut Criterion) {
    c.benchmark_group("token")
        .bench_function("new", |b| b.iter(|| black_box(CancellationToken::new())));
}

fn policy(c: &mut Criterion) {
    let rt = runtime();
    let app = rt
        .block_on(App::builder().module::<BenchModule>().build())
        .expect("the policies boot");
    answering_by_mechanism(c, &rt, &app);
    judged(c, &rt, &app);
    refused(c, &rt, &app);
    contended(c, &app);
}

criterion_group!(benches, policy, token);
criterion_main!(benches);

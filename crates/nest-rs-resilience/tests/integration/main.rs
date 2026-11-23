//! `nest-rs-resilience` in process: the documented wiring booted — one policy
//! per instance, imported bare or pinned — a refusal answered by every edge,
//! `#[resilient]`'s expansion run on each host it takes, and the middleware on
//! a reqwest client sending real requests to a TLS peer double whose
//! certificate a `TestAuthority` issued. The binary's allocator counts what a
//! run allocates on its thread.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod config;
mod error;
mod middleware;
mod module;
mod policy;
mod rate;
mod resilient;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use nest_rs_core::{Instance, module};
use nest_rs_resilience::{BreakerState, ResilienceConfig, ResilienceModule, ResiliencePolicy};
use nest_rs_testing::HeadlessApp;

/// No breaker and no retry: a row of a test sets what it is about.
fn quiet() -> ResilienceConfig {
    ResilienceConfig {
        retries: None,
        breaker_window: None,
        ..ResilienceConfig::default()
    }
}

/// The default instance pinned to `config`, as `for_root` pins it.
async fn policy(config: ResilienceConfig) -> (HeadlessApp, Arc<ResiliencePolicy>) {
    #[module(imports = [ResilienceModule])]
    struct Bare;

    let app = HeadlessApp::builder()
        .module::<Bare>()
        .provide(config)
        .build_headless()
        .await
        .expect("a policy boots");
    let policy = held::<nest_rs_core::Unnamed>(&app);
    (app, policy)
}

/// The policy of the instance `I` the booted `app` holds.
fn held<I: Instance>(app: &HeadlessApp) -> Arc<ResiliencePolicy<I>> {
    app.container()
        .get::<ResiliencePolicy<I>>()
        .expect("the module built the instance's policy")
}

/// The whole seconds `policy`'s open breaker tells a call it refuses now to
/// wait, as its `Retry-After` says them. A test of a refusal sent over a real
/// socket keeps the real clock — paused, tokio moves it to the next timer while
/// the socket is read — so it reads this before and after the refusal: the
/// two bound what the refusal says, however long the runner stalls between.
fn breaker_wait<I: Instance>(policy: &ResiliencePolicy<I>) -> u64 {
    let state = policy.breaker_state();
    let Some(BreakerState::Open { retry_after, .. }) = state else {
        panic!("the breaker is open: {state:?}");
    };
    u64::from(nest_rs_core::__private::retry_after_secs(retry_after))
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The system allocator, counting the allocations made on a thread while it
/// is asked to.
struct Counting;

thread_local! {
    /// The allocations made on this thread since counting began, `None` while
    /// nothing counts.
    static COUNTED: Cell<Option<usize>> = const { Cell::new(None) };
}

fn count() {
    // Off a thread's storage, as it is torn down, nothing is being counted.
    if let Ok(Some(made)) = COUNTED.try_with(Cell::get) {
        COUNTED.set(Some(made + 1));
    }
}

#[expect(
    unsafe_code,
    reason = "an allocator is an unsafe trait; each call goes to the system allocator unchanged"
)]
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count();
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

/// What the future `make` builds answers at its first poll, on this thread,
/// and how many allocations building and polling it made; a future that waits
/// fails the test.
fn allocations<F: Future>(make: impl FnOnce() -> F) -> (F::Output, usize) {
    COUNTED.set(Some(0));
    let mut fut = pin!(make());
    let polled = fut.as_mut().poll(&mut Context::from_waker(Waker::noop()));
    let made = COUNTED.replace(None).unwrap_or_default();
    let Poll::Ready(answer) = polled else {
        panic!("a call whose allocations are counted answers at its first poll");
    };
    (answer, made)
}

/// What an attempt's `CancellationToken` allocates, which every attempt hands
/// its call.
fn token_allocations() -> usize {
    allocations(|| std::future::ready(nest_rs_resilience::CancellationToken::new())).1
}

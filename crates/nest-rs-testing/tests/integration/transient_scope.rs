//! `#[injectable(scope = transient)]`: a fresh instance on **every** resolution
//! (no caching), able to depend on singletons, and — when it depends on itself —
//! a clear cycle diagnostic at first resolution rather than at boot.
//!
//! Gives the transient scope its first product-shaped test + use site, and
//! exercises the documented self-cycle panic that was otherwise unverified.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use nest_rs_core::{App, RequestScope, injectable, module};

#[injectable]
#[derive(Default)]
struct Counter {
    _n: AtomicU64,
}

// Rebuilt on every `get`, but its singleton dep is shared across instances.
#[injectable(scope = transient)]
struct Ticket {
    #[inject]
    counter: Arc<Counter>,
}

#[module(providers = [Counter, Ticket])]
struct TransientModule;

#[tokio::test]
async fn transient_rebuilds_on_every_resolution_but_shares_its_singleton_dep() {
    let app = App::new::<TransientModule>().expect("boots");
    let container = app.container();

    let first: Arc<Ticket> = container.get().expect("ticket resolves");
    let second: Arc<Ticket> = container.get().expect("ticket resolves");

    // Distinct instances — no caching across resolutions.
    assert!(
        !Arc::ptr_eq(&first, &second),
        "a transient must be rebuilt on every resolution"
    );
    // ...yet the injected singleton is the same shared instance.
    assert!(
        Arc::ptr_eq(&first.counter, &second.counter),
        "a transient depends on the singleton root, not a fresh copy"
    );
}

// B-CORE: a transient that injects a **request-scoped** provider. This is the
// macro path (`#[injectable(scope = transient)]` now emits `from_scope`, which
// resolves `#[inject]` deps through the `RequestScope`). Before the fix this
// booted clean then panicked on first resolution.
#[injectable(scope = request)]
#[derive(Default)]
struct RequestState {
    _n: AtomicU64,
}

#[injectable(scope = transient)]
struct Handle {
    #[inject]
    state: Arc<RequestState>,
}

#[module(providers = [RequestState, Handle])]
struct MixedScopeModule;

#[tokio::test]
async fn a_transient_injecting_a_request_scoped_provider_resolves_through_the_scope() {
    let app = App::new::<MixedScopeModule>()
        .expect("boots — a transient injecting a request-scoped dep is legal");
    let scope = RequestScope::new(app.container().clone());

    let first: Arc<Handle> = scope
        .get()
        .expect("transient resolves inside the request scope");
    let second: Arc<Handle> = scope.get().expect("transient resolves again");

    // Fresh transient each resolution...
    assert!(
        !Arc::ptr_eq(&first, &second),
        "a transient is rebuilt on every resolution",
    );
    // ...but both share the request's single request-scoped instance.
    assert!(
        Arc::ptr_eq(&first.state, &second.state),
        "the injected request-scoped provider is shared across the request",
    );
}

// A transient that injects itself: boot succeeds (transients report no
// register-phase dependencies, so the singleton fixpoint can't see the cycle);
// the cycle is caught at first resolution with a chain-naming panic.
#[injectable(scope = transient)]
struct Cyclic {
    #[inject]
    _me: Arc<Cyclic>,
}

#[module(providers = [Cyclic])]
struct CyclicModule;

#[tokio::test]
#[should_panic(expected = "transient provider cycle")]
async fn self_referential_transient_panics_at_resolution() {
    let app = App::new::<CyclicModule>().expect("boots — the cycle is lazy, not a boot error");
    let _ = app.container().get::<Cyclic>();
}

//! `#[injectable(scope = transient)]`: a fresh instance on **every** resolution,
//! able to depend on singletons, and a cycle diagnostic at first resolution.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use nest_rs_core::{App, RequestScope, injectable, module};

#[injectable]
#[derive(Default)]
struct Counter {
    _n: AtomicU64,
}

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

    assert!(
        !Arc::ptr_eq(&first, &second),
        "a transient must be rebuilt on every resolution"
    );
    assert!(
        Arc::ptr_eq(&first.counter, &second.counter),
        "a transient depends on the singleton root, not a fresh copy"
    );
}

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

    assert!(
        !Arc::ptr_eq(&first, &second),
        "a transient is rebuilt on every resolution",
    );
    assert!(
        Arc::ptr_eq(&first.state, &second.state),
        "the injected request-scoped provider is shared across the request",
    );
}

// Transients report no register-phase dependencies, so the boot cannot see
// this cycle.
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

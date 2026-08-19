//! End-to-end check of the module access graph through the real `#[module]` /
//! `#[injectable]` macros and the `App` boot path. The link-time registry is
//! shared across a test binary, so the graphs below use disjoint types.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::{App, ContainerBuilder, Discoverable, injectable, module};

// A type no module provides — the dependency a scoped provider will fail to
// resolve. Not `#[injectable]`, so nothing ever registers it.
struct AbsentDep;

#[allow(dead_code)]
#[injectable(scope = request)]
struct ScopedNeedy {
    #[inject]
    dep: Arc<AbsentDep>,
}

#[module(providers = [ScopedNeedy])]
struct ScopedMissingModule;

#[tokio::test]
async fn a_scoped_providers_missing_dependency_is_a_boot_error_not_a_panic() {
    // A request-scoped provider builds lazily, so a missing dependency used to
    // slip through boot and panic at the first `get(...).expect(...)`. The access
    // graph now catches it up front, naming both the provider and the dependency.
    let err = App::builder()
        .module::<ScopedMissingModule>()
        .build()
        .await
        .err()
        .expect("a scoped provider with an unprovided dependency must fail the boot, not panic");
    let msg = err.to_string();
    assert!(msg.contains("ScopedNeedy"), "names the provider: {msg}");
    assert!(
        msg.contains("AbsentDep"),
        "names the missing dependency: {msg}"
    );
}

#[injectable]
struct ServiceA;

#[allow(dead_code)]
#[injectable]
struct ServiceB {
    #[inject]
    svc: Arc<ServiceA>,
}

#[module(providers = [ServiceA])]
struct ModuleA;

#[module(providers = [ServiceB])]
struct LeakyModuleB;

// `ModuleA` listed first lets the flat container's order-dependent fixpoint
// silently resolve `ServiceA`; the access check turns that into a deterministic
// boot error.
#[module(imports = [ModuleA, LeakyModuleB])]
struct LeakyRoot;

#[tokio::test]
async fn unimported_cross_module_dependency_is_rejected_at_boot() {
    let err = App::builder()
        .module::<LeakyRoot>()
        .build()
        .await
        .err()
        .expect("boot must reject a dependency crossing a non-imported boundary");
    let msg = err.to_string();
    assert!(
        msg.contains("ServiceB"),
        "names the offending provider: {msg}"
    );
    assert!(msg.contains("LeakyModuleB"), "names the module: {msg}");
    assert!(
        msg.contains("ModuleA"),
        "suggests the module to import: {msg}"
    );
}

#[injectable]
struct FixedServiceA;

#[allow(dead_code)]
#[injectable]
struct FixedServiceB {
    #[inject]
    svc: Arc<FixedServiceA>,
}

#[module(providers = [FixedServiceA])]
struct FixedModuleA;

#[module(imports = [FixedModuleA], providers = [FixedServiceB])]
struct FixedModuleB;

#[module(imports = [FixedModuleA, FixedModuleB])]
struct FixedRoot;

#[tokio::test]
async fn imported_cross_module_dependency_boots() {
    App::builder()
        .module::<FixedRoot>()
        .build()
        .await
        .expect("declaring the import makes the cross-module dependency legal");
}

// A lazily-built provider (controller / cron job / processor shape): empty
// `dependencies`, non-empty `injected`. The graph reads `injected`, so this is
// still under contract.
#[injectable]
struct LazyDep;

struct LazyConsumer;
impl Discoverable for LazyConsumer {
    fn injected() -> Vec<TypeId> {
        vec![TypeId::of::<LazyDep>()]
    }
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
    }
}

#[module(providers = [LazyDep])]
struct LazyDepModule;

#[module(providers = [LazyConsumer])]
struct LazyLeakyModule;

#[module(imports = [LazyDepModule, LazyLeakyModule])]
struct LazyLeakyRoot;

#[tokio::test]
async fn lazily_built_provider_injection_is_checked_via_injected_not_dependencies() {
    assert!(
        LazyConsumer::dependencies().is_empty(),
        "the lazy provider blocks no register ordering",
    );
    let err = App::builder()
        .module::<LazyLeakyRoot>()
        .build()
        .await
        .err()
        .expect("a lazily-built provider's injection still crosses the import boundary");
    let msg = err.to_string();
    assert!(
        msg.contains("LazyConsumer"),
        "names the lazy provider: {msg}"
    );
    assert!(msg.contains("LazyLeakyModule"), "names the module: {msg}");
    assert!(msg.contains("LazyDepModule"), "suggests the import: {msg}");
}

// --- a singleton may not inject what only exists inside a request ------------
//
// It used to fail **silently**, and that is why this is a suite test rather than
// a unit one: the register phase gates readiness on the singleton map, so such a
// provider never became ready, was classified unprovided and was dropped — with
// everything downstream of it — while the boot returned `Ok` and emitted
// nothing. `Container::get` then answered `None` for a provider written in
// `providers = [...]`, far from the cause. Only a real boot shows that.

#[injectable(scope = request)]
#[derive(Default)]
struct PerRequest;

#[injectable]
#[allow(dead_code)]
struct SingletonHoldingRequestScoped {
    #[inject]
    dep: Arc<PerRequest>,
}

#[injectable]
#[allow(dead_code)]
struct DownstreamOfIt {
    #[inject]
    host: Arc<SingletonHoldingRequestScoped>,
}

#[module(providers = [PerRequest, SingletonHoldingRequestScoped, DownstreamOfIt])]
struct SingletonScopeViolationModule;

#[tokio::test]
async fn a_singleton_injecting_a_request_scoped_provider_fails_the_boot_by_name() {
    let err = App::builder()
        .module::<SingletonScopeViolationModule>()
        .build()
        .await
        .err()
        .expect("a singleton cannot hold a provider that exists only inside a request");
    let msg = err.to_string();
    assert!(
        msg.contains("SingletonHoldingRequestScoped"),
        "names the consumer: {msg}",
    );
    assert!(msg.contains("PerRequest"), "names the dependency: {msg}");
    assert!(
        msg.contains("Scoped<T>"),
        "and carries the remedy — reach it through the request boundary: {msg}",
    );
}

#[injectable(scope = transient)]
#[derive(Default)]
struct PerResolution;

#[injectable]
#[allow(dead_code)]
struct SingletonHoldingTransient {
    #[inject]
    dep: Arc<PerResolution>,
}

#[module(providers = [PerResolution, SingletonHoldingTransient])]
struct SingletonTransientViolationModule;

/// The check's **second arm**, which had no test while the error's own sentence
/// described only the first.
///
/// A transient is not "only inside a request" — `Container::get` opens a
/// throwaway scope and builds one, which `Discoverable`'s table states outright.
/// What it shares with a request-scoped provider is the fact the check actually
/// reads: neither is ever placed in the singleton map, so the register-phase
/// fixpoint never marks the consumer ready and drops it with everything
/// downstream. Same silent drop, same boot error, and now the same coverage.
#[tokio::test]
async fn a_singleton_injecting_a_transient_provider_fails_the_boot_by_name() {
    let err = App::builder()
        .module::<SingletonTransientViolationModule>()
        .build()
        .await
        .err()
        .expect("a singleton cannot hold a provider rebuilt on every resolution");
    let msg = err.to_string();
    assert!(
        msg.contains("SingletonHoldingTransient"),
        "names the consumer: {msg}",
    );
    assert!(msg.contains("PerResolution"), "names the dependency: {msg}");
}

// --- and the two directions that are legal stay legal ------------------------

#[injectable]
#[derive(Default)]
struct PlainSingleton;

#[injectable(scope = request)]
#[allow(dead_code)]
struct ScopedOnSingleton {
    #[inject]
    dep: Arc<PlainSingleton>,
}

#[injectable(scope = request)]
#[allow(dead_code)]
struct ScopedOnScoped {
    #[inject]
    dep: Arc<ScopedOnSingleton>,
}

#[injectable(scope = transient)]
#[allow(dead_code)]
struct TransientOnScoped {
    #[inject]
    dep: Arc<ScopedOnSingleton>,
}

#[module(providers = [
    PlainSingleton,
    ScopedOnSingleton,
    ScopedOnScoped,
    TransientOnScoped,
])]
struct LegalScopeDirectionsModule;

/// One level deep is about the *singleton*, not about the scope: a
/// request-scoped provider resolves its own deps through the scope, so it may
/// hold another request-scoped provider (sharing the request's instance) and a
/// transient may hold either. Only the reverse is impossible.
#[tokio::test]
async fn a_request_scoped_provider_may_hold_singletons_and_its_own_kind() {
    App::builder()
        .module::<LegalScopeDirectionsModule>()
        .build()
        .await
        .expect("scoped→singleton, scoped→scoped and transient→scoped are all legal");
}

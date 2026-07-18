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

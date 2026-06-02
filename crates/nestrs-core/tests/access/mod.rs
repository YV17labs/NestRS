//! End-to-end check of the module access graph through the
//! real `#[module]` / `#[injectable]` macros and the `App` boot path.
//!
//! Each `#[module]` submits its descriptor to the link-time registry, which is
//! shared across a test binary — so this file is its own integration-test crate
//! (separate binary), and the two graphs below use disjoint types so their
//! validations never interfere.

use std::any::TypeId;
use std::sync::Arc;

use nestrs_core::{injectable, module, App, ContainerBuilder, Discoverable};

// --- A leaky graph: a provider reaches a module its own module never imports.

#[injectable]
struct ServiceA;

#[allow(dead_code)]
#[injectable]
struct ServiceB {
    #[inject]
    a: Arc<ServiceA>,
}

#[module(providers = [ServiceA])]
struct ModuleA;

// `ServiceB` injects `ServiceA`, but `LeakyModuleB` does not import `ModuleA`.
#[module(providers = [ServiceB])]
struct LeakyModuleB;

// Imports `ModuleA` *before* `LeakyModuleB`, so the flat container's
// registration-order fixpoint happens to resolve `ServiceA` — the silent,
// order-dependent success the access check turns into a deterministic boot error.
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

// --- The same shape, but with the import declared: it must boot cleanly.

#[injectable]
struct FixedServiceA;

#[allow(dead_code)]
#[injectable]
struct FixedServiceB {
    #[inject]
    a: Arc<FixedServiceA>,
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

// --- A *lazily-built* provider (a controller / cron job / processor shape):
// empty `dependencies` (it does not block register ordering), but a non-empty
// `injected`. The access graph reads `injected`, so this is still checked — the
// hole that motivated splitting the two methods.

#[injectable]
struct LazyDep;

// Hand-written `Discoverable` mirroring what `#[controller]`/`#[cron_job]`/
// `#[processor]` emit: registers nothing eagerly (`dependencies` empty, so it is
// built later from the assembled container) yet declares its injected key.
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

// Injects `LazyDep` but does not import `LazyDepModule`.
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

//! Covers `src/access.rs` through the real macros and boot path. The link-time
//! registry is shared across a test binary, so the graphs below use disjoint types.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::{
    App, ContainerBuilder, Discoverable, DynamicModule, Registering, injectable, module,
};

struct AbsentDep;

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable(scope = request)]
struct ScopedNeedy {
    #[inject]
    dep: Arc<AbsentDep>,
}

#[module(providers = [ScopedNeedy])]
struct ScopedMissingModule;

#[tokio::test]
async fn a_scoped_providers_missing_dependency_is_a_boot_error_not_a_panic() {
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

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct ServiceB {
    #[inject]
    svc: Arc<ServiceA>,
}

#[module(providers = [ServiceA])]
struct ModuleA;

#[module(providers = [ServiceB])]
struct LeakyModuleB;

// `ModuleA` listed first lets the flat container's fixpoint resolve `ServiceA`
// anyway; only the access check refuses it.
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

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
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

#[injectable]
struct PinnedClient;

#[module(providers = [PinnedClient])]
struct ClientModule;

/// The `for_root` shape of a module owning a config (`ConfigSetup<M, C>`): the
/// dynamic import registers the module it configures.
struct ClientSetup;

fn client_for_root() -> ClientSetup {
    ClientSetup
}

impl DynamicModule for ClientSetup {
    fn module() -> TypeId {
        TypeId::of::<ClientModule>()
    }

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.import::<ClientModule>()
    }
}

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct ClientUser {
    #[inject]
    client: Arc<PinnedClient>,
}

#[module(imports = [client_for_root()], providers = [ClientUser])]
struct ClientUserModule;

#[tokio::test]
async fn a_provider_injects_what_a_dynamic_import_registers_on_the_async_path() {
    App::builder()
        .module::<ClientUserModule>()
        .build()
        .await
        .expect("a dynamic import imports the module it registers");
}

#[injectable]
struct Prerequisite;

#[module(providers = [Prerequisite])]
struct PrerequisiteModule;

/// A `for_root` whose module needs another one wired first, so its `register`
/// registers that one before its own.
struct OrderedSetup;

fn ordered_for_root() -> OrderedSetup {
    OrderedSetup
}

impl DynamicModule for OrderedSetup {
    fn module() -> TypeId {
        TypeId::of::<ClientModule>()
    }

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
            .import::<PrerequisiteModule>()
            .import::<ClientModule>()
    }
}

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct OrderedClientUser {
    #[inject]
    client: Arc<PinnedClient>,
}

#[module(imports = [ordered_for_root()], providers = [OrderedClientUser])]
struct OrderedClientUserModule;

#[tokio::test]
async fn a_dynamic_import_is_the_module_it_declares_whatever_it_registers_first() {
    App::builder()
        .module::<OrderedClientUserModule>()
        .build()
        .await
        .expect("importing the setup imports the module it is, not the first one it wires");
}

// A lazily-built provider (controller shape): empty `dependencies`, non-empty
// `injected`, which is what the graph reads.
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

#[injectable(scope = request)]
#[derive(Default)]
struct PerRequest;

#[injectable]
#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
struct SingletonHoldingRequestScoped {
    #[inject]
    dep: Arc<PerRequest>,
}

#[injectable]
#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
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
#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
struct SingletonHoldingTransient {
    #[inject]
    dep: Arc<PerResolution>,
}

#[module(providers = [PerResolution, SingletonHoldingTransient])]
struct SingletonTransientViolationModule;

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

#[injectable]
#[derive(Default)]
struct PlainSingleton;

#[injectable(scope = request)]
#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
struct ScopedOnSingleton {
    #[inject]
    dep: Arc<PlainSingleton>,
}

#[injectable(scope = request)]
#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
struct ScopedOnScoped {
    #[inject]
    dep: Arc<ScopedOnSingleton>,
}

#[injectable(scope = transient)]
#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
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

#[tokio::test]
async fn a_request_scoped_provider_may_hold_singletons_and_its_own_kind() {
    App::builder()
        .module::<LegalScopeDirectionsModule>()
        .build()
        .await
        .expect("scoped→singleton, scoped→scoped and transient→scoped are all legal");
}

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct CycleLeft {
    #[inject]
    right: Arc<CycleRight>,
}

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct CycleRight {
    #[inject]
    left: Arc<CycleLeft>,
}

#[module(providers = [CycleLeft, CycleRight])]
struct CycleModule;

#[test]
fn providers_waiting_on_each_other_fail_the_boot_naming_the_cycle() {
    let Err(refused) = App::new::<CycleModule>() else {
        panic!("two providers waiting on each other cannot both be built");
    };
    let cycle = refused
        .downcast_ref::<nest_rs_core::ProviderCycleError>()
        .unwrap_or_else(|| panic!("not the cycle refusal: {refused:#}"));
    assert_eq!(cycle.module, "CycleModule");
    assert_eq!(cycle.type_names, ["CycleLeft", "CycleRight"]);
}

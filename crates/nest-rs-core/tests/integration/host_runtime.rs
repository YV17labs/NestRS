//! Covers `src/host_runtime.rs` — what a listed host's decorator brings is
//! imported in both phases of the module listing it, and walked by the access
//! graph like an import; what no module the app reaches lists brings nothing.

use std::any::TypeId;
use std::sync::{Arc, Mutex};

use nest_rs_core::__private::{HostMount, HostRuntime, RuntimeModule};
use nest_rs_core::{
    App, Collecting, Composition, Container, ContainerBuilder, ContainerError, Discoverable,
    Module, ReachableProviders, Registering, UnbuiltCause, UnbuiltProviderError, hooks, injectable,
    inventory, module, target,
};
use nest_rs_testing::LogCapture;

use crate::refused_boot;

/// What a host injects, listed by a module its adapter never names.
#[injectable]
#[derive(Default)]
struct Registry;

#[module(providers = [Registry])]
struct RegistryModule;

/// What the factory a runtime's `collect` queues makes.
struct Collected;

/// What a runtime's `register` provides: registered twice, the boot refuses it.
struct Registered;

/// A runtime as an edge's is written: a factory in its collect, its wiring in
/// its register.
struct PhasesModule;

impl Module for PhasesModule {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder.provide_factory(|_| async { Ok(Collected) })
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide(Registered)
    }
}

const RUNTIME: &[RuntimeModule] = &[
    RuntimeModule::of::<RegistryModule>(),
    RuntimeModule::of::<PhasesModule>(),
];

#[injectable]
struct Host {
    #[inject]
    registry: Arc<Registry>,
}

inventory::submit! {
    HostRuntime {
        host: || TypeId::of::<Host>(),
        name: std::any::type_name::<Host>,
        mount: HostMount::Linked,
        modules: RUNTIME,
    }
}

/// An adapter naming nothing of what its host runs on.
#[module(providers = [Host])]
struct HostModule;

#[tokio::test]
async fn a_listed_host_brings_what_it_runs_on_through_both_phases() {
    let app = App::builder()
        .module::<HostModule>()
        .build()
        .await
        .expect("the access graph follows what the host brings");
    let container = app.container();

    let host = container.get::<Host>().expect("the host is built");
    assert!(Arc::ptr_eq(
        &host.registry,
        &container.get::<Registry>().expect("its module is imported"),
    ));
    assert!(container.get::<Collected>().is_some(), "its collect ran");
    assert!(container.get::<Registered>().is_some(), "its register ran");
}

#[tokio::test]
async fn what_a_host_brings_is_reached_by_every_reader_of_the_composition() {
    let app = App::builder()
        .module::<HostModule>()
        .build()
        .await
        .expect("the module boots");
    let container = app.container();

    let reachable = ReachableProviders::of(container, "a test").expect("the boot seals the set");
    assert!(reachable.reaches(TypeId::of::<Registry>()));
    let composition = container.get::<Composition>().expect("the boot records it");
    assert!(composition.reaches(TypeId::of::<RegistryModule>()));
    assert!(composition.reaches(TypeId::of::<PhasesModule>()));
}

/// An app that links the adapter and does not import it.
#[module]
struct ElsewhereModule;

#[tokio::test]
async fn a_host_of_a_module_the_app_does_not_import_brings_nothing() {
    let app = App::builder()
        .module::<ElsewhereModule>()
        .build()
        .await
        .expect("the module boots");
    let container = app.container();

    assert!(container.get::<Registry>().is_none());
    assert!(container.get::<Registered>().is_none());
    let reachable = ReachableProviders::of(container, "a test").expect("the boot seals the set");
    assert!(!reachable.reaches(TypeId::of::<Registry>()));
}

/// A provider no edge's decorator made a host: its module imports its transport
/// itself, as one holding only an interceptor does.
#[injectable]
#[derive(Default)]
struct NoHost;

#[module(providers = [NoHost])]
struct NoHostModule;

#[tokio::test]
async fn a_provider_no_decorator_made_a_host_brings_nothing() {
    let app = App::builder()
        .module::<NoHostModule>()
        .build()
        .await
        .expect("the module boots");

    assert!(app.container().get::<NoHost>().is_some());
    assert!(app.container().get::<Registered>().is_none());
}

/// A module writing out what its host brings, as an adapter may.
#[module(imports = [PhasesModule, RegistryModule], providers = [Host])]
struct ExplicitHostModule;

#[tokio::test]
async fn an_import_beside_what_a_host_brings_wires_once() {
    let app = App::builder()
        .module::<ExplicitHostModule>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("one registration of each: {refused:#}"));

    assert!(app.container().get::<Registered>().is_some());
}

trait Bound: Send + Sync {}

impl Bound for Host {}

#[module(providers = [Host as dyn Bound])]
struct BoundHostModule;

#[tokio::test]
async fn a_host_bound_under_a_trait_brings_what_it_runs_on() {
    let app = App::builder()
        .module::<BoundHostModule>()
        .build()
        .await
        .expect("the binding holds the host under its own type too");

    assert!(app.container().get::<Registered>().is_some());
}

/// A second edge's runtime, for a host serving two.
struct SecondEdgeModule;

struct SecondEdge;

impl Module for SecondEdgeModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide(SecondEdge)
    }
}

const FIRST_EDGE: &[RuntimeModule] = &[RuntimeModule::of::<PhasesModule>()];
const SECOND_EDGE: &[RuntimeModule] = &[RuntimeModule::of::<SecondEdgeModule>()];

#[injectable]
#[derive(Default)]
struct TwoEdges;

inventory::submit! {
    HostRuntime {
        host: || TypeId::of::<TwoEdges>(),
        name: std::any::type_name::<TwoEdges>,
        mount: HostMount::Linked,
        modules: FIRST_EDGE,
    }
}

inventory::submit! {
    HostRuntime {
        host: || TypeId::of::<TwoEdges>(),
        name: std::any::type_name::<TwoEdges>,
        mount: HostMount::Linked,
        modules: SECOND_EDGE,
    }
}

#[module(providers = [TwoEdges])]
struct TwoEdgesModule;

#[tokio::test]
async fn a_host_of_two_edges_brings_what_each_runs_on() {
    let app = App::builder()
        .module::<TwoEdgesModule>()
        .build()
        .await
        .expect("the module boots");

    assert!(app.container().get::<Registered>().is_some());
    assert!(app.container().get::<SecondEdge>().is_some());
}

/// A host whose runtime fails at the boot fails that boot, listed or imported.
struct RefusingModule;

impl Module for RefusingModule {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder.provide_factory(|_| async { Err::<Collected, _>(anyhow::anyhow!("no backend")) })
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }
}

const REFUSING: &[RuntimeModule] = &[RuntimeModule::of::<RefusingModule>()];

#[injectable]
#[derive(Default)]
struct RefusedHost;

inventory::submit! {
    HostRuntime {
        host: || TypeId::of::<RefusedHost>(),
        name: std::any::type_name::<RefusedHost>,
        mount: HostMount::Linked,
        modules: REFUSING,
    }
}

#[module(providers = [RefusedHost])]
struct RefusedHostModule;

#[tokio::test]
async fn a_runtime_failing_fails_the_boot_of_the_app_listing_its_host() {
    let refused = refused_boot::<RefusedHostModule>().await;

    assert!(format!("{refused:#}").contains("no backend"), "{refused:#}");
}

/// A container built by hand drains no factory, so a seed stands in for what a
/// host's runtime reads, as it does for an import written out.
#[test]
fn a_container_built_by_hand_is_held_to_what_a_host_brings() {
    let Err(ContainerError::LateFactory(late)) =
        Container::builder().import::<HostModule>().build()
    else {
        panic!("the factory `PhasesModule` queued would never run");
    };
    assert_eq!(late.type_name, std::any::type_name::<Collected>());

    let seeded = Container::builder()
        .provide(Collected)
        .import::<HostModule>()
        .build()
        .expect("the seed stands in for the factory");
    assert!(seeded.get::<Registered>().is_some());
}

static INITIALIZED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

#[injectable]
#[derive(Default)]
struct Brought;

#[hooks]
impl Brought {
    #[on_module_init]
    async fn init(&self) {
        INITIALIZED.lock().unwrap().push("Brought");
    }
}

#[module(providers = [Brought])]
struct BroughtModule;

const BROUGHT: &[RuntimeModule] = &[RuntimeModule::of::<BroughtModule>()];

#[injectable]
#[derive(Default)]
struct Listing;

#[hooks]
impl Listing {
    #[on_module_init]
    async fn init(&self) {
        INITIALIZED.lock().unwrap().push("Listing");
    }
}

inventory::submit! {
    HostRuntime {
        host: || TypeId::of::<Listing>(),
        name: std::any::type_name::<Listing>,
        mount: HostMount::Linked,
        modules: BROUGHT,
    }
}

#[module(providers = [Listing])]
struct ListingModule;

/// No injection orders the two, so the composition does: what a module imports
/// comes first, whether the module wrote the import or its host brought it.
#[tokio::test]
async fn what_a_host_brings_inits_before_the_module_listing_it() {
    let app = App::builder()
        .module::<ListingModule>()
        .build()
        .await
        .expect("the module boots");

    app.init().await.expect("the hooks run");

    assert_eq!(*INITIALIZED.lock().unwrap(), ["Brought", "Listing"]);
}

#[injectable]
#[derive(Default)]
struct Far;

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct Near {
    #[inject]
    far: Arc<Far>,
}

#[injectable]
#[derive(Default)]
struct CycleHost;

#[module(imports = [CycleModule], providers = [Near])]
struct CycleRuntimeModule;

const CYCLE: &[RuntimeModule] = &[RuntimeModule::of::<CycleRuntimeModule>()];

inventory::submit! {
    HostRuntime {
        host: || TypeId::of::<CycleHost>(),
        name: std::any::type_name::<CycleHost>,
        mount: HostMount::Linked,
        modules: CYCLE,
    }
}

#[module(providers = [CycleHost, Far])]
struct CycleModule;

/// The way back is no line a module wrote, and the boot still names the cycle.
#[tokio::test]
async fn an_import_cycle_closing_through_what_a_host_brings_is_named() {
    let refused = refused_boot::<CycleModule>().await;
    let unbuilt = refused
        .downcast_ref::<UnbuiltProviderError>()
        .unwrap_or_else(|| panic!("not the unbuilt-provider refusal: {refused:#}"));

    assert_eq!(unbuilt.provider, "Near", "{refused:#}");
    let UnbuiltCause::ImportCycle(modules) = &unbuilt.cause else {
        panic!("not an import cycle: {unbuilt}");
    };
    assert_eq!(modules.len(), 2, "{unbuilt}");
    for module in ["CycleModule", "CycleRuntimeModule"] {
        assert!(modules.contains(&module), "{unbuilt}");
    }
}

/// What `App::run` files for a host no import of the boot brings its edge to.
const UNSERVED: &str = "host not served: its edge's transport is not attached";

/// What a run-time host's edge runs on: empty, the import alone tells.
#[module]
struct EdgeRuntimeModule;

const EDGE_RUNTIME: &[RuntimeModule] = &[RuntimeModule::of::<EdgeRuntimeModule>()];

#[injectable]
#[derive(Default)]
struct HandRegisteredHost;

inventory::submit! {
    HostRuntime {
        host: || TypeId::of::<HandRegisteredHost>(),
        name: std::any::type_name::<HandRegisteredHost>,
        mount: HostMount::Linked,
        modules: EDGE_RUNTIME,
    }
}

/// Registers the host itself, as a `#[module]` listing it would not, and imports
/// nothing it runs on.
struct HandRegistersItModule;

impl Module for HandRegistersItModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide(HandRegisteredHost)
    }
}

async fn logs_of_a_run(app: nest_rs_core::AppBuilder) -> LogCapture {
    let logs = LogCapture::install();
    app.build()
        .await
        .expect("the app boots")
        .run()
        .await
        .expect("an app serving nothing stops cleanly");
    logs
}

#[tokio::test]
async fn a_host_registered_by_hand_with_nothing_serving_its_edge_is_named_at_run() {
    let logs = logs_of_a_run(App::builder().module::<HandRegistersItModule>()).await;
    let unserved = logs.expect_one(target::APP, UNSERVED);
    assert_eq!(unserved.level, "warn");
    assert_eq!(
        unserved.field("host").as_deref(),
        Some("HandRegisteredHost")
    );
    assert_eq!(
        unserved.field("import").as_deref(),
        Some("EdgeRuntimeModule")
    );
}

#[module]
struct SecondRuntimeModule;

const TWO_RUNTIMES: &[RuntimeModule] = &[
    RuntimeModule::of::<EdgeRuntimeModule>(),
    RuntimeModule::of::<SecondRuntimeModule>(),
];

#[injectable]
#[derive(Default)]
struct TwoRuntimesHost;

inventory::submit! {
    HostRuntime {
        host: || TypeId::of::<TwoRuntimesHost>(),
        name: std::any::type_name::<TwoRuntimesHost>,
        mount: HostMount::Linked,
        modules: TWO_RUNTIMES,
    }
}

// A second method's entry for the same host, as a provider-hosted decorator
// submits one per method.
inventory::submit! {
    HostRuntime {
        host: || TypeId::of::<TwoRuntimesHost>(),
        name: std::any::type_name::<TwoRuntimesHost>,
        mount: HostMount::Linked,
        modules: TWO_RUNTIMES,
    }
}

#[tokio::test]
async fn a_host_on_two_modules_is_named_once_with_both() {
    let logs = logs_of_a_run(App::builder().provide(TwoRuntimesHost)).await;
    let unserved = logs.expect_one(target::APP, UNSERVED);
    assert_eq!(
        unserved.field("import").as_deref(),
        Some("EdgeRuntimeModule, SecondRuntimeModule")
    );
}

/// One module of the two imported: the edge runs, and the app chose what else.
struct ImportsOneOfTwoModule;

impl Module for ImportsOneOfTwoModule {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder.import::<SecondRuntimeModule>()
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.import::<SecondRuntimeModule>()
    }
}

#[tokio::test]
async fn a_host_one_of_whose_modules_is_imported_is_not_named() {
    let logs = logs_of_a_run(
        App::builder()
            .provide(TwoRuntimesHost)
            .module::<ImportsOneOfTwoModule>(),
    )
    .await;
    logs.expect_none(target::APP, UNSERVED);
}

#[tokio::test]
async fn a_seeded_host_with_nothing_serving_its_edge_is_named_at_run() {
    let logs = logs_of_a_run(App::builder().provide(HandRegisteredHost)).await;
    logs.expect_one(target::APP, UNSERVED);
}

/// Registers the host and imports what it runs on, as the docs tell one to.
struct HandRegistersItBesideItsRuntimeModule;

impl Module for HandRegistersItBesideItsRuntimeModule {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder.import::<EdgeRuntimeModule>()
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
            .import::<EdgeRuntimeModule>()
            .provide(HandRegisteredHost)
    }
}

#[tokio::test]
async fn a_host_registered_by_hand_beside_its_runtime_is_not_named() {
    let logs =
        logs_of_a_run(App::builder().module::<HandRegistersItBesideItsRuntimeModule>()).await;
    logs.expect_none(target::APP, UNSERVED);
}

#[injectable]
#[derive(Default)]
struct ListedHost;

inventory::submit! {
    HostRuntime {
        host: || TypeId::of::<ListedHost>(),
        name: std::any::type_name::<ListedHost>,
        mount: HostMount::Linked,
        modules: EDGE_RUNTIME,
    }
}

#[module(providers = [ListedHost])]
struct ListsItModule;

#[tokio::test]
async fn a_host_a_module_lists_brings_its_runtime_and_is_not_named() {
    let logs = logs_of_a_run(App::builder().module::<ListsItModule>()).await;
    logs.expect_none(target::APP, UNSERVED);
}

#[tokio::test]
async fn a_host_the_app_does_not_hold_is_not_named() {
    let logs = logs_of_a_run(App::builder().module::<ElsewhereModule>()).await;
    logs.expect_none(target::APP, UNSERVED);
}

#[module]
struct FirstEdgeModule;

#[module]
struct SecondEdgeRuntimeModule;

const ONLY_THE_FIRST_EDGE: &[RuntimeModule] = &[RuntimeModule::of::<FirstEdgeModule>()];
const ONLY_THE_SECOND_EDGE: &[RuntimeModule] = &[RuntimeModule::of::<SecondEdgeRuntimeModule>()];

#[injectable]
#[derive(Default)]
struct OnTwoEdges;

inventory::submit! {
    HostRuntime {
        host: || TypeId::of::<OnTwoEdges>(),
        name: std::any::type_name::<OnTwoEdges>,
        mount: HostMount::Linked,
        modules: ONLY_THE_FIRST_EDGE,
    }
}

inventory::submit! {
    HostRuntime {
        host: || TypeId::of::<OnTwoEdges>(),
        name: std::any::type_name::<OnTwoEdges>,
        mount: HostMount::Linked,
        modules: ONLY_THE_SECOND_EDGE,
    }
}

#[tokio::test]
async fn a_host_on_two_edges_with_neither_imported_names_both() {
    let logs = logs_of_a_run(App::builder().provide(OnTwoEdges)).await;
    let unserved = logs.expect_one(target::APP, UNSERVED);
    assert_eq!(
        unserved.field("import").as_deref(),
        Some("FirstEdgeModule, SecondEdgeRuntimeModule")
    );
}

/// One edge runs, and following the line above ends here: the other is still
/// unserved.
struct ImportsTheFirstEdgeModule;

impl Module for ImportsTheFirstEdgeModule {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder.import::<FirstEdgeModule>()
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.import::<FirstEdgeModule>()
    }
}

#[tokio::test]
async fn a_host_on_two_edges_with_one_imported_names_the_other() {
    let logs = logs_of_a_run(
        App::builder()
            .provide(OnTwoEdges)
            .module::<ImportsTheFirstEdgeModule>(),
    )
    .await;
    let unserved = logs.expect_one(target::APP, UNSERVED);
    assert_eq!(
        unserved.field("import").as_deref(),
        Some("SecondEdgeRuntimeModule")
    );
}

/// What `App::run` files for an instance of a host its edge mounts from a
/// registration.
const INSTANCE: &str = "host not served: its edge does not mount an instance";

/// A host as a controller is one: its registration attaches what the edge mounts,
/// and the container holds nothing under its type.
#[derive(Default)]
struct AttachingHost;

struct WhatTheEdgeMounts;

impl Discoverable for AttachingHost {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<AttachingHost, WhatTheEdgeMounts>(WhatTheEdgeMounts)
    }
}

inventory::submit! {
    HostRuntime {
        host: || TypeId::of::<AttachingHost>(),
        name: std::any::type_name::<AttachingHost>,
        mount: HostMount::Attached,
        modules: EDGE_RUNTIME,
    }
}

struct RegistersTheAttachingHost;

impl Module for RegistersTheAttachingHost {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        <AttachingHost as Discoverable>::register(builder)
    }
}

struct RegistersItBesideItsRuntime;

impl Module for RegistersItBesideItsRuntime {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder.import::<EdgeRuntimeModule>()
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        <AttachingHost as Discoverable>::register(builder.import::<EdgeRuntimeModule>())
    }
}

struct ImportsTheRuntimeAlone;

impl Module for ImportsTheRuntimeAlone {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder.import::<EdgeRuntimeModule>()
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.import::<EdgeRuntimeModule>()
    }
}

#[tokio::test]
async fn a_host_whose_registration_attached_what_its_edge_mounts_is_named_at_run() {
    let logs = logs_of_a_run(App::builder().module::<RegistersTheAttachingHost>()).await;
    let unserved = logs.expect_one(target::APP, UNSERVED);
    assert_eq!(unserved.field("host").as_deref(), Some("AttachingHost"));
    assert_eq!(
        unserved.field("import").as_deref(),
        Some("EdgeRuntimeModule")
    );
    logs.expect_none(target::APP, INSTANCE);
}

#[tokio::test]
async fn a_host_registered_beside_its_edge_is_not_named() {
    let logs = logs_of_a_run(App::builder().module::<RegistersItBesideItsRuntime>()).await;
    logs.expect_none(target::APP, UNSERVED);
    logs.expect_none(target::APP, INSTANCE);
}

/// An instance on such an edge serves nothing even with the runtime imported,
/// so the import is not what the line tells.
#[tokio::test]
async fn an_instance_on_an_edge_mounting_none_is_told_so_whatever_is_imported() {
    for app in [
        App::builder().provide(AttachingHost),
        App::builder()
            .provide(AttachingHost)
            .module::<ImportsTheRuntimeAlone>(),
    ] {
        let logs = logs_of_a_run(app).await;
        let instance = logs.expect_one(target::APP, INSTANCE);
        assert_eq!(instance.level, "warn");
        assert_eq!(instance.field("host").as_deref(), Some("AttachingHost"));
        assert!(
            instance
                .field("hint")
                .is_some_and(|hint| hint.contains("`providers`")),
            "{instance:?}"
        );
        logs.expect_none(target::APP, UNSERVED);
    }
}

/// Registered too, so what the edge mounts is there, and the instance is beside
/// the point.
#[tokio::test]
async fn an_instance_beside_a_registration_is_judged_as_the_registration() {
    let logs = logs_of_a_run(
        App::builder()
            .provide(AttachingHost)
            .module::<RegistersTheAttachingHost>(),
    )
    .await;
    logs.expect_one(target::APP, UNSERVED);
    logs.expect_none(target::APP, INSTANCE);
}

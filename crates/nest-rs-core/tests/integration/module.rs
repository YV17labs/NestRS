//! Covers `src/module.rs` — a dynamic import (`Foo::for_root(opts)`) is
//! evaluated exactly once, and the value collected is the value registered —
//! and the register phase `#[module]` runs: a provider it lists is built, or
//! the boot refuses naming why.

use std::any::TypeId;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{
    AccessGraphError, App, Collecting, ContainerBuilder, DynamicModule, Instance, Instanced,
    LateFactoryError, LateOptionalDependencyError, Module, ProviderCycleError, Registering,
    UnbuiltCause, UnbuiltProviderError, injectable, instance, module,
};

use crate::refused_on_both_paths;

static BUILDS: AtomicUsize = AtomicUsize::new(0);

struct Installed(usize);

struct Collected(usize);

struct CountingSetup {
    serial: usize,
}

fn for_root() -> CountingSetup {
    CountingSetup {
        serial: BUILDS.fetch_add(1, Ordering::SeqCst),
    }
}

impl DynamicModule for CountingSetup {
    fn module() -> TypeId {
        TypeId::of::<Self>()
    }

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide(Installed(self.serial))
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder.provide(Collected(self.serial))
    }
}

#[module(imports = [for_root()])]
struct CountingModule;

#[tokio::test]
async fn a_dynamic_import_is_evaluated_once_on_the_async_path() {
    BUILDS.store(0, Ordering::SeqCst);

    let app = App::builder()
        .module::<CountingModule>()
        .build()
        .await
        .expect("the module boots");

    assert_eq!(
        BUILDS.load(Ordering::SeqCst),
        1,
        "collect + register must share one construction of the import expression",
    );
    let collected: Arc<Collected> = app.container().get().expect("collect ran");
    let installed: Arc<Installed> = app.container().get().expect("register ran");
    assert_eq!(collected.0, installed.0);
}

struct SyncSetup;

impl DynamicModule for SyncSetup {
    fn module() -> TypeId {
        TypeId::of::<Self>()
    }

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide(Installed(BUILDS.fetch_add(1, Ordering::SeqCst)))
    }
}

fn sync_for_root() -> SyncSetup {
    BUILDS.fetch_add(1, Ordering::SeqCst);
    SyncSetup
}

#[module(imports = [sync_for_root()])]
struct SyncModule;

#[test]
fn a_dynamic_import_is_evaluated_once_on_the_sync_path() {
    BUILDS.store(0, Ordering::SeqCst);

    let app = App::new::<SyncModule>().expect("the module boots");

    assert!(
        app.container().get::<Installed>().is_some(),
        "the synchronous path registers the dynamic module",
    );
    assert_eq!(
        BUILDS.load(Ordering::SeqCst),
        2,
        "one construction (+1) then one register (+1) — never a second construction",
    );
}

struct Tagged(&'static str);

#[instance]
struct First;

#[instance]
struct Second;

/// One setup type imported twice, each value holding its tag under its own
/// instance.
struct TaggingSetup {
    tag: &'static str,
    install: fn(ContainerBuilder, Tagged) -> ContainerBuilder,
}

impl TaggingSetup {
    fn of<I: Instance>(tag: &'static str) -> Self {
        Self {
            tag,
            install: |builder, tagged| builder.provide(Instanced::<I, _>::from(tagged)),
        }
    }
}

impl DynamicModule for TaggingSetup {
    fn module() -> TypeId {
        TypeId::of::<Self>()
    }

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        (self.install)(builder, Tagged(self.tag))
    }
}

#[module(imports = [TaggingSetup::of::<First>("first"), TaggingSetup::of::<Second>("second")])]
struct TwoDynamicImportsModule;

#[tokio::test]
async fn two_dynamic_imports_in_one_module_keep_their_own_values() {
    let app = App::builder()
        .module::<TwoDynamicImportsModule>()
        .build()
        .await
        .expect("the module boots");

    let first = app.container().get::<Instanced<First, Tagged>>();
    let second = app.container().get::<Instanced<Second, Tagged>>();
    assert_eq!(first.expect("the first import registered").0, "first");
    assert_eq!(second.expect("the second import registered").0, "second");
}

struct Mixed(usize);

struct MixedSetup;

impl DynamicModule for MixedSetup {
    fn module() -> TypeId {
        TypeId::of::<Self>()
    }

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide(Mixed(7))
    }
}

#[module]
struct StaticLeaf;

#[module(imports = [StaticLeaf, MixedSetup {}, StaticLeaf])]
struct MixedImportsModule;

#[tokio::test]
async fn a_dynamic_import_after_a_static_one_still_resolves_its_site() {
    let app = App::builder()
        .module::<MixedImportsModule>()
        .build()
        .await
        .expect("the module boots");

    assert_eq!(
        app.container()
            .get::<Mixed>()
            .expect("dynamic import ran")
            .0,
        7,
        "a mismatched collect/register index would leave the value parked",
    );
}

struct Opened;

struct OpensModule;

impl Module for OpensModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }

    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder.provide_factory(|_| async { Ok(Opened) })
    }
}

#[module(imports = [OpensModule])]
struct OpeningModule;

struct UncollectingSetup;

impl DynamicModule for UncollectingSetup {
    fn module() -> TypeId {
        TypeId::of::<OpeningModule>()
    }

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.import::<OpeningModule>()
    }
}

#[module(imports = [UncollectingSetup {}])]
struct UncollectedModule;

#[tokio::test]
async fn a_module_imported_in_a_register_alone_fails_the_boot_naming_what_it_opens() {
    let Err(refused) = App::builder().module::<UncollectedModule>().build().await else {
        panic!("the factory its import queues would never run");
    };
    let late = refused
        .downcast_ref::<LateFactoryError>()
        .unwrap_or_else(|| panic!("not the late-factory refusal: {refused:#}"));
    assert!(late.type_name.ends_with("Opened"), "{late:?}");
}

#[injectable]
#[derive(Default)]
struct CrossingAService;

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct CrossingBService {
    #[inject]
    a: Arc<CrossingAService>,
}

#[module(imports = [CrossingBModule], providers = [CrossingAService])]
struct CrossingAModule;

#[module(imports = [CrossingAModule], providers = [CrossingBService])]
struct CrossingBModule;

#[tokio::test]
async fn a_provider_crossing_an_import_cycle_fails_the_boot_naming_the_cycle() {
    for refused in refused_on_both_paths::<CrossingAModule>().await {
        let unbuilt = refused
            .downcast_ref::<UnbuiltProviderError>()
            .unwrap_or_else(|| panic!("not the unbuilt-provider refusal: {refused:#}"));
        assert_eq!(unbuilt.module, "CrossingBModule");
        assert_eq!(unbuilt.provider, "CrossingBService");
        assert_eq!(unbuilt.missing, ["CrossingAService"]);
        let UnbuiltCause::ImportCycle(modules) = &unbuilt.cause else {
            panic!("not an import cycle: {unbuilt}");
        };
        assert_eq!(modules.len(), 2, "{unbuilt}");
        for module in ["CrossingAModule", "CrossingBModule"] {
            assert!(modules.contains(&module), "{unbuilt}");
        }
        assert!(
            unbuilt
                .to_string()
                .contains("move what both need into a module both import"),
            "{unbuilt}"
        );
    }
}

#[injectable]
#[derive(Default)]
struct Atlas;

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct Cartographer {
    #[inject]
    atlas: Arc<Atlas>,
}

#[module(imports = [HubModule], providers = [Cartographer])]
struct SurveyModule;

#[module(imports = [AtlasModule, SurveyModule])]
struct HubModule;

#[module(imports = [HubModule], providers = [Atlas])]
struct AtlasModule;

#[tokio::test]
async fn an_import_cycle_through_a_module_reached_from_both_sides_names_each_module_once() {
    for refused in refused_on_both_paths::<AtlasModule>().await {
        let unbuilt = refused
            .downcast_ref::<UnbuiltProviderError>()
            .unwrap_or_else(|| panic!("not the unbuilt-provider refusal: {refused:#}"));
        assert_eq!(
            unbuilt.cause,
            UnbuiltCause::ImportCycle(vec!["SurveyModule", "HubModule", "AtlasModule"]),
            "{unbuilt}"
        );
    }
}

#[injectable]
#[derive(Default)]
struct QuietAService;

#[injectable]
#[derive(Default)]
struct QuietBService;

#[module(imports = [QuietBModule], providers = [QuietAService])]
struct QuietAModule;

#[module(imports = [QuietAModule], providers = [QuietBService])]
struct QuietBModule;

#[tokio::test]
async fn an_import_cycle_no_provider_crosses_boots() {
    let app = App::new::<QuietAModule>().expect("no provider crosses the cycle");
    assert!(app.container().get::<QuietBService>().is_some());
    let app = App::builder()
        .module::<QuietAModule>()
        .build()
        .await
        .expect("no provider crosses the cycle");
    assert!(app.container().get::<QuietAService>().is_some());
}

/// A type only a hand-written module registers, imperatively.
struct Ledger;

struct LedgerModule;

impl Module for LedgerModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide(Ledger)
    }
}

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct Bookkeeper {
    #[inject]
    ledger: Arc<Ledger>,
}

#[module(providers = [Bookkeeper])]
struct BookkeepingModule;

#[module(imports = [BookkeepingModule, LedgerModule])]
struct LedgerAfterRoot;

#[tokio::test]
async fn a_provider_injecting_what_a_hand_written_module_registers_later_fails_the_boot() {
    for refused in refused_on_both_paths::<LedgerAfterRoot>().await {
        let unbuilt = refused
            .downcast_ref::<UnbuiltProviderError>()
            .unwrap_or_else(|| panic!("not the unbuilt-provider refusal: {refused:#}"));
        assert_eq!(unbuilt.module, "BookkeepingModule");
        assert_eq!(unbuilt.provider, "Bookkeeper");
        assert_eq!(unbuilt.missing, ["Ledger"]);
        assert_eq!(unbuilt.cause, UnbuiltCause::RegisteredLater, "{unbuilt}");
        assert!(
            unbuilt
                .to_string()
                .contains("import that module in `BookkeepingModule`'s `imports`"),
            "{unbuilt}"
        );
    }
}

#[module(imports = [LedgerModule], providers = [Bookkeeper])]
struct ImportingBookkeepingModule;

#[module(imports = [ImportingBookkeepingModule, LedgerModule])]
struct LedgerImportedRoot;

#[tokio::test]
async fn a_module_importing_the_hand_written_one_builds_its_provider() {
    let app = App::new::<LedgerImportedRoot>().expect("the import registers it first");
    assert!(app.container().get::<Bookkeeper>().is_some());
}

#[module(imports = [LedgerModule, BookkeepingModule])]
struct LedgerFirstRoot;

#[tokio::test]
async fn what_a_hand_written_module_registers_is_outside_the_access_graph() {
    let app = App::new::<LedgerFirstRoot>().expect("registered first, the ledger is reached");
    assert!(app.container().get::<Bookkeeper>().is_some());
    let app = App::builder()
        .module::<LedgerFirstRoot>()
        .build()
        .await
        .expect("registered first, the ledger is reached");
    assert!(app.container().get::<Bookkeeper>().is_some());
}

#[injectable]
#[derive(Default)]
struct Archive;

#[module(providers = [Archive])]
struct ArchiveModule;

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct Archivist {
    #[inject]
    archive: Arc<Archive>,
}

#[module(providers = [Archivist])]
struct ArchivistModule;

#[module(imports = [BookkeepingModule, LedgerModule, ArchiveModule, ArchivistModule])]
struct StallAndLeakRoot;

#[tokio::test]
async fn an_access_violation_is_refused_before_a_provider_never_built() {
    for refused in refused_on_both_paths::<StallAndLeakRoot>().await {
        let leak = refused
            .downcast_ref::<AccessGraphError>()
            .unwrap_or_else(|| panic!("not the access violation: {refused:#}"));
        assert_eq!(leak.consumer, "Archivist");
        assert_eq!(leak.owner, "ArchiveModule");
    }
}

#[tokio::test]
async fn a_test_override_of_a_provider_never_built_stands_in_for_it() {
    let app = App::builder()
        .module::<LedgerAfterRoot>()
        .override_value(Bookkeeper {
            ledger: Arc::new(Ledger),
        })
        .build()
        .await
        .expect("the override is the provider the test asked for");
    assert!(app.container().get::<Bookkeeper>().is_some());
}

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct Drafts {
    #[inject]
    reviews: Arc<Reviews>,
}

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct Reviews {
    #[inject]
    drafts: Arc<Drafts>,
}

#[module(providers = [Drafts, Reviews, Bookkeeper])]
struct CycleBesideAStallModule;

#[module(imports = [CycleBesideAStallModule, LedgerModule])]
struct CycleBesideAStallRoot;

#[tokio::test]
async fn a_provider_cycle_is_refused_beside_a_provider_waiting_outside_its_module() {
    for refused in refused_on_both_paths::<CycleBesideAStallRoot>().await {
        let cycle = refused
            .downcast_ref::<ProviderCycleError>()
            .unwrap_or_else(|| panic!("not the cycle refusal: {refused:#}"));
        assert_eq!(cycle.module, "CycleBesideAStallModule");
        assert_eq!(cycle.chain, ["Drafts", "Reviews", "Drafts"]);
    }
}

/// A type only a hand-written module registers, which a provider reads when
/// present.
struct Clock;

struct ClockModule;

impl Module for ClockModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide(Clock)
    }
}

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct Scheduler {
    #[inject]
    clock: Option<Arc<Clock>>,
}

#[module(providers = [Scheduler])]
struct SchedulerModule;

#[module(imports = [SchedulerModule, ClockModule])]
struct ClockAfterRoot;

#[tokio::test]
async fn a_provider_built_before_its_optional_dependency_registers_fails_the_boot() {
    for refused in refused_on_both_paths::<ClockAfterRoot>().await {
        let late = refused
            .downcast_ref::<LateOptionalDependencyError>()
            .unwrap_or_else(|| panic!("not the late optional refusal: {refused:#}"));
        assert_eq!(late.module, "SchedulerModule");
        assert_eq!(late.provider, "Scheduler");
        assert_eq!(late.dependency, "Clock");
        assert_eq!(late.cause, UnbuiltCause::RegisteredLater, "{late}");
    }
}

#[injectable]
#[derive(Default)]
struct Glossary;

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct SpellChecker {
    #[inject]
    glossary: Option<Arc<Glossary>>,
}

#[module(imports = [SpellCheckModule], providers = [Glossary])]
struct GlossaryModule;

#[module(imports = [GlossaryModule], providers = [SpellChecker])]
struct SpellCheckModule;

#[tokio::test]
async fn a_provider_built_across_an_import_cycle_without_its_optional_fails_the_boot() {
    for refused in refused_on_both_paths::<GlossaryModule>().await {
        let late = refused
            .downcast_ref::<LateOptionalDependencyError>()
            .unwrap_or_else(|| panic!("not the late optional refusal: {refused:#}"));
        assert_eq!(late.provider, "SpellChecker");
        assert_eq!(late.dependency, "Glossary");
        let UnbuiltCause::ImportCycle(modules) = &late.cause else {
            panic!("not an import cycle: {late}");
        };
        for module in ["GlossaryModule", "SpellCheckModule"] {
            assert!(modules.contains(&module), "{late}");
        }
    }
}

#[injectable]
#[derive(Default)]
struct Outbox;

#[injectable]
struct Dispatcher {
    #[inject]
    outbox: Option<Arc<Outbox>>,
}

#[module(providers = [Outbox])]
struct OutboxModule;

#[module(imports = [OutboxModule], providers = [Dispatcher])]
struct DispatcherModule;

#[module(imports = [DispatcherModule, OutboxModule])]
struct DispatcherFirstRoot;

#[module(imports = [OutboxModule, DispatcherModule])]
struct OutboxFirstRoot;

#[tokio::test]
async fn an_optional_dependency_its_module_imports_is_held_whatever_the_roots_order() {
    for app in [
        App::new::<DispatcherFirstRoot>().expect("the import registers it first"),
        App::new::<OutboxFirstRoot>().expect("the import registers it first"),
        App::builder()
            .module::<DispatcherFirstRoot>()
            .build()
            .await
            .expect("the import registers it first"),
    ] {
        let dispatcher = app.container().get::<Dispatcher>().expect("built");
        assert!(dispatcher.outbox.is_some());
    }
}

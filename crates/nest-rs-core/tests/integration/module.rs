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

use crate::refused_boot;

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
async fn a_dynamic_import_is_evaluated_once_for_both_phases() {
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

struct RegisterOnlySetup;

impl DynamicModule for RegisterOnlySetup {
    fn module() -> TypeId {
        TypeId::of::<Self>()
    }

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide(Installed(BUILDS.fetch_add(1, Ordering::SeqCst)))
    }
}

fn register_only_for_root() -> RegisterOnlySetup {
    BUILDS.fetch_add(1, Ordering::SeqCst);
    RegisterOnlySetup
}

#[module(imports = [register_only_for_root()])]
struct RegisterOnlyModule;

#[tokio::test]
async fn a_dynamic_import_with_no_collect_is_evaluated_once() {
    BUILDS.store(0, Ordering::SeqCst);

    let app = App::builder()
        .module::<RegisterOnlyModule>()
        .build()
        .await
        .expect("the module boots");

    assert!(
        app.container().get::<Installed>().is_some(),
        "the boot registers the dynamic module",
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
    let refused = refused_boot::<CrossingAModule>().await;
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
    let refused = refused_boot::<AtlasModule>().await;
    let unbuilt = refused
        .downcast_ref::<UnbuiltProviderError>()
        .unwrap_or_else(|| panic!("not the unbuilt-provider refusal: {refused:#}"));
    assert_eq!(
        unbuilt.cause,
        UnbuiltCause::ImportCycle(vec!["SurveyModule", "HubModule", "AtlasModule"]),
        "{unbuilt}"
    );
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
    let app = App::builder()
        .module::<QuietAModule>()
        .build()
        .await
        .expect("no provider crosses the cycle");
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
    let refused = refused_boot::<LedgerAfterRoot>().await;
    let unbuilt = refused
        .downcast_ref::<UnbuiltProviderError>()
        .unwrap_or_else(|| panic!("not the unbuilt-provider refusal: {refused:#}"));
    assert_eq!(unbuilt.module, "BookkeepingModule");
    assert_eq!(unbuilt.provider, "Bookkeeper");
    assert_eq!(unbuilt.missing, ["Ledger"]);
    assert_eq!(
        unbuilt.cause,
        UnbuiltCause::RegisteredLater(vec!["LedgerModule"]),
        "{unbuilt}"
    );
    assert!(
        unbuilt
            .to_string()
            .contains("`LedgerModule` registers it later: import that module in `BookkeepingModule`'s `imports`"),
        "{unbuilt}"
    );
}

/// What a port's crate names its own module for, and an app's hand-written
/// module registers instead.
struct Almanac;

const ALMANAC_REMEDY: &str = "Import `AlmanacPortModule`, which binds the default.";

nest_rs_core::inventory::submit! {
    nest_rs_core::DependencyRemedy::new(std::any::TypeId::of::<Almanac>, ALMANAC_REMEDY)
}

struct OwnAlmanacModule;

impl Module for OwnAlmanacModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide(Almanac)
    }
}

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct AlmanacReader {
    #[inject]
    almanac: Arc<Almanac>,
}

#[module(providers = [AlmanacReader])]
struct AlmanacReaderModule;

#[module(imports = [AlmanacReaderModule, OwnAlmanacModule])]
struct OwnAlmanacAfterRoot;

/// The port would register the key a second time: the module registering it is
/// the fix, whatever the key's crate names.
#[tokio::test]
async fn a_dependency_registered_later_names_the_module_registering_it_over_the_crates_line() {
    let said = refused_boot::<OwnAlmanacAfterRoot>().await.to_string();
    assert!(
        said.contains("`OwnAlmanacModule` registers it later"),
        "{said}"
    );
    assert!(!said.contains(ALMANAC_REMEDY), "the wrong fix: {said}");
}

/// A root written by hand, importing the consumer's module before the one
/// registering what it injects.
struct HandWrittenRoot;

impl Module for HandWrittenRoot {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder
            .import::<BookkeepingModule>()
            .import::<LedgerModule>()
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
            .import::<BookkeepingModule>()
            .import::<LedgerModule>()
    }
}

/// The module whose phase registered the key is named, never the root that
/// imported it, which the consumer's module cannot import.
#[tokio::test]
async fn a_dependency_a_hand_written_root_imports_later_names_the_module_registering_it() {
    let refused = refused_boot::<HandWrittenRoot>().await;
    let unbuilt = refused
        .downcast_ref::<UnbuiltProviderError>()
        .unwrap_or_else(|| panic!("not the unbuilt-provider refusal: {refused:#}"));
    assert_eq!(
        unbuilt.cause,
        UnbuiltCause::RegisteredLater(vec!["LedgerModule"]),
        "{unbuilt}"
    );
}

/// A second root registering it is named by its type's name, and importing it
/// is the fix.
#[tokio::test]
async fn a_dependency_a_later_root_registers_names_that_root() {
    let Err(refused) = App::builder()
        .module::<BookkeepingModule>()
        .module::<LedgerModule>()
        .build()
        .await
    else {
        panic!("the ledger registers after its reader");
    };
    let unbuilt = refused
        .downcast_ref::<UnbuiltProviderError>()
        .unwrap_or_else(|| panic!("not the unbuilt-provider refusal: {refused:#}"));
    assert_eq!(
        unbuilt.cause,
        UnbuiltCause::RegisteredLater(vec!["LedgerModule"]),
        "{unbuilt}"
    );
}

/// A root written by hand registering what an import it made before injects.
struct RegistersAfterItsImport;

impl Module for RegistersAfterItsImport {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder.import::<BookkeepingModule>()
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.import::<BookkeepingModule>().provide(Ledger)
    }
}

/// No module to import registers it, so the edit is the root's own order.
#[tokio::test]
async fn a_dependency_a_hand_written_root_registers_after_its_import_is_told_the_order() {
    let refused = refused_boot::<RegistersAfterItsImport>().await;
    let unbuilt = refused
        .downcast_ref::<UnbuiltProviderError>()
        .unwrap_or_else(|| panic!("not the unbuilt-provider refusal: {refused:#}"));
    assert_eq!(
        unbuilt.cause,
        UnbuiltCause::RegisteredByARoot {
            root: "RegistersAfterItsImport",
            imports: vec![],
        },
        "{unbuilt}"
    );
    assert!(
        unbuilt.to_string().ends_with(
            "; the root `RegistersAfterItsImport` registers it after `BookkeepingModule`'s \
             providers were built: register it before importing `BookkeepingModule`, or in a \
             module `BookkeepingModule` imports"
        ),
        "{unbuilt}"
    );
}

/// That sentence followed: the root registers before importing.
struct RegistersBeforeItsImport;

impl Module for RegistersBeforeItsImport {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder.import::<BookkeepingModule>()
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide(Ledger).import::<BookkeepingModule>()
    }
}

#[tokio::test]
async fn following_the_roots_order_builds_the_provider() {
    let app = App::builder()
        .module::<RegistersBeforeItsImport>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("the order is the fix: {refused:#}"));
    assert!(app.container().get::<Bookkeeper>().is_some());
}

/// A second type only a root registers, imperatively.
struct Journal;

#[expect(
    dead_code,
    reason = "the dependencies are declared for the container to resolve, never read"
)]
#[injectable]
struct JournalKeeper {
    #[inject]
    ledger: Arc<Ledger>,
    #[inject]
    journal: Arc<Journal>,
}

#[module(providers = [JournalKeeper])]
struct JournalKeepingModule;

/// A root importing the consumer's module, then the ledger's, then registering
/// the journal itself: each of the two absent keys has its own edit.
struct RegistersTheJournalAfterItsImport;

impl Module for RegistersTheJournalAfterItsImport {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder
            .import::<JournalKeepingModule>()
            .import::<LedgerModule>()
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
            .import::<JournalKeepingModule>()
            .import::<LedgerModule>()
            .provide(Journal)
    }
}

/// The root's order is said beside the import registering the other key, never
/// in its place.
#[tokio::test]
async fn a_root_registering_one_of_two_later_names_the_import_registering_the_other() {
    let refused = refused_boot::<RegistersTheJournalAfterItsImport>().await;
    let unbuilt = refused
        .downcast_ref::<UnbuiltProviderError>()
        .unwrap_or_else(|| panic!("not the unbuilt-provider refusal: {refused:#}"));
    assert_eq!(unbuilt.missing, ["Ledger", "Journal"]);
    assert_eq!(
        unbuilt.cause,
        UnbuiltCause::RegisteredByARoot {
            root: "RegistersTheJournalAfterItsImport",
            imports: vec!["LedgerModule"],
        },
        "{unbuilt}"
    );
    assert!(
        unbuilt.to_string().ends_with(
            "; `LedgerModule` registers some of them later, and the root \
             `RegistersTheJournalAfterItsImport` the others after `JournalKeepingModule`'s \
             providers were built: import that module in `JournalKeepingModule`'s `imports`, \
             and register the others before importing `JournalKeepingModule`, or in a module \
             `JournalKeepingModule` imports"
        ),
        "{unbuilt}"
    );
}

#[module(imports = [LedgerModule], providers = [JournalKeeper])]
struct JournalKeepingOverTheLedger;

/// Both edits followed: the module imports the ledger's, and the root registers
/// the journal before importing it.
struct RegistersTheJournalFirst;

impl Module for RegistersTheJournalFirst {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder.import::<JournalKeepingOverTheLedger>()
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
            .provide(Journal)
            .import::<JournalKeepingOverTheLedger>()
    }
}

#[tokio::test]
async fn following_both_edits_builds_the_provider() {
    let app = App::builder()
        .module::<RegistersTheJournalFirst>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("both edits are the fix: {refused:#}"));
    assert!(app.container().get::<JournalKeeper>().is_some());
}

/// A module written by hand that imports the registrant on the app's behalf.
struct LedgerWiring;

impl Module for LedgerWiring {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder.import::<LedgerModule>()
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.import::<LedgerModule>()
    }
}

#[module(imports = [BookkeepingModule, LedgerWiring])]
struct WiredLaterRoot;

/// The import is named as the app writes it, the hand-written module whose phase
/// imported the registrant, and importing it is the fix.
#[tokio::test]
async fn a_dependency_a_hand_written_module_imports_later_names_that_module() {
    let refused = refused_boot::<WiredLaterRoot>().await;
    let unbuilt = refused
        .downcast_ref::<UnbuiltProviderError>()
        .unwrap_or_else(|| panic!("not the unbuilt-provider refusal: {refused:#}"));
    assert_eq!(
        unbuilt.cause,
        UnbuiltCause::RegisteredLater(vec!["LedgerWiring"]),
        "{unbuilt}"
    );
}

#[module(imports = [LedgerWiring], providers = [Bookkeeper])]
struct BookkeepingOverTheWiring;

#[module(imports = [BookkeepingOverTheWiring, LedgerWiring])]
struct WiredFirstRoot;

#[tokio::test]
async fn following_that_sentence_builds_the_provider() {
    let app = App::builder()
        .module::<WiredFirstRoot>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("the import is the fix: {refused:#}"));
    assert!(app.container().get::<Bookkeeper>().is_some());
}

#[module(imports = [LedgerModule], providers = [Bookkeeper])]
struct ImportingBookkeepingModule;

#[module(imports = [ImportingBookkeepingModule, LedgerModule])]
struct LedgerImportedRoot;

#[tokio::test]
async fn a_module_importing_the_hand_written_one_builds_its_provider() {
    let app = App::builder()
        .module::<LedgerImportedRoot>()
        .build()
        .await
        .expect("the import registers it first");
    assert!(app.container().get::<Bookkeeper>().is_some());
}

#[module(imports = [LedgerModule, BookkeepingModule])]
struct LedgerFirstRoot;

#[tokio::test]
async fn what_a_hand_written_module_registers_is_outside_the_access_graph() {
    let app = App::builder()
        .module::<LedgerFirstRoot>()
        .build()
        .await
        .expect("registered first, the ledger is reached");
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
    let refused = refused_boot::<StallAndLeakRoot>().await;
    let leak = refused
        .downcast_ref::<AccessGraphError>()
        .unwrap_or_else(|| panic!("not the access violation: {refused:#}"));
    assert_eq!(leak.consumer, "Archivist");
    assert_eq!(leak.owner, "ArchiveModule");
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
    let refused = refused_boot::<CycleBesideAStallRoot>().await;
    let cycle = refused
        .downcast_ref::<ProviderCycleError>()
        .unwrap_or_else(|| panic!("not the cycle refusal: {refused:#}"));
    assert_eq!(cycle.module, "CycleBesideAStallModule");
    assert_eq!(cycle.chain, ["Drafts", "Reviews", "Drafts"]);
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
    let refused = refused_boot::<ClockAfterRoot>().await;
    let late = refused
        .downcast_ref::<LateOptionalDependencyError>()
        .unwrap_or_else(|| panic!("not the late optional refusal: {refused:#}"));
    assert_eq!(late.module, "SchedulerModule");
    assert_eq!(late.provider, "Scheduler");
    assert_eq!(late.dependency, "Clock");
    assert_eq!(
        late.cause,
        UnbuiltCause::RegisteredLater(vec!["ClockModule"]),
        "{late}"
    );
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
    let refused = refused_boot::<GlossaryModule>().await;
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
        App::builder()
            .module::<DispatcherFirstRoot>()
            .build()
            .await
            .expect("the import registers it first"),
        App::builder()
            .module::<OutboxFirstRoot>()
            .build()
            .await
            .expect("the import registers it first"),
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

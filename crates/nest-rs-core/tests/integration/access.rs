//! Covers `src/access.rs` through the real macros and boot path. The link-time
//! registry is shared across a test binary, so the graphs below use disjoint types.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::{
    AccessGraphError, App, ContainerBuilder, Discoverable, DynamicModule, ProviderCycleError,
    ProviderScope, ReachableProviders, Registering, ScopeViolationError, injectable, module,
};

use crate::refused_on_both_paths;

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
        .downcast_ref::<ProviderCycleError>()
        .unwrap_or_else(|| panic!("not the cycle refusal: {refused:#}"));
    assert_eq!(cycle.module, "CycleModule");
    assert_eq!(cycle.scope, ProviderScope::Singleton);
    assert_eq!(cycle.chain, ["CycleLeft", "CycleRight", "CycleLeft"]);
}

trait Ledger: Send + Sync {}

#[injectable]
#[derive(Default)]
struct BoundLedger;

impl Ledger for BoundLedger {}

#[module(providers = [BoundLedger as dyn Ledger])]
struct BoundLedgerModule;

#[test]
fn a_provider_bound_as_dyn_is_reachable_under_its_own_type() {
    let app = App::new::<BoundLedgerModule>().expect("the module boots");
    let reachable = app
        .container()
        .get::<ReachableProviders>()
        .expect("the boot seeds the reachable set");
    assert!(
        ReachableProviders::reaches(Some(&reachable), TypeId::of::<BoundLedger>()),
        "a listener, processor, tick or indicator on the host is gated by its own type"
    );
}

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct StrayLedgerReader {
    #[inject]
    ledger: Arc<BoundLedger>,
}

#[module(providers = [StrayLedgerReader])]
struct StrayLedgerReaderModule;

#[module(imports = [BoundLedgerModule, StrayLedgerReaderModule])]
struct StrayLedgerRoot;

#[test]
fn a_consumer_of_a_dyn_bound_provider_under_its_own_type_imports_its_module() {
    let Err(refused) = App::new::<StrayLedgerRoot>() else {
        panic!("a consumer reaching a provider its module does not import must fail the boot");
    };
    let refused = refused
        .downcast::<AccessGraphError>()
        .unwrap_or_else(|refused| panic!("not the access violation: {refused:#}"));
    assert_eq!(refused.consumer, "StrayLedgerReader");
    assert_eq!(refused.owner, "BoundLedgerModule", "{refused}");
}

#[injectable(scope = request)]
#[derive(Default)]
struct PerRequestAudit;

#[injectable]
#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
struct SingletonMaybeAuditing {
    #[inject]
    audit: Option<Arc<PerRequestAudit>>,
}

#[module(providers = [PerRequestAudit, SingletonMaybeAuditing])]
struct OptionalScopeViolationModule;

#[test]
fn a_singleton_holding_a_request_scoped_provider_optionally_fails_the_boot_by_name() {
    let Err(refused) = App::new::<OptionalScopeViolationModule>() else {
        panic!("a singleton's optional request-scoped dependency would be `None` forever");
    };
    let violation = refused
        .downcast_ref::<ScopeViolationError>()
        .unwrap_or_else(|| panic!("not the scope violation: {refused:#}"));
    assert_eq!(violation.consumer, "SingletonMaybeAuditing");
    assert_eq!(violation.dependency, "PerRequestAudit");
}

#[injectable]
#[derive(Default)]
struct Mailer;

#[module(providers = [Mailer])]
struct MailerModule;

#[injectable]
struct Notifier {
    #[inject]
    mailer: Option<Arc<Mailer>>,
}

#[module(providers = [Notifier])]
struct NotifierModule;

#[module(imports = [MailerModule, NotifierModule])]
struct OptionalLeakRoot;

#[test]
fn an_optional_dependency_another_reached_module_provides_needs_its_import() {
    let Err(refused) = App::new::<OptionalLeakRoot>() else {
        panic!("the app reaches `Mailer`, so `Notifier` would hold it or not by import order");
    };
    let leak = refused
        .downcast::<AccessGraphError>()
        .unwrap_or_else(|refused| panic!("not the access violation: {refused:#}"));
    assert_eq!(leak.consumer, "Notifier");
    assert_eq!(leak.owner, "MailerModule");
}

#[module(imports = [NotifierModule])]
struct NoMailerRoot;

#[test]
fn an_optional_dependency_no_reached_module_provides_is_absent() {
    let app = App::new::<NoMailerRoot>()
        .expect("`MailerModule` is linked but another app's: the capability is absent here");
    let notifier = app.container().get::<Notifier>().expect("built without it");
    assert!(notifier.mailer.is_none());
}

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable(scope = request)]
struct DraftView {
    #[inject]
    review: Arc<ReviewView>,
}

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable(scope = request)]
struct ReviewView {
    #[inject]
    draft: Arc<DraftView>,
}

#[module(providers = [DraftView, ReviewView])]
struct RequestCycleModule;

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable(scope = transient)]
struct SlugDraft {
    #[inject]
    slug: Arc<SlugCheck>,
}

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable(scope = transient)]
struct SlugCheck {
    #[inject]
    draft: Option<Arc<SlugDraft>>,
}

#[module(providers = [SlugDraft, SlugCheck])]
struct TransientCycleModule;

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable(scope = request)]
struct Visit {
    #[inject]
    tracker: Arc<VisitTracker>,
}

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable(scope = transient)]
struct VisitTracker {
    #[inject]
    visit: Arc<Visit>,
}

#[module(providers = [Visit, VisitTracker])]
struct MixedScopeCycleModule;

#[module(imports = [MixedScopeCycleModule])]
struct MixedScopeCycleRoot;

/// The cycle booting `M` fails with, on both paths, each the same.
async fn cycle_of<M: nest_rs_core::Module>() -> ProviderCycleError {
    let [sync, built] = refused_on_both_paths::<M>().await.map(|refused| {
        refused
            .downcast::<ProviderCycleError>()
            .unwrap_or_else(|refused| panic!("not the cycle refusal: {refused:#}"))
    });
    assert_eq!(sync.to_string(), built.to_string());
    sync
}

#[tokio::test]
async fn request_scoped_providers_waiting_on_each_other_fail_the_boot() {
    let cycle = cycle_of::<RequestCycleModule>().await;
    assert_eq!(cycle.module, "RequestCycleModule");
    assert_eq!(cycle.scope, ProviderScope::Request);
    assert_eq!(cycle.chain, ["DraftView", "ReviewView", "DraftView"]);
}

#[tokio::test]
async fn transient_providers_waiting_on_each_other_fail_the_boot_an_optional_edge_included() {
    let cycle = cycle_of::<TransientCycleModule>().await;
    assert_eq!(cycle.scope, ProviderScope::Transient);
    assert_eq!(cycle.chain, ["SlugDraft", "SlugCheck", "SlugDraft"]);
}

#[tokio::test]
async fn a_cycle_across_the_request_and_transient_scopes_fails_the_boot() {
    let cycle = cycle_of::<MixedScopeCycleRoot>().await;
    assert_eq!(cycle.module, "MixedScopeCycleModule");
    assert_eq!(cycle.scope, ProviderScope::Request);
    assert_eq!(cycle.chain, ["Visit", "VisitTracker", "Visit"]);
}

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct AuthorsService {
    #[inject]
    books: Arc<BooksService>,
}

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct BooksService {
    #[inject]
    authors: Arc<AuthorsService>,
}

#[module(imports = [BooksModule], providers = [AuthorsService])]
struct AuthorsModule;

#[module(imports = [AuthorsModule], providers = [BooksService])]
struct BooksModule;

#[tokio::test]
async fn singletons_waiting_on_each_other_across_an_import_cycle_fail_the_boot_as_a_provider_cycle()
{
    let cycle = cycle_of::<AuthorsModule>().await;
    assert_eq!(
        cycle.module, "BooksModule",
        "registered first, it stalled first"
    );
    assert_eq!(cycle.scope, ProviderScope::Singleton);
    assert_eq!(
        cycle.chain,
        ["BooksService", "AuthorsService", "BooksService"]
    );
    let cycle = cycle_of::<BooksModule>().await;
    assert_eq!(cycle.module, "AuthorsModule");
    assert_eq!(
        cycle.chain,
        ["AuthorsService", "BooksService", "AuthorsService"]
    );
}

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct Editor {
    #[inject]
    desk: Option<Arc<ReviewDesk>>,
}

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct ReviewDesk {
    #[inject]
    editor: Arc<Editor>,
}

#[module(imports = [ReviewDeskModule], providers = [Editor])]
struct EditorModule;

#[module(imports = [EditorModule], providers = [ReviewDesk])]
struct ReviewDeskModule;

#[tokio::test]
async fn a_singleton_cycle_across_modules_through_an_optional_dependency_fails_the_boot_from_either_root()
 {
    let cycle = cycle_of::<EditorModule>().await;
    assert_eq!(cycle.scope, ProviderScope::Singleton);
    assert_eq!(cycle.chain, ["ReviewDesk", "Editor", "ReviewDesk"]);
    let cycle = cycle_of::<ReviewDeskModule>().await;
    assert_eq!(
        cycle.module, "EditorModule",
        "built without its optional, then the other"
    );
    assert_eq!(cycle.chain, ["Editor", "ReviewDesk", "Editor"]);
}

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct Tagger {
    #[inject]
    classifier: Option<Arc<Classifier>>,
}

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct Classifier {
    #[inject]
    tagger: Option<Arc<Tagger>>,
}

#[module(imports = [ClassifierModule], providers = [Tagger])]
struct TaggerModule;

#[module(imports = [TaggerModule], providers = [Classifier])]
struct ClassifierModule;

#[tokio::test]
async fn singletons_holding_each_other_optionally_across_modules_fail_the_boot_as_a_provider_cycle()
{
    let cycle = cycle_of::<TaggerModule>().await;
    assert_eq!(cycle.module, "ClassifierModule");
    assert_eq!(cycle.scope, ProviderScope::Singleton);
    assert_eq!(cycle.chain, ["Classifier", "Tagger", "Classifier"]);
}

trait Pricing: Send + Sync {}

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct Catalog {
    #[inject]
    pricing: Arc<dyn Pricing>,
}

#[expect(
    dead_code,
    reason = "the dependency is declared for the container to resolve, never read"
)]
#[injectable]
struct PriceList {
    #[inject]
    catalog: Arc<Catalog>,
}

impl Pricing for PriceList {}

#[module(imports = [PriceListModule], providers = [Catalog])]
struct CatalogModule;

#[module(imports = [CatalogModule], providers = [PriceList as dyn Pricing])]
struct PriceListModule;

#[tokio::test]
async fn a_cycle_through_a_dyn_binding_names_the_provider_bound() {
    let cycle = cycle_of::<CatalogModule>().await;
    assert_eq!(cycle.module, "PriceListModule");
    assert_eq!(cycle.chain, ["PriceList", "Catalog", "PriceList"]);
}

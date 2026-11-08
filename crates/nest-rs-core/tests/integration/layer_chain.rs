//! Covers `src/layer_chain.rs` through the declarations it backs: a pool
//! registers each layer it lists that nothing else in the app registered, once
//! every module has, under the checks a listed provider passes. The link-time
//! registry is shared across a test binary, so each graph uses its own types.
#![expect(
    dead_code,
    reason = "a dependency is declared for the boot to resolve, and most are never read"
)]

use std::any::TypeId;
use std::sync::{Arc, Mutex};

use nest_rs_core::__private::InjectedFactory;
use nest_rs_core::{
    AccessGraphError, App, Caught, Container, ContainerBuilder, Discoverable, ExceptionFilter,
    GlobalExceptionFilters, GlobalInterceptors, Interceptor, LateOptionalDependencyError, Layer,
    MissingDependencyError, Next, Outcome, OverriddenDependencyError, ProviderCycleError,
    ReachableProviders, Registering, ScopeViolationError, ScopedLayerError, UnbuiltCause,
    UnbuiltProviderError, UnitContext, async_trait, hooks, injectable, module,
};

use crate::refused_boot;

/// `$name` as an interceptor doing nothing.
macro_rules! passes_through {
    ($name:ident) => {
        #[async_trait]
        impl Interceptor for $name {
            async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
                next.run(cx).await
            }
        }
    };
}

/// `$name` as a pooled interceptor doing nothing, its fields as written.
macro_rules! layer {
    ($name:ident $({ $($field:tt)* })?) => {
        #[injectable]
        #[derive(Layer)]
        struct $name { $($($field)*)? }

        passes_through!($name);
    };
}

async fn booted<M: nest_rs_core::Module>() -> App {
    App::builder()
        .module::<M>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("the app boots: {refused:#}"))
}

layer!(ListedOnce);

#[module(providers = [GlobalInterceptors<(ListedOnce,)>])]
struct ListsItOnce;

#[tokio::test]
async fn a_pool_registers_the_layer_it_alone_lists() {
    let app = booted::<ListsItOnce>().await;
    assert!(app.container().get::<ListedOnce>().is_some());
    let reachable = ReachableProviders::of(app.container(), "test").expect("a boot sealed it");
    assert!(
        reachable.reaches(TypeId::of::<ListedOnce>()),
        "what it hosts is discovered like any provider's",
    );
}

#[injectable]
struct Clock;

layer!(ReadsTheClock {
    #[inject]
    clock: Arc<Clock>,
});

#[module(providers = [GlobalInterceptors<(ReadsTheClock,)>, Clock])]
struct ListsItsDependencyAfterThePool;

#[tokio::test]
async fn a_pools_layer_injects_what_its_root_lists_after_the_declaration() {
    let app = booted::<ListsItsDependencyAfterThePool>().await;
    let layer = app.container().get::<ReadsTheClock>().expect("registered");
    let clock = app.container().get::<Clock>().expect("listed");
    assert!(Arc::ptr_eq(&layer.clock, &clock));
}

layer!(AModulesLayer);

#[module(providers = [AModulesLayer])]
struct ListsTheLayer;

#[module(imports = [ListsTheLayer], providers = [GlobalInterceptors<(AModulesLayer,)>])]
struct ImportsWhatListsIt;

#[tokio::test]
async fn a_layer_a_module_lists_stays_that_modules() {
    let app = booted::<ImportsWhatListsIt>().await;
    assert!(app.container().get::<AModulesLayer>().is_some());
}

layer!(ListedBeside);

#[module(providers = [ListedBeside, GlobalInterceptors<(ListedBeside,)>])]
struct ListsItBeforeThePool;

layer!(ListedBesideAfter);

#[module(providers = [GlobalInterceptors<(ListedBesideAfter,)>, ListedBesideAfter])]
struct ListsItAfterThePool;

#[tokio::test]
async fn a_layer_the_root_lists_beside_its_pool_registers_once_in_either_order() {
    assert!(
        booted::<ListsItBeforeThePool>()
            .await
            .container()
            .get::<ListedBeside>()
            .is_some()
    );
    assert!(
        booted::<ListsItAfterThePool>()
            .await
            .container()
            .get::<ListedBesideAfter>()
            .is_some()
    );
}

layer!(FromTheOtherRoot);

#[module(providers = [FromTheOtherRoot])]
struct OtherRoot;

#[module(providers = [GlobalInterceptors<(FromTheOtherRoot,)>])]
struct PoolRoot;

#[tokio::test]
async fn a_layer_a_later_root_lists_is_that_roots() {
    let app = App::builder()
        .module::<PoolRoot>()
        .module::<OtherRoot>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("the app boots: {refused:#}"));
    assert!(app.container().get::<FromTheOtherRoot>().is_some());
}

/// Says who built it.
#[injectable]
#[derive(Default, Layer)]
struct Tagged {
    tag: &'static str,
}

passes_through!(Tagged);

#[module(providers = [GlobalInterceptors<(Tagged,)>])]
struct ListsTagged;

#[tokio::test]
async fn a_seed_of_a_pools_layer_stands_in_its_place() {
    let app = App::builder()
        .provide(Tagged { tag: "seeded" })
        .module::<ListsTagged>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("the app boots: {refused:#}"));
    let tagged = app.container().get::<Tagged>().expect("seeded");
    assert_eq!(tagged.tag, "seeded");
}

#[tokio::test]
async fn a_tests_override_is_claimed_by_the_pool_that_alone_lists_the_layer() {
    let app = App::builder()
        .override_value(Tagged { tag: "double" })
        .module::<ListsTagged>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("the override is claimed: {refused:#}"));
    let tagged = app.container().get::<Tagged>().expect("the double");
    assert_eq!(tagged.tag, "double");
}

/// Never built from what it injects, which nothing could provide: the fence
/// a layer a factory builds puts around any other path.
struct OnlyTheFactoryMakesThis;

layer!(Fenced {
    #[inject]
    fence: Arc<OnlyTheFactoryMakesThis>,
});

#[module(providers = [GlobalInterceptors<(Fenced,)>])]
struct ListsTheFenced;

#[tokio::test]
async fn a_layer_the_app_holds_answers_for_nothing_it_would_inject() {
    let fence = Arc::new(OnlyTheFactoryMakesThis);
    let app = App::builder()
        .provide_factory(move |_| async move { Ok(Fenced { fence }) })
        .module::<ListsTheFenced>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("the factory's layer stands: {refused:#}"));
    assert!(app.container().get::<Fenced>().is_some());
}

layer!(InTwoPools);

#[async_trait]
impl ExceptionFilter for InTwoPools {
    type Exception = dyn std::error::Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, _: &mut UnitContext<'_>) -> Caught {
        Caught::Declined
    }
}

#[module(providers = [
    GlobalInterceptors<(InTwoPools, InTwoPools)>,
    GlobalExceptionFilters<(InTwoPools,)>,
])]
struct ListsItInTwoPools;

#[tokio::test]
async fn a_layer_two_lists_name_registers_once() {
    let app = booted::<ListsItInTwoPools>().await;
    assert!(app.container().get::<InTwoPools>().is_some());
}

layer!(InnerLayer);

layer!(OuterLayer {
    #[inject]
    inner: Arc<InnerLayer>,
});

#[module(providers = [GlobalInterceptors<(OuterLayer, InnerLayer)>])]
struct ListsDependentFirst;

#[tokio::test]
async fn a_pool_registers_its_layers_dependencies_first_whatever_the_order_written() {
    let app = booted::<ListsDependentFirst>().await;
    let outer = app.container().get::<OuterLayer>().expect("registered");
    let inner = app.container().get::<InnerLayer>().expect("registered");
    assert!(Arc::ptr_eq(&outer.inner, &inner));
}

#[injectable]
struct Unreached;

#[module(providers = [Unreached])]
struct UnreachedModule;

layer!(NeedsTheUnreached {
    #[inject]
    unreached: Arc<Unreached>,
});

#[module(providers = [GlobalInterceptors<(NeedsTheUnreached,)>])]
struct DoesNotImportIt;

#[tokio::test]
async fn a_layer_a_pool_registers_is_held_to_its_roots_reach() {
    let refused = refused_boot::<DoesNotImportIt>().await;
    let reach = refused
        .downcast_ref::<AccessGraphError>()
        .unwrap_or_else(|| panic!("not the access graph's: {refused:#}"));
    assert!(reach.dependency.ends_with("Unreached"), "{refused:#}");
    assert_eq!(reach.owner, "UnreachedModule", "{refused:#}");
    assert!(reach.consumer.contains("NeedsTheUnreached"), "{refused:#}");
}

struct NothingProvides;

layer!(NeedsWhatNothingProvides {
    #[inject]
    nothing: Arc<NothingProvides>,
});

#[module(providers = [GlobalInterceptors<(NeedsWhatNothingProvides,)>])]
struct ListsTheNeedy;

#[tokio::test]
async fn a_layer_needing_what_nothing_provides_fails_the_boot_naming_it() {
    let refused = refused_boot::<ListsTheNeedy>().await;
    let unmet = refused
        .downcast_ref::<MissingDependencyError>()
        .unwrap_or_else(|| panic!("not the access graph's: {refused:#}"));
    assert!(unmet.dependency.ends_with("NothingProvides"), "{refused:#}");
    assert!(
        unmet.consumer.contains("NeedsWhatNothingProvides"),
        "{refused:#}"
    );
}

layer!(OnlyTheRootLists);

#[injectable]
struct FeatureReachesUp {
    #[inject]
    layer: Arc<OnlyTheRootLists>,
}

#[module(providers = [FeatureReachesUp])]
struct FeatureOfTheRoot;

#[module(imports = [FeatureOfTheRoot], providers = [GlobalInterceptors<(OnlyTheRootLists,)>])]
struct RootOfTheFeature;

#[tokio::test]
async fn a_module_injecting_a_layer_only_its_root_lists_is_refused_naming_the_root() {
    let refused = refused_boot::<RootOfTheFeature>().await;
    let reach = refused
        .downcast_ref::<AccessGraphError>()
        .unwrap_or_else(|| panic!("not the access graph's: {refused:#}"));
    assert_eq!(reach.module, "FeatureOfTheRoot", "{refused:#}");
    assert_eq!(reach.owner, "RootOfTheFeature", "{refused:#}");
}

layer!(ASiblingLists);

#[module(providers = [ASiblingLists])]
struct SiblingListing;

#[injectable]
struct FeatureMissesItsImport {
    #[inject]
    layer: Arc<ASiblingLists>,
}

#[module(providers = [FeatureMissesItsImport])]
struct FeatureWithoutTheImport;

#[module(
    imports = [FeatureWithoutTheImport, SiblingListing],
    providers = [GlobalInterceptors<(ASiblingLists,)>],
)]
struct RootOfBoth;

#[tokio::test]
async fn a_missing_import_names_the_module_listing_the_layer_never_the_pool() {
    let refused = refused_boot::<RootOfBoth>().await;
    let reach = refused
        .downcast_ref::<AccessGraphError>()
        .unwrap_or_else(|| panic!("not the access graph's: {refused:#}"));
    assert_eq!(reach.owner, "SiblingListing", "{refused:#}");
}

layer!(WantedByASibling);

#[injectable]
struct WantsThePoolsLayer {
    #[inject]
    layer: Arc<WantedByASibling>,
}

#[module(providers = [WantsThePoolsLayer, GlobalInterceptors<(WantedByASibling,)>])]
struct ProviderWantsAPoolsLayer;

#[tokio::test]
async fn a_provider_injecting_a_layer_only_a_pool_lists_is_told_to_list_it() {
    let refused = refused_boot::<ProviderWantsAPoolsLayer>().await;
    let unbuilt = refused
        .downcast_ref::<UnbuiltProviderError>()
        .unwrap_or_else(|| panic!("not the seal's: {refused:#}"));
    assert_eq!(unbuilt.provider, "WantsThePoolsLayer");
    let said = refused.to_string();
    assert!(
        said.contains("once every module is built")
            && said.contains("`ProviderWantsAPoolsLayer`'s `providers`"),
        "{said}",
    );
}

layer!(WantedOptionally);

#[injectable]
struct WantsItOptionally {
    #[inject]
    layer: Option<Arc<WantedOptionally>>,
}

#[module(providers = [WantsItOptionally, GlobalInterceptors<(WantedOptionally,)>])]
struct ProviderMayWantAPoolsLayer;

#[tokio::test]
async fn a_provider_built_without_a_layer_only_a_pool_lists_is_told_to_list_it() {
    let refused = refused_boot::<ProviderMayWantAPoolsLayer>().await;
    assert!(
        refused
            .downcast_ref::<LateOptionalDependencyError>()
            .is_some(),
        "not the seal's: {refused:#}"
    );
    let said = refused.to_string();
    assert!(said.contains("once every module is built"), "{said}");
}

layer!(CyclingFirst {
    #[inject]
    other: Arc<CyclingSecond>,
});

layer!(CyclingSecond {
    #[inject]
    other: Arc<CyclingFirst>,
});

#[module(providers = [GlobalInterceptors<(CyclingFirst, CyclingSecond)>])]
struct ListsACycle;

#[tokio::test]
async fn layers_of_a_pool_waiting_on_each_other_fail_the_boot_naming_the_chain() {
    let refused = refused_boot::<ListsACycle>().await;
    let cycle = refused
        .downcast_ref::<ProviderCycleError>()
        .unwrap_or_else(|| panic!("not a cycle's refusal: {refused:#}"));
    let chain = cycle.chain.join(" ");
    assert!(
        chain.contains("CyclingFirst") && chain.contains("CyclingSecond"),
        "{refused:#}",
    );
}

#[injectable(scope = request)]
struct PerRequestDependency;

layer!(SingletonOverRequest {
    #[inject]
    per_request: Arc<PerRequestDependency>,
});

#[module(providers = [PerRequestDependency, GlobalInterceptors<(SingletonOverRequest,)>])]
struct ListsASingletonOverRequest;

#[tokio::test]
async fn a_layer_a_pool_registers_injecting_a_scoped_provider_is_refused_naming_the_layer() {
    let refused = refused_boot::<ListsASingletonOverRequest>().await;
    let violation = refused
        .downcast_ref::<ScopeViolationError>()
        .unwrap_or_else(|| panic!("not a scope violation: {refused:#}"));
    assert!(
        violation.consumer.ends_with("SingletonOverRequest"),
        "{refused:#}"
    );
    assert!(
        violation.dependency.ends_with("PerRequestDependency"),
        "{refused:#}"
    );
}

#[injectable(scope = request)]
#[derive(Layer)]
struct ScopedLayer;

passes_through!(ScopedLayer);

#[module(providers = [GlobalInterceptors<(ScopedLayer,)>])]
struct ListsAScopedLayer;

#[tokio::test]
async fn a_scoped_layer_a_pool_alone_lists_is_refused_as_a_layer() {
    let refused = refused_boot::<ListsAScopedLayer>().await;
    let scoped = refused
        .downcast_ref::<ScopedLayerError>()
        .unwrap_or_else(|| panic!("not the layer's refusal: {refused:#}"));
    assert_eq!(scoped.host, "GlobalInterceptors<(ScopedLayer,)>");
}

/// Its registration registers nothing, as a hand-written one may.
#[derive(Layer)]
struct RegistersNothing;

passes_through!(RegistersNothing);

impl Discoverable for RegistersNothing {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
    }
}

#[module(providers = [GlobalInterceptors<(RegistersNothing,)>])]
struct ListsWhatRegistersNothing;

#[tokio::test]
async fn a_layer_whose_registration_leaves_it_absent_fails_the_boot_never_runs_without_it() {
    let refused = refused_boot::<ListsWhatRegistersNothing>().await;
    let unmet = refused
        .downcast_ref::<MissingDependencyError>()
        .unwrap_or_else(|| panic!("a pool would silently drop it: {refused:#}"));
    assert!(
        unmet.dependency.ends_with("RegistersNothing"),
        "{refused:#}"
    );
    assert!(unmet.consumer.contains("RegistersNothing"), "{refused:#}");
}

static HOOKS_RAN: Mutex<Vec<&str>> = Mutex::new(Vec::new());

fn ran(hook: &'static str) {
    HOOKS_RAN.lock().expect("the trail").push(hook);
}

#[injectable]
struct HookedDependency;

#[hooks]
impl HookedDependency {
    #[on_module_init]
    async fn dependency_up(&self) {
        ran("dependency");
    }
}

layer!(HookedInner {
    #[inject]
    dependency: Arc<HookedDependency>,
});

#[hooks]
impl HookedInner {
    #[on_module_init]
    async fn inner_up(&self) {
        ran("inner");
    }
}

layer!(HookedOuter {
    #[inject]
    inner: Arc<HookedInner>,
});

#[hooks]
impl HookedOuter {
    #[on_module_init]
    async fn outer_up(&self) {
        ran("outer");
    }
}

#[module(providers = [GlobalInterceptors<(HookedOuter, HookedInner)>, HookedDependency])]
struct ListsHookedLayers;

#[tokio::test]
async fn a_pools_layer_runs_its_hooks_dependencies_first() {
    let app = booted::<ListsHookedLayers>().await;
    app.init().await.expect("the hooks run");
    assert_eq!(
        *HOOKS_RAN.lock().expect("the trail"),
        ["dependency", "inner", "outer"],
    );
}

/// Loads itself once a reached provider injects it.
struct SelfLoading;

nest_rs_core::inventory::submit! {
    InjectedFactory::new(
        TypeId::of::<SelfLoading>,
        |builder| builder.provide_factory(|_| async { Ok(SelfLoading) }),
        |_: &Container| Ok(()),
    )
}

layer!(InjectsTheSelfLoading {
    #[inject]
    loaded: Arc<SelfLoading>,
});

#[module(providers = [GlobalInterceptors<(InjectsTheSelfLoading,)>])]
struct ListsTheLoader;

#[tokio::test]
async fn what_a_pools_layer_injects_loads_itself_as_for_a_listed_provider() {
    let app = booted::<ListsTheLoader>().await;
    assert!(app.container().get::<SelfLoading>().is_some());
}

layer!(ByHand);

#[tokio::test]
async fn a_container_built_by_hand_registers_what_a_pool_lists_too() {
    let builder = <GlobalInterceptors<(ByHand,)> as Discoverable>::register(Container::builder());
    let container = builder.build().expect("the container builds");
    assert!(container.get::<ByHand>().is_some());
}

trait Port: Send + Sync {}

#[injectable]
struct Bound;

impl Port for Bound {}

struct DoublePort;

impl Port for DoublePort {}

#[module(providers = [Bound as dyn Port])]
struct BindsPort;

layer!(ListedAndDoubled {
    #[inject]
    bound: Arc<Bound>,
});

#[module(
    imports = [BindsPort],
    providers = [ListedAndDoubled, GlobalInterceptors<(ListedAndDoubled,)>],
)]
struct ListsWhatATestDoubles;

layer!(PooledAndDoubled {
    #[inject]
    bound: Arc<Bound>,
});

#[module(imports = [BindsPort], providers = [GlobalInterceptors<(PooledAndDoubled,)>])]
struct PoolsWhatATestDoubles;

#[tokio::test]
async fn a_doubled_layer_answers_for_nothing_it_would_inject() {
    App::builder()
        .override_value(ListedAndDoubled {
            bound: Arc::new(Bound),
        })
        .override_dyn::<dyn Port>(Arc::new(DoublePort))
        .module::<ListsWhatATestDoubles>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("a module lists it: {refused:#}"));
    App::builder()
        .override_value(PooledAndDoubled {
            bound: Arc::new(Bound),
        })
        .override_dyn::<dyn Port>(Arc::new(DoublePort))
        .module::<PoolsWhatATestDoubles>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("a pool alone lists it: {refused:#}"));
}

layer!(LeftWithoutItsDependency {
    #[inject]
    bound: Arc<Bound>,
});

#[module(imports = [BindsPort], providers = [GlobalInterceptors<(LeftWithoutItsDependency,)>])]
struct PoolsWhatATestStarves;

#[tokio::test]
async fn a_layer_a_pool_registers_injecting_what_a_test_removed_is_told_so() {
    let Err(refused) = App::builder()
        .override_dyn::<dyn Port>(Arc::new(DoublePort))
        .module::<PoolsWhatATestStarves>()
        .build()
        .await
    else {
        panic!("the layer is built from nothing");
    };
    let removed = refused
        .downcast_ref::<OverriddenDependencyError>()
        .unwrap_or_else(|| panic!("not the override's: {refused:#}"));
    assert!(removed.dependency.ends_with("Bound"), "{refused:#}");
    assert!(
        removed.consumer.contains("LeftWithoutItsDependency"),
        "{refused:#}"
    );
}

/// The refusal of a provider of `M`'s, told to list what it injects in `M`.
async fn told_to_list_it<M: nest_rs_core::Module>(module: &str, provider: &str) {
    let refused = refused_boot::<M>().await;
    let unbuilt = refused
        .downcast_ref::<UnbuiltProviderError>()
        .unwrap_or_else(|| panic!("not told to list it: {refused:#}"));
    assert_eq!(unbuilt.provider, provider, "{refused:#}");
    assert_eq!(
        unbuilt.cause,
        UnbuiltCause::RegisteredByAPool,
        "{refused:#}"
    );
    let said = refused.to_string();
    assert!(
        said.contains(&format!("`{module}`'s `providers`")),
        "{said}"
    );
}

layer!(SharedInner);

layer!(SharedOuter {
    #[inject]
    inner: Arc<SharedInner>,
});

#[injectable]
struct SharesTheInner {
    #[inject]
    inner: Arc<SharedInner>,
}

#[module(providers = [SharesTheInner, GlobalInterceptors<(SharedOuter, SharedInner)>])]
struct PoolWhoseLayersShare;

#[tokio::test]
async fn a_pools_layers_injecting_each_other_are_no_cycle_with_what_injects_them() {
    told_to_list_it::<PoolWhoseLayersShare>("PoolWhoseLayersShare", "SharesTheInner").await;
}

layer!(LayerAProviderWants);

#[injectable]
struct WantsOneLayer {
    #[inject]
    layer: Arc<LayerAProviderWants>,
}

layer!(LayerWantingTheProvider {
    #[inject]
    provider: Arc<WantsOneLayer>,
});

#[module(providers = [
    WantsOneLayer,
    GlobalInterceptors<(LayerAProviderWants, LayerWantingTheProvider)>,
])]
struct PoolAroundAProvider;

#[module(providers = [
    LayerAProviderWants,
    WantsOneLayer,
    GlobalInterceptors<(LayerAProviderWants, LayerWantingTheProvider)>,
])]
struct ListsTheLayerItWasToldTo;

#[tokio::test]
async fn two_layers_of_a_pool_are_no_cycle_through_a_provider_between_them() {
    told_to_list_it::<PoolAroundAProvider>("PoolAroundAProvider", "WantsOneLayer").await;
    assert!(
        booted::<ListsTheLayerItWasToldTo>()
            .await
            .container()
            .get::<LayerWantingTheProvider>()
            .is_some(),
        "the remedy is the sentence's",
    );
}

layer!(LayerOnACycle {
    #[inject]
    provider: Arc<ProviderOnACycle>,
});

#[injectable]
struct ProviderOnACycle {
    #[inject]
    layer: Arc<LayerOnACycle>,
}

#[module(providers = [ProviderOnACycle, GlobalInterceptors<(LayerOnACycle,)>])]
struct PoolOnACycle;

#[module(providers = [LayerOnACycle, ProviderOnACycle, GlobalInterceptors<(LayerOnACycle,)>])]
struct ListsTheLayerOnACycle;

#[tokio::test]
async fn a_cycle_through_a_layer_only_a_pool_lists_shows_once_it_is_listed() {
    told_to_list_it::<PoolOnACycle>("PoolOnACycle", "ProviderOnACycle").await;
    let refused = refused_boot::<ListsTheLayerOnACycle>().await;
    let cycle = refused
        .downcast_ref::<ProviderCycleError>()
        .unwrap_or_else(|| panic!("not a cycle's refusal: {refused:#}"));
    let chain = cycle.chain.join(" ");
    assert!(
        chain.contains("LayerOnACycle") && chain.contains("ProviderOnACycle"),
        "{refused:#}",
    );
}

struct RegisteredLate;

struct LateModule;

impl nest_rs_core::Module for LateModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide(RegisteredLate)
    }
}

layer!(WaitsOnTheLate {
    #[inject]
    late: Arc<RegisteredLate>,
});

#[module(providers = [WaitsOnTheLate, GlobalInterceptors<(WaitsOnTheLate,)>])]
struct ListsWhatStalls;

layer!(WaitsOnTheLateToo {
    #[inject]
    late: Arc<RegisteredLate>,
});

#[injectable]
struct WantsWhatStalled {
    #[inject]
    layer: Arc<WaitsOnTheLateToo>,
}

#[module(providers = [
    WaitsOnTheLateToo,
    WantsWhatStalled,
    GlobalInterceptors<(WaitsOnTheLateToo,)>,
])]
struct ListsWhatStallsAndItsConsumer;

#[tokio::test]
async fn a_layer_its_module_could_not_build_stays_that_modules_refusal() {
    for (built, layer) in [
        (
            App::builder()
                .module::<ListsWhatStalls>()
                .module::<LateModule>()
                .build()
                .await,
            "WaitsOnTheLate",
        ),
        (
            App::builder()
                .module::<ListsWhatStallsAndItsConsumer>()
                .module::<LateModule>()
                .build()
                .await,
            "WaitsOnTheLateToo",
        ),
    ] {
        let Err(refused) = built else {
            panic!("the pool built what `{layer}`'s module could not");
        };
        let unbuilt = refused
            .downcast_ref::<UnbuiltProviderError>()
            .unwrap_or_else(|| panic!("not the seal's: {refused:#}"));
        assert_eq!(unbuilt.provider, layer, "{refused:#}");
        assert!(
            matches!(unbuilt.cause, UnbuiltCause::RegisteredLater(_)),
            "{refused:#}"
        );
    }
}

#[injectable]
struct AcrossTheCycle;

layer!(WaitsAcrossTheCycle {
    #[inject]
    across: Arc<AcrossTheCycle>,
});

#[injectable]
struct WantsWhatWaitsAcross {
    #[inject]
    layer: Arc<WaitsAcrossTheCycle>,
}

#[module(imports = [SecondOfTheCycle], providers = [WaitsAcrossTheCycle, WantsWhatWaitsAcross])]
struct FirstOfTheCycle;

#[module(imports = [FirstOfTheCycle], providers = [AcrossTheCycle])]
struct SecondOfTheCycle;

#[module(imports = [SecondOfTheCycle], providers = [GlobalInterceptors<(WaitsAcrossTheCycle,)>])]
struct RootOfTheCycle;

#[tokio::test]
async fn an_import_cycle_stalling_a_layer_its_module_lists_is_named() {
    let refused = refused_boot::<RootOfTheCycle>().await;
    let unbuilt = refused
        .downcast_ref::<UnbuiltProviderError>()
        .unwrap_or_else(|| panic!("not the seal's: {refused:#}"));
    assert_eq!(unbuilt.provider, "WaitsAcrossTheCycle", "{refused:#}");
    assert_eq!(
        unbuilt.cause,
        UnbuiltCause::ImportCycle(vec!["FirstOfTheCycle", "SecondOfTheCycle"]),
        "{refused:#}",
    );
}

layer!(WantedInARequest);

#[injectable(scope = request)]
struct BuiltPerRequest {
    #[inject]
    layer: Arc<WantedInARequest>,
}

#[module(providers = [BuiltPerRequest, GlobalInterceptors<(WantedInARequest,)>])]
struct ARequestWantsAPoolsLayer;

#[tokio::test]
async fn a_request_scoped_provider_injects_a_layer_only_a_pool_lists() {
    let app = booted::<ARequestWantsAPoolsLayer>().await;
    assert!(app.container().get::<WantedInARequest>().is_some());
}

struct NothingHere;

layer!(HandBuiltNeedy {
    #[inject]
    nothing: Arc<NothingHere>,
});

#[module(providers = [GlobalInterceptors<(HandBuiltNeedy,)>])]
struct HandBuiltRoot;

#[test]
fn a_refusal_on_a_container_built_by_hand_names_the_module_listing_the_pool() {
    let Err(refused) = Container::builder().import::<HandBuiltRoot>().build() else {
        panic!("the layer is built from nothing");
    };
    let said = refused.to_string();
    assert!(said.starts_with("module `HandBuiltRoot`: "), "{said}");
    assert!(
        said.ends_with(
            "`HandBuiltRoot` was imported on a container built by hand while nothing there \
             provided it: provide it, or import what provides it, before `HandBuiltRoot`"
        ),
        "{said}",
    );
}

layer!(NeedyWithNoModule {
    #[inject]
    nothing: Arc<NothingHere>,
});

#[test]
fn a_refusal_with_no_module_names_the_declaration() {
    type Declaration = GlobalInterceptors<(NeedyWithNoModule,)>;

    let builder = <Declaration as Discoverable>::register(Container::builder());
    let Err(refused) = builder.build() else {
        panic!("the layer is built from nothing");
    };
    let said = refused.to_string();
    assert!(
        said.starts_with(&format!(
            "module `{}`: ",
            std::any::type_name::<Declaration>()
        )),
        "{said}",
    );
    assert!(!said.contains("no module"), "{said}");
}

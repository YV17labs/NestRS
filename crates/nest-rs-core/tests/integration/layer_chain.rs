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
    MissingDependencyError, Next, Outcome, ProviderCycleError, ReachableProviders,
    ScopeViolationError, ScopedLayerError, UnbuiltProviderError, UnitContext, async_trait, hooks,
    injectable, module,
};

use crate::refused_boot;

/// `$name` as an interceptor doing nothing.
macro_rules! passes_through {
    ($name:ident) => {
        impl Layer for $name {}

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
#[derive(Default)]
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

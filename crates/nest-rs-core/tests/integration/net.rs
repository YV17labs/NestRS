//! Covers `src/net.rs` — the boot refuses every budget at or past a net that
//! reaches it, and only those: a net over a resource reaches that resource's
//! budget alone, a net around a provider's code what the code injects at any
//! depth, optionally or not, through the bindings the app imports, and every
//! ambient resource — and it refuses as soon as the budget's resource exists.

use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use nest_rs_core::{
    App, Budget, BudgetPastNetError, Collecting, ContainerBuilder, Module, Net, Registering,
    injectable, module,
};

/// A resource that waits what it holds for an answer.
struct Pool(Duration);

/// A second resource, so a net can wait on one and miss the other's budget.
struct Cache;

const NET: Duration = Duration::from_secs(20);

fn pool_budget() -> Budget {
    Budget::of::<Pool>("the test pool", "TEST_POOL_WAIT", |pool| Some(pool.0))
}

fn ambient_pool_budget() -> Budget {
    Budget::ambient::<Pool>("the test pool", "TEST_POOL_WAIT", |pool| Some(pool.0))
}

#[module]
struct EmptyModule;

#[injectable]
struct Lookup {
    #[inject]
    #[expect(
        dead_code,
        reason = "injected only so the strategy's code reaches the pool"
    )]
    pool: Arc<Pool>,
}

#[injectable]
struct ReachingStrategy {
    #[inject]
    #[expect(
        dead_code,
        reason = "injected only so the strategy's code reaches the pool"
    )]
    lookup: Arc<Lookup>,
}

#[module(providers = [Lookup, ReachingStrategy])]
struct ReachingModule;

#[injectable]
struct LocalStrategy;

#[module(providers = [LocalStrategy])]
struct LocalModule;

fn refusal(booted: anyhow::Result<App>) -> BudgetPastNetError {
    let Err(error) = booted else {
        panic!("the boot must refuse a budget at a net reaching it");
    };
    error
        .downcast::<BudgetPastNetError>()
        .unwrap_or_else(|other| panic!("not a budget refusal: {other:#}"))
}

#[tokio::test]
async fn a_budget_at_the_net_over_its_resource_fails_the_boot_naming_both() {
    let refused = refusal(
        App::builder()
            .provide(Pool(NET))
            .provide_meta(pool_budget())
            .provide_meta(Net::over::<Pool>("the test port", NET))
            .module::<EmptyModule>()
            .build()
            .await,
    );
    assert_eq!(
        (refused.resource, refused.budget, refused.port, refused.net),
        ("the test pool", NET, "the test port", NET)
    );
    let said = refused.to_string();
    assert!(
        said.starts_with(
            "the test pool's budget (20s) must be shorter than the test port's net (20s)"
        ),
        "{said}"
    );
    assert!(
        said.ends_with("lose its cause: lower TEST_POOL_WAIT"),
        "{said}"
    );
}

#[tokio::test]
async fn a_budget_below_the_net_over_its_resource_boots() {
    App::builder()
        .provide(Pool(NET - Duration::from_millis(1)))
        .provide_meta(pool_budget())
        .provide_meta(Net::over::<Pool>("the test port", NET))
        .module::<EmptyModule>()
        .build()
        .await
        .expect("a budget under the net boots");
}

#[tokio::test]
async fn a_net_over_one_resource_leaves_another_resources_budget_alone() {
    App::builder()
        .provide(Pool(NET * 2))
        .provide_meta(ambient_pool_budget())
        .provide(Cache)
        .provide_meta(Net::over::<Cache>("the cache port", NET))
        .module::<EmptyModule>()
        .build()
        .await
        .expect("a net over the cache never waits on the pool");
}

#[tokio::test]
async fn a_net_around_a_provider_reaches_what_it_injects_at_any_depth() {
    let refused = refusal(
        App::builder()
            .provide(Pool(NET))
            .provide_meta(pool_budget())
            .provide_meta(Net::around::<ReachingStrategy>("the test guard", NET))
            .module::<ReachingModule>()
            .build()
            .await,
    );
    assert_eq!(
        (refused.resource, refused.port),
        ("the test pool", "the test guard")
    );
}

#[tokio::test]
async fn a_net_around_a_provider_leaves_what_it_cannot_reach_alone() {
    App::builder()
        .provide(Pool(NET * 2))
        .provide_meta(pool_budget())
        .provide_meta(Net::around::<LocalStrategy>("the test guard", NET))
        .module::<LocalModule>()
        .build()
        .await
        .expect("a strategy injecting nothing never waits on the pool");
}

#[tokio::test]
async fn an_ambient_budget_is_reached_by_every_net_around_code() {
    let refused = refusal(
        App::builder()
            .provide(Pool(NET))
            .provide_meta(ambient_pool_budget())
            .provide_meta(Net::around::<LocalStrategy>("the test guard", NET))
            .module::<LocalModule>()
            .build()
            .await,
    );
    assert_eq!(refused.port, "the test guard");
}

#[tokio::test]
async fn a_budget_whose_resource_was_never_built_is_not_held() {
    App::builder()
        .provide_meta(pool_budget())
        .provide_meta(Net::over::<Pool>("the test port", NET))
        .module::<EmptyModule>()
        .build()
        .await
        .expect("no pool, nothing to wait on");
}

/// Declares from its own `register`, as a hand-written binding does.
struct DeclaringModule;

impl Module for DeclaringModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
            .provide(Pool(NET))
            .provide_meta(pool_budget())
            .provide_meta(Net::over::<Pool>("the test port", NET))
    }
}

#[test]
fn the_synchronous_boot_refuses_it_too() {
    let Err(error) = App::new::<DeclaringModule>() else {
        panic!("App::new must refuse a budget at a net reaching it");
    };
    assert!(
        error.downcast_ref::<BudgetPastNetError>().is_some(),
        "{error:#}"
    );
}

#[injectable]
struct OptionalStrategy {
    #[inject]
    #[expect(
        dead_code,
        reason = "injected only so the strategy's code reaches the pool"
    )]
    pool: Option<Arc<Pool>>,
}

#[module(providers = [OptionalStrategy])]
struct OptionalModule;

#[tokio::test]
async fn a_net_around_a_provider_reaches_what_it_injects_optionally() {
    let refused = refusal(
        App::builder()
            .provide(Pool(NET))
            .provide_meta(pool_budget())
            .provide_meta(Net::around::<OptionalStrategy>("the test guard", NET))
            .module::<OptionalModule>()
            .build()
            .await,
    );
    assert_eq!(refused.port, "the test guard");
}

trait Store: Send + Sync {}

#[injectable]
struct LocalStore;

impl Store for LocalStore {}

#[injectable]
struct PooledStore {
    #[inject]
    #[expect(
        dead_code,
        reason = "injected only so the store's code reaches the pool"
    )]
    pool: Arc<Pool>,
}

impl Store for PooledStore {}

#[module(providers = [LocalStore as dyn Store])]
struct LocalStoreModule;

// Linked into the suite beside `LocalStoreModule`, binding the same key.
#[module(providers = [PooledStore as dyn Store])]
struct PooledStoreModule;

#[injectable]
struct StoreStrategy {
    #[inject]
    #[expect(
        dead_code,
        reason = "injected only so the strategy's code reaches the store"
    )]
    store: Arc<dyn Store>,
}

#[module(imports = [LocalStoreModule], providers = [StoreStrategy])]
struct LocalStoreApp;

#[module(imports = [PooledStoreModule], providers = [StoreStrategy])]
struct PooledStoreApp;

#[tokio::test]
async fn a_net_around_a_provider_follows_the_binding_the_app_imports() {
    let refused = refusal(
        App::builder()
            .provide(Pool(NET))
            .provide_meta(pool_budget())
            .provide_meta(Net::around::<StoreStrategy>("the test guard", NET))
            .module::<PooledStoreApp>()
            .build()
            .await,
    );
    assert_eq!(refused.port, "the test guard");
}

#[tokio::test]
async fn a_net_around_a_provider_never_follows_a_binding_another_composition_imports() {
    App::builder()
        .provide(Pool(NET * 2))
        .provide_meta(pool_budget())
        .provide_meta(Net::around::<StoreStrategy>("the test guard", NET))
        .module::<LocalStoreApp>()
        .build()
        .await
        .expect("the store this app binds never waits on the pool");
}

/// Reads the pool from its snapshot, and fails saying it ran.
struct Dependent;

/// Opens the pool past the net over it, then a factory that reads the pool.
struct OpeningModule;

impl Module for OpeningModule {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder
            .provide_meta(pool_budget())
            .provide_meta(Net::over::<Pool>("the test port", NET))
            .provide_factory(|_| async { Ok(Pool(NET)) })
            .provide_factory_after::<Dependent, Pool, _, _>(|_| async {
                Err(anyhow!("the factory after the pool ran"))
            })
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }
}

/// The refusal comes as soon as the pool exists, before a factory reading it
/// does its own work and fails on something else first.
#[tokio::test]
async fn a_budget_is_refused_once_its_resource_is_built_before_the_factories_after_it() {
    let refused = refusal(App::builder().module::<OpeningModule>().build().await);
    assert_eq!(refused.port, "the test port");
}

/// Queues a factory that fails, as one doing I/O would.
struct FailingFactoryModule;

impl Module for FailingFactoryModule {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder
            .provide_meta(pool_budget())
            .provide_meta(Net::over::<Pool>("the test port", NET))
            .provide_factory::<Dependent, _, _>(|_| async { Err(anyhow!("a factory ran")) })
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }
}

#[tokio::test]
async fn a_seeded_resources_budget_is_refused_before_any_factory_runs() {
    let refused = refusal(
        App::builder()
            .provide(Pool(NET))
            .module::<FailingFactoryModule>()
            .build()
            .await,
    );
    assert_eq!(refused.port, "the test port");
}

#[tokio::test]
async fn a_budget_declared_twice_is_held_by_its_ambient_declaration() {
    let refused = refusal(
        App::builder()
            .provide(Pool(NET))
            .provide_meta(pool_budget())
            .provide_meta(ambient_pool_budget())
            .provide_meta(Net::around::<LocalStrategy>("the test guard", NET))
            .module::<LocalModule>()
            .build()
            .await,
    );
    assert_eq!(refused.port, "the test guard");
}

/// One resource can wait in two ways — a pool's acquire and its statements —
/// each its own budget: the second is held under the net, never merged into
/// the first and dropped.
#[tokio::test]
async fn two_budgets_of_one_resource_are_each_held() {
    let refused = refusal(
        App::builder()
            .provide(Pool(NET))
            .provide_meta(Budget::of::<Pool>(
                "the test pool's acquire",
                "TEST_POOL_ACQUIRE",
                |_| Some(Duration::from_secs(1)),
            ))
            .provide_meta(pool_budget())
            .provide_meta(Net::over::<Pool>("the test port", NET))
            .module::<EmptyModule>()
            .build()
            .await,
    );
    assert_eq!(refused.resource, "the test pool");
}

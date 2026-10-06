//! Covers `src/net.rs` — the boot refuses every budget at or past a net that
//! reaches it, and only those: a net over a resource reaches that resource's
//! budget alone, a net around a provider's code what the code injects at any
//! depth and every ambient resource.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::{
    App, Budget, BudgetPastNetError, ContainerBuilder, Module, Net, injectable, module,
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
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
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

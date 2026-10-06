//! Covers `src/module.rs` — the pool `SeaOrmModule` opens declares its budget,
//! read off the pool itself, so the boot holds it under the net of a guard
//! whose strategy injects the pool: a pool waiting as long as the
//! authentication guard would turn a dry pool into a denial that names
//! neither. Without the binding that installs `Repo`'s executor
//! (`database/module.rs`), code that does not inject the pool never reaches it.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nest_rs_authn::{AUTHENTICATE_TIMEOUT, AuthError, AuthnGuard, Strategy};
use nest_rs_core::{App, BudgetPastNetError, injectable, module};
use nest_rs_seaorm::{SeaOrmConfig, SeaOrmModule};
use poem::Request;
use sea_orm::DatabaseConnection;

/// A strategy that injects the pool, as one looking an identity up does.
#[injectable]
struct PoolStrategy {
    #[inject]
    #[expect(
        dead_code,
        reason = "injected only so the strategy's code reaches the pool"
    )]
    db: Arc<DatabaseConnection>,
}

#[async_trait]
impl Strategy for PoolStrategy {
    type Principal = ();

    async fn authenticate(&self, _req: &mut Request) -> Result<(), AuthError> {
        Err(AuthError::MissingCredentials)
    }
}

type PoolGuard = AuthnGuard<PoolStrategy>;

#[module(
    imports = [SeaOrmModule::for_root(None)],
    providers = [PoolStrategy, PoolGuard],
)]
struct GuardedPoolModule;

/// A strategy that injects nothing.
#[injectable]
struct LocalStrategy;

#[async_trait]
impl Strategy for LocalStrategy {
    type Principal = ();

    async fn authenticate(&self, _req: &mut Request) -> Result<(), AuthError> {
        Err(AuthError::MissingCredentials)
    }
}

type LocalGuard = AuthnGuard<LocalStrategy>;

#[module(
    imports = [SeaOrmModule::for_root(None)],
    providers = [LocalStrategy, LocalGuard],
)]
struct UnboundPoolModule;

/// Seeded rather than pinned, so the suite's own variables cannot move it.
pub(crate) fn config(budget: Option<Duration>) -> SeaOrmConfig {
    SeaOrmConfig {
        url: crate::harness::url(),
        connect_timeout_secs: budget.map(|budget| budget.as_secs()),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_pool_budget_at_the_net_of_a_guard_injecting_it_fails_the_boot() {
    let Err(refused) = App::builder()
        .provide(config(Some(AUTHENTICATE_TIMEOUT)))
        .module::<GuardedPoolModule>()
        .build()
        .await
    else {
        panic!("a pool waiting as long as the guard must not boot");
    };
    let refused = refused
        .downcast::<BudgetPastNetError>()
        .unwrap_or_else(|other| panic!("not a budget refusal: {other:#}"));
    assert_eq!(
        (refused.resource, refused.port, refused.budget, refused.net),
        (
            "the SeaORM pool",
            "the authentication guard",
            AUTHENTICATE_TIMEOUT,
            AUTHENTICATE_TIMEOUT
        )
    );
    assert!(
        refused
            .setting
            .contains(&nest_rs_config::var_name("seaorm", "CONNECT_TIMEOUT_SECS")),
        "{refused}"
    );
}

#[tokio::test]
async fn the_default_pool_boots_beside_the_authentication_guard() {
    App::builder()
        .provide(config(None))
        .module::<GuardedPoolModule>()
        .build()
        .await
        .expect("the default budget sits under every net");
}

#[tokio::test]
async fn a_pool_no_executor_installs_is_not_held_under_a_strategy_injecting_nothing() {
    App::builder()
        .provide(config(Some(AUTHENTICATE_TIMEOUT)))
        .module::<UnboundPoolModule>()
        .build()
        .await
        .expect("without the database binding, `Repo` reaches no pool from the strategy");
}

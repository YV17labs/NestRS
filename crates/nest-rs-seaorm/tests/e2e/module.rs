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
use sea_orm::{ConnectionTrait, DatabaseConnection};

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

/// A pool whose statements Postgres cancels past `statement_timeout`.
fn bounding_statements(bound: Duration) -> SeaOrmConfig {
    SeaOrmConfig {
        statement_timeout_secs: Some(bound.as_secs()),
        ..config(None)
    }
}

#[module(imports = [SeaOrmModule::for_root(None)])]
struct PoolModule;

/// Postgres cancels a statement running past the bound and says why, so a lock
/// held elsewhere or a plan gone wrong ends as the database's own error.
#[tokio::test]
async fn a_statement_past_its_bound_is_cancelled_by_postgres() {
    let app = App::builder()
        .provide(bounding_statements(Duration::from_secs(1)))
        .module::<PoolModule>()
        .build()
        .await
        .expect("boots");
    let db = app
        .container()
        .get::<DatabaseConnection>()
        .expect("the pool");
    let started = std::time::Instant::now();
    let cancelled = db
        .execute_unprepared("SELECT pg_sleep(3)")
        .await
        .expect_err("a statement past its bound is cancelled");
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "{:?}",
        started.elapsed()
    );
    assert!(
        cancelled.to_string().contains("statement timeout"),
        "{cancelled}"
    );
}

#[tokio::test]
async fn a_statement_bound_at_the_net_of_a_guard_injecting_the_pool_fails_the_boot() {
    let Err(refused) = App::builder()
        .provide(bounding_statements(AUTHENTICATE_TIMEOUT))
        .module::<GuardedPoolModule>()
        .build()
        .await
    else {
        panic!("statements running as long as the guard must not boot");
    };
    let refused = refused
        .downcast::<BudgetPastNetError>()
        .unwrap_or_else(|other| panic!("not a budget refusal: {other:#}"));
    assert_eq!(
        (refused.resource, refused.budget),
        ("the SeaORM statements", AUTHENTICATE_TIMEOUT)
    );
    assert!(
        refused.setting.contains(&nest_rs_config::var_name(
            "seaorm",
            "STATEMENT_TIMEOUT_SECS"
        )),
        "{refused}"
    );
}

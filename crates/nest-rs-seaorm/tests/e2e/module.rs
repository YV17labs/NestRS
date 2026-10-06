//! Covers `src/module.rs` — the pool `SeaOrmModule` opens declares its budget,
//! read off the pool itself, and `Repo` reaches the pool from any unit of
//! work, so the boot holds it under the net of every guard around developer
//! code: a pool waiting as long as the authentication guard would turn a dry
//! pool into a denial that names neither.

use std::time::Duration;

use async_trait::async_trait;
use nest_rs_authn::{AUTHENTICATE_TIMEOUT, AuthError, AuthnGuard, Strategy};
use nest_rs_core::{App, BudgetPastNetError, injectable, module};
use nest_rs_seaorm::{SeaOrmConfig, SeaOrmModule};
use poem::Request;

/// A strategy that injects nothing, so only the pool's ambience reaches it.
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
struct GuardedPoolModule;

/// Seeded rather than pinned, so the suite's own variables cannot move it.
fn config(budget: Option<Duration>) -> SeaOrmConfig {
    SeaOrmConfig {
        url: crate::harness::url(),
        connect_timeout_secs: budget.map(|budget| budget.as_secs()),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_pool_budget_at_the_authentication_guards_net_fails_the_boot() {
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

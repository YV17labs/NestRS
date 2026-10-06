//! Covers `src/database/module.rs` — the binding installs the executor `Repo`
//! reads in every unit of work, so it declares the pool's budget ambient: every
//! net around developer code reaches it, a strategy injecting nothing included.

use async_trait::async_trait;
use nest_rs_authn::{AUTHENTICATE_TIMEOUT, AuthError, AuthnGuard, Strategy};
use nest_rs_core::{App, BudgetPastNetError, injectable, module};
use nest_rs_seaorm::{SeaOrmDatabaseModule, SeaOrmModule};
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
    imports = [SeaOrmModule::for_root(None), SeaOrmDatabaseModule],
    providers = [LocalStrategy, LocalGuard],
)]
struct BoundPoolModule;

#[tokio::test]
async fn a_pool_budget_at_the_authentication_guards_net_fails_the_boot() {
    let Err(refused) = App::builder()
        .provide(crate::module::config(Some(AUTHENTICATE_TIMEOUT)))
        .module::<BoundPoolModule>()
        .build()
        .await
    else {
        panic!("a pool `Repo` reaches from the strategy must not wait as long as the guard");
    };
    let refused = refused
        .downcast::<BudgetPastNetError>()
        .unwrap_or_else(|other| panic!("not a budget refusal: {other:#}"));
    assert_eq!(
        (refused.resource, refused.port),
        ("the SeaORM pool", "the authentication guard")
    );
}

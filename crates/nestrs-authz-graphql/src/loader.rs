//! [`LoaderScope`] — re-installs the request's ambient data context inside a
//! `#[dataloader]` batch, so a loader's `Repo` reads scope to the caller exactly
//! like a resolver. It implements `nestrs-graphql`'s [`BatchContext`] seam, the
//! loader-side counterpart to [`GraphqlAbilityBridge`](crate::GraphqlAbilityBridge)'s
//! per-operation bridge.
//!
//! async-graphql runs every batch on a *spawned* task (concurrent `load_one`s
//! collapse into one query — the point of a DataLoader), and a spawned task
//! starts with empty task-local storage. So the ambient [`Ability`] the
//! operation bridge installs, and the ambient [`Executor`] the `DbContext`
//! interceptor installs, are both gone by the time a batch loads — a loader's
//! `Repo` would read unscoped. `LoaderScope` closes that: its
//! [`spawner`](BatchContext::spawner) runs while each per-request loader is built
//! (inside the operation's ambient scope, so the ability is live), snapshots that
//! state, and returns a spawner that re-establishes it around every batch future.
//!
//! It binds the connection **pool**, never the request's transaction: a batch
//! runs concurrently off the request task, and reclaiming the transaction `Arc`
//! to commit would race the auto-commit's `Arc::try_unwrap`. Loader reads do not
//! need the request's transaction — this mirrors the WebSocket data context,
//! which binds the pool for the same reason.
//!
//! Bind it by listing `LoaderScope as dyn BatchContext` in the app's GraphQL
//! authorization module, beside the operation bridge.

use std::sync::Arc;

use nestrs_authz::{current_ability, with_ability};
use nestrs_core::injectable;
use nestrs_database::{with_request_executor, Executor};
use nestrs_graphql::{BatchContext, BatchSpawner};
use sea_orm::DatabaseConnection;

/// Scopes every `#[dataloader]` batch to the caller, re-installing the ambient
/// ability and a pool executor on the batch's spawned task. Inject it by listing
/// `LoaderScope as dyn BatchContext`.
#[injectable]
pub struct LoaderScope {
    #[inject]
    db: Arc<DatabaseConnection>,
}

impl BatchContext for LoaderScope {
    fn spawner(&self) -> BatchSpawner {
        let ability = current_ability();
        let executor = Executor::Pool(self.db.clone());
        Box::new(move |fut| {
            let ability = ability.clone();
            let executor = executor.clone();
            tokio::spawn(async move {
                let scoped = with_request_executor(executor, fut);
                match ability {
                    Some(ability) => with_ability(ability, scoped).await,
                    None => scoped.await,
                }
            });
        })
    }
}

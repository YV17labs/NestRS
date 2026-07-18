//! [`LoaderScope`] — re-installs the ambient data context inside a
//! `#[dataloader]` batch. async-graphql runs every batch on a spawned task
//! (the whole point of a DataLoader), which starts with empty task-local
//! storage — the ambient ability and executor would be gone, and a loader
//! would read unscoped. `LoaderScope::spawner` runs while the per-request
//! loader is built (the ability is still live), snapshots it, and re-installs
//! both around each batch future.
//!
//! Binds the **pool**, never the request transaction: a batch runs off the
//! request task, and reclaiming the txn `Arc` to commit would race the
//! auto-commit's `Arc::try_unwrap`. Mirrors the WS data context for the same
//! reason.

use std::sync::Arc;

use nest_rs_authz::{current_ability, with_ability};
use nest_rs_core::injectable;
use nest_rs_graphql::{GraphqlBatchContext, GraphqlBatchSpawner};
use sea_orm::DatabaseConnection;

use crate::{Executor, with_request_executor};

/// Scopes every `#[dataloader]` batch to the caller. List
/// `LoaderScope as dyn GraphqlBatchContext` on the GraphQL authz module.
#[injectable]
pub struct LoaderScope {
    #[inject]
    db: Arc<DatabaseConnection>,
}

impl GraphqlBatchContext for LoaderScope {
    fn spawner(&self) -> GraphqlBatchSpawner {
        let ability = current_ability();
        let executor = Executor::Pool((*self.db).clone());
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

#[cfg(test)]
mod tests {
    use nest_rs_authz::AbilityBuilder;

    use super::*;
    use crate::current_executor;

    // `LoaderScope::spawner` snapshots the ambient executor + ability while the
    // per-request loader is built, then re-installs both around each spawned
    // batch — async-graphql runs every batch on a fresh task whose task-locals
    // are empty, so without this a loader's `Repo` reads would run unscoped.
    // Proven end-to-end by the api GraphQL relation e2e; pinned here without a
    // DB by observing the ambient state *inside* a batch spawned from outside
    // any scope.
    #[tokio::test]
    async fn spawner_reinstalls_the_snapshot_executor_and_ability_in_the_batch() {
        let scope = LoaderScope {
            db: Arc::new(DatabaseConnection::default()),
        };
        let ability = Arc::new(AbilityBuilder::new().build().expect("valid test ability"));

        // Build the spawner *inside* the ability scope, exactly as async-graphql
        // builds the per-request loader while the request's ability is live.
        let spawner = with_ability(ability.clone(), async { scope.spawner() }).await;

        // We are now outside any scope: a bare `tokio::spawn` batch would see
        // empty task-locals — the exact hazard `LoaderScope` closes.
        assert!(current_executor().is_none());
        assert!(current_ability().is_none());

        // The batch reports back the ambient state it observed once re-installed.
        let (tx, rx) = tokio::sync::oneshot::channel();
        spawner(Box::pin(async move {
            let _ = tx.send((current_executor(), current_ability()));
        }));
        let (executor, seen_ability) = rx.await.expect("the batch future resolves");

        assert!(
            matches!(executor, Some(Executor::Pool(_))),
            "the spawner re-installs a pool executor around the batch",
        );
        let seen_ability =
            seen_ability.expect("the spawner re-installs the ability around the batch");
        assert!(
            Arc::ptr_eq(&seen_ability, &ability),
            "the snapshot ability is re-installed, not a fresh one",
        );
    }
}

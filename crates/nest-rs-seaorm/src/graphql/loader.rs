//! A batch binds the **pool**, never the request transaction: reclaiming the
//! transaction's `Arc` off the request task would race the commit's
//! `Arc::try_unwrap`. So a relation resolved in a mutation's own response reads
//! the pre-mutation state.

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

    #[tokio::test]
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the test's receiver may already be gone"
    )]
    async fn spawner_reinstalls_the_snapshot_executor_and_ability_in_the_batch() {
        let scope = LoaderScope {
            db: Arc::new(DatabaseConnection::default()),
        };
        let ability = Arc::new(AbilityBuilder::new().build().expect("valid test ability"));

        let spawner = with_ability(ability.clone(), async { scope.spawner() }).await;

        assert!(current_executor().is_none());
        assert!(current_ability().is_none());

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

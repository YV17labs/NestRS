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

use nestrs_authz::{current_ability, with_ability};
use nestrs_core::injectable;
use nestrs_graphql::{BatchContext, BatchSpawner};
use sea_orm::DatabaseConnection;

use crate::{with_request_executor, Executor};

/// Scopes every `#[dataloader]` batch to the caller. List
/// `LoaderScope as dyn BatchContext` on the GraphQL authz module.
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

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nest_rs_database::{AtomicBlock, AtomicError, Deferred};

use crate::executor::{Executor, FinalizeOutcome, LazyTransaction};
use crate::savepoint::Savepoint;

/// The executor an atomic block installs: a [`Repo`](crate::Repo) reads the
/// SeaORM executor inside, and the block settles it once.
pub(crate) struct AtomicExecutor(Executor);

impl AtomicExecutor {
    /// A block running on a transaction of its own.
    pub(crate) fn own(transaction: Arc<LazyTransaction>) -> Self {
        Self(Executor::Lazy(transaction))
    }

    /// A block running inside a savepoint of its enclosing unit.
    pub(crate) fn savepoint(savepoint: Arc<Savepoint>) -> Self {
        Self(Executor::Savepoint(savepoint))
    }
}

impl nest_rs_database::Executor for AtomicExecutor {
    fn as_any(&self) -> &dyn Any {
        &self.0
    }

    fn non_transactional(&self) -> Option<Arc<dyn nest_rs_database::Executor>> {
        self.0.non_transactional()
    }

    fn after_commit(&self, work: Deferred) -> Option<Deferred> {
        nest_rs_database::Executor::after_commit(&self.0, work)
    }

    fn __pool(&self) -> Option<Arc<dyn nest_rs_database::Executor>> {
        self.0.__pool()
    }

    fn atomic(&self) -> Option<Arc<dyn AtomicBlock>> {
        self.0.atomic()
    }
}

impl AtomicBlock for AtomicExecutor {
    fn settle(
        self: Arc<Self>,
        ok: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), AtomicError>> + Send>> {
        let executor = self.0.clone();
        // A handle still held past this point is one that escaped the block.
        drop(self);
        Box::pin(async move {
            match executor {
                Executor::Savepoint(savepoint) => savepoint.settle(ok).await,
                Executor::Lazy(own) => settled(own.finalize(ok).await),
                Executor::Pool(_) | Executor::Txn(_) => Ok(()),
            }
        })
    }
}

/// What a block's own transaction settling means to the block.
fn settled(outcome: FinalizeOutcome) -> Result<(), AtomicError> {
    match outcome {
        FinalizeOutcome::NoTransaction
        | FinalizeOutcome::Committed
        | FinalizeOutcome::RolledBack => Ok(()),
        FinalizeOutcome::Escaped => Err(AtomicError::Escaped),
        FinalizeOutcome::CommitFailed(err) => Err(AtomicError::Database(Box::new(err.0))),
        FinalizeOutcome::Poisoned { retryable } => Err(AtomicError::Poisoned { retryable }),
    }
}

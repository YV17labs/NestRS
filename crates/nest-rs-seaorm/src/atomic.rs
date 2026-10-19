use std::any::Any;
use std::future::Future;
use std::panic::resume_unwind;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{Continuation, TaskContext};
use nest_rs_database::{AtomicBlock, AtomicError, Deferred};

use crate::executor::{Executor, FinalizeOutcome, LazyTransaction};
use crate::savepoint::Savepoint;
use crate::unit::Settles;

/// The executor an atomic block installs: a [`Repo`](crate::Repo) reads the
/// SeaORM executor inside, and the block settles it once.
pub(crate) struct AtomicExecutor {
    executor: Executor,
    /// Where a block of its own on a framework pool unit commits.
    on_pool: Option<PoolCommit>,
}

impl AtomicExecutor {
    /// A block running on a transaction of its own.
    pub(crate) fn own(transaction: Arc<LazyTransaction>) -> Self {
        Self {
            executor: Executor::Lazy(transaction),
            on_pool: None,
        }
    }

    /// A block running on a transaction of its own, opened on a pool unit,
    /// whose commit runs where `on_pool` says.
    pub(crate) fn own_on_pool(transaction: Arc<LazyTransaction>, on_pool: PoolCommit) -> Self {
        Self {
            executor: Executor::Lazy(transaction),
            on_pool: Some(on_pool),
        }
    }

    /// A block running inside a savepoint of its enclosing unit.
    pub(crate) fn savepoint(savepoint: Arc<Savepoint>) -> Self {
        Self {
            executor: Executor::Savepoint(savepoint),
            on_pool: None,
        }
    }
}

impl nest_rs_database::Executor for AtomicExecutor {
    fn as_any(&self) -> &dyn Any {
        &self.executor
    }

    fn non_transactional(&self) -> Option<Arc<dyn nest_rs_database::Executor>> {
        self.executor.non_transactional()
    }

    fn after_commit(&self, work: Deferred) -> Option<Deferred> {
        nest_rs_database::Executor::after_commit(&self.executor, work)
    }

    fn atomic(&self) -> Option<Arc<dyn AtomicBlock>> {
        self.executor.atomic()
    }

    #[cfg(feature = "queue")]
    fn write_outbox(
        &self,
        records: &mut dyn Iterator<Item = nest_rs_database::OutboxRecord>,
    ) -> Option<futures_util::future::BoxFuture<'_, Result<(), nest_rs_database::OutboxError>>>
    {
        nest_rs_database::Executor::write_outbox(&self.executor, records)
    }

    #[cfg(feature = "queue")]
    fn remove_outbox<'a>(
        &'a self,
        queue: &'a str,
        id: uuid::Uuid,
    ) -> Option<futures_util::future::BoxFuture<'a, Result<bool, nest_rs_database::OutboxError>>>
    {
        nest_rs_database::Executor::remove_outbox(&self.executor, queue, id)
    }

    #[cfg(feature = "queue")]
    fn remove_outbox_unique<'a>(
        &'a self,
        queue: &'a str,
        key: &'a str,
    ) -> Option<futures_util::future::BoxFuture<'a, Result<bool, nest_rs_database::OutboxError>>>
    {
        nest_rs_database::Executor::remove_outbox_unique(&self.executor, queue, key)
    }
}

impl AtomicBlock for AtomicExecutor {
    fn settle(
        self: Arc<Self>,
        ok: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), AtomicError>> + Send>> {
        let (executor, on_pool) = (self.executor.clone(), self.on_pool.clone());
        // A handle still held past this point is one that escaped the block.
        drop(self);
        Box::pin(async move {
            match executor {
                Executor::Savepoint(savepoint) => savepoint.settle(ok).await,
                Executor::Lazy(own) => match on_pool {
                    Some(on_pool) if ok && own.is_opened() => on_pool.commit(own).await,
                    _ => settled(own.finalize(ok).await),
                },
                Executor::Pool(_) | Executor::Txn(_) => Ok(()),
            }
        })
    }
}

/// Where a block of its own on a pool unit commits: on the binding's settle
/// task, so the body's cut never drops its `COMMIT`, counted until the block
/// sees the answer.
#[derive(Clone)]
pub(crate) struct PoolCommit {
    settles: Settles,
    /// The pool unit's count of commits its body has not seen answered.
    unseen: Arc<AtomicUsize>,
}

impl PoolCommit {
    pub(crate) fn new(settles: Settles, unseen: Arc<AtomicUsize>) -> Self {
        Self { settles, unseen }
    }

    /// Commit `own` on the settle task, then run the work it held.
    async fn commit(self, own: Arc<LazyTransaction>) -> Result<(), AtomicError> {
        self.unseen.fetch_add(1, Ordering::Relaxed);
        let answering = TaskContext::capture(Continuation::Commit).carry(own.answer(true));
        let answered = self.settles.spawn(answering).await;
        // A body cut before here leaves the count up: its unit answers in doubt.
        self.unseen.fetch_sub(1, Ordering::Relaxed);
        match answered {
            Ok((outcome, held)) => {
                held.run().await;
                settled(outcome)
            }
            Err(join) if join.is_panic() => resume_unwind(join.into_panic()),
            Err(join) => Err(AtomicError::Database(Box::new(join))),
        }
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

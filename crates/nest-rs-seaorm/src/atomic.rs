use std::future::Future;
use std::panic::resume_unwind;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{Continuation, TaskContext};
use nest_rs_database::{AtomicBlock, AtomicError, Holds};

use crate::executor::{FinalizeOutcome, InstalledTransaction, LazyTransaction};
use crate::savepoint::Savepoint;
use crate::unit::Settles;

/// The executor an atomic block installs: it holds the block's transaction,
/// which a [`Repo`](crate::Repo) reads, and the block settles it once.
pub(crate) struct AtomicExecutor {
    held: InstalledTransaction,
    /// Where a block of its own on a framework pool unit commits.
    on_pool: Option<PoolCommit>,
}

impl AtomicExecutor {
    /// A block running on a transaction of its own.
    pub(crate) fn own(transaction: Arc<LazyTransaction>) -> Self {
        Self {
            held: InstalledTransaction::Lazy(transaction),
            on_pool: None,
        }
    }

    /// A block running on a transaction of its own, opened on a pool unit,
    /// whose commit runs where `on_pool` says.
    pub(crate) fn own_on_pool(transaction: Arc<LazyTransaction>, on_pool: PoolCommit) -> Self {
        Self {
            held: InstalledTransaction::Lazy(transaction),
            on_pool: Some(on_pool),
        }
    }

    /// A block running inside a savepoint of its enclosing unit.
    pub(crate) fn savepoint(savepoint: Arc<Savepoint>) -> Self {
        Self {
            held: InstalledTransaction::Savepoint(savepoint),
            on_pool: None,
        }
    }
}

impl nest_rs_database::Executor for AtomicExecutor {
    fn holds(&self) -> Holds<'_> {
        Holds::Transaction(&self.held)
    }
}

impl AtomicBlock for AtomicExecutor {
    fn settle(
        self: Arc<Self>,
        ok: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), AtomicError>> + Send>> {
        let (held, on_pool) = (self.held.clone(), self.on_pool.clone());
        // A handle still held past this point is one that escaped the block.
        drop(self);
        Box::pin(async move {
            match held {
                InstalledTransaction::Savepoint(savepoint) => savepoint.settle(ok).await,
                InstalledTransaction::Lazy(own) => match on_pool {
                    Some(on_pool) if ok && own.is_opened() => on_pool.commit(own).await,
                    _ => settled(own.finalize(ok).await),
                },
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
        FinalizeOutcome::CommitFailed(err) => {
            Err(AtomicError::Database(Box::new(crate::RepoError::from(err))))
        }
        FinalizeOutcome::Poisoned { retryable } => Err(AtomicError::Poisoned { retryable }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A refused `COMMIT` reaches the port's `AtomicError::Database` as the
    /// framework's database failure: no reader of the field prints the row.
    #[test]
    fn a_refused_commit_hands_the_port_a_failure_naming_no_value() {
        let refused = settled(FinalizeOutcome::CommitFailed(
            crate::error::tests::taken_email(),
        ));
        let Err(AtomicError::Database(held)) = refused else {
            panic!("a refused commit is the database's failure: {refused:?}");
        };
        let printed = nest_rs_testing::printed(&*held);
        assert!(printed.contains("23505"), "{printed}");
        assert!(!printed.contains("ada@example.com"), "{printed}");
    }
}

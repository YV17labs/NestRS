//! A job has no caller, hence no ambient ability: its `Repo` reads and writes
//! are unscoped, as system work with no principal to scope to.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nest_rs_core::injectable;
use nest_rs_worker::{JobContext, JobSettlement, JobTransaction, Unhonoured};
use sea_orm::DatabaseConnection;

use crate::executor::{Executor, FinalizeOutcome, LazyTransaction, with_job_executor};

/// The label `finalize` logs under: not `queue`, since the scheduler runs
/// through this bridge too.
const TRANSPORT: &str = "worker";

// What could not be done, never why the database refused: `finalize` already
// logs that, with the error.
const ESCAPED: &str =
    "the job's transaction handle outlived the attempt, so nothing it wrote could be committed";
const POISONED: &str =
    "a statement failed inside the job's transaction, so nothing it wrote could be committed";
const COMMIT_FAILED: &str = "the job's transaction could not be committed";

/// Installs the request-less executor around a worker job. Bound to
/// `dyn JobContext` by [`SeaOrmDatabaseModule`](crate::SeaOrmDatabaseModule).
#[injectable]
#[derive(Clone)]
pub struct WorkerDbContext {
    #[inject]
    db: Arc<DatabaseConnection>,
}

impl JobContext for WorkerDbContext {
    fn scope<'a>(
        &'a self,
        transaction: JobTransaction,
        inner: Pin<Box<dyn Future<Output = bool> + Send + 'a>>,
    ) -> Pin<Box<dyn Future<Output = JobSettlement> + Send + 'a>> {
        match transaction {
            JobTransaction::Pool => Box::pin(async move {
                with_job_executor(Executor::Pool((*self.db).clone()), inner).await;
                JobSettlement::Settled
            }),
            JobTransaction::PerAttempt => Box::pin(async move {
                let lazy = Arc::new(LazyTransaction::new((*self.db).clone(), TRANSPORT));
                let success = with_job_executor(Executor::Lazy(lazy.clone()), inner).await;
                match lazy.finalize(success).await {
                    FinalizeOutcome::NoTransaction
                    | FinalizeOutcome::Committed
                    | FinalizeOutcome::RolledBack => JobSettlement::Settled,
                    // Deterministic: the job spawned a task holding the
                    // executor, and the next attempt spawns the same one.
                    FinalizeOutcome::Escaped => {
                        if success {
                            JobSettlement::Unhonoured(Unhonoured::deterministic(ESCAPED))
                        } else {
                            JobSettlement::Settled
                        }
                    }
                    // The job swallowed a failed statement and returned `Ok`:
                    // nothing it wrote could land.
                    FinalizeOutcome::Poisoned { retryable } => {
                        JobSettlement::Unhonoured(Unhonoured {
                            reason: POISONED,
                            retryable,
                        })
                    }
                    // A commit lost mid-`COMMIT` may have landed, so only a
                    // serialization failure or a deadlock is replayed.
                    FinalizeOutcome::CommitFailed(err) => {
                        let retryable = err.is_retryable_conflict();
                        tracing::error!(
                            target: crate::TARGET,
                            transport = TRANSPORT,
                            error = %nest_rs_core::error_message(&err),
                            retryable,
                            "job transaction commit failed",
                        );
                        JobSettlement::Unhonoured(Unhonoured {
                            reason: COMMIT_FAILED,
                            retryable,
                        })
                    }
                }
            }),
        }
    }
}

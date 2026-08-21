//! [`WorkerDbContext`] — ORM bridge for the worker-transport [`JobContext`]
//! seam (queue + schedule), the cron/queue counterpart of
//! [`DbContext`](crate::DbContext). Auto-bound by
//! [`SeaOrmDatabaseModule`](crate::SeaOrmDatabaseModule).
//!
//! A job runs in **one transaction per attempt** by default, opened lazily on
//! its first data-layer touch and settled by the same
//! [`LazyTransaction::finalize`] every request-carrying edge settles through —
//! so commit, rollback, an escaped handle and a failed commit mean the same
//! thing on a queue job as they do on an HTTP request.
//!
//! [`JobTransaction::Pool`] is the opt-out for the job that brackets long
//! non-database work, and it is what every job did before.
//!
//! No caller ⇒ no ambient ability ⇒ `Repo` reads/writes are unscoped — correct
//! for system work with no principal to scope to, and unchanged by any of this.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nest_rs_core::injectable;
use nest_rs_worker::{JobContext, JobSettlement, JobTransaction, Unhonoured};
use sea_orm::DatabaseConnection;

use crate::executor::{Executor, FinalizeOutcome, LazyTransaction, with_job_executor};

/// The span target and the label `finalize` logs its outcomes under. Not
/// `nest_rs::queue`: this bridge serves the scheduler too, and a rollback in a
/// *cron* job misattributed to the queue concern is a log nobody can act on.
const TRANSPORT: &str = "worker";

/// The three sentences an unhonourable attempt reports, one per settlement the
/// context cannot honour. They name what could not be done, never why the
/// database refused: `finalize` and the arm below already log that at `error`,
/// with the error itself, and a dead-letter record repeating it in prose is the
/// same fact said twice in the place with less of it.
const ESCAPED: &str =
    "the job's transaction handle outlived the attempt, so nothing it wrote could be committed";
const POISONED: &str =
    "a statement failed inside the job's transaction, so nothing it wrote could be committed";
const COMMIT_FAILED: &str = "the job's transaction could not be committed";

/// Installs the request-less executor around a worker job. Bound to
/// `dyn JobContext` by [`SeaOrmDatabaseModule`](crate::SeaOrmDatabaseModule).
#[injectable]
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
                // Nothing was opened, so there is nothing to settle: every
                // statement already committed on its own.
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
                    // A handle that escaped into a spawned task cannot be
                    // committed. If the job failed anyway the rollback is the
                    // right answer and its own error stands; if it succeeded,
                    // saying so would report writes that were never made.
                    //
                    // Deterministic: what escaped is the job's own shape — it
                    // spawned a task holding the executor — so the next attempt
                    // spawns the same one.
                    FinalizeOutcome::Escaped => {
                        if success {
                            JobSettlement::Unhonoured(Unhonoured::deterministic(ESCAPED))
                        } else {
                            JobSettlement::Settled
                        }
                    }
                    // A statement failed inside the attempt and the job
                    // returned `Ok` anyway — the most common shape being a job
                    // that catches a `DbErr` and carries on. Nothing it wrote
                    // could land, so the attempt is a failure and the retry
                    // gets a clean slate. `finalize` already logged it, and
                    // classified it from the error the statement actually
                    // raised.
                    FinalizeOutcome::Poisoned { retryable } => {
                        JobSettlement::Unhonoured(Unhonoured {
                            reason: POISONED,
                            retryable,
                        })
                    }
                    // The one classification that decides a retry budget, and
                    // the database is what decides it: a serialization failure
                    // or a deadlock is worth another attempt, a deferred
                    // constraint violation fails identically forever, and a
                    // commit whose outcome is unknown (the connection lost
                    // mid-`COMMIT`) may have landed — so none of the last two
                    // are replayed.
                    FinalizeOutcome::CommitFailed(err) => {
                        let retryable = err.is_retryable_conflict();
                        tracing::error!(
                            target: crate::TARGET,
                            transport = TRANSPORT,
                            error = %err,
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

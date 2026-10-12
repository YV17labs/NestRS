//! Worker-execution ambient-data seam. A worker transport (`Scheduler`,
//! `QueueWorker`) resolves an optional [`JobContext`] from the container and
//! wraps each job in it; with nothing bound a job runs bare.

/// This crate's span target.
pub const TARGET: &str = "nest_rs::worker";

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::error::Unhonoured;

/// How one job attempt's data-layer work is settled, declared on the job's
/// decorator (`transactional = …`).
///
/// It can grow within 7.x, so a driver's `match` keeps a `_` arm that refuses
/// the job, answering [`Unhonoured::deterministic`]:
///
/// ```compile_fail,E0004
/// use nest_rs_worker::JobTransaction;
///
/// fn opens_one(transaction: JobTransaction) -> bool {
///     match transaction {
///         JobTransaction::PerAttempt => true,
///         JobTransaction::Pool => false,
///     }
/// }
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum JobTransaction {
    /// **The default.** One transaction per attempt, opened on the job's first
    /// data-layer touch, committed when the job returns `Ok` and rolled back
    /// otherwise. A job that never touches the database never opens one.
    #[default]
    PerAttempt,
    /// The connection pool, with no transaction — each statement commits on its
    /// own. For a job that would pin a pooled connection across minutes of other
    /// work; it owns its own consistency.
    Pool,
}

/// What the context could do with a job — read by [`run_in_job_context`] and
/// by nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum JobSettlement {
    /// The job's outcome stands: committed, rolled back, or nothing to settle.
    Settled,
    /// The job reported success and the context could **not** honour it, or
    /// the context could not run it at all: the attempt becomes the failure the
    /// [`Unhonoured`] describes.
    Unhonoured(Unhonoured),
}

/// Wraps one worker job's execution with ambient context installed.
pub trait JobContext: Send + Sync + 'static {
    /// Wrap `inner` with the ambient context installed for its duration, then
    /// settle whatever that context opened.
    ///
    /// `inner` yields **whether the job succeeded**, which is all a context
    /// needs to choose commit over rollback; the job's actual output is carried
    /// across the seam by [`run_in_job_context`].
    ///
    /// # Contract
    ///
    /// An impl **must** poll `inner` to completion before returning — normally
    /// by `.await`ing it inside whatever ambient it installs — or refuse the
    /// job without polling it, answering [`JobSettlement::Unhonoured`] (a
    /// `transaction` it does not support). One that returns
    /// [`JobSettlement::Settled`] for a job it never ran fails that job:
    /// [`run_in_job_context`] logs an error on `nest_rs::worker` and unwinds
    /// into the transport's per-job boundary.
    fn scope<'a>(
        &'a self,
        transaction: JobTransaction,
        inner: Pin<Box<dyn Future<Output = bool> + Send + 'a>>,
    ) -> Pin<Box<dyn Future<Output = JobSettlement> + Send + 'a>>;
}

/// What a boot refusing two job contexts tells the reader; a binding declares
/// `Arc<dyn JobContext>` carrying it (`ContainerBuilder::provide_declared_factory`).
pub const BACKEND_REMEDY: &str = "Import exactly one job context binding: \
     `nest_rs::seaorm::SeaOrmDatabaseModule` runs each job over SeaORM's executor; with none, \
     a job runs bare.";

/// Run `fut` inside `ctx` when one is bound, preserving its output. With no
/// context (`None`) the future runs bare and `transaction` is moot — there is
/// nothing bound that could open one.
///
/// `succeeded` reads the transport's own outcome type to decide commit vs
/// rollback; `unhonoured` builds that transport's failure, in its own
/// vocabulary, for the one case where a *successful* job cannot be honoured.
pub async fn run_in_job_context<T: Send>(
    ctx: Option<&Arc<dyn JobContext>>,
    transaction: JobTransaction,
    fut: impl Future<Output = T> + Send,
    succeeded: fn(&T) -> bool,
    unhonoured: fn(Unhonoured) -> T,
) -> T {
    match ctx {
        None => fut.await,
        Some(ctx) => {
            let mut out: Option<T> = None;
            let slot = &mut out;
            let settlement = ctx
                .scope(
                    transaction,
                    Box::pin(async move {
                        let value = fut.await;
                        let ok = succeeded(&value);
                        *slot = Some(value);
                        ok
                    }),
                )
                .await;
            match (out, settlement) {
                (Some(value), JobSettlement::Settled) => value,
                // Ran: only reachable when the job reported success — a context
                // never withholds a failure the job already declared. Not run:
                // the context refused the job.
                (_, JobSettlement::Unhonoured(why)) => unhonoured(why),
                // A broken `JobContext::scope` never ran the job, and no `T` can be
                // synthesized: unwind into the transport's per-job boundary.
                #[expect(
                    clippy::panic,
                    reason = "a JobContext that returns without running the job breaks its contract; the job's own catch_unwind turns this into a dead letter"
                )]
                (None, JobSettlement::Settled) => {
                    tracing::error!(
                        target: TARGET,
                        job_context = ::std::any::type_name::<dyn JobContext>(),
                        "job context returned without running the job to completion; failing this job",
                    );
                    panic!(
                        "JobContext::scope contract violation: the impl must drive `inner` to \
                         completion before returning (see the nest_rs::worker error event)"
                    );
                }
            }
        }
    }
}

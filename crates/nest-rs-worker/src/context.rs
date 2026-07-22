//! Worker-execution ambient-data seam — the cron/queue counterpart to HTTP's
//! `DbContext` interceptor and WebSocket's `SocketContext`. A worker transport
//! (`Scheduler`, `QueueWorker`) resolves an optional [`JobContext`] from the
//! container and wraps each job, letting e.g. `nest-rs-seaorm`'s
//! `WorkerDbContext` install a pool executor so a job's `Repo` calls join a
//! connection without injecting one. With nothing bound a job runs bare.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Wraps one worker job's execution with ambient context installed.
pub trait JobContext: Send + Sync + 'static {
    /// Wrap `inner` with the ambient context installed for its duration. The
    /// inner future yields `()`; a job's own result is preserved across this
    /// seam by [`run_in_job_context`] so the trait stays free of the
    /// transport's result/error type.
    ///
    /// # Contract
    ///
    /// An impl **must** poll `inner` to completion before returning — normally
    /// by `.await`ing it inside whatever ambient it installs (a `task_local!`
    /// scope, a span, …). `inner` *is* the job: returning without driving it to
    /// completion means the job never ran, and there is then no result to hand
    /// back. [`run_in_job_context`] cannot synthesize one for an arbitrary
    /// output type, so it treats a broken impl as a failure of **that single
    /// job** — it logs an error on `nest_rs::worker` and unwinds. The unwind is
    /// isolated by the transport's per-job boundary (the queue worker's
    /// `CatchPanicLayer`, which turns it into a job abort; the scheduler's
    /// per-job task), so one bad impl fails its own job while the worker keeps
    /// consuming. A correct impl always awaits `inner`.
    fn scope<'a>(
        &'a self,
        inner: Pin<Box<dyn Future<Output = ()> + Send + 'a>>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

/// Run `fut` inside `ctx` when one is bound, preserving its output. With no
/// context (`None`) the future runs bare.
pub async fn run_in_job_context<T: Send>(
    ctx: Option<&Arc<dyn JobContext>>,
    fut: impl Future<Output = T> + Send,
) -> T {
    match ctx {
        None => fut.await,
        Some(ctx) => {
            let mut out: Option<T> = None;
            let slot = &mut out;
            ctx.scope(Box::pin(async move {
                *slot = Some(fut.await);
            }))
            .await;
            match out {
                Some(value) => value,
                // Broken `JobContext::scope` impl: it returned without ever
                // driving `inner` to completion, so the job never ran and there
                // is no `T` to return (a job's output cannot be synthesized for
                // an arbitrary type). Fail *this* job, not the worker: record
                // the contract violation, then unwind — the transport's per-job
                // boundary (the queue worker's `CatchPanicLayer`, → job abort;
                // the scheduler's per-job task) catches it, so the consumer
                // loop keeps running instead of the whole worker going down.
                None => {
                    // `nest_rs::worker`, not `nest_rs::queue`: this seam is
                    // shared by the queue worker AND the scheduler — a broken
                    // context in a *scheduled* job must not be misattributed
                    // to the queue concern.
                    tracing::error!(
                        target: "nest_rs::worker",
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

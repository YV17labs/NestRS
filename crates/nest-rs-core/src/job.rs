//! Worker-execution ambient-data seam — the cron/queue counterpart to HTTP's
//! `DbContext` interceptor and WebSocket's `SocketContext`. A worker transport
//! (`Scheduler`, `QueueWorker`) resolves an optional [`JobContext`] from the
//! container and wraps each job, letting e.g. `nestrs-seaorm`'s
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
            out.expect("the job-context scope ran the inner future")
        }
    }
}

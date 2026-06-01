//! [`JobContext`] — the worker-execution ambient-data seam, the cron/queue
//! counterpart to HTTP's `DbContext` interceptor and WebSocket's `SocketContext`.
//!
//! A scheduled tick (`#[cron_job]`) and a queue job (`#[processor]`) run on a
//! worker task with no request behind them, so the ORM executor task-local an HTTP
//! request installs is absent — a job that wants to query through `Repo` has no
//! ambient connection. `JobContext` is the orm/authz-agnostic hook a worker
//! transport resolves from the container ([`Container::get_dyn`](crate::Container::get_dyn))
//! and wraps each job execution with; `nestrs-database`'s `WorkerDbContext` implements it
//! to install a pool executor, so a job's `Repo` calls join a connection without
//! injecting one. With nothing bound (no database module imported) a job runs bare
//! — the current default.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Wraps one worker job's execution with ambient context installed (e.g. an ORM
/// executor). A worker transport ([`Scheduler`], [`QueueWorker`]) resolves an
/// optional implementor from the container and wraps each job through it.
///
/// [`Scheduler`]: https://docs.rs/nestrs-schedule
/// [`QueueWorker`]: https://docs.rs/nestrs-queue
pub trait JobContext: Send + Sync + 'static {
    /// Wrap `inner` — one job's execution — with the ambient context installed for
    /// its duration. The inner future yields `()`; a job's own result is preserved
    /// across this seam by [`run_in_job_context`], not threaded through here, so the
    /// trait stays free of the transport's result/error type.
    fn scope<'a>(
        &'a self,
        inner: Pin<Box<dyn Future<Output = ()> + Send + 'a>>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

/// Run `fut` inside `ctx` when one is bound, preserving its output. With no context
/// (`None`) the future runs bare. The result is captured across the unit-returning
/// [`JobContext::scope`] seam, so a transport keeps a job's `Result` for retry or
/// logging while the seam itself never names that type.
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

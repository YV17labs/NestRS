//! Worker-execution ambient-data seam — the cron/queue counterpart to HTTP's
//! `DbContext` interceptor and WebSocket's `SocketContext`. A worker transport
//! (`Scheduler`, `QueueWorker`) resolves an optional [`JobContext`] from the
//! container and wraps each job, letting e.g. `nest-rs-seaorm`'s
//! `WorkerDbContext` install an executor so a job's `Repo` calls join a
//! connection without injecting one. With nothing bound a job runs bare.

/// This crate's span target.
///
/// Declared here, like every crate's: a target names **where** an event came
/// from, so the crate that **owns** the concern names it and everything emitting
/// on it reads the constant. Re-exported at the root as `nest_rs_worker::TARGET`,
/// which is the path callers use.
pub const TARGET: &str = "nest_rs::worker";

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// How one job attempt's data-layer work is settled.
///
/// A worker job has no safe/mutating method to classify the way an HTTP verb or
/// a WebSocket message does — which is why this is *declared*, at the
/// `#[process]` / `#[every]` / `#[cron]` / `#[after]` that already declares
/// everything else about the job, rather than inferred from something the
/// framework cannot see.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JobTransaction {
    /// **The default.** One transaction per attempt, opened on the job's first
    /// data-layer touch, committed when the job returns `Ok` and rolled back
    /// otherwise.
    ///
    /// A job that fails halfway would otherwise leave its writes behind for the
    /// **retry** to find and repeat — a silent partial write, which is a
    /// correctness defect with no knob. Holding a pooled connection for the
    /// attempt is the cost, and it is a tuning problem with one: see
    /// [`Pool`](Self::Pool). The transaction is *lazy*, so a job that never
    /// touches the database never opens one and pays nothing.
    #[default]
    PerAttempt,
    /// The connection pool, with no transaction — each statement commits on its
    /// own, which is what every job did before this existed.
    ///
    /// For the job whose shape defeats the default: read, then minutes of work
    /// that is not the database's, then write. `PerAttempt` would pin a pooled
    /// connection across the middle. Such a job owns its own consistency, and an
    /// idempotency key is what makes its retry safe — the transaction never was.
    Pool,
}

/// What the context could do with a finished job — read by
/// [`run_in_job_context`] and by nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobSettlement {
    /// The job's outcome stands: committed, rolled back, or nothing to settle.
    Settled,
    /// The job reported success and the context could **not** honour it.
    /// Reporting success here would lose the writes silently, so the attempt is
    /// turned into the failure the [`Unhonoured`] describes.
    Unhonoured(Unhonoured),
}

/// A successful job its context could not honour: what went wrong, and whether
/// running the same attempt again could end differently.
///
/// **The classification is the whole point of carrying a value here.** A job's
/// retry replays its body, including every side effect that is not the
/// database's — an HTTP call, an S3 write, a mail. Spending a retry budget on a
/// failure that will repeat identically pays that price several times over and
/// dead-letters anyway, which is why `#[process]` already aborts on its three
/// other deterministic failures (an unsupported wire version, an
/// undeserializable payload, a missing provider).
///
/// A context answers from what the database reported, never from a guess: a
/// serialization failure or a deadlock is retryable, a constraint violation is
/// not, and a commit whose outcome is *unknown* — the connection lost mid-`COMMIT`
/// — is not either. That last one is the deliberate asymmetry: the transaction
/// may have landed, and a framework that replays it turns "may have written
/// once" into "wrote twice". Only a failure the framework knows rolled back is
/// worth repeating.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unhonoured {
    /// What the context could not do, as the sentence the transport reports.
    /// `'static` because the database's own error is logged at `error` by the
    /// context that saw it — repeating it in the job's failure would say the
    /// same thing twice, in the place with less of it.
    pub reason: &'static str,
    /// Whether re-running the attempt could produce a different outcome. `false`
    /// is *deterministic*: the retry re-fails identically, having replayed
    /// everything the job body does outside the transaction.
    pub retryable: bool,
}

impl Unhonoured {
    /// A failure a retry could clear.
    pub const fn retryable(reason: &'static str) -> Self {
        Self {
            reason,
            retryable: true,
        }
    }

    /// A failure a retry would only repeat.
    pub const fn deterministic(reason: &'static str) -> Self {
        Self {
            reason,
            retryable: false,
        }
    }
}

impl std::fmt::Display for Unhonoured {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason)
    }
}

impl std::error::Error for Unhonoured {}

/// Wraps one worker job's execution with ambient context installed.
pub trait JobContext: Send + Sync + 'static {
    /// Wrap `inner` with the ambient context installed for its duration, then
    /// settle whatever that context opened.
    ///
    /// `inner` yields **whether the job succeeded**, which is all a context
    /// needs to choose commit over rollback and is what keeps this trait free of
    /// the transport's own result/error type; the job's actual output is carried
    /// across the seam by [`run_in_job_context`].
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
        transaction: JobTransaction,
        inner: Pin<Box<dyn Future<Output = bool> + Send + 'a>>,
    ) -> Pin<Box<dyn Future<Output = JobSettlement> + Send + 'a>>;
}

/// Run `fut` inside `ctx` when one is bound, preserving its output. With no
/// context (`None`) the future runs bare and `transaction` is moot — there is
/// nothing bound that could open one.
///
/// `succeeded` reads the transport's own outcome type to decide commit vs
/// rollback; `unhonoured` builds that transport's failure for the one case where
/// a *successful* job cannot be honoured. Same two functions, for the same
/// reason, as `nest_rs_seaorm`'s `with_data_context` on the request-carrying
/// edges — the settling rule is one rule, and a job is not an exception to it.
///
/// `unhonoured` is handed the [`Unhonoured`] so the transport can render the
/// classification in its own vocabulary — the queue as `JobError::retryable`,
/// the schedule as the sentence it logs, because a schedule has no retry budget
/// to spend and fires again at its next occurrence either way.
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
            match out {
                Some(value) => match settlement {
                    JobSettlement::Settled => value,
                    // Only reachable when the job reported success — a context
                    // never withholds a failure the job already declared.
                    JobSettlement::Unhonoured(why) => unhonoured(why),
                },
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

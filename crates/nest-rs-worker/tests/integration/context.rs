//! [`JobContext`] exercised through `run_in_job_context`: a bound context
//! installs its ambient for the wrapped job, the job's result is preserved
//! across the `bool`-returning `scope`, and a context that cannot honour a
//! successful job turns it into a failure. No context runs the job bare.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nest_rs_worker::{JobContext, JobSettlement, JobTransaction, Unhonoured, run_in_job_context};

tokio::task_local! {
    static MARKER: u32;
}

struct MarkerContext(u32);

impl JobContext for MarkerContext {
    fn scope<'a>(
        &'a self,
        _transaction: JobTransaction,
        inner: Pin<Box<dyn Future<Output = bool> + Send + 'a>>,
    ) -> Pin<Box<dyn Future<Output = JobSettlement> + Send + 'a>> {
        Box::pin(async move {
            MARKER.scope(self.0, inner).await;
            JobSettlement::Settled
        })
    }
}

fn observe_marker() -> Option<u32> {
    MARKER.try_with(|m| *m).ok()
}

#[tokio::test]
async fn runs_inside_the_bound_context_and_preserves_the_result() {
    let ctx: Arc<dyn JobContext> = Arc::new(MarkerContext(42));
    let seen = run_in_job_context(
        Some(&ctx),
        JobTransaction::PerAttempt,
        async { observe_marker() },
        Option::is_some,
        |_| None,
    )
    .await;
    assert_eq!(
        seen,
        Some(42),
        "the job observes the context's ambient value"
    );
}

#[tokio::test]
async fn runs_bare_without_a_context() {
    let seen = run_in_job_context::<Option<u32>>(
        None,
        JobTransaction::PerAttempt,
        async { observe_marker() },
        Option::is_some,
        |_| None,
    )
    .await;
    assert_eq!(
        seen, None,
        "with no context the job runs without any ambient"
    );
}

/// A context that reports it could not settle what the job did — the shape a
/// failed commit or an escaped transaction handle takes — carrying the
/// classification it reached.
struct UnhonouringContext(Unhonoured);

impl JobContext for UnhonouringContext {
    fn scope<'a>(
        &'a self,
        _transaction: JobTransaction,
        inner: Pin<Box<dyn Future<Output = bool> + Send + 'a>>,
    ) -> Pin<Box<dyn Future<Output = JobSettlement> + Send + 'a>> {
        Box::pin(async move {
            inner.await;
            JobSettlement::Unhonoured(self.0)
        })
    }
}

async fn unhonoured_outcome(why: Unhonoured) -> Result<&'static str, Unhonoured> {
    let ctx: Arc<dyn JobContext> = Arc::new(UnhonouringContext(why));
    run_in_job_context(
        Some(&ctx),
        JobTransaction::PerAttempt,
        async { Ok("the job's own success") },
        Result::is_ok,
        Err,
    )
    .await
}

#[tokio::test]
async fn a_job_the_context_cannot_settle_is_reported_as_failed() {
    // The whole point: the job body succeeded, and reporting that would claim
    // writes that were never committed. The transport's failure stands in.
    let outcome = unhonoured_outcome(Unhonoured::retryable("not committed")).await;
    assert_eq!(
        outcome,
        Err(Unhonoured::retryable("not committed")),
        "a success the context could not honour never reaches the transport as one",
    );
}

#[tokio::test]
async fn the_transport_is_told_whether_repeating_the_attempt_could_help() {
    // A transport with a retry budget spends it replaying the job body, side
    // effects and all. The context is what knows whether that could ever end
    // differently, so it says — rather than the transport assuming one answer.
    let transient = unhonoured_outcome(Unhonoured::retryable("a conflict at commit"))
        .await
        .expect_err("the context could not honour it");
    assert!(
        transient.retryable,
        "a transient conflict is worth another attempt",
    );

    let deterministic = unhonoured_outcome(Unhonoured::deterministic(
        "a constraint violation at commit",
    ))
    .await
    .expect_err("the context could not honour it");
    assert!(
        !deterministic.retryable,
        "a failure that repeats identically is not — the budget would buy nothing \
         and every non-transactional side effect would run again",
    );
    assert_eq!(
        deterministic.to_string(),
        "a constraint violation at commit",
        "and the sentence the context wrote is what the transport reports",
    );
}

/// A contract-breaking impl: returns without ever driving `inner`, so the job
/// never runs.
struct BrokenContext;

impl JobContext for BrokenContext {
    fn scope<'a>(
        &'a self,
        _transaction: JobTransaction,
        _inner: Pin<Box<dyn Future<Output = bool> + Send + 'a>>,
    ) -> Pin<Box<dyn Future<Output = JobSettlement> + Send + 'a>> {
        // Drops `inner` on the floor instead of awaiting it.
        Box::pin(async { JobSettlement::Settled })
    }
}

#[tokio::test]
#[should_panic(expected = "JobContext::scope contract violation")]
async fn broken_context_that_skips_the_job_fails_that_job() {
    // The broken impl fails *this* job — surfaced as a panic the transport's
    // per-job boundary (CatchPanicLayer / per-job task) isolates, so the worker
    // keeps consuming rather than the failure taking down the consumer loop.
    let ctx: Arc<dyn JobContext> = Arc::new(BrokenContext);
    let _ = run_in_job_context(
        Some(&ctx),
        JobTransaction::PerAttempt,
        async { 1u32 },
        |_| true,
        |_| 0u32,
    )
    .await;
}

/// A `JobContext` impl that never drives `inner` — the one contract violation
/// `run_in_job_context` cannot recover from.
///
/// The job's output type is arbitrary, so nothing can be synthesized: the seam
/// fails *this* job and unwinds, and the transport's per-job boundary catches
/// it. Which means the panic message reaches a dead-letter record at best, and
/// on the scheduler side nowhere at all — the error event is what names the
/// offending impl, and `nest_rs::worker` rather than `nest_rs::queue` because
/// the same seam serves the scheduler.
mod a_context_that_never_runs_the_job {
    use std::pin::Pin;
    use std::sync::Arc;

    use nest_rs_testing::LogCapture;
    use nest_rs_worker::{JobContext, JobSettlement, JobTransaction, run_in_job_context};

    struct NeverAwaits;

    impl JobContext for NeverAwaits {
        fn scope<'a>(
            &'a self,
            _transaction: JobTransaction,
            _inner: Pin<Box<dyn Future<Output = bool> + Send + 'a>>,
        ) -> Pin<Box<dyn Future<Output = JobSettlement> + Send + 'a>> {
            // Returns without ever polling `inner`: the job never runs.
            Box::pin(async { JobSettlement::Settled })
        }
    }

    #[tokio::test]
    async fn fails_that_job_and_names_the_impl_at_error() {
        let logs = LogCapture::install();
        let ctx: Arc<dyn JobContext> = Arc::new(NeverAwaits);

        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let unwound = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
            run_in_job_context(
                Some(&ctx),
                JobTransaction::PerAttempt,
                async { 7u8 },
                |_| true,
                |_| 0u8,
            ),
        ))
        .await;
        std::panic::set_hook(previous);

        assert!(
            unwound.is_err(),
            "a job that never ran has no output to hand back",
        );

        let event = logs.expect_one(
            "nest_rs::worker",
            "job context returned without running the job to completion; failing this job",
        );
        assert_eq!(event.level, "error");
        assert!(
            event
                .field("job_context")
                .is_some_and(|c| c.contains("JobContext")),
            "the event names the seam whose contract was broken, got {:?}",
            event.fields,
        );
    }
}

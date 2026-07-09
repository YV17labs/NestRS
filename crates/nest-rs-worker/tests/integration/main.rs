//! [`JobContext`] exercised through `run_in_job_context`: a bound context
//! installs its ambient for the wrapped job and the job's result is preserved
//! across the unit-returning `scope`; no context runs the job bare.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nest_rs_worker::{JobContext, run_in_job_context};

tokio::task_local! {
    static MARKER: u32;
}

struct MarkerContext(u32);

impl JobContext for MarkerContext {
    fn scope<'a>(
        &'a self,
        inner: Pin<Box<dyn Future<Output = ()> + Send + 'a>>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(MARKER.scope(self.0, inner))
    }
}

fn observe_marker() -> Option<u32> {
    MARKER.try_with(|m| *m).ok()
}

#[tokio::test]
async fn runs_inside_the_bound_context_and_preserves_the_result() {
    let ctx: Arc<dyn JobContext> = Arc::new(MarkerContext(42));
    let seen = run_in_job_context(Some(&ctx), async { observe_marker() }).await;
    assert_eq!(
        seen,
        Some(42),
        "the job observes the context's ambient value"
    );
}

#[tokio::test]
async fn runs_bare_without_a_context() {
    let seen = run_in_job_context::<Option<u32>>(None, async { observe_marker() }).await;
    assert_eq!(
        seen, None,
        "with no context the job runs without any ambient"
    );
}

/// A contract-breaking impl: returns without ever driving `inner`, so the job
/// never runs.
struct BrokenContext;

impl JobContext for BrokenContext {
    fn scope<'a>(
        &'a self,
        _inner: Pin<Box<dyn Future<Output = ()> + Send + 'a>>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        // Drops `inner` on the floor instead of awaiting it.
        Box::pin(async {})
    }
}

#[tokio::test]
#[should_panic(expected = "JobContext::scope contract violation")]
async fn broken_context_that_skips_the_job_fails_that_job() {
    // The broken impl fails *this* job — surfaced as a panic the transport's
    // per-job boundary (CatchPanicLayer / per-job task) isolates, so the worker
    // keeps consuming rather than the failure taking down the consumer loop.
    let ctx: Arc<dyn JobContext> = Arc::new(BrokenContext);
    let _ = run_in_job_context(Some(&ctx), async { 1u32 }).await;
}

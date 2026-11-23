//! Covers `src/policy.rs` through the HTTP edge: an attempt ends by the
//! request's deadline, before the policy's own timeout, so its client is told
//! by the policy and its breaker counts the attempt.

use std::sync::Arc;
use std::time::Duration;

use http::StatusCode;
use nest_rs_core::{Problem, ToProblem, module};
use nest_rs_http::{HttpConfig, HttpModule, controller, routes};
use nest_rs_resilience::{ResilienceConfig, ResilienceError, ResilienceModule, ResiliencePolicy};
use nest_rs_testing::TestApp;

#[derive(Debug, thiserror::Error)]
#[error("the upstream never answers")]
struct Silent;

impl ToProblem for Silent {
    fn to_problem(&self) -> Option<Problem> {
        None
    }
}

#[controller(path = "/slow")]
struct SlowController {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
}

#[routes]
impl SlowController {
    #[get("/")]
    #[public]
    async fn wait(&self) -> Result<String, ResilienceError<Silent>> {
        self.upstream
            .run(|_| std::future::pending::<Result<String, Silent>>())
            .await
    }
}

#[module(
    imports = [
        HttpModule::for_root(HttpConfig {
            request_timeout: Some(Duration::from_secs(1)),
            ..HttpConfig::default()
        }),
        ResilienceModule::for_root(ResilienceConfig {
            timeout: Duration::from_secs(10),
            retries: None,
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: Duration::from_secs(30),
            ..ResilienceConfig::default()
        }),
    ],
    providers = [SlowController],
)]
struct SlowModule;

#[tokio::test(start_paused = true)]
async fn an_attempt_ends_by_the_requests_deadline_and_its_breaker_counts_it() {
    let app = TestApp::for_module::<SlowModule>()
        .await
        .expect("a route over a policy boots");

    let started = tokio::time::Instant::now();
    let given_up = app.http().get("/slow").send().await;
    given_up.assert_status(StatusCode::SERVICE_UNAVAILABLE);
    given_up.assert_header_is_not_exist("retry-after");
    assert_eq!(
        started.elapsed(),
        Duration::from_secs(1),
        "the request's deadline, not the policy's 10 s"
    );

    let refused = app.http().get("/slow").send().await;
    refused.assert_status(StatusCode::SERVICE_UNAVAILABLE);
    refused.assert_header("retry-after", "30");
}

/// What a body below holds across its `.await`, or its closure moves in: past
/// anything a run holds, so a second copy of it shows.
const HELD: usize = 16 * 1024;

/// What a run holds whatever its call: its slot, its attempt's pass and
/// bounds, the timers it waits on.
const RUN: usize = 1024;

/// The size of the future `call` makes, read off its type.
fn made<F, Fut>(_call: &F) -> usize
where
    F: FnOnce(nest_rs_resilience::CancellationToken) -> Fut,
{
    size_of::<Fut>()
}

#[tokio::test]
async fn a_runs_future_holds_its_calls_closure_and_each_attempts_future_once() {
    let (_app, policy) = crate::policy(crate::quiet()).await;
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let holding = |_| async {
        let buffer = [7_u8; HELD];
        tokio::task::yield_now().await;
        Ok::<_, Silent>(std::hint::black_box(&buffer).len())
    };
    let buffer = [7_u8; HELD];
    let moving = move |_| async move {
        tokio::task::yield_now().await;
        Ok::<_, Silent>(std::hint::black_box(&buffer).len())
    };
    for (shape, ran, held) in [
        (
            "run once, its future holding a buffer across an `.await`",
            size_of_val(&policy.run(holding)),
            made(&holding),
        ),
        (
            "run again, its future holding a buffer across an `.await`",
            size_of_val(&policy.run_idempotent(holding)),
            size_of_val(&holding) + made(&holding),
        ),
        (
            "run once, its closure moving a buffer in",
            size_of_val(&policy.run(moving)),
            size_of_val(&moving).max(made(&moving)),
        ),
        (
            "run again, its closure moving a buffer in",
            size_of_val(&policy.run_idempotent(moving)),
            size_of_val(&moving) + made(&moving),
        ),
    ] {
        assert!(
            ran <= held + RUN,
            "in a {profile} build, a call {shape} is a run of {ran} B, for {held} B of its own"
        );
    }
}

/// tokio's default worker stack, which `#[nest_rs::main]`'s runtime keeps.
const WORKER_STACK: usize = 2 * 1024 * 1024;

/// What the calls below hold across their `.await` on that stack: a third of
/// it, which a method holds there undecorated too.
const HELD_ON_A_WORKER: usize = WORKER_STACK / 3;

/// A stack overflow aborts the process: nextest, which runs each test in a
/// process of its own, reports this one failed on `SIGABRT` and runs the rest.
#[test]
fn a_run_holding_a_large_buffer_across_an_await_runs_on_a_worker_of_tokios_default_stack() {
    // A runtime of the test's own, for its worker's stack.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_stack_size(WORKER_STACK)
        .enable_all()
        .build()
        .expect("the runtime builds");
    runtime.block_on(async {
        let (_app, policy) = crate::policy(crate::quiet()).await;
        let holding = |_| async {
            let buffer = [7_u8; HELD_ON_A_WORKER];
            tokio::task::yield_now().await;
            Ok::<_, Silent>(std::hint::black_box(&buffer).len())
        };
        let once = tokio::spawn({
            let policy = Arc::clone(&policy);
            async move { policy.run(holding).await }
        });
        assert!(matches!(once.await, Ok(Ok(HELD_ON_A_WORKER))));
        let again = tokio::spawn(async move { policy.run_idempotent(holding).await });
        assert!(matches!(again.await, Ok(Ok(HELD_ON_A_WORKER))));
    });
}

//! Covers `src/policy.rs`: through the HTTP edge, an attempt ends by the
//! request's deadline, before the policy's own timeout, so its client is told
//! by the policy and its breaker counts the attempt; and, under no edge, what a
//! run's future holds — its size against its call's, what it allocates, and a
//! large one run on a worker of tokio's default stack.

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

    /// A call whose first poll takes time — a request built, a handshake
    /// begun — before it waits.
    #[get("/busy")]
    #[public]
    async fn busy(&self) -> Result<String, ResilienceError<Silent>> {
        self.upstream
            .run(|_| async {
                tokio::time::advance(Duration::from_millis(3)).await;
                std::future::pending::<Result<String, Silent>>().await
            })
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

/// The policy gives the attempt up at the request's deadline, however long its
/// call took to first wait, so the edge's own cut there finds it counted.
#[tokio::test(start_paused = true)]
async fn an_attempt_ends_by_the_requests_deadline_and_its_breaker_counts_it() {
    for path in ["/slow", "/slow/busy"] {
        let app = TestApp::for_module::<SlowModule>()
            .await
            .expect("a route over a policy boots");

        let started = tokio::time::Instant::now();
        let given_up = app.http().get(path).send().await;
        given_up.assert_status(StatusCode::SERVICE_UNAVAILABLE);
        given_up.assert_header_is_not_exist("retry-after");
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(1),
            "{path}: the request's deadline, not the policy's 10 s"
        );

        let refused = app.http().get(path).send().await;
        refused.assert_status(StatusCode::SERVICE_UNAVAILABLE);
        refused.assert_header("retry-after", "30");
    }
}

/// What a body below holds across its `.await`, or its closure moves in: past
/// anything a run holds, so a second copy of it shows.
const HELD: usize = 16 * 1024;

/// What the rows under tokio's threshold hold: a quarter of it.
const UNDER: usize = 4 * 1024;

/// tokio's threshold for a future it spawns in a release build, which
/// `src/call.rs` mirrors for a run's closure.
const THRESHOLD: usize = 16 * 1024;

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

/// Each shape of a call holding or moving `N` bytes: the size of its run, and
/// what the call holds of its own.
fn shapes<const N: usize>(policy: &ResiliencePolicy) -> [(&'static str, usize, usize, usize); 4] {
    let holding = |_| async {
        let buffer = [7_u8; N];
        tokio::task::yield_now().await;
        Ok::<_, Silent>(std::hint::black_box(&buffer).len())
    };
    let buffer = [7_u8; N];
    let moving = move |_| async move {
        tokio::task::yield_now().await;
        Ok::<_, Silent>(std::hint::black_box(&buffer).len())
    };
    [
        (
            "run once, its future holding a buffer across an `.await`",
            N,
            size_of_val(&policy.run(holding)),
            made(&holding),
        ),
        (
            "run again, its future holding a buffer across an `.await`",
            N,
            size_of_val(&policy.run_idempotent(holding)),
            size_of_val(&holding) + made(&holding),
        ),
        (
            "run once, its closure moving a buffer in",
            N,
            size_of_val(&policy.run(moving)),
            size_of_val(&moving).max(made(&moving)),
        ),
        (
            "run again, its closure moving a buffer in",
            N,
            size_of_val(&policy.run_idempotent(moving)),
            size_of_val(&moving) + made(&moving),
        ),
    ]
}

/// A release build holds a call's closure and each attempt's future in place,
/// once, where a debug build boxes both and holds neither.
#[tokio::test]
async fn a_runs_future_holds_its_calls_closure_and_each_attempts_future_once() {
    let (_app, policy) = crate::policy(crate::quiet()).await;
    for (shape, bytes, ran, held) in shapes::<HELD>(&policy)
        .into_iter()
        .chain(shapes::<UNDER>(&policy))
    {
        if cfg!(debug_assertions) {
            assert!(
                ran <= RUN,
                "in a debug build, a call {shape}, of {bytes} B, is a run of {ran} B, holding \
                 {held} B of its own"
            );
        } else {
            assert!(
                held <= ran && ran <= held + RUN,
                "in a release build, a call {shape}, of {bytes} B, is a run of {ran} B, for \
                 {held} B of its own"
            );
        }
    }
}

/// tokio's default worker stack, which `#[nest_rs::main]`'s runtime keeps.
const WORKER_STACK: usize = 2 * 1024 * 1024;

/// What the calls below hold across their `.await` on that stack: a third of
/// it, which a method holds there undecorated too.
const HELD_ON_A_WORKER: usize = WORKER_STACK / 3;

/// A runtime of a test's own, one worker on tokio's default stack.
fn worker() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_stack_size(WORKER_STACK)
        .enable_all()
        .build()
        .expect("the runtime builds")
}

/// A stack overflow aborts the process: nextest, which runs each test in a
/// process of its own, reports this one failed on `SIGABRT` and runs the rest.
#[test]
fn a_run_holding_a_large_buffer_across_an_await_runs_on_a_worker_of_tokios_default_stack() {
    worker().block_on(async {
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

/// What the call below moves into its closure on that stack: three eighths of
/// it.
#[cfg(not(debug_assertions))]
const MOVED_ON_A_WORKER: usize = WORKER_STACK * 3 / 8;

/// A call sent once whose closure moves a buffer in.
// Inlined into the task that awaits it, the two frames merge and the copy a boxed
// closure saves no longer shows.
#[cfg(not(debug_assertions))]
#[inline(never)]
async fn moving_once<const N: usize>(
    policy: &ResiliencePolicy,
) -> Result<usize, ResilienceError<Silent>> {
    let buffer = [7_u8; N];
    policy
        .run(move |_| async move {
            tokio::task::yield_now().await;
            Ok(std::hint::black_box(&buffer).len())
        })
        .await
}

/// A release build boxes a closure past tokio's threshold, with the future a
/// call sent once makes of it, so a closure moving a large buffer in runs on a
/// worker's stack; a debug build boxes every closure. A stack overflow aborts,
/// as above.
#[cfg(not(debug_assertions))]
#[test]
fn a_call_sent_once_moving_a_large_buffer_into_its_closure_runs_on_a_worker_of_tokios_default_stack()
 {
    worker().block_on(async {
        let (_app, policy) = crate::policy(crate::quiet()).await;
        let once = tokio::spawn(async move { moving_once::<MOVED_ON_A_WORKER>(&policy).await });
        assert!(matches!(once.await, Ok(Ok(MOVED_ON_A_WORKER))));
    });
}

/// Under tokio's threshold a release build holds the call and its future in
/// the run's own future, so a run allocates nothing of its own; a debug build
/// boxes both, a closure holding nothing in a box that allocates nothing.
#[tokio::test]
async fn a_call_answering_at_once_allocates_nothing_but_its_token_in_a_release_build() {
    let (_app, policy) = crate::policy(ResilienceConfig::default()).await;
    let token = crate::token_allocations();
    let boxed = usize::from(cfg!(debug_assertions));
    for (shape, (answered, made)) in [
        (
            "run once",
            crate::allocations(|| policy.run(|_| async { Ok::<_, Silent>(7) })),
        ),
        (
            "run again",
            crate::allocations(|| policy.run_idempotent(|_| async { Ok::<_, Silent>(7) })),
        ),
    ] {
        assert!(matches!(answered, Ok(7)), "{shape}: {answered:?}");
        assert_eq!(
            made,
            token + boxed,
            "a call {shape} allocates its token, {token}, and in a debug build its future's box"
        );
    }
}

/// A release build holds an attempt's future where its run is whatever its
/// size, past tokio's threshold too, so such a call allocates nothing of its
/// own; a debug build boxes it.
#[tokio::test]
async fn a_call_whose_future_is_past_tokios_threshold_allocates_nothing_but_its_token_in_a_release_build()
 {
    let (_app, policy) = crate::policy(ResilienceConfig::default()).await;
    let token = crate::token_allocations();
    let large = |_| async {
        let buffer = [7_u8; HELD];
        std::future::ready(()).await;
        Ok::<_, Silent>(std::hint::black_box(&buffer).len())
    };
    assert!(made(&large) > THRESHOLD, "the future is past the threshold");
    let boxed = usize::from(cfg!(debug_assertions));
    for (shape, (answered, made)) in [
        ("run once", crate::allocations(|| policy.run(large))),
        (
            "run again",
            crate::allocations(|| policy.run_idempotent(large)),
        ),
    ] {
        assert!(matches!(answered, Ok(HELD)), "{shape}: {answered:?}");
        assert_eq!(
            made,
            token + boxed,
            "a call {shape} whose future is past the threshold allocates its token, {token}, and \
             in a debug build its future's box"
        );
    }
}

/// A closure of `N` bytes, the buffer it moves in, making a future that holds
/// only the length it read.
fn closure_of<const N: usize>()
-> impl FnMut(nest_rs_resilience::CancellationToken) -> std::future::Ready<Result<usize, Silent>> {
    let buffer = [7_u8; N];
    move |_| std::future::ready(Ok(std::hint::black_box(&buffer).len()))
}

/// The threshold `src/call.rs` mirrors, held on both sides: in a release
/// build, a closure of exactly tokio's threshold is held where its run is, and
/// one a byte past it is boxed, with the future a call sent once makes of it. A
/// debug build boxes every closure and every future.
#[tokio::test]
async fn a_closure_up_to_tokios_threshold_is_held_in_place_and_one_past_it_boxed_in_a_release_build()
 {
    assert_eq!(size_of_val(&closure_of::<THRESHOLD>()), THRESHOLD);
    let (_app, policy) = crate::policy(ResilienceConfig::default()).await;
    let token = crate::token_allocations();
    // A debug build boxes the closure and the future it makes, whatever their size.
    let in_release = |boxes: usize| if cfg!(debug_assertions) { 2 } else { boxes };
    for (shape, boxes, (answered, made)) in [
        (
            "of the threshold, run once",
            in_release(0),
            crate::allocations(|| policy.run(closure_of::<THRESHOLD>())),
        ),
        (
            "of the threshold, run again",
            in_release(0),
            crate::allocations(|| policy.run_idempotent(closure_of::<THRESHOLD>())),
        ),
        (
            "a byte past it, run once",
            in_release(2),
            crate::allocations(|| policy.run(closure_of::<{ THRESHOLD + 1 }>())),
        ),
        (
            "a byte past it, run again",
            in_release(1),
            crate::allocations(|| policy.run_idempotent(closure_of::<{ THRESHOLD + 1 }>())),
        ),
    ] {
        assert!(answered.is_ok(), "{shape}: {answered:?}");
        assert_eq!(
            made,
            token + boxes,
            "a call whose closure is {shape} allocates its token, {token}, and {boxes} box(es)"
        );
    }
}

//! Covers `src/policy.rs`: through the HTTP edge, an attempt ends by the
//! request's deadline, before the policy's own timeout, so its client is told
//! by the policy and its breaker counts the attempt; through the health edge,
//! by the cut of the indicator sending it, a probe's turn included; and, under
//! no edge, what a
//! run's future holds — its size against its call's, what it allocates, alone,
//! through every mechanism its policy takes and inside a unit of work whose
//! transaction each attempt opens a savepoint of, and a large one run on a
//! worker of tokio's default stack.

use std::sync::Arc;
use std::time::Duration;

use http::StatusCode;
use nest_rs_core::{Problem, ToProblem, injectable, module};
use nest_rs_health::{
    HealthConfig, HealthModule, HealthService, IndicatorStatus, ProbeKind, indicators,
};
use nest_rs_http::{HttpConfig, HttpModule, controller, routes};
use nest_rs_resilience::{
    BreakerState, ResilienceConfig, ResilienceError, ResilienceModule, ResiliencePolicy, resilient,
};
use nest_rs_testing::{HeadlessApp, TestApp};

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

#[injectable]
struct SilentDependency {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
}

#[indicators]
impl SilentDependency {
    #[readiness]
    #[resilient(upstream)]
    async fn dependency_answers(&self) -> Result<(), ResilienceError<Silent>> {
        std::future::pending().await
    }
}

/// How long the breaker below stays open.
const OPEN: Duration = Duration::from_secs(30);

#[module(
    imports = [
        HealthModule,
        ResilienceModule::for_root(ResilienceConfig {
            retries: None,
            breaker_window: Some(2),
            breaker_failures: 2,
            breaker_open: OPEN,
            ..ResilienceConfig::default()
        }),
    ],
    providers = [SilentDependency],
)]
struct SilentHealthModule;

/// The health edge cuts an indicator well before the policy's own timeout: the
/// attempt ends there, a timeout its breaker counts, so a dependency that
/// accepts and never answers opens the breaker a readiness indicator runs
/// under, and a probe sent from one fails rather than being handed on.
#[tokio::test(start_paused = true)]
async fn an_attempt_ends_by_its_indicators_cut_and_its_breaker_counts_it() {
    let cut = HealthConfig::default().indicator_timeout;
    assert!(
        cut < ResilienceConfig::default().timeout,
        "the edge cuts first"
    );
    let app = HeadlessApp::builder()
        .module::<SilentHealthModule>()
        .build_headless()
        .await
        .expect("a decorated indicator boots");
    let health = app.container().get::<HealthService>().expect("provided");
    let upstream = crate::held::<nest_rs_core::Unnamed>(&app);

    let started = tokio::time::Instant::now();
    let probed = health.probe(ProbeKind::Readiness).await;
    assert_eq!(probed.status, IndicatorStatus::Down);
    assert_eq!(started.elapsed(), cut);
    let counted = upstream.breaker_stats().expect("a breaker");
    assert_eq!(
        (counted.outcomes, counted.failures),
        (1, 1),
        "the attempt its indicator cut is a timeout"
    );

    health.probe(ProbeKind::Readiness).await;
    let opened = upstream.breaker_state();
    assert!(
        matches!(opened, Some(BreakerState::Open { retry_after, .. }) if retry_after == OPEN),
        "two timeouts of two: {opened:?}"
    );

    tokio::time::advance(OPEN).await;
    assert_eq!(upstream.breaker_state(), Some(BreakerState::HalfOpen));
    health.probe(ProbeKind::Readiness).await;
    let reopened = upstream.breaker_state();
    assert!(
        matches!(reopened, Some(BreakerState::Open { retry_after, .. }) if retry_after == OPEN),
        "the probe its indicator cut has failed: {reopened:?}"
    );
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
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "pins the allocations a run makes on Linux, where the dev container and CI run it"
)]
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

/// Every mechanism a policy takes, on: the breaker and the retries of the
/// defaults, its outcomes given an age, a bulkhead with a wait and a bound on
/// its waiters, and a rate with a wait, a slot and a token free for every
/// call a test makes.
fn every_mechanism() -> ResilienceConfig {
    ResilienceConfig {
        breaker_age: Some(Duration::from_secs(60)),
        concurrency: Some(8),
        concurrency_wait: Some(Duration::from_secs(1)),
        concurrency_waiters: Some(8),
        rate_limit: Some(1_000),
        rate_window: Duration::from_secs(1),
        rate_wait: Some(Duration::from_secs(1)),
        ..ResilienceConfig::default()
    }
}

/// A host built by hand over a policy, each method answering at once.
struct Answering {
    policy: Arc<ResiliencePolicy>,
}

impl Answering {
    #[resilient(policy)]
    async fn once(&self) -> Result<u32, ResilienceError<Silent>> {
        Ok(7)
    }

    #[resilient(policy, idempotent)]
    async fn again(&self) -> Result<u32, ResilienceError<Silent>> {
        Ok(7)
    }

    #[resilient(policy, fallback = stale)]
    async fn kept(&self) -> Result<u32, ResilienceError<Silent>> {
        Ok(7)
    }

    /// The fallback of [`kept`](Self::kept), which answers and never calls it.
    async fn stale(&self, _error: ResilienceError<Silent>) -> Result<u32, ResilienceError<Silent>> {
        Ok(0)
    }
}

/// A slot of the bulkhead, a token of the rate and the breaker's pass, its
/// outcome counted with its age, cost a call answering at once no allocation,
/// by whichever path an app calls its policy, a fallback it declares never
/// called: in a release build it allocates its token alone. A debug build
/// boxes its future, and a decorated method's closure where it holds
/// something, as an idempotent one's does.
#[tokio::test(start_paused = true)]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "pins the allocations a run makes on Linux, where the dev container and CI run it"
)]
async fn a_call_answering_at_once_through_every_mechanism_allocates_nothing_but_its_token_in_a_release_build()
 {
    let (_app, policy) = crate::policy(every_mechanism()).await;
    let host = Answering {
        policy: Arc::clone(&policy),
    };
    let token = crate::token_allocations();
    let debug = usize::from(cfg!(debug_assertions));
    for (shape, boxed, (answered, made)) in [
        (
            "run once",
            debug,
            crate::allocations(|| policy.run(|_| async { Ok::<_, Silent>(7) })),
        ),
        (
            "run again",
            debug,
            crate::allocations(|| policy.run_idempotent(|_| async { Ok::<_, Silent>(7) })),
        ),
        (
            "of a decorated method run once",
            debug,
            crate::allocations(|| host.once()),
        ),
        (
            "of a decorated method run again",
            2 * debug,
            crate::allocations(|| host.again()),
        ),
        (
            "of a decorated method declaring a fallback",
            debug,
            crate::allocations(|| host.kept()),
        ),
    ] {
        assert!(matches!(answered, Ok(7)), "{shape}: {answered:?}");
        assert_eq!(
            made,
            token + boxed,
            "through a breaker, a bulkhead and a rate, a call {shape} allocates its token, \
             {token}, and in a debug build {boxed} box(es)"
        );
    }
    assert_eq!(
        (
            policy.concurrency_running(),
            policy.rate_available(),
            policy.breaker_stats().map(|stats| stats.outcomes),
        ),
        (Some(0), Some(995), Some(5)),
        "each of the five took a slot, given back, and a token, and counted in the breaker"
    );
}

/// A release build holds an attempt's future where its run is whatever its
/// size, past tokio's threshold too, so such a call allocates nothing of its
/// own; a debug build boxes it.
#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "pins the allocations a run makes on Linux, where the dev container and CI run it"
)]
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
/// one a byte past it is boxed when its call is sent once, with the future it
/// makes; a call made again holds its closure in place whatever its size. A
/// debug build boxes every closure and every future.
#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "pins the allocations a run makes on Linux, where the dev container and CI run it"
)]
async fn a_closure_up_to_tokios_threshold_is_held_in_place_and_one_past_it_boxed_when_sent_once_in_a_release_build()
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
            in_release(0),
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

/// A unit of work holding a transaction, whose savepoint allocates itself and
/// the future settling it, each once.
struct InTransaction;

impl nest_rs_database::Executor for InTransaction {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn savepoint(&self) -> Option<Arc<dyn nest_rs_database::AtomicBlock>> {
        Some(Arc::new(InSavepoint))
    }
}

struct InSavepoint;

impl nest_rs_database::Executor for InSavepoint {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl nest_rs_database::AtomicBlock for InSavepoint {
    fn settle(
        self: Arc<Self>,
        _ok: bool,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), nest_rs_database::AtomicError>> + Send>>
    {
        Box::pin(std::future::ready(Ok(())))
    }
}

/// What a savepoint costs the attempt running in it, beside what its driver
/// allocates to open and settle it: the port's install, made as the savepoint
/// is opened, and one box holding the answer while the savepoint settles — the
/// same for a call declared idempotent whether or not its policy has retries.
/// A call sent once reads nothing of its unit and allocates nothing for it.
#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "pins the allocations a run makes on Linux, where the dev container and CI run it"
)]
async fn inside_a_transaction_an_idempotent_calls_attempt_allocates_its_savepoints_install_and_settling()
 {
    use nest_rs_database::__private::with_request_executor;

    let (_app, once) = crate::policy(crate::quiet()).await;
    let (_app, again) = crate::policy(ResilienceConfig {
        breaker_window: None,
        ..ResilienceConfig::default()
    })
    .await;
    let unit = || Arc::new(InTransaction) as Arc<dyn nest_rs_database::Executor>;
    let token = crate::token_allocations();
    let boxed = usize::from(cfg!(debug_assertions));
    let (_, installed) =
        crate::allocations(|| with_request_executor(unit(), std::future::ready(())));

    // The double's savepoint and its settling, then the port's install (the
    // executor as the unit carries it, and the unit's context), then the box
    // the answer waits in.
    const DRIVER: usize = 2;
    const INSTALL: usize = 2;
    const SETTLING: usize = 1;
    for (shape, own, (answered, made)) in [
        (
            "sent once",
            0,
            crate::allocations(|| {
                with_request_executor(unit(), once.run(|_| async { Ok::<_, Silent>(7) }))
            }),
        ),
        (
            "sent again, its retries off",
            DRIVER + INSTALL + SETTLING,
            crate::allocations(|| {
                with_request_executor(
                    unit(),
                    once.run_idempotent(|_| async { Ok::<_, Silent>(7) }),
                )
            }),
        ),
        (
            "sent again, its retries on",
            DRIVER + INSTALL + SETTLING,
            crate::allocations(|| {
                with_request_executor(
                    unit(),
                    again.run_idempotent(|_| async { Ok::<_, Silent>(7) }),
                )
            }),
        ),
    ] {
        assert!(matches!(answered, Ok(7)), "{shape}: {answered:?}");
        assert_eq!(
            made,
            installed + token + boxed + own,
            "inside a transaction, a call {shape} allocates its unit's install, {installed}, its \
             token, {token}, in a debug build its future's box, and {own} for its savepoint"
        );
    }
}

/// Waits `polls` times, allocating nothing, before it answers.
struct Waits(usize);

impl Future for Waits {
    type Output = ();

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        match self.0.checked_sub(1) {
            Some(left) => {
                self.0 = left;
                std::task::Poll::Pending
            }
            None => std::task::Poll::Ready(()),
        }
    }
}

/// What the future `make` builds answers, polled on this thread until it
/// does, and how many allocations building and polling it made.
fn allocations_until_answered<F: Future>(make: impl FnOnce() -> F) -> (F::Output, usize) {
    crate::COUNTED.set(Some(0));
    let mut fut = std::pin::pin!(make());
    let cx = &mut std::task::Context::from_waker(std::task::Waker::noop());
    let answer = loop {
        if let std::task::Poll::Ready(answer) = fut.as_mut().poll(cx) {
            break answer;
        }
    };
    (answer, crate::COUNTED.replace(None).unwrap_or_default())
}

/// The savepoint is installed once per attempt, never per poll: a call that
/// waits, polled again and again, allocates for its savepoint what one
/// answering at its first poll does, over what the same call allocates sent
/// once, which opens none.
#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "pins the allocations a run makes on Linux, where the dev container and CI run it"
)]
async fn inside_a_transaction_an_attempt_polled_many_times_allocates_its_savepoints_install_once() {
    use nest_rs_database::__private::with_request_executor;

    let (_app, policy) = crate::policy(crate::quiet()).await;
    let unit = || Arc::new(InTransaction) as Arc<dyn nest_rs_database::Executor>;

    // As the test above counts them.
    const DRIVER: usize = 2;
    const INSTALL: usize = 2;
    const SETTLING: usize = 1;
    for polls in [1, 2, 9] {
        let (once, without) = allocations_until_answered(|| {
            with_request_executor(
                unit(),
                policy.run(move |_| async move {
                    Waits(polls).await;
                    Ok::<_, Silent>(7)
                }),
            )
        });
        let (again, within) = allocations_until_answered(|| {
            with_request_executor(
                unit(),
                policy.run_idempotent(move |_| async move {
                    Waits(polls).await;
                    Ok::<_, Silent>(7)
                }),
            )
        });
        assert!(matches!((once, again), (Ok(7), Ok(7))));
        assert_eq!(
            within,
            without + DRIVER + INSTALL + SETTLING,
            "a call waiting {polls} time(s) allocates {without} sent once, and for its \
             savepoint what a call answering at its first poll does"
        );
    }
}

//! Covers `src/policy.rs`: through the HTTP edge, an attempt ends by the
//! request's deadline, before the policy's own timeout, so its client is told
//! by the policy and its breaker counts the attempt; through the health edge,
//! by the cut of the indicator sending it, a probe's turn included; under a
//! method of another dependency's policy, by that one's timeout, which a
//! fallback runs outside; an inner policy's refusal met by an idempotent
//! method, sent again as the `Err` it is; at its unit's deadline, told before
//! its savepoint settles; and, under no edge, what a
//! run's future holds — its size against its call's, what it allocates, alone,
//! through every mechanism its policy takes and inside a unit of work whose
//! transaction each attempt opens a savepoint of, and a large one run on a
//! worker of tokio's default stack.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use http::StatusCode;
use nest_rs_core::{Problem, ToProblem, injectable, instance, module};
use nest_rs_health::{
    HealthConfig, HealthModule, HealthService, IndicatorStatus, ProbeKind, indicators,
};
use nest_rs_http::{HttpConfig, HttpModule, controller, routes};
use nest_rs_resilience::{
    BreakerState, ResilienceConfig, ResilienceError, ResilienceEvent, ResilienceModule,
    ResiliencePolicy, resilient,
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

#[instance]
struct Haulier;

#[instance]
struct Broker;

/// What a call to the broker fails on when the haulier's policy did.
#[derive(Debug, thiserror::Error)]
#[error("the haulier's call failed")]
struct Forwarded(#[from] ResilienceError<Silent>);

/// A service whose methods under the broker's policy call the ones under the
/// haulier's, a dependency that accepts and never answers.
#[injectable]
struct Shipments {
    #[inject]
    haulier: Arc<ResiliencePolicy<Haulier>>,
    #[inject]
    broker: Arc<ResiliencePolicy<Broker>>,
}

impl Shipments {
    #[resilient(haulier)]
    async fn track(&self) -> Result<u32, ResilienceError<Silent>> {
        std::future::pending().await
    }

    #[resilient(broker, timeout = "2s")]
    async fn forward(&self) -> Result<u32, ResilienceError<Forwarded>> {
        Ok(self.track().await?)
    }

    #[resilient(broker, timeout = "2s", fallback = last_known)]
    async fn tracked(&self) -> Result<u32, ResilienceError<Forwarded>> {
        Ok(self.track().await?)
    }

    /// The fallback of [`tracked`](Self::tracked), itself a call under the
    /// haulier's policy.
    async fn last_known(
        &self,
        _error: ResilienceError<Forwarded>,
    ) -> Result<u32, ResilienceError<Forwarded>> {
        self.archived()
            .await
            .map_err(|error| ResilienceError::Failed(Forwarded(error)))
    }

    /// Answers after longer than the broker's attempt may run.
    #[resilient(haulier)]
    async fn archived(&self) -> Result<u32, ResilienceError<Silent>> {
        tokio::time::sleep(Duration::from_secs(3)).await;
        Ok(7)
    }
}

#[module(
    imports = [
        ResilienceModule::for_root((Haulier, ResilienceConfig {
            retries: None,
            breaker_window: Some(2),
            breaker_failures: 2,
            breaker_open: OPEN,
            ..ResilienceConfig::default()
        })),
        ResilienceModule::for_root((Broker, crate::quiet())),
    ],
    providers = [Shipments],
)]
struct ShipmentsModule;

async fn shipments() -> (HeadlessApp, Arc<Shipments>, Arc<ResiliencePolicy<Haulier>>) {
    let app = HeadlessApp::builder()
        .module::<ShipmentsModule>()
        .build_headless()
        .await
        .expect("two policies boot");
    let shipments = app.container().get::<Shipments>().expect("provided");
    let haulier = crate::held::<Haulier>(&app);
    (app, shipments, haulier)
}

/// A decorated method called from one under another policy's shorter timeout
/// is cut there, before its own five seconds: its attempt has timed out, so a
/// dependency that never answers opens its own breaker, and the calls after
/// it are refused at once rather than each waiting its caller's timeout out.
#[tokio::test(start_paused = true)]
async fn a_dependency_hanging_under_its_callers_shorter_timeout_opens_its_own_breaker() {
    let (_app, shipments, haulier) = shipments().await;
    let short = Duration::from_secs(2);
    assert!(short < ResilienceConfig::default().timeout);

    for _hung in 0..2 {
        let forwarded = shipments.forward().await;
        assert!(
            matches!(forwarded, Err(ResilienceError::TimedOut { timeout, .. }) if timeout == short),
            "{forwarded:?}"
        );
    }
    let opened = haulier.breaker_state();
    assert!(
        matches!(opened, Some(BreakerState::Open { retry_after, .. }) if retry_after == OPEN),
        "two timeouts of two: {opened:?}"
    );

    let started = tokio::time::Instant::now();
    let refused = shipments.forward().await;
    assert!(
        matches!(
            refused,
            Err(ResilienceError::Failed(Forwarded(
                ResilienceError::Open { .. }
            )))
        ),
        "{refused:?}"
    );
    assert_eq!(started.elapsed(), Duration::ZERO, "refused unsent");
}

/// A fallback runs once its run has answered, outside its attempt's timeout:
/// a call it makes under another policy is cut by nothing but its own bound,
/// and answers after longer than the attempt it replaces was given.
#[tokio::test(start_paused = true)]
async fn a_call_its_fallback_makes_is_not_cut_by_the_timeout_of_the_run_it_answers_for() {
    let (_app, shipments, haulier) = shipments().await;
    let started = tokio::time::Instant::now();
    let answered = shipments.tracked().await;
    assert!(matches!(answered, Ok(7)), "{answered:?}");
    assert_eq!(
        started.elapsed(),
        Duration::from_secs(5),
        "the attempt's 2 s, then the fallback's call, 3 s"
    );
    let counted = haulier.breaker_stats().expect("a breaker");
    assert_eq!(
        (counted.outcomes, counted.failures),
        (2, 1),
        "the call the broker's timeout cut, then the fallback's, which answered"
    );
}

/// The longest wait a dependency asks that [`Relayed`]'s outer policy honours.
const CEILING: Duration = Duration::from_secs(5);

/// A host the test builds itself: methods under its `outer` policy, declared
/// idempotent, calling one under its `inner` policy, which answers at once
/// whenever that policy lets it through.
struct Relayed {
    inner: Arc<ResiliencePolicy>,
    outer: Arc<ResiliencePolicy>,
    /// How many attempts the outer policy made.
    sent: AtomicU32,
}

impl Relayed {
    #[resilient(inner)]
    async fn quote(&self) -> Result<u32, ResilienceError<Silent>> {
        Ok(7)
    }

    #[resilient(outer, idempotent)]
    async fn relay(&self) -> Result<u32, ResilienceError<Forwarded>> {
        self.sent.fetch_add(1, Ordering::Relaxed);
        Ok(self.quote().await?)
    }

    /// [`relay`](Self::relay), reading the inner policy's refusal as no
    /// failure and nothing to send again.
    #[resilient(
        outer,
        idempotent,
        retry_if = |forwarded| matches!(forwarded.0, ResilienceError::Failed(_)),
        record_if = |forwarded| matches!(forwarded.0, ResilienceError::Failed(_))
    )]
    async fn relay_what_answered(&self) -> Result<u32, ResilienceError<Forwarded>> {
        self.sent.fetch_add(1, Ordering::Relaxed);
        Ok(self.quote().await?)
    }

    fn sent(&self) -> u32 {
        self.sent.load(Ordering::Relaxed)
    }
}

/// A [`Relayed`] whose inner policy has one slot and a breaker, and whose
/// outer one sends again once, after about 100 ms of its own or the wait a
/// dependency asks up to [`CEILING`], and counts each attempt in a breaker
/// the two of a run never open.
async fn relayed() -> ([HeadlessApp; 2], Relayed) {
    let (inner_app, inner) = crate::policy(ResilienceConfig {
        concurrency: Some(1),
        ..counting()
    })
    .await;
    let (outer_app, outer) = crate::policy(ResilienceConfig {
        retries: Some(1),
        backoff_first: Duration::from_millis(100),
        backoff_ceiling: CEILING,
        breaker_window: Some(4),
        breaker_failures: 4,
        ..ResilienceConfig::default()
    })
    .await;
    let relayed = Relayed {
        inner,
        outer,
        sent: AtomicU32::new(0),
    };
    ([inner_app, outer_app], relayed)
}

/// An inner policy's open breaker is an `Err` like any other to the method
/// calling it: a failure in the outer breaker, sent again after the whole
/// `Retry-After` it carries when that is within the backoff's ceiling — the
/// retry then the inner breaker's probe — and not at all past it.
#[tokio::test(start_paused = true)]
async fn an_inner_policys_open_breaker_is_sent_again_after_its_whole_wait_or_not_at_all_past_the_ceiling()
 {
    let within = Duration::from_secs(2);
    let (_apps, host) = relayed().await;
    host.inner.open_breaker(within).expect("a breaker");
    let started = tokio::time::Instant::now();
    let answered = host.relay().await;
    assert!(matches!(answered, Ok(7)), "{answered:?}");
    assert_eq!(host.sent(), 2);
    let waited = started.elapsed();
    assert!(
        within <= waited && waited <= within + within / 5,
        "the {within:?} the refusal said, stretched by a fifth at most: {waited:?}"
    );
    assert_eq!(counted(&host.outer), (2, 1), "the refusal, then the answer");

    let (_apps, host) = relayed().await;
    host.inner
        .open_breaker(CEILING + Duration::from_secs(1))
        .expect("a breaker");
    let started = tokio::time::Instant::now();
    let refused = host.relay().await;
    assert!(
        matches!(
            refused,
            Err(ResilienceError::Failed(Forwarded(
                ResilienceError::Open { .. }
            )))
        ),
        "{refused:?}"
    );
    assert_eq!(host.sent(), 1, "a retry left, given up");
    assert_eq!(started.elapsed(), Duration::ZERO);
    assert_eq!(counted(&host.outer), (1, 1));
}

/// An inner policy's full bulkhead says no `Retry-After`: the method calling
/// it is sent again on the outer policy's own backoff, each refusal a failure
/// in the outer breaker.
#[tokio::test(start_paused = true)]
async fn an_inner_policys_full_bulkhead_is_sent_again_on_the_plain_backoff() {
    let (_apps, host) = relayed().await;
    let inner = Arc::clone(&host.inner);
    let holding = tokio::spawn(async move {
        inner
            .run(|_| std::future::pending::<Result<(), Silent>>())
            .await
    });
    tokio::task::yield_now().await;
    assert_eq!(host.inner.concurrency_running(), Some(1));

    let started = tokio::time::Instant::now();
    let refused = host.relay().await;
    assert!(
        matches!(
            refused,
            Err(ResilienceError::Failed(Forwarded(
                ResilienceError::Full { .. }
            )))
        ),
        "{refused:?}"
    );
    assert_eq!(host.sent(), 2);
    let waited = started.elapsed();
    assert!(
        Duration::from_millis(80) <= waited && waited <= Duration::from_millis(120),
        "the first backoff, 100 ms within a fifth: {waited:?}"
    );
    assert_eq!(counted(&host.outer), (2, 2));
    holding.abort();
}

/// An inner policy's refusal its caller's `retry_if` and `record_if` refuse
/// ends the run at its one attempt, counted in neither direction in the
/// outer breaker: a wait within the ceiling, or none, is waited by no retry.
#[tokio::test(start_paused = true)]
async fn an_inner_policys_refusal_its_callers_predicates_refuse_is_sent_once_and_counted_nowhere() {
    let (_apps, host) = relayed().await;
    host.inner
        .open_breaker(Duration::from_secs(2))
        .expect("a breaker");
    let started = tokio::time::Instant::now();
    let refused = host.relay_what_answered().await;
    assert!(
        matches!(
            refused,
            Err(ResilienceError::Failed(Forwarded(
                ResilienceError::Open { .. }
            )))
        ),
        "{refused:?}"
    );
    assert_eq!(host.sent(), 1);
    assert_eq!(started.elapsed(), Duration::ZERO);
    assert_eq!(counted(&host.outer), (0, 0));
}

/// A unit of work holding a transaction whose savepoints settle after a round
/// trip to its database.
struct RoundTrip(Duration);

impl nest_rs_database::Executor for RoundTrip {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn savepoint(&self) -> Option<Arc<dyn nest_rs_database::AtomicBlock>> {
        Some(Arc::new(Self(self.0)))
    }
}

impl nest_rs_database::AtomicBlock for RoundTrip {
    fn settle(
        self: Arc<Self>,
        _ok: bool,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), nest_rs_database::AtomicError>> + Send>>
    {
        let round_trip = self.0;
        Box::pin(async move {
            tokio::time::sleep(round_trip).await;
            Ok(())
        })
    }
}

/// `call` as a unit's body, in a transaction whose savepoints settle in
/// `round_trip`, cut by its edge at the deadline it is bound by, a second on:
/// what it answered, or `None` once the edge cut it.
async fn cut_in_a_second<T>(round_trip: Duration, call: impl Future<Output = T>) -> Option<T> {
    use nest_rs_database::__private::with_request_executor;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    let unit = nest_rs_core::__private::with_deadline(
        Some(deadline),
        with_request_executor(Arc::new(RoundTrip(round_trip)), call),
    );
    tokio::time::timeout_at(deadline, unit).await.ok()
}

/// No retry, and a breaker two failures open: one timeout is read in its
/// window, and tells no transition.
fn counting() -> ResilienceConfig {
    ResilienceConfig {
        retries: None,
        breaker_window: Some(2),
        breaker_failures: 2,
        ..ResilienceConfig::default()
    }
}

/// The outcomes `policy`'s breaker counts, and the failures among them.
fn counted(policy: &ResiliencePolicy) -> (usize, usize) {
    let stats = policy.breaker_stats().expect("a breaker");
    (stats.outcomes, stats.failures)
}

/// An idempotent call given up at its unit's deadline, its savepoint opened,
/// is told as it ends: the edge cuts the unit at that instant, while the
/// savepoint is rolled back, and the timeout its breaker counted is told by
/// its event all the same.
#[tokio::test(start_paused = true)]
async fn a_timeout_at_the_units_deadline_is_told_though_its_savepoint_still_settles() {
    let (_app, policy) = crate::policy(counting()).await;
    let mut events = policy.events();
    let call = policy.run_idempotent(|_| std::future::pending::<Result<u32, Silent>>());
    let cut = cut_in_a_second(Duration::from_millis(1), call).await;
    assert!(cut.is_none(), "cut while its savepoint settles: {cut:?}");
    assert_eq!(counted(&policy), (1, 1), "the timeout its breaker counted");
    let told = crate::told(&mut events);
    assert!(
        matches!(told[..], [ResilienceEvent::Timeout { .. }]),
        "{told:?}"
    );
}

/// A host whose method never answers and declares a fallback that does, at once.
struct Hanging {
    policy: Arc<ResiliencePolicy>,
}

impl Hanging {
    #[resilient(policy, fallback = stale)]
    async fn live(&self) -> Result<u32, ResilienceError<Silent>> {
        std::future::pending().await
    }

    async fn stale(&self, _error: ResilienceError<Silent>) -> Result<u32, ResilienceError<Silent>> {
        Ok(0)
    }
}

/// A limit: a fallback is asked once its attempt's savepoint is rolled back,
/// so its write never lands above the failed attempt's. At its unit's
/// deadline, one answering at once answers for the unit only where that
/// savepoint settles at once; a round trip to the database is past the
/// deadline, the edge cuts the unit there, and the fallback is never asked —
/// the timeout told all the same.
#[tokio::test(start_paused = true)]
async fn a_fallback_at_its_units_deadline_has_no_time_left_once_its_savepoint_takes_a_round_trip() {
    for (round_trip, answers) in [(Duration::ZERO, true), (Duration::from_millis(1), false)] {
        let (_app, policy) = crate::policy(counting()).await;
        let mut events = policy.events();
        let host = Hanging {
            policy: Arc::clone(&policy),
        };
        let cut = cut_in_a_second(round_trip, host.live()).await;
        assert_eq!(counted(&policy), (1, 1), "the timeout its breaker counted");
        let told = crate::told(&mut events);
        if answers {
            assert!(matches!(cut, Some(Ok(0))), "{cut:?}");
            assert!(
                matches!(
                    told[..],
                    [
                        ResilienceEvent::Timeout { .. },
                        ResilienceEvent::Fallback { .. }
                    ]
                ),
                "{told:?}"
            );
        } else {
            assert!(cut.is_none(), "cut while its savepoint settles: {cut:?}");
            assert!(
                matches!(told[..], [ResilienceEvent::Timeout { .. }]),
                "its fallback is never asked: {told:?}"
            );
        }
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

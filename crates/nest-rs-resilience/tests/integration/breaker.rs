//! Covers `src/breaker.rs` beside the breaker of `@nestjs/resilience` 0.0.3,
//! ported below as a model and driven through the same timed traces: what
//! `breaker_age` and `breaker_probes` give of that breaker's time window and
//! of its half-open probes.
//!
//! **The time window's case** is what a deployment sets `slidingWindow: { type:
//! 'time' }` for: an outcome stops counting by its age, whatever the calls
//! after it; a quiet spell longer than the window empties it; failures
//! answered within one window of each other open the breaker. **The probes'
//! case** is what it sets `halfOpenMaxCalls` above one for: several probes
//! answer before the breaker closes, and a failing probe opens it again.
//!
//! **The mapping.** The reference's `{ slidingWindow: time A,
//! failureRateThreshold T, minimumCalls M, openDuration D, halfOpenMaxCalls P }`
//! is the policy `{ breaker_age: A, breaker_window: 2M at least,
//! breaker_failures: ⌈T × M / 100⌉, breaker_open: D, breaker_probes: P }` —
//! the fewest failures the reference opens on, counted over a window wide
//! enough that its rate, reached over more calls than the window holds, has
//! put that many within it already — and its count window of N is
//! `breaker_window: N` with no age ([`mapped`]).
//!
//! **The same at the same call**, each a test below:
//!
//! - every behaviour of the two cases;
//! - a call its caller dropped, and an `Err` a decorated method's `record_if`
//!   refuses, the reference's `recordIf` answering `false`: in neither
//!   breaker's window and in neither's series of probes, the probe's turn
//!   handed on in both;
//! - a call sent before an opening and answering after the closing: a success
//!   or a failure, its answer counts in neither breaker's new window;
//! - on the reference's bucket boundaries, a dependency down, trace for trace,
//!   once `breaker_age` is 9A/10.
//!
//! **Where the two first differ, this breaker is the stricter.** Under the
//! mapping, on every generated trace — calls answered one after the other,
//! and up to [`IN_FLIGHT`] in flight together answered in any order — the two
//! answer alike until one of:
//!
//! - a failure both sent that opens this breaker and not the reference:
//!   counted where the reference's rate, or its minimum of calls, is not
//!   reached (`.claude/decisions/resilience-policy.md`, *Refused from #7*);
//!   one the reference's bucket dropped up to a tenth of the window early; or
//!   a probe's, which opens this breaker at once where the reference waits for
//!   its series and judges its rate;
//! - a call arriving while a probe is out, refused here, where the reference
//!   sends its probes together.
//!
//! Never the reverse on those traces: sampled, not proved.
//!
//! **Past its first difference a trace is compared no further, and the two
//! are out of step** by up to the open time, neither the stricter throughout:
//! having opened first, this breaker also probes first and closes first, so
//! it sends a probe, then every call, while the reference, opened later,
//! still refuses
//! (`having_opened_first_this_breaker_probes_and_closes_while_the_reference_still_refuses`).
//!
//! **The mapping's range:**
//!
//! - `breaker_age`: off its bucket boundaries the reference drops an outcome
//!   by tenths of its window, between 9A/10 and A after its answer, and this
//!   breaker at its age exactly. Aged A it counts a failure the reference
//!   dropped; aged 9A/10 it forgets one the reference still holds, and there
//!   the reference is the stricter;
//! - `breaker_window`: with more calls in one window than it holds, a window
//!   of M forgets by count failures the reference's rate still reads, and is
//!   the laxer. Of 2M it is the laxer on none of the crowded traces drawn,
//!   answered in turn or in flight together: 2M is shown enough on them, not
//!   the least that is.

use std::cell::Cell;
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::Poll;
use std::time::Duration;

use nest_rs_resilience::{
    BreakerState, ResilienceConfig, ResilienceError, ResiliencePolicy, resilient,
};
use nest_rs_testing::HeadlessApp;
use tokio::sync::oneshot;
use tokio::time::Instant;

use self::reference::{Options, Outcome, Permit, SlidingWindow, State, WindowStats};

/// The reference's breaker — `@nestjs/resilience` 0.0.3,
/// `dist/policies/circuit-breaker.policy.js` — ported as a model to compare
/// against, never shipped: `CountWindow`, `TimeWindow`, `acquire`, `release`,
/// `checkOpenDuration` and `transition`, each under its name there. Left out:
/// the events it emits, `trip` and `reset`, and `recordIf` and the abort
/// signal, whose verdict a trace hands it as [`Outcome::Ignored`].
mod reference {
    use std::time::Duration;

    use tokio::time::Instant;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum State {
        Closed,
        Open,
        HalfOpen,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum Outcome {
        Success,
        Failure,
        /// Neither: the caller went away before an answer — the reference's
        /// aborted signal, and a run's future dropped — or the call answered
        /// an error `recordIf` refuses, a decorated method's `record_if`.
        Ignored,
    }

    #[derive(Clone, Copy, Debug)]
    pub(super) enum SlidingWindow {
        Count(usize),
        Time(Duration),
    }

    /// `ResolvedCircuitBreakerOptions`, the threshold in whole per cent.
    #[derive(Clone, Copy, Debug)]
    pub(super) struct Options {
        pub(super) failure_rate_threshold: u32,
        pub(super) minimum_calls: usize,
        pub(super) sliding_window: SlidingWindow,
        pub(super) open_duration: Duration,
        pub(super) half_open_max_calls: usize,
    }

    #[derive(Clone, Copy, Debug)]
    pub(super) struct WindowStats {
        pub(super) total: usize,
        pub(super) failures: usize,
    }

    /// `(failures / total) * 100`, in the reference's floating point.
    fn failure_rate(failures: usize, total: usize) -> f64 {
        if total == 0 {
            0.0
        } else {
            failures as f64 / total as f64 * 100.0
        }
    }

    impl Options {
        /// `release`'s condition for opening a closed breaker on `stats`.
        pub(super) fn opens_on(&self, stats: WindowStats) -> bool {
            stats.total >= self.minimum_calls
                && failure_rate(stats.failures, stats.total)
                    >= f64::from(self.failure_rate_threshold)
        }
    }

    struct CountWindow {
        size: usize,
        outcomes: Vec<bool>,
        next: usize,
        failures: usize,
    }

    impl CountWindow {
        fn stats(&self) -> WindowStats {
            WindowStats {
                total: self.outcomes.len(),
                failures: self.failures,
            }
        }

        fn record(&mut self, failure: bool) {
            if self.outcomes.len() < self.size {
                self.outcomes.push(failure);
            } else {
                if self.outcomes[self.next] {
                    self.failures -= 1;
                }
                self.outcomes[self.next] = failure;
                self.next = (self.next + 1) % self.size;
            }
            if failure {
                self.failures += 1;
            }
        }

        fn reset(&mut self) {
            self.outcomes.clear();
            self.next = 0;
            self.failures = 0;
        }
    }

    const BUCKETS: u64 = 10;

    /// A bucket no call was recorded in holds no `id`, the reference's
    /// `-Infinity`.
    #[derive(Clone, Copy, Default)]
    struct Bucket {
        id: Option<u64>,
        total: usize,
        failures: usize,
    }

    /// Ten buckets of `size / 10` ms each; a bucket drops out once it is `size`
    /// ms old.
    struct TimeWindow {
        buckets: Vec<Bucket>,
        width: u64,
    }

    impl TimeWindow {
        /// The reference divides `size` as a float; a size in whole tens of
        /// milliseconds, the only ones a trace here takes, divides alike.
        fn new(size: Duration) -> Self {
            let size = millis(size);
            assert!(
                size > 0 && size.is_multiple_of(BUCKETS),
                "a time window here is whole tens of milliseconds"
            );
            Self {
                buckets: vec![Bucket::default(); BUCKETS as usize],
                width: size / BUCKETS,
            }
        }

        fn stats(&self, now: u64) -> WindowStats {
            let current = now / self.width;
            let mut stats = WindowStats {
                total: 0,
                failures: 0,
            };
            for bucket in &self.buckets {
                // `bucket.id <= oldest`, `oldest` being `current - BUCKETS`.
                if bucket.id.is_none_or(|id| id + BUCKETS <= current) {
                    continue;
                }
                stats.total += bucket.total;
                stats.failures += bucket.failures;
            }
            stats
        }

        fn record(&mut self, now: u64, failure: bool) {
            let id = now / self.width;
            let bucket = &mut self.buckets[(id % BUCKETS) as usize];
            if bucket.id != Some(id) {
                *bucket = Bucket {
                    id: Some(id),
                    ..Bucket::default()
                };
            }
            bucket.total += 1;
            if failure {
                bucket.failures += 1;
            }
        }

        fn reset(&mut self) {
            self.buckets.fill(Bucket::default());
        }
    }

    enum Window {
        Count(CountWindow),
        Time(TimeWindow),
    }

    #[derive(Default)]
    struct Probes {
        started: usize,
        finished: usize,
        failures: usize,
    }

    /// What `acquire` hands a call it lets through.
    #[derive(Debug)]
    pub(super) struct Permit {
        generation: u64,
        half_open: bool,
    }

    /// `CircuitBreakerPolicy`, its `Date.now()` read off tokio's clock.
    pub(super) struct Breaker {
        options: Options,
        current: State,
        generation: u64,
        opened_at: u64,
        window: Window,
        probes: Probes,
        /// `Date.now()` as the breaker was built, and tokio's instant then.
        started: (u64, Instant),
    }

    fn millis(duration: Duration) -> u64 {
        u64::try_from(duration.as_millis()).expect("a trace's times are milliseconds in a `u64`")
    }

    impl Breaker {
        /// The breaker `options` describe, built when `Date.now()` reads
        /// `started`.
        pub(super) fn new(options: Options, started: u64) -> Self {
            let window = match options.sliding_window {
                SlidingWindow::Count(size) => {
                    assert!(
                        options.minimum_calls <= size,
                        "the reference refuses a `minimumCalls` its count window never holds"
                    );
                    Window::Count(CountWindow {
                        size,
                        outcomes: Vec::new(),
                        next: 0,
                        failures: 0,
                    })
                }
                SlidingWindow::Time(size) => Window::Time(TimeWindow::new(size)),
            };
            Self {
                options,
                current: State::Closed,
                generation: 0,
                opened_at: 0,
                window,
                probes: Probes::default(),
                started: (started, Instant::now()),
            }
        }

        fn now(&self) -> u64 {
            let (started, at) = self.started;
            started + millis(at.elapsed())
        }

        pub(super) fn state(&mut self) -> State {
            self.check_open_duration();
            self.current
        }

        fn stats(&self) -> WindowStats {
            match &self.window {
                Window::Count(window) => window.stats(),
                Window::Time(window) => window.stats(self.now()),
            }
        }

        /// A permit, or the `retryAfter` of the `CircuitOpenError` it throws.
        pub(super) fn acquire(&mut self) -> Result<Permit, Duration> {
            self.check_open_duration();
            if self.current == State::Closed {
                return Ok(Permit {
                    generation: self.generation,
                    half_open: false,
                });
            }
            if self.current == State::HalfOpen
                && self.probes.started < self.options.half_open_max_calls
            {
                self.probes.started += 1;
                return Ok(Permit {
                    generation: self.generation,
                    half_open: true,
                });
            }
            let retry_after = if self.current == State::Open {
                (self.opened_at + millis(self.options.open_duration)).saturating_sub(self.now())
            } else {
                0
            };
            Err(Duration::from_millis(retry_after))
        }

        pub(super) fn release(&mut self, permit: Permit, outcome: Outcome) {
            // Outcomes of calls admitted under an earlier state are stale.
            if permit.generation != self.generation {
                return;
            }
            if !permit.half_open {
                if outcome == Outcome::Ignored {
                    return;
                }
                let now = self.now();
                match &mut self.window {
                    Window::Count(window) => window.record(outcome == Outcome::Failure),
                    Window::Time(window) => window.record(now, outcome == Outcome::Failure),
                }
                if self.options.opens_on(self.stats()) {
                    self.transition(State::Open);
                }
                return;
            }
            if outcome == Outcome::Ignored {
                self.probes.started -= 1;
                return;
            }
            self.probes.finished += 1;
            if outcome == Outcome::Failure {
                self.probes.failures += 1;
            }
            if self.probes.finished < self.options.half_open_max_calls {
                return;
            }
            let rate = failure_rate(self.probes.failures, self.probes.finished);
            self.transition(if rate >= f64::from(self.options.failure_rate_threshold) {
                State::Open
            } else {
                State::Closed
            });
        }

        fn check_open_duration(&mut self) {
            if self.current == State::Open
                && self.now() - self.opened_at >= millis(self.options.open_duration)
            {
                self.transition(State::HalfOpen);
            }
        }

        fn transition(&mut self, to: State) {
            self.current = to;
            self.generation += 1;
            match to {
                State::Open => self.opened_at = self.now(),
                State::Closed => match &mut self.window {
                    Window::Count(window) => window.reset(),
                    Window::Time(window) => window.reset(),
                },
                State::HalfOpen => self.probes = Probes::default(),
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("the dependency is down")]
struct Down;

/// What the dependency answers a call of [`Client`] in error: its failure, or
/// an answer of its own.
#[derive(Debug, thiserror::Error)]
enum Unserved {
    #[error("the dependency is down")]
    Down,
    #[error("the dependency declined the request")]
    Declined,
}

/// A host calling the dependency through a decorated method whose `record_if`
/// refuses what the dependency declined: the reference's `recordIf` answering
/// `false`, which it releases as [`Outcome::Ignored`].
struct Client {
    policy: Arc<ResiliencePolicy>,
    sent: AtomicBool,
}

impl Client {
    #[resilient(policy, record_if = |error| matches!(error, Unserved::Down))]
    async fn call(&self, outcome: Outcome) -> Result<(), ResilienceError<Unserved>> {
        self.sent.store(true, Ordering::Relaxed);
        match outcome {
            Outcome::Success => Ok(()),
            Outcome::Failure => Err(Unserved::Down),
            Outcome::Ignored => Err(Unserved::Declined),
        }
    }
}

/// What each attempt is sent under: no call of a trace waits that long.
const TIMEOUT: Duration = Duration::from_secs(5);

/// `Date.now()` as a trace starts, unless it draws its own: on a bucket's
/// boundary of every time window here.
const STARTED: u64 = 1_800_000_000_000;

/// The fewest failures the reference opens on: `minimumCalls` of them reaching
/// its rate.
fn fewest_failures(options: Options) -> usize {
    (options.failure_rate_threshold as usize * options.minimum_calls).div_ceil(100)
}

/// The mapping: the policy that stands for the reference's `options`.
fn mapped(options: Options) -> ResilienceConfig {
    let (window, age) = match options.sliding_window {
        SlidingWindow::Count(size) => (size, None),
        SlidingWindow::Time(size) => (2 * options.minimum_calls, Some(size)),
    };
    ResilienceConfig {
        timeout: TIMEOUT,
        retries: None,
        breaker_window: Some(window),
        breaker_failures: fewest_failures(options),
        breaker_open: options.open_duration,
        breaker_age: age,
        breaker_probes: options.half_open_max_calls,
        ..ResilienceConfig::default()
    }
}

/// What a call met at a breaker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Decision {
    Sent,
    /// Refused unsent, told this wait.
    Refused {
        retry_after: Duration,
    },
}

/// One reading of each breaker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Both<T> {
    ours: T,
    reference: T,
}

fn both<T: Copy>(same: T) -> Both<T> {
    Both {
        ours: same,
        reference: same,
    }
}

type Run = Pin<Box<dyn Future<Output = Result<(), ResilienceError<Down>>>>>;

/// One call in flight, held by each breaker that sent it.
struct Flight {
    ours: Option<(Run, oneshot::Sender<Result<(), Down>>)>,
    reference: Option<Permit>,
}

/// A policy and the reference's breaker, driven together on tokio's paused
/// clock: the policy through `run`, a decorated method and `breaker_state`
/// alone.
struct Pair {
    _app: HeadlessApp,
    ours: Arc<ResiliencePolicy>,
    client: Client,
    reference: reference::Breaker,
}

impl Pair {
    /// The policy `config` pins beside the reference's breaker of `options`,
    /// built when `Date.now()` reads `started`.
    async fn new(config: ResilienceConfig, options: Options, started: u64) -> Self {
        let (app, ours) = crate::policy(config).await;
        Self {
            _app: app,
            client: Client {
                policy: Arc::clone(&ours),
                sent: AtomicBool::new(false),
            },
            ours,
            reference: reference::Breaker::new(options, started),
        }
    }

    /// The reference's breaker of `options` beside the policy the mapping
    /// gives it.
    async fn mapping(options: Options) -> Self {
        Self::new(mapped(options), options, STARTED).await
    }

    /// [`mapping`](Self::mapping), once `minimumCalls` failures at one instant
    /// opened both: the policy on the fewest of them.
    async fn opened(options: Options) -> Self {
        let mut pair = Self::mapping(options).await;
        for _ in 0..options.minimum_calls {
            pair.call(Outcome::Failure).await;
        }
        assert_eq!(pair.states(), both(State::Open));
        pair
    }

    /// One call arriving at both breakers: sent by each that lets it through,
    /// and left waiting for its answer.
    async fn start(&mut self) -> (Flight, Both<Decision>) {
        let (answer, answered) = oneshot::channel::<Result<(), Down>>();
        let sent = Rc::new(Cell::new(false));
        let (policy, called) = (Arc::clone(&self.ours), Rc::clone(&sent));
        let mut run: Run = Box::pin(async move {
            policy
                .run(move |_| {
                    called.set(true);
                    async move {
                        answered
                            .await
                            .expect("a flight is answered or dropped whole")
                    }
                })
                .await
        });
        let ours = match poll_fn(|cx| Poll::Ready(run.as_mut().poll(cx))).await {
            Poll::Pending if sent.get() => Decision::Sent,
            Poll::Ready(Err(ResilienceError::Open { retry_after, .. })) if !sent.get() => {
                Decision::Refused { retry_after }
            }
            met => panic!("a call is sent and waits, or the breaker refuses it unsent: {met:?}"),
        };
        let permit = self.reference.acquire();
        let reference = match permit {
            Ok(_) => Decision::Sent,
            Err(retry_after) => Decision::Refused { retry_after },
        };
        let flight = Flight {
            ours: (ours == Decision::Sent).then_some((run, answer)),
            reference: permit.ok(),
        };
        (flight, Both { ours, reference })
    }

    /// The flight's call answering `outcome`, in each breaker that sent it.
    async fn settle(&mut self, flight: Flight, outcome: Outcome) {
        if let Some((run, answer)) = flight.ours {
            let answering = match outcome {
                Outcome::Success => Some(Ok(())),
                Outcome::Failure => Some(Err(Down)),
                Outcome::Ignored => None,
            };
            if let Some(answering) = answering {
                answer.send(answering).expect("the run holds its call");
                match (outcome, run.await) {
                    (Outcome::Success, Ok(()))
                    | (Outcome::Failure, Err(ResilienceError::Failed(Down))) => {}
                    (_, answered) => panic!("a run answers as its call did: {answered:?}"),
                }
            }
        }
        if let Some(permit) = flight.reference {
            self.reference.release(permit, outcome);
        }
    }

    /// One call arriving at both breakers and answering `outcome` at once.
    async fn call(&mut self, outcome: Outcome) -> Both<Decision> {
        let (flight, met) = self.start().await;
        self.settle(flight, outcome).await;
        met
    }

    /// One call arriving at both breakers and answering `outcome` at once,
    /// this policy's through [`Client::call`]: an [`Outcome::Ignored`] is then
    /// an answer of the dependency's that `record_if` refuses, where
    /// [`call`](Self::call) drops its run unanswered.
    async fn judged(&mut self, outcome: Outcome) -> Both<Decision> {
        self.client.sent.store(false, Ordering::Relaxed);
        let answered = self.client.call(outcome).await;
        let sent = self.client.sent.load(Ordering::Relaxed);
        let ours = match (outcome, answered) {
            (Outcome::Success, Ok(()))
            | (Outcome::Failure, Err(ResilienceError::Failed(Unserved::Down)))
            | (Outcome::Ignored, Err(ResilienceError::Failed(Unserved::Declined)))
                if sent =>
            {
                Decision::Sent
            }
            (_, Err(ResilienceError::Open { retry_after, .. })) if !sent => {
                Decision::Refused { retry_after }
            }
            (_, met) => panic!("a call answers as it was told to, or is refused unsent: {met:?}"),
        };
        let reference = match self.reference.acquire() {
            Ok(permit) => {
                self.reference.release(permit, outcome);
                Decision::Sent
            }
            Err(retry_after) => Decision::Refused { retry_after },
        };
        Both { ours, reference }
    }

    /// The state a call arriving now meets at each breaker.
    fn states(&mut self) -> Both<State> {
        let ours = match self.ours.breaker_state() {
            Some(BreakerState::Closed) => State::Closed,
            Some(BreakerState::Open { .. }) => State::Open,
            Some(BreakerState::HalfOpen) => State::HalfOpen,
            read => panic!("a state the reference has no word for: {read:?}"),
        };
        Both {
            ours,
            reference: self.reference.state(),
        }
    }
}

const WINDOW: Duration = Duration::from_secs(10);

/// A tenth of [`WINDOW`]: one of the reference's buckets.
const TENTH: Duration = Duration::from_secs(1);

const OPEN: Duration = Duration::from_secs(30);

/// The reference's options over a time window of [`WINDOW`], open for [`OPEN`].
fn timed(failure_rate_threshold: u32, minimum_calls: usize, half_open_max_calls: usize) -> Options {
    Options {
        failure_rate_threshold,
        minimum_calls,
        sliding_window: SlidingWindow::Time(WINDOW),
        open_duration: OPEN,
        half_open_max_calls,
    }
}

/// Failures never three within one window of each other open neither breaker,
/// where a window counting its latest calls however old opens at the third.
#[tokio::test(start_paused = true)]
async fn an_outcome_stops_counting_past_its_age() {
    let apart = WINDOW * 6 / 10;
    let mut aged = Pair::mapping(timed(100, 3, 1)).await;
    let mut ageless = Pair::mapping(Options {
        sliding_window: SlidingWindow::Count(20),
        ..timed(100, 3, 1)
    })
    .await;

    for _ in 0..2 {
        assert_eq!(aged.call(Outcome::Failure).await, both(Decision::Sent));
        assert_eq!(ageless.call(Outcome::Failure).await, both(Decision::Sent));
        tokio::time::advance(apart).await;
    }
    assert_eq!(ageless.call(Outcome::Failure).await, both(Decision::Sent));
    assert_eq!(ageless.states(), both(State::Open));

    for failure in 3..=8 {
        assert_eq!(aged.call(Outcome::Failure).await, both(Decision::Sent));
        assert_eq!(aged.states(), both(State::Closed), "failure {failure}");
        tokio::time::advance(apart).await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_quiet_spell_empties_the_window() {
    let mut pair = Pair::mapping(timed(100, 3, 1)).await;
    for _ in 0..2 {
        pair.call(Outcome::Failure).await;
    }
    assert_eq!(pair.states(), both(State::Closed));

    tokio::time::advance(WINDOW + Duration::from_millis(1)).await;
    for _ in 0..2 {
        assert_eq!(pair.call(Outcome::Failure).await, both(Decision::Sent));
        assert_eq!(pair.states(), both(State::Closed));
    }
    assert_eq!(pair.call(Outcome::Failure).await, both(Decision::Sent));
    assert_eq!(pair.states(), both(State::Open));
}

#[tokio::test(start_paused = true)]
async fn a_burst_inside_the_window_opens_it() {
    let mut pair = Pair::mapping(timed(100, 3, 1)).await;
    for _ in 0..2 {
        assert_eq!(pair.call(Outcome::Failure).await, both(Decision::Sent));
        assert_eq!(pair.states(), both(State::Closed));
        tokio::time::advance(WINDOW * 45 / 100).await;
    }
    assert_eq!(pair.call(Outcome::Failure).await, both(Decision::Sent));
    assert_eq!(pair.states(), both(State::Open));
    assert_eq!(
        pair.call(Outcome::Success).await,
        both(Decision::Refused { retry_after: OPEN })
    );
}

/// The series closes both at its last probe, each window counting afresh.
#[tokio::test(start_paused = true)]
async fn several_probes_answer_before_it_closes() {
    let mut pair = Pair::opened(timed(100, 3, 3)).await;
    tokio::time::advance(OPEN).await;
    for (probe, then) in [State::HalfOpen, State::HalfOpen, State::Closed]
        .into_iter()
        .enumerate()
    {
        assert_eq!(pair.states(), both(State::HalfOpen), "probe {probe}");
        assert_eq!(pair.call(Outcome::Success).await, both(Decision::Sent));
        assert_eq!(pair.states(), both(then), "probe {probe}");
        tokio::time::advance(TENTH).await;
    }

    for _ in 0..2 {
        pair.call(Outcome::Failure).await;
    }
    assert_eq!(pair.states(), both(State::Closed));
    pair.call(Outcome::Failure).await;
    assert_eq!(pair.states(), both(State::Open));
}

/// The one probe of a series of one, and the last of a series of three whose
/// one failure reaches the reference's rate: open again for the whole time.
#[tokio::test(start_paused = true)]
async fn a_failing_probe_opens_it_again() {
    for (options, before) in [(timed(100, 3, 1), 0), (timed(25, 4, 3), 2)] {
        let mut pair = Pair::opened(options).await;
        tokio::time::advance(OPEN).await;
        for _ in 0..before {
            assert_eq!(pair.call(Outcome::Success).await, both(Decision::Sent));
            assert_eq!(pair.states(), both(State::HalfOpen));
        }
        assert_eq!(pair.call(Outcome::Failure).await, both(Decision::Sent));
        assert_eq!(pair.states(), both(State::Open));
        assert_eq!(
            pair.call(Outcome::Success).await,
            both(Decision::Refused { retry_after: OPEN })
        );
    }
}

/// In neither window, and in neither series of probes, which keeps its count
/// and hands the probe's turn on.
#[tokio::test(start_paused = true)]
async fn a_call_its_caller_dropped_counts_in_neither() {
    let mut pair = Pair::mapping(timed(100, 3, 2)).await;
    for outcome in [Outcome::Failure, Outcome::Failure, Outcome::Ignored] {
        assert_eq!(pair.call(outcome).await, both(Decision::Sent));
        assert_eq!(pair.states(), both(State::Closed));
    }
    pair.call(Outcome::Failure).await;
    assert_eq!(pair.states(), both(State::Open));

    tokio::time::advance(OPEN).await;
    for (outcome, then) in [
        (Outcome::Ignored, State::HalfOpen),
        (Outcome::Success, State::HalfOpen),
        (Outcome::Ignored, State::HalfOpen),
        (Outcome::Success, State::Closed),
    ] {
        assert_eq!(pair.call(outcome).await, both(Decision::Sent));
        assert_eq!(pair.states(), both(then));
    }
}

/// Answers `record_if` refuses between its failures leave each window as it
/// was, a count window and a time window alike: both open at the third
/// failure, though more answers came between than this policy's window holds.
#[tokio::test(start_paused = true)]
async fn an_err_record_if_refuses_counts_in_neither_window() {
    for options in [
        timed(100, 3, 1),
        Options {
            sliding_window: SlidingWindow::Count(4),
            ..timed(100, 3, 1)
        },
    ] {
        let mut pair = Pair::mapping(options).await;
        for _ in 0..2 {
            assert_eq!(pair.judged(Outcome::Failure).await, both(Decision::Sent));
            for _ in 0..7 {
                assert_eq!(pair.judged(Outcome::Ignored).await, both(Decision::Sent));
                assert_eq!(pair.states(), both(State::Closed), "{options:?}");
            }
        }
        assert_eq!(pair.judged(Outcome::Failure).await, both(Decision::Sent));
        assert_eq!(pair.states(), both(State::Open), "{options:?}");
        assert_eq!(
            pair.judged(Outcome::Success).await,
            both(Decision::Refused { retry_after: OPEN })
        );
    }
}

/// A probe answering what `record_if` refuses leaves both half-open, the next
/// call the probe, and a series its count: two successes close it, however
/// many such answers come before, between and after them.
#[tokio::test(start_paused = true)]
async fn a_probe_answering_an_err_record_if_refuses_hands_its_turn_on_in_both() {
    let mut pair = Pair::opened(timed(100, 3, 2)).await;
    tokio::time::advance(OPEN).await;
    for (outcome, then) in [
        (Outcome::Ignored, State::HalfOpen),
        (Outcome::Ignored, State::HalfOpen),
        (Outcome::Success, State::HalfOpen),
        (Outcome::Ignored, State::HalfOpen),
        (Outcome::Success, State::Closed),
    ] {
        assert_eq!(pair.judged(outcome).await, both(Decision::Sent));
        assert_eq!(pair.states(), both(then), "after {outcome:?}");
    }
}

/// Such a probe neither closes nor opens: the failure after it is the probe
/// that opens both again, for the whole open time.
#[tokio::test(start_paused = true)]
async fn a_failing_probe_after_one_record_if_refuses_opens_both_again() {
    let mut pair = Pair::opened(timed(100, 3, 1)).await;
    tokio::time::advance(OPEN).await;
    assert_eq!(pair.judged(Outcome::Ignored).await, both(Decision::Sent));
    assert_eq!(pair.states(), both(State::HalfOpen));
    assert_eq!(pair.judged(Outcome::Failure).await, both(Decision::Sent));
    assert_eq!(pair.states(), both(State::Open));
    assert_eq!(
        pair.judged(Outcome::Success).await,
        both(Decision::Refused { retry_after: OPEN })
    );
}

/// A failure falling at the end of its bucket is dropped nine tenths of the
/// window later, and one falling at its start a whole window later: aged the
/// window, this breaker counts both, and aged nine tenths of it, neither.
#[tokio::test(start_paused = true)]
async fn the_references_window_forgets_between_nine_tenths_of_its_size_and_the_whole() {
    let options = timed(100, 3, 1);
    let late = Duration::from_millis(999);
    for (into_its_bucket, before_the_rest, reference) in [
        (late, WINDOW - late, State::Closed),
        (Duration::ZERO, WINDOW * 95 / 100, State::Open),
    ] {
        let mut whole = Pair::mapping(options).await;
        let mut nine_tenths = Pair::new(
            ResilienceConfig {
                breaker_age: Some(WINDOW * 9 / 10),
                ..mapped(options)
            },
            options,
            STARTED,
        )
        .await;

        tokio::time::advance(into_its_bucket).await;
        whole.call(Outcome::Failure).await;
        nine_tenths.call(Outcome::Failure).await;
        tokio::time::advance(before_the_rest).await;
        for _ in 0..2 {
            whole.call(Outcome::Failure).await;
            nine_tenths.call(Outcome::Failure).await;
        }

        let (counted, forgotten) = (State::Open, State::Closed);
        assert_eq!(
            whole.states(),
            Both {
                ours: counted,
                reference
            }
        );
        assert_eq!(
            nine_tenths.states(),
            Both {
                ours: forgotten,
                reference
            }
        );
    }
}

/// This breaker the stricter: two failures in five calls where the reference
/// asks half of them, and five failures where it asks ten calls first.
#[tokio::test(start_paused = true)]
async fn a_count_of_failures_opens_before_the_references_rate() {
    use Outcome::{Failure, Success};

    for (options, trace) in [
        (
            timed(50, 4, 1),
            [Success, Success, Success, Failure, Failure],
        ),
        (timed(50, 10, 1), [Failure; 5]),
    ] {
        let mut pair = Pair::mapping(options).await;
        for outcome in trace {
            assert_eq!(pair.states(), both(State::Closed));
            assert_eq!(pair.call(outcome).await, both(Decision::Sent));
        }
        assert_eq!(
            pair.states(),
            Both {
                ours: State::Open,
                reference: State::Closed
            }
        );
    }
}

/// This breaker the laxer, off the mapping: six failures in twelve calls of
/// one age open the reference, and a window of its ten calls' minimum never
/// held five of them; of twice that, it opens at the fifth, a call before.
#[tokio::test(start_paused = true)]
async fn a_window_under_twice_the_minimum_of_calls_forgets_by_count_first() {
    use Outcome::{Failure, Success};

    let options = timed(50, 10, 1);
    let trace = [
        Failure, Failure, Success, Success, Success, Success, Success, Success, Failure, Failure,
        Failure, Failure,
    ];

    let mut under = Pair::new(
        ResilienceConfig {
            breaker_window: Some(10),
            ..mapped(options)
        },
        options,
        STARTED,
    )
    .await;
    for outcome in trace {
        assert_eq!(under.call(outcome).await, both(Decision::Sent));
    }
    assert_eq!(
        under.states(),
        Both {
            ours: State::Closed,
            reference: State::Open
        }
    );

    let mut twice = Pair::mapping(options).await;
    for outcome in &trace[..11] {
        assert_eq!(twice.call(*outcome).await, both(Decision::Sent));
    }
    assert_eq!(
        twice.states(),
        Both {
            ours: State::Open,
            reference: State::Closed
        }
    );
}

/// This breaker the stricter: of three calls arriving together the reference
/// sends three and closes on their answers, where one probes here, the others
/// refused for as long as its own bound, and two more follow it.
#[tokio::test(start_paused = true)]
async fn probes_go_one_at_a_time_where_the_references_go_together() {
    let mut pair = Pair::opened(timed(100, 3, 3)).await;
    tokio::time::advance(OPEN).await;

    let beside_a_probe = Decision::Refused {
        retry_after: TIMEOUT,
    };
    let (first, met) = pair.start().await;
    assert_eq!(met, both(Decision::Sent));
    let mut together = vec![first];
    for _ in 0..2 {
        let (flight, met) = pair.start().await;
        assert_eq!(
            met,
            Both {
                ours: beside_a_probe,
                reference: Decision::Sent
            }
        );
        together.push(flight);
    }
    assert_eq!(
        pair.start().await.1,
        Both {
            ours: beside_a_probe,
            reference: Decision::Refused {
                retry_after: Duration::ZERO
            }
        }
    );

    for flight in together {
        pair.settle(flight, Outcome::Success).await;
    }
    assert_eq!(
        pair.states(),
        Both {
            ours: State::HalfOpen,
            reference: State::Closed
        }
    );
    for _ in 0..2 {
        assert_eq!(pair.call(Outcome::Success).await, both(Decision::Sent));
    }
    assert_eq!(pair.states(), both(State::Closed));
}

/// This breaker the stricter: the reference sends the rest of the series to
/// the dependency that just failed, and closes on one failure in three, under
/// its rate of a half.
#[tokio::test(start_paused = true)]
async fn a_failing_probe_opens_at_once_where_the_reference_judges_its_series() {
    let mut pair = Pair::opened(timed(50, 4, 3)).await;
    tokio::time::advance(OPEN).await;

    assert_eq!(pair.call(Outcome::Failure).await, both(Decision::Sent));
    assert_eq!(
        pair.states(),
        Both {
            ours: State::Open,
            reference: State::HalfOpen
        }
    );
    for _ in 0..2 {
        assert_eq!(
            pair.call(Outcome::Success).await,
            Both {
                ours: Decision::Refused { retry_after: OPEN },
                reference: Decision::Sent
            }
        );
    }
    assert_eq!(
        pair.states(),
        Both {
            ours: State::Open,
            reference: State::Closed
        }
    );
}

/// What a trace of calls outliving the open time is drawn on: a second.
const BRIEFLY: Duration = Duration::from_secs(1);

/// The mapping of a breaker two failures open for [`BRIEFLY`], once `early`
/// calls were sent and left waiting, two failures opened both and a probe
/// closed both: the calls still in flight were admitted before the opening.
async fn closed_again_with_calls_in_flight(early: usize) -> (Pair, Vec<Flight>) {
    let mut pair = Pair::mapping(Options {
        open_duration: BRIEFLY,
        ..timed(100, 2, 1)
    })
    .await;
    let mut flights = Vec::new();
    for _ in 0..early {
        let (flight, met) = pair.start().await;
        assert_eq!(met, both(Decision::Sent));
        flights.push(flight);
    }
    for _ in 0..2 {
        assert_eq!(pair.call(Outcome::Failure).await, both(Decision::Sent));
    }
    assert_eq!(pair.states(), both(State::Open));
    tokio::time::advance(BRIEFLY).await;
    assert_eq!(pair.call(Outcome::Success).await, both(Decision::Sent));
    assert_eq!(pair.states(), both(State::Closed));
    (pair, flights)
}

/// A failure sent before an opening and answered after the closing opens
/// neither on evidence older than the recovery: each window is one failure
/// short after the next, and opens at the one after it.
#[tokio::test(start_paused = true)]
async fn a_failure_sent_before_an_opening_counts_in_neither_once_it_answers_after_the_closing() {
    let (mut pair, mut early) = closed_again_with_calls_in_flight(1).await;
    let early = early.pop().expect("one call in flight");

    pair.settle(early, Outcome::Failure).await;
    assert_eq!(pair.states(), both(State::Closed));
    assert_eq!(pair.call(Outcome::Failure).await, both(Decision::Sent));
    assert_eq!(pair.states(), both(State::Closed));
    assert_eq!(pair.call(Outcome::Failure).await, both(Decision::Sent));
    assert_eq!(pair.states(), both(State::Open));
}

/// Successes sent before an opening and answered after the closing push no
/// failure out of either window, as many of them as this policy's holds: both
/// open at the second failure since the closing, and refuse the call after.
#[tokio::test(start_paused = true)]
async fn successes_sent_before_an_opening_push_no_failure_out_once_they_answer_after_the_closing() {
    let (mut pair, early) = closed_again_with_calls_in_flight(4).await;

    assert_eq!(pair.call(Outcome::Failure).await, both(Decision::Sent));
    for flight in early {
        pair.settle(flight, Outcome::Success).await;
    }
    assert_eq!(pair.call(Outcome::Failure).await, both(Decision::Sent));
    assert_eq!(pair.states(), both(State::Open));
    assert_eq!(
        pair.call(Outcome::Failure).await,
        both(Decision::Refused {
            retry_after: BRIEFLY
        })
    );
}

/// What a deployment moving from the reference must know: the two are alike
/// until the first failure that opens this breaker alone, and from there
/// their cycles are out of step by up to the open time. Opened half the open
/// time before the reference, this breaker probes while the reference still
/// refuses, and closed by that probe sends every call the reference refuses.
#[tokio::test(start_paused = true)]
async fn having_opened_first_this_breaker_probes_and_closes_while_the_reference_still_refuses() {
    let half = BRIEFLY / 2;
    let mut pair = Pair::mapping(Options {
        open_duration: BRIEFLY,
        ..timed(50, 4, 1)
    })
    .await;
    for _ in 0..2 {
        assert_eq!(pair.call(Outcome::Failure).await, both(Decision::Sent));
    }
    assert_eq!(
        pair.states(),
        Both {
            ours: State::Open,
            reference: State::Closed
        }
    );

    tokio::time::advance(half).await;
    for _ in 0..2 {
        assert_eq!(
            pair.call(Outcome::Failure).await,
            Both {
                ours: Decision::Refused { retry_after: half },
                reference: Decision::Sent
            }
        );
    }
    assert_eq!(pair.states(), both(State::Open));

    tokio::time::advance(half).await;
    let still_refused = Both {
        ours: Decision::Sent,
        reference: Decision::Refused { retry_after: half },
    };
    assert_eq!(
        pair.call(Outcome::Success).await,
        still_refused,
        "the probe"
    );
    assert_eq!(
        pair.states(),
        Both {
            ours: State::Closed,
            reference: State::Open
        }
    );
    assert_eq!(pair.call(Outcome::Success).await, still_refused);
}

/// A host polling the dependency through a decorated method whose
/// `retry_ok_if` sends every `Ok` again, counting the attempts sent.
struct Poller {
    policy: Arc<ResiliencePolicy>,
    attempts: AtomicUsize,
}

impl Poller {
    #[resilient(policy, idempotent, retry_ok_if = |_| true)]
    async fn poll(&self) -> Result<(), ResilienceError<Down>> {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// An `Ok` its method's `retry_ok_if` sends again is a success of its
/// attempt: each is an outcome of a closed breaker's window, and a probe's
/// closes a half-open one, the attempts sent after it counted in the window
/// it opened.
#[tokio::test(start_paused = true)]
async fn an_ok_sent_again_counts_as_a_success_in_the_window_and_as_a_probe() {
    let (_app, policy) = crate::policy(ResilienceConfig {
        retries: Some(2),
        breaker_window: Some(4),
        breaker_failures: 2,
        ..ResilienceConfig::default()
    })
    .await;
    let poller = Poller {
        policy: Arc::clone(&policy),
        attempts: AtomicUsize::new(0),
    };
    let counted = || {
        let stats = policy.breaker_stats().expect("the policy has a breaker");
        (stats.outcomes, stats.failures)
    };

    assert!(poller.poll().await.is_ok());
    assert_eq!(
        (poller.attempts.swap(0, Ordering::Relaxed), counted()),
        (3, (3, 0)),
        "sent, then again twice, each `Ok` a success of the window"
    );

    policy.open_breaker(BRIEFLY).expect("a breaker to open");
    tokio::time::advance(BRIEFLY).await;
    assert_eq!(policy.breaker_state(), Some(BreakerState::HalfOpen));
    assert!(poller.poll().await.is_ok());
    assert_eq!(poller.attempts.load(Ordering::Relaxed), 3);
    assert_eq!(
        (policy.breaker_state(), counted()),
        (Some(BreakerState::Closed), (2, 0)),
        "the probe's `Ok` closed it, sent again though it was, and the two after it count"
    );
}

/// SplitMix64: a trace is drawn from its index alone, the same on every run.
#[derive(Clone)]
struct Draws(u64);

impl Draws {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut drawn = self.0;
        drawn = (drawn ^ (drawn >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        drawn = (drawn ^ (drawn >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        drawn ^ (drawn >> 31)
    }

    fn of<T: Copy>(&mut self, among: &[T]) -> T {
        among[(self.next() % among.len() as u64) as usize]
    }

    fn chance(&mut self, per_cent: u64) -> bool {
        self.next() % 100 < per_cent
    }
}

/// How many traces a generated test draws, and how many calls each makes.
const TRACES: u64 = 240;
const CALLS: usize = 40;

/// Gaps anywhere in a window, in quarters of a bucket: none, within a bucket,
/// across a few, under nine tenths of the window, between that and the whole,
/// and past it.
const ANYWHERE: [u32; 14] = [0, 0, 1, 1, 2, 3, 5, 9, 17, 35, 37, 39, 40, 41];

/// Gaps that keep a whole trace within one window, in quarters of a bucket.
const CROWDED: [u32; 4] = [0, 0, 0, 1];

/// Gaps on bucket boundaries, in whole buckets, to either side of the window.
const ON_BOUNDARIES: [u32; 11] = [0, 0, 1, 1, 2, 3, 5, 8, 9, 10, 11];

/// How a trace's calls are drawn.
#[derive(Clone, Copy)]
struct Shape {
    /// What a gap between two calls is a multiple of, one of `gaps`.
    grain: Duration,
    gaps: &'static [u32],
    /// How many gaps in a hundred are the open time instead.
    resting: u64,
    /// How many answers in a hundred are failures.
    failing: u64,
    /// Whether the dependency is down for the closed breakers: every answer
    /// they count a failure, `failing` being the probes' alone.
    down: bool,
    /// Whether a call counting in neither breaker answered what `record_if`
    /// refuses ([`Pair::judged`]), where its caller drops it otherwise.
    declined: bool,
}

/// Where two breakers driven through one trace first differ.
#[derive(Debug)]
struct Difference {
    /// The state both were in as the call arrived.
    met: State,
    outcome: Outcome,
    decision: Both<Decision>,
    then: Both<State>,
    /// Whether the reference's own condition opens on that call once read over
    /// what the trace answered within its window exactly, no bucket rounding.
    exactly: bool,
}

impl Difference {
    /// At a failure both sent, which opened this breaker and not the
    /// reference: this breaker the stricter.
    fn opens_ours_alone(&self) -> bool {
        self.outcome == Outcome::Failure
            && self.decision == both(Decision::Sent)
            && self.then.ours == State::Open
            && self.then.reference != State::Open
    }

    /// At a call both closed breakers sent, which opened the reference and
    /// not this breaker: this breaker the laxer.
    fn opens_the_reference_alone(&self) -> bool {
        let then = Both {
            ours: State::Closed,
            reference: State::Open,
        };
        self.met == State::Closed && self.decision == both(Decision::Sent) && self.then == then
    }
}

/// What the closed breakers were `answered` since they closed, within
/// `window` now: its latest calls, or those no older than its size.
fn within(answered: &[(Instant, bool)], window: SlidingWindow) -> WindowStats {
    let now = Instant::now();
    let held = match window {
        SlidingWindow::Count(size) => &answered[answered.len().saturating_sub(size)..],
        SlidingWindow::Time(_) => answered,
    };
    let held = held.iter().filter(|(at, _)| match window {
        SlidingWindow::Count(_) => true,
        SlidingWindow::Time(size) => now.duration_since(*at) <= size,
    });
    let (mut total, mut failures) = (0, 0);
    for (_, failed) in held {
        total += 1;
        failures += usize::from(*failed);
    }
    WindowStats { total, failures }
}

/// Draws [`CALLS`] calls of `shape`, each answered at once, through `pair`:
/// the first whose decision or whose state after it differs between the two,
/// if one does.
async fn first_difference(
    pair: &mut Pair,
    options: Options,
    draws: &mut Draws,
    shape: Shape,
) -> Option<Difference> {
    let mut answered = Vec::new();
    for _ in 0..CALLS {
        let gap = if draws.chance(shape.resting) {
            options.open_duration
        } else {
            shape.grain * draws.of(shape.gaps)
        };
        if !gap.is_zero() {
            tokio::time::advance(gap).await;
        }
        let met = pair.states();
        assert_eq!(met.ours, met.reference, "time alone moves both alike");
        let outcome = if draws.chance(5) {
            Outcome::Ignored
        } else if (shape.down && met.ours == State::Closed) || draws.chance(shape.failing) {
            Outcome::Failure
        } else {
            Outcome::Success
        };

        let decision = if shape.declined {
            pair.judged(outcome).await
        } else {
            pair.call(outcome).await
        };
        let then = pair.states();
        if met.ours == State::Closed && outcome != Outcome::Ignored {
            answered.push((Instant::now(), outcome == Outcome::Failure));
        }
        if decision.ours != decision.reference || then.ours != then.reference {
            return Some(Difference {
                met: met.ours,
                outcome,
                decision,
                then,
                exactly: options.opens_on(within(&answered, options.sliding_window)),
            });
        }
        if then.ours != State::Closed {
            answered.clear();
        }
    }
    None
}

/// How many calls a trace of calls in flight together leaves unanswered at
/// most.
const IN_FLIGHT: usize = 4;

/// What each attempt of such a trace is sent under, the longest a policy
/// takes, and how long one of its calls is left unanswered at most: none
/// times out.
const PATIENCE: Duration = Duration::from_secs(600);
const LONGEST: Duration = Duration::from_secs(300);

/// `config`, its attempts sent under [`PATIENCE`].
fn patient(config: ResilienceConfig) -> ResilienceConfig {
    ResilienceConfig {
        timeout: PATIENCE,
        ..config
    }
}

/// One step of a trace of calls in flight together.
#[derive(Clone, Copy, Debug)]
enum Step {
    /// A call arrived, and each breaker met it so.
    Arrived(Both<Decision>),
    /// A call both had sent answered.
    Answered(Outcome),
}

/// Where two breakers driven through one trace of calls in flight together
/// first differ.
#[derive(Debug)]
struct Apart {
    /// The state both were in before the step.
    met: State,
    step: Step,
    then: Both<State>,
}

impl Apart {
    /// At a failure both had sent, which opened this breaker and not the
    /// reference: this breaker the stricter.
    fn opens_ours_alone(&self) -> bool {
        matches!(self.step, Step::Answered(Outcome::Failure))
            && self.then.ours == State::Open
            && self.then.reference != State::Open
    }

    /// At a call arriving while a probe is out, which this breaker refuses
    /// and the reference sends as a probe of its own: this breaker the
    /// stricter.
    fn refused_beside_a_probe(&self) -> bool {
        self.met == State::HalfOpen
            && matches!(
                self.step,
                Step::Arrived(Both {
                    ours: Decision::Refused { .. },
                    reference: Decision::Sent,
                })
            )
            && self.then == both(State::HalfOpen)
    }
}

/// Draws twice [`CALLS`] steps of `shape` through `pair`, whose policy is
/// [`patient`] — a call arriving, left unanswered among [`IN_FLIGHT`] at
/// most, or any one of those answering: the first step whose decision or
/// whose state after it differs between the two, if one does.
async fn first_apart(
    pair: &mut Pair,
    options: Options,
    draws: &mut Draws,
    shape: Shape,
) -> Option<Apart> {
    let mut flights: Vec<(Instant, Flight)> = Vec::new();
    for _ in 0..2 * CALLS {
        let gap = if draws.chance(shape.resting) {
            options.open_duration
        } else {
            shape.grain * draws.of(shape.gaps)
        };
        let overdue = flights
            .iter()
            .position(|(sent, _)| sent.elapsed() + gap >= LONGEST);
        if overdue.is_none() && !gap.is_zero() {
            tokio::time::advance(gap).await;
        }
        let met = pair.states();
        assert_eq!(met.ours, met.reference, "time alone moves both alike");

        let arrives = flights.is_empty() || (flights.len() < IN_FLIGHT && draws.chance(50));
        let step = if overdue.is_none() && arrives {
            let (flight, decision) = pair.start().await;
            if decision == both(Decision::Sent) {
                flights.push((Instant::now(), flight));
            }
            Step::Arrived(decision)
        } else {
            let answering =
                overdue.unwrap_or_else(|| (draws.next() % flights.len() as u64) as usize);
            let (_, flight) = flights.swap_remove(answering);
            let outcome = if draws.chance(5) {
                Outcome::Ignored
            } else if draws.chance(shape.failing) {
                Outcome::Failure
            } else {
                Outcome::Success
            };
            pair.settle(flight, outcome).await;
            Step::Answered(outcome)
        };
        let then = pair.states();
        let met_alike = match step {
            // Beside a probe both refuse a call, the reference telling no
            // wait and this breaker what is left of the probe's bound.
            Step::Arrived(Both {
                ours: Decision::Refused { .. },
                reference: Decision::Refused { .. },
            }) if met.ours == State::HalfOpen => true,
            Step::Arrived(decision) => decision.ours == decision.reference,
            Step::Answered(_) => true,
        };
        if !met_alike || then.ours != then.reference {
            return Some(Apart {
                met: met.ours,
                step,
                then,
            });
        }
    }
    None
}

/// Any options under the mapping, any instants, any answers — a call counting
/// in neither dropped by its caller on one trace in two, answered what
/// `record_if` refuses on the other: the two differ first, when they do, at a
/// failure both sent that opens this breaker alone.
#[tokio::test(start_paused = true)]
async fn generated_traces_differ_only_where_this_breaker_is_the_stricter() {
    let (mut alike, mut counted, mut dropped_early, mut probed) = (0, 0, 0, 0);
    for index in 0..TRACES {
        let mut draws = Draws(index);
        let sliding_window = if draws.chance(25) {
            SlidingWindow::Count(draws.of(&[10, 20]))
        } else {
            SlidingWindow::Time(Duration::from_secs(draws.of(&[1, 10, 60])))
        };
        let options = Options {
            failure_rate_threshold: draws.of(&[25, 50, 100, 100]),
            minimum_calls: draws.of(&[1, 2, 4, 8, 10]),
            sliding_window,
            open_duration: Duration::from_secs(draws.of(&[1, 5, 30])),
            half_open_max_calls: draws.of(&[1, 2, 3]),
        };
        let shape = Shape {
            grain: match sliding_window {
                SlidingWindow::Count(_) => Duration::from_millis(250),
                SlidingWindow::Time(size) => size / 40,
            },
            gaps: &ANYWHERE,
            resting: 15,
            failing: draws.of(&[40, 60, 100, 100]),
            down: false,
            declined: index % 2 == 1,
        };
        let started = STARTED + draws.next() % 60_000;
        let mut pair = Pair::new(mapped(options), options, started).await;

        let Some(difference) = first_difference(&mut pair, options, &mut draws, shape).await else {
            alike += 1;
            continue;
        };
        assert!(
            difference.opens_ours_alone(),
            "trace {index}, of {options:?}: {difference:?}"
        );
        match (difference.met, difference.exactly) {
            (State::HalfOpen, _) => probed += 1,
            (State::Closed, false) => counted += 1,
            (State::Closed, true) => {
                assert!(
                    matches!(sliding_window, SlidingWindow::Time(_)),
                    "trace {index}: a count window rounds nothing: {difference:?}"
                );
                dropped_early += 1;
            }
            (State::Open, _) => panic!("trace {index}: an open breaker sends nothing"),
        }
    }
    assert!(
        alike > 0 && counted > 0 && dropped_early > 0 && probed > 0,
        "every class is drawn: {alike} alike, {counted} on a count under the rate, \
         {dropped_early} on a failure dropped early, {probed} on a failing probe"
    );
}

/// Any options under the mapping, any instants, any answers, up to
/// [`IN_FLIGHT`] calls in flight together and answered in any order: the two
/// differ first, when they do, where this breaker is the stricter — at a
/// failure both sent that opens it alone, or at a call it refuses beside its
/// probe, which the reference sends as a probe of its own.
#[tokio::test(start_paused = true)]
async fn traces_of_calls_in_flight_together_differ_only_where_this_breaker_is_the_stricter() {
    let (mut alike, mut opened, mut refused) = (0, 0, 0);
    for index in 0..TRACES {
        let mut draws = Draws(index);
        let sliding_window = if draws.chance(25) {
            SlidingWindow::Count(draws.of(&[10, 20]))
        } else {
            SlidingWindow::Time(Duration::from_secs(draws.of(&[1, 10, 60])))
        };
        let options = Options {
            failure_rate_threshold: draws.of(&[25, 50, 100, 100]),
            minimum_calls: draws.of(&[1, 2, 4, 8, 10]),
            sliding_window,
            open_duration: Duration::from_secs(draws.of(&[1, 5, 30])),
            half_open_max_calls: draws.of(&[1, 2, 3]),
        };
        let shape = Shape {
            grain: match sliding_window {
                SlidingWindow::Count(_) => Duration::from_millis(250),
                SlidingWindow::Time(size) => size / 40,
            },
            gaps: &ANYWHERE,
            resting: 15,
            failing: draws.of(&[40, 60, 100, 100]),
            down: false,
            declined: false,
        };
        let started = STARTED + draws.next() % 60_000;
        let mut pair = Pair::new(patient(mapped(options)), options, started).await;

        let Some(apart) = first_apart(&mut pair, options, &mut draws, shape).await else {
            alike += 1;
            continue;
        };
        if apart.opens_ours_alone() {
            opened += 1;
        } else {
            assert!(
                apart.refused_beside_a_probe(),
                "trace {index}, of {options:?}: {apart:?}"
            );
            refused += 1;
        }
    }
    assert!(
        alike > 0 && opened > 0 && refused > 0,
        "every class is drawn: {alike} alike, {opened} on a failure opening this breaker \
         alone, {refused} on a call refused beside a probe"
    );
}

/// A whole trace within one window, more calls than the policy's holds, their
/// failures at the reference's own rate: counting twice the reference's
/// minimum of calls it is never the laxer, its calls answered in turn or in
/// flight together, and counting that minimum alone it is, on the traces
/// whose failures the rate reads further back than it counts.
#[tokio::test(start_paused = true)]
async fn crowded_traces_find_a_window_of_twice_the_minimum_never_the_laxer() {
    let mut laxer_under = 0;
    for index in 0..TRACES {
        let mut draws = Draws(index);
        let threshold = draws.of(&[25, 50, 75]);
        let options = timed(threshold, draws.of(&[8, 10]), 1);
        let shape = Shape {
            grain: WINDOW / 40,
            gaps: &CROWDED,
            resting: 0,
            failing: u64::from(threshold),
            down: false,
            declined: false,
        };

        let mut twice = Pair::mapping(options).await;
        let difference = first_difference(&mut twice, options, &mut draws.clone(), shape).await;
        assert!(
            difference.as_ref().is_none_or(Difference::opens_ours_alone),
            "trace {index}, of {options:?}: {difference:?}"
        );

        let mut together = Pair::new(patient(mapped(options)), options, STARTED).await;
        let apart = first_apart(&mut together, options, &mut draws.clone(), shape).await;
        assert!(
            apart.as_ref().is_none_or(Apart::opens_ours_alone),
            "trace {index}, of {options:?}: {apart:?}"
        );

        let under = ResilienceConfig {
            breaker_window: Some(options.minimum_calls),
            ..mapped(options)
        };
        let mut under = Pair::new(under, options, STARTED).await;
        let difference = first_difference(&mut under, options, &mut draws, shape).await;
        laxer_under += usize::from(difference.is_some_and(|met| met.opens_the_reference_alone()));
    }
    assert!(
        laxer_under > 0,
        "no trace found a window of the minimum the laxer"
    );
}

/// The options of a dependency down behind a time window: every answer in the
/// window a failure, so the reference's rate is its count, and a probe failing
/// only as the one of its series, so its series is this breaker's.
fn down(draws: &mut Draws, open_in_tenths: bool) -> (Options, Shape) {
    let size = Duration::from_secs(draws.of(&[1, 10, 60]));
    let half_open_max_calls = draws.of(&[1, 2, 3]);
    let options = Options {
        failure_rate_threshold: 100,
        minimum_calls: draws.of(&[1, 2, 3, 5]),
        sliding_window: SlidingWindow::Time(size),
        open_duration: if open_in_tenths {
            size / 10 * draws.of(&[3, 10, 25])
        } else {
            Duration::from_secs(draws.of(&[1, 5, 30]))
        },
        half_open_max_calls,
    };
    let shape = Shape {
        grain: size / 10,
        gaps: &ON_BOUNDARIES,
        resting: 15,
        failing: if half_open_max_calls == 1 {
            draws.of(&[30, 60, 90])
        } else {
            0
        },
        down: true,
        declined: false,
    };
    (options, shape)
}

/// [`mapped`], its age nine tenths of the reference's window.
fn aged_nine_tenths(options: Options) -> ResilienceConfig {
    let SlidingWindow::Time(size) = options.sliding_window else {
        panic!("an age stands for a time window: {options:?}");
    };
    ResilienceConfig {
        breaker_age: Some(size * 9 / 10),
        ..mapped(options)
    }
}

/// Every instant on a bucket's boundary, the reference drops an outcome nine
/// buckets after its own: an age of nine tenths of its window, to the call.
#[tokio::test(start_paused = true)]
async fn on_bucket_boundaries_a_dependency_down_reads_the_same_trace_for_trace() {
    for index in 0..TRACES {
        let mut draws = Draws(index);
        let (options, shape) = down(&mut draws, true);
        let mut pair = Pair::new(aged_nine_tenths(options), options, STARTED).await;
        let difference = first_difference(&mut pair, options, &mut draws, shape).await;
        assert!(
            difference.is_none(),
            "trace {index}, of {options:?}: {difference:?}"
        );
    }
}

/// Off the boundaries, that age is the lower bound: it forgets a failure the
/// reference's bucket still holds, so the two differ first, when they do, at
/// a failure that opens the reference alone.
#[tokio::test(start_paused = true)]
async fn off_them_an_age_of_nine_tenths_differs_only_where_the_reference_is_the_stricter() {
    let (mut alike, mut forgotten) = (0, 0);
    for index in 0..TRACES {
        let mut draws = Draws(index);
        let (options, on_boundaries) = down(&mut draws, false);
        let shape = Shape {
            grain: on_boundaries.grain / 4,
            gaps: &ANYWHERE,
            ..on_boundaries
        };
        let started = STARTED + draws.next() % 60_000;
        let mut pair = Pair::new(aged_nine_tenths(options), options, started).await;

        let Some(difference) = first_difference(&mut pair, options, &mut draws, shape).await else {
            alike += 1;
            continue;
        };
        assert!(
            difference.outcome == Outcome::Failure && difference.opens_the_reference_alone(),
            "trace {index}, of {options:?}: {difference:?}"
        );
        forgotten += 1;
    }
    assert!(
        alike > 0 && forgotten > 0,
        "both are drawn: {alike} alike, {forgotten} on a failure forgotten first"
    );
}

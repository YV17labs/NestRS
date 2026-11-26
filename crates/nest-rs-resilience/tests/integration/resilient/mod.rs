//! Covers `src/resilient.rs`, what `#[resilient]` expands to: a method's body
//! run under its policy as its attribute declares, on each receiver and host it
//! takes (`expansion`), a refusal answered `503` by every edge serving a
//! decorated handler, a method of every impl half that takes one, called
//! through the framework's own path, attempted again within that one call, the
//! batch of a `#[dataloader]` host, whose error the loader clones for each key
//! (`dataloader`), a refusal where no client reads a `503` — a tick, a
//! listener, an indicator, a hook — filed as that unit's failure with what the
//! framework does next, what a decorated method's future holds (`footprint`):
//! its size against its undecorated twin's, and a large one run on a worker of
//! tokio's default stack — and what its other keys do: the predicates saying
//! how an answer counts (`judged`), the fallback answering in place of a run's
//! error (`fallback`), the attempt's token in the body (`cancel`), and all of
//! them on each receiver and host (`keyed`), beside an instrument in either
//! order (`instrumented`), and when its arguments and what its body's tail
//! leaves drop, beside its undecorated twin (`dropped`).

mod cancel;
mod dataloader;
mod dropped;
mod expansion;
mod fallback;
mod footprint;
mod hooks;
mod indicators;
mod instrumented;
mod judged;
mod keyed;
mod listeners;
mod messages;
mod operations;
mod processor;
mod routes;
mod scheduled;
mod shadowing;
mod told;
mod tools;

use std::future::Future;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nest_rs_core::{Module, error_message, injectable, instance, module};
use nest_rs_resilience::{
    BreakerState, ResilienceConfig, ResilienceError, ResilienceEvent, ResilienceModule,
    ResiliencePolicy, ResilienceRefusal, resilient,
};
use nest_rs_testing::{HeadlessApp, LogCapture};

use crate::{held, quiet};

#[instance]
struct Courier;

#[derive(Debug, thiserror::Error)]
#[error("the carrier did not answer")]
struct CarrierDown;

/// What looking a parcel up answers when the carrier knows none by its number.
#[derive(Debug, thiserror::Error)]
#[error("no parcel by that number")]
struct NoParcel;

impl From<NoParcel> for CarrierDown {
    fn from(_: NoParcel) -> Self {
        Self
    }
}

fn depot(parcel: &str) -> Result<&'static str, NoParcel> {
    match parcel {
        "P1" => Ok("Lyon"),
        _ => Err(NoParcel),
    }
}

type Answer<T> = Result<T, ResilienceError<CarrierDown>>;

/// Where a parcel leaves from, which a decorated method answers borrowed.
struct Hub(String);

impl Default for Hub {
    fn default() -> Self {
        Self("the Lyon hub".to_owned())
    }
}

/// Whether `answer` is the call's own failure, as its last attempt answered it.
fn failed<T>(answer: &Answer<T>) -> bool {
    matches!(answer, Err(ResilienceError::Failed(CarrierDown)))
}

async fn boot<M: Module + 'static>() -> HeadlessApp {
    HeadlessApp::builder()
        .module::<M>()
        .build_headless()
        .await
        .expect("the module boots")
}

/// One retry, no breaker: a row of a test sets what it is about.
fn retrying_once() -> ResilienceConfig {
    ResilienceConfig {
        retries: Some(1),
        breaker_window: None,
        ..ResilienceConfig::default()
    }
}

/// Opening on its one failure, which the docs' `Test it` recipe pins.
fn opening_on_the_first_failure() -> ResilienceConfig {
    ResilienceConfig {
        retries: None,
        breaker_window: Some(1),
        breaker_failures: 1,
        breaker_open: Duration::from_secs(30),
        ..ResilienceConfig::default()
    }
}

/// What a dependency answers on every edge below: a failure no client reads.
#[derive(Debug, thiserror::Error)]
#[error("the upstream answered 502")]
struct UpstreamDown;

impl nest_rs_core::ToProblem for UpstreamDown {
    fn to_problem(&self) -> Option<nest_rs_core::Problem> {
        None
    }
}

/// How many times the decorated methods below ran, each failing its first
/// attempt and answering its retry.
#[injectable]
#[derive(Default)]
struct Attempts {
    made: AtomicU32,
}

impl Attempts {
    /// Whether the attempt about to run is the first, which fails.
    fn first(&self) -> bool {
        self.made.fetch_add(1, Ordering::Relaxed) == 0
    }

    fn made(&self) -> u32 {
        self.made.load(Ordering::Relaxed)
    }
}

/// One retry a millisecond or two after the failure, for a test on the real
/// clock.
fn retrying_at_once() -> ResilienceConfig {
    ResilienceConfig {
        backoff_first: Duration::from_millis(1),
        backoff_ceiling: Duration::from_millis(2),
        ..retrying_once()
    }
}

/// The one retry `logs` holds, which is `method` of `host`'s: a handler sent
/// again within the call that reached it files what a provider's method does.
#[track_caller]
fn retried_once(logs: &LogCapture, host: &str, method: &str) {
    let retried = logs.expect_one(
        nest_rs_resilience::TARGET,
        "attempt failed; retrying the call",
    );
    assert_eq!(retried.field("host").as_deref(), Some(host), "{retried:?}");
    assert_eq!(retried.field("method").as_deref(), Some(method));
    assert_eq!(
        retried.field("error"),
        Some(error_message(&UpstreamDown)),
        "{retried:?}"
    );
}

/// The dependency a unit no client waits on calls below: it answers at once,
/// or never while a test stalls it.
#[injectable]
#[derive(Default)]
struct Dependency {
    calls: AtomicU32,
    stalled: AtomicBool,
}

impl Dependency {
    async fn call(&self) -> Result<(), UpstreamDown> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.stalled.load(Ordering::Relaxed) {
            std::future::pending::<()>().await;
        }
        Ok(())
    }

    fn called(&self) -> u32 {
        self.calls.load(Ordering::Relaxed)
    }
}

/// The timeout each unit below pins, `timeout = "100ms"`, which a stalled
/// dependency outlasts.
const PINNED: Duration = Duration::from_millis(100);

/// How many tokens the rate of [`refusing`] holds, every one of which a test
/// takes to have it refuse.
const TOKENS: u8 = 8;

/// A breaker only a hand opens, one slot of the bulkhead — what a refused
/// unit leaves held, the next one meets — and a rate a test drains, giving a
/// token back every 30 seconds after.
fn refusing() -> ResilienceConfig {
    refusing_for(REFUSED_FOR)
}

/// How long a refusal [`Refusing::arrange`] sets up lasts unlifted: the open
/// time of the breaker it opens, and the time a drained [`refusing`] rate
/// takes to give a token back.
const REFUSED_FOR: Duration = Duration::from_secs(30);

/// [`refusing`], its drained rate giving a token back after `refill`.
fn refusing_for(refill: Duration) -> ResilienceConfig {
    ResilienceConfig {
        retries: None,
        breaker_window: Some(8),
        breaker_failures: 8,
        concurrency: Some(1),
        rate_limit: Some(usize::from(TOKENS)),
        rate_window: refill * u32::from(TOKENS),
        ..ResilienceConfig::default()
    }
}

/// What a decorated method of a unit below answers.
type Refusable = Result<(), ResilienceError<UpstreamDown>>;

/// How a test has its policy refuse the call of a unit no client reads the
/// `503` of: each refusal the policy makes, two telling their wait and two
/// telling none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refusing {
    /// The breaker opened by hand: the call is never sent.
    Open,
    /// The dependency stalled past the timeout the method pins.
    TimedOut,
    /// The bulkhead's one slot held by another call: the call is never sent.
    Full,
    /// Every token of the rate taken: the call is never sent.
    Limited,
}

/// A refusal [`Refusing::arrange`] set up: the policy and the dependency the
/// container holds, and the call holding the bulkhead's one slot while it is
/// full.
struct Refused {
    refusing: Refusing,
    upstream: Arc<ResiliencePolicy>,
    dependency: Arc<Dependency>,
    /// How long the refusal lasts unlifted.
    lasting: Duration,
    holding: Option<std::pin::Pin<Box<dyn Future<Output = Refusable>>>>,
}

impl Refusing {
    const EACH: [Self; 4] = [Self::Open, Self::TimedOut, Self::Full, Self::Limited];

    /// Has the policy and the dependency `container` holds refuse every call
    /// this way until it is lifted, or for [`REFUSED_FOR`].
    async fn arrange(self, container: &nest_rs_core::Container) -> Refused {
        self.arrange_for(container, REFUSED_FOR).await
    }

    /// [`arrange`](Self::arrange), for `lasting` alone: the open time it gives
    /// the breaker, and the refill of the rate the module pinned
    /// ([`refusing_for`]).
    async fn arrange_for(self, container: &nest_rs_core::Container, lasting: Duration) -> Refused {
        let upstream = container
            .get::<ResiliencePolicy>()
            .expect("the module built the policy");
        let dependency = container.get::<Dependency>().expect("provided");
        let mut holding = None;
        match self {
            Self::Open => upstream
                .open_breaker(lasting)
                .expect("the policy has a breaker, and takes the open time"),
            Self::TimedOut => dependency.stalled.store(true, Ordering::Relaxed),
            Self::Full => {
                let holder = Arc::clone(&upstream);
                let mut held: std::pin::Pin<Box<dyn Future<Output = Refusable>>> =
                    Box::pin(async move {
                        holder
                            .run(|_| std::future::pending::<Result<(), UpstreamDown>>())
                            .await
                    });
                let waker = std::task::Waker::noop();
                assert!(
                    held.as_mut()
                        .poll(&mut std::task::Context::from_waker(waker))
                        .is_pending(),
                    "the holding call waits on its dependency"
                );
                assert_eq!(upstream.concurrency_running(), Some(1));
                holding = Some(held);
            }
            Self::Limited => {
                while upstream.rate_available() != Some(0) {
                    upstream
                        .run(|_| async { Ok::<_, UpstreamDown>(()) })
                        .await
                        .expect("a token is free");
                }
            }
        }
        Refused {
            refusing: self,
            upstream,
            dependency,
            lasting,
            holding,
        }
    }

    /// Whether the refusal ends of itself once [`REFUSED_FOR`], or what
    /// [`arrange_for`](Self::arrange_for) was given, has passed: the breaker
    /// half-open, the rate's token back.
    fn lapses(self) -> bool {
        matches!(self, Self::Open | Self::Limited)
    }

    /// How often the dependency was called by a unit refused this way: only
    /// a timeout sends it.
    fn sent(self) -> u32 {
        match self {
            Self::TimedOut => 1,
            Self::Open | Self::Full | Self::Limited => 0,
        }
    }

    /// The slots of the bulkhead taken once a unit refused this way ended:
    /// the holding call's alone.
    fn held(self) -> usize {
        usize::from(self == Self::Full)
    }

    /// The chain a line tells `refused` by, which is this refusal: what the
    /// decorated method answers a caller of its own, as the unit was answered.
    #[track_caller]
    fn chain(self, refused: Refusable) -> String {
        let error = refused.expect_err("the policy refuses the call");
        let wait =
            nest_rs_core::ToProblem::to_problem(&error).and_then(|problem| problem.retry_after());
        match (self, &error) {
            (
                Self::Open,
                ResilienceError::Refused {
                    refusal: ResilienceRefusal::Open { .. },
                    ..
                },
            ) => {
                assert!(wait.is_some(), "an open breaker tells its wait: {error:?}");
            }
            (
                Self::TimedOut,
                ResilienceError::Refused {
                    refusal: ResilienceRefusal::TimedOut { timeout, .. },
                    ..
                },
            ) => {
                assert_eq!(*timeout, PINNED);
                assert_eq!(wait, None, "a timeout tells no wait: {error:?}");
            }
            (
                Self::Full,
                ResilienceError::Refused {
                    refusal: ResilienceRefusal::Full,
                    ..
                },
            ) => {
                assert_eq!(wait, None, "a full bulkhead tells no wait: {error:?}");
            }
            (
                Self::Limited,
                ResilienceError::Refused {
                    refusal: ResilienceRefusal::Limited { .. },
                    ..
                },
            ) => {
                assert!(
                    wait.is_some(),
                    "a reached rate tells the time to its next token: {error:?}"
                );
            }
            _ => panic!("refused {self:?}: {error:?}"),
        }
        error_message(&error)
    }

    /// Holds that `told`, what a subscription of the policy read, says
    /// `method` of `host` was refused this way: what the app reads of it in
    /// code.
    #[track_caller]
    fn told(self, told: &[ResilienceEvent], host: &str, method: &str) {
        let named = |told_host: &Option<String>, told_method: &Option<&'static str>| {
            told_host.as_deref() == Some(host) && *told_method == Some(method)
        };
        let said = told.iter().any(|event| match (self, event) {
            (
                Self::TimedOut,
                ResilienceEvent::Timeout {
                    host,
                    method,
                    timeout,
                    ..
                },
            ) => named(host, method) && *timeout == PINNED,
            (
                Self::Open | Self::Full | Self::Limited,
                ResilienceEvent::Refusal {
                    host,
                    method,
                    refusal,
                    ..
                },
            ) => {
                named(host, method)
                    && matches!(
                        (self, refusal),
                        (Self::Open, ResilienceRefusal::Open { .. })
                            | (Self::Full, ResilienceRefusal::Full)
                            | (Self::Limited, ResilienceRefusal::Limited { .. })
                    )
            }
            _ => false,
        });
        assert!(said, "{self:?} of {host}::{method} is told: {told:#?}");
    }
}

impl Refused {
    /// Lets the next call through: the breaker closed, the dependency
    /// answering, the holding call dropped, a token back.
    async fn lift(&mut self) {
        match self.refusing {
            Refusing::Open => self
                .upstream
                .close_breaker()
                .expect("the policy has a breaker"),
            Refusing::TimedOut => self.dependency.stalled.store(false, Ordering::Relaxed),
            Refusing::Full => {
                self.holding = None;
                assert_eq!(self.upstream.concurrency_running(), Some(0));
            }
            Refusing::Limited => {
                tokio::time::advance(self.lasting).await;
                assert!(self.upstream.rate_available() >= Some(1));
            }
        }
    }
}

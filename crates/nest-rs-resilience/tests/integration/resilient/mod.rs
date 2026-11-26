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

/// A breaker only a hand opens, and one slot of the bulkhead: what a refused
/// unit leaves held, the next one meets.
fn refusing() -> ResilienceConfig {
    ResilienceConfig {
        retries: None,
        breaker_window: Some(8),
        breaker_failures: 8,
        concurrency: Some(1),
        ..ResilienceConfig::default()
    }
}

/// How a test has its policy refuse the call of a unit no client reads the
/// `503` of: one refusal telling its wait, and one telling none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refusing {
    /// The breaker opened by hand: the call is never sent.
    Open,
    /// The dependency stalled past the timeout the method pins.
    TimedOut,
}

impl Refusing {
    const EACH: [Self; 2] = [Self::Open, Self::TimedOut];

    /// Has the policy and the dependency `container` holds refuse every call
    /// this way until it is lifted.
    fn arrange(
        self,
        container: &nest_rs_core::Container,
    ) -> (Arc<ResiliencePolicy>, Arc<Dependency>) {
        self.arrange_for(container, Duration::from_secs(30))
    }

    /// [`arrange`](Self::arrange), the breaker open for `open` alone.
    fn arrange_for(
        self,
        container: &nest_rs_core::Container,
        open: Duration,
    ) -> (Arc<ResiliencePolicy>, Arc<Dependency>) {
        let upstream = container
            .get::<ResiliencePolicy>()
            .expect("the module built the policy");
        let dependency = container.get::<Dependency>().expect("provided");
        match self {
            Self::Open => upstream
                .open_breaker(open)
                .expect("the policy has a breaker, and takes the open time"),
            Self::TimedOut => dependency.stalled.store(true, Ordering::Relaxed),
        }
        (upstream, dependency)
    }

    /// Lets the next call through: the breaker closed, the dependency
    /// answering.
    fn lift(self, upstream: &ResiliencePolicy, dependency: &Dependency) {
        match self {
            Self::Open => upstream.close_breaker().expect("the policy has a breaker"),
            Self::TimedOut => dependency.stalled.store(false, Ordering::Relaxed),
        }
    }

    /// How often the dependency was called by a unit refused this way: an
    /// open breaker sends nothing.
    fn sent(self) -> u32 {
        match self {
            Self::Open => 0,
            Self::TimedOut => 1,
        }
    }

    /// The chain a line tells `refused` by, which is this refusal: what the
    /// decorated method answers a caller of its own, as the unit was answered.
    #[track_caller]
    fn chain(self, refused: Result<(), ResilienceError<UpstreamDown>>) -> String {
        let error = refused.expect_err("the policy refuses the call");
        let wait =
            nest_rs_core::ToProblem::to_problem(&error).and_then(|problem| problem.retry_after());
        match (self, &error) {
            (Self::Open, ResilienceError::Open { .. }) => {
                assert!(wait.is_some(), "an open breaker tells its wait: {error:?}");
            }
            (Self::TimedOut, ResilienceError::TimedOut { timeout, .. }) => {
                assert_eq!(*timeout, PINNED);
                assert_eq!(wait, None, "a timeout tells no wait: {error:?}");
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
                Self::Open,
                ResilienceEvent::Refusal {
                    host,
                    method,
                    refusal: ResilienceRefusal::Open { .. },
                    ..
                },
            ) => named(host, method),
            (
                Self::TimedOut,
                ResilienceEvent::Timeout {
                    host,
                    method,
                    timeout,
                    ..
                },
            ) => named(host, method) && *timeout == PINNED,
            _ => false,
        });
        assert!(said, "{self:?} of {host}::{method} is told: {told:#?}");
    }
}

//! Covers `src/resilient.rs`, what `#[resilient]` expands to: a method's body
//! sent once or again as its attribute declares, written out or by a macro,
//! its answer borrowed through the lifetime its receiver names, its arguments
//! read again at each attempt, `?` and `return` answering one attempt, each
//! answer coerced as the undecorated method's is, the `timeout` and `retries`
//! a method pins, an attempt given up at its unit's deadline, a probe dropped
//! mid-call handing its turn on before its slot of the bulkhead, the lines
//! naming it, the docs' recipes as they run (an answer
//! that is no failure, a fallback, a breaker opened by hand), each receiver and
//! host it takes, a refusal answered `503` by every edge serving a decorated
//! handler, a method of every impl half that takes one, called through the
//! framework's own path, attempted again within that one call, a refusal where
//! no client reads a `503` — a tick, a listener, an indicator, a hook — filed
//! as that unit's failure with what the framework does next, what a
//! decorated method's future holds (`footprint`): its size against its
//! undecorated twin's, and a large one run on a worker of tokio's default stack
//! — and what its other keys do: the predicates saying how an answer counts
//! (`judged`), the fallback answering in place of a run's error (`fallback`),
//! the attempt's token in the body (`cancel`), and all of them on each
//! receiver and host (`keyed`), beside an instrument in either order
//! (`instrumented`).

use std::cell::Cell;
use std::future::Future;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use nest_rs_core::{Module, error_message, injectable, instance, module};
use nest_rs_resilience::{
    BreakerState, ResilienceConfig, ResilienceError, ResilienceEvent, ResilienceModule,
    ResiliencePolicy, ResilienceRefusal, resilient,
};
use nest_rs_testing::{
    HeadlessApp, HttpsPeer, LogCapture, PeerAnswer, TestAuthority, TestCertificate,
};

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

/// What the carrier answers when it asks to be called again later: a `Problem`
/// in its chain asking the wait.
#[derive(Debug, thiserror::Error)]
#[error("the carrier asked to be called again later")]
struct CarrierBusy(#[source] nest_rs_core::Problem);

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

#[injectable]
struct ParcelsService {
    #[inject]
    courier: Arc<ResiliencePolicy<Courier>>,
    attempts: AtomicU32,
    seen: Mutex<Vec<String>>,
    hub: Hub,
}

impl ParcelsService {
    /// Whether the attempt about to run is among the first `failing`.
    fn fails(&self, failing: u32) -> bool {
        self.attempts.fetch_add(1, Ordering::Relaxed) < failing
    }

    fn attempted(&self) -> u32 {
        self.attempts.swap(0, Ordering::Relaxed)
    }

    #[resilient(courier)]
    async fn ship(&self) -> Answer<()> {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        Err(CarrierDown)
    }

    #[resilient(courier, idempotent)]
    async fn track(&self) -> Answer<()> {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        Err(CarrierDown)
    }

    #[resilient(courier, idempotent, retries = 4)]
    async fn insist(&self) -> Answer<()> {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        Err(CarrierDown)
    }

    /// Fails asking a minute's wait, past the backoff's ceiling.
    #[resilient(courier, idempotent)]
    async fn reroute(&self) -> Result<(), ResilienceError<CarrierBusy>> {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        Err(CarrierBusy(
            nest_rs_core::Problem::new(429, nest_rs_core::code::RATE_LIMITED).with_retry_after(60),
        ))
    }

    /// Owned and borrowed arguments, read by every attempt.
    #[resilient(courier, idempotent)]
    async fn label(&self, parcel: String, zone: &str) -> Answer<String> {
        self.seen
            .lock()
            .expect("unpoisoned")
            .push(format!("{parcel}@{zone}"));
        if self.fails(2) {
            return Err(CarrierDown);
        }
        Ok(format!("{parcel} to {zone}"))
    }

    /// An owned argument consumed and a `mut` binding changed, as in an
    /// undecorated body: a method run once moves them into its one attempt.
    #[resilient(courier)]
    async fn relabel(&self, mut parcel: String, suffix: String) -> Answer<String> {
        parcel.push_str(&suffix);
        drop(suffix);
        Ok(parcel)
    }

    #[resilient(courier, idempotent)]
    async fn locate(&self, parcel: &str) -> Answer<&'static str> {
        if parcel.is_empty() {
            return Ok("nowhere");
        }
        let depot = depot(parcel)?;
        Ok(depot)
    }

    #[resilient(courier)]
    async fn slow(&self) -> Answer<&'static str> {
        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok("answered")
    }

    #[resilient(courier, timeout = "100ms")]
    async fn hurried(&self) -> Answer<&'static str> {
        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok("answered")
    }

    /// Slow to first wait — a request built, a handshake begun — then never
    /// answering, sent again once after a failure.
    #[resilient(courier, idempotent, retries = 1)]
    async fn stalled(&self) -> Answer<()> {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        tokio::time::advance(Duration::from_millis(3)).await;
        std::future::pending().await
    }

    /// Never answering.
    #[resilient(courier)]
    async fn held_out(&self) -> Answer<()> {
        std::future::pending().await
    }

    #[resilient(courier, idempotent)]
    async fn flaky(&self) -> Answer<()> {
        if self.fails(1) {
            return Err(CarrierDown);
        }
        Ok(())
    }

    #[resilient(courier, idempotent)]
    async fn quote(&self) -> Answer<u32> {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        Ok(42)
    }

    /// A tally the task awaiting it holds alone: `Send`, and not `Sync`.
    #[resilient(courier)]
    async fn tally(&self) -> Answer<Cell<u32>> {
        Ok(Cell::new(self.attempts.fetch_add(1, Ordering::Relaxed) + 1))
    }

    /// The parcels on their way, answered as a stream is — boxed, `Send` and
    /// not `Sync` — after a first attempt failing.
    #[resilient(courier, idempotent)]
    async fn feed(&self) -> Answer<Feed> {
        if self.fails(1) {
            return Err(CarrierDown);
        }
        let feed: Feed = Box::new(["P1", "P2"].into_iter());
        Ok(feed)
    }

    #[resilient(courier)]
    async fn origin<'a>(&'a self) -> Answer<&'a str> {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        Ok(self.hub.0.as_str())
    }

    #[resilient(courier, idempotent)]
    async fn sorted<'a>(&'a self) -> Answer<&'a str> {
        if self.fails(1) {
            return Err(CarrierDown);
        }
        Ok(self.hub.0.as_str())
    }

    /// The hub a parcel leaves from, its `String` answered as the `&str` the
    /// method says.
    #[resilient(courier)]
    async fn departure<'a>(&'a self) -> Answer<&'a str> {
        Ok(&self.hub.0)
    }

    /// The hub a parcel comes back to, answered as `departure` is, after a
    /// first attempt failing.
    #[resilient(courier, idempotent)]
    async fn arrival<'a>(&'a self) -> Answer<&'a str> {
        if self.fails(1) {
            return Err(CarrierDown);
        }
        Ok(&self.hub.0)
    }

    /// The parcels leaving with `parcel`, each box the method answers made at
    /// an early `return` or at the tail, a lookup's error converted by `?`.
    #[resilient(courier)]
    async fn manifest(&self, parcel: &str) -> Answer<Feed> {
        if parcel.is_empty() {
            return Ok(Box::new(std::iter::empty()));
        }
        let depot = depot(parcel)?;
        Ok(Box::new([depot, "Paris"].into_iter()))
    }

    /// The parcels coming back with `parcel`, answered as `manifest` is, after
    /// a first attempt failing.
    #[resilient(courier, idempotent)]
    async fn returns(&self, parcel: &str) -> Answer<Feed> {
        if self.fails(1) {
            return Err(CarrierDown);
        }
        if parcel.is_empty() {
            return Ok(Box::new(std::iter::empty()));
        }
        let depot = depot(parcel)?;
        Ok(Box::new([depot].into_iter()))
    }
}

/// What an `#[sse]` route's or a `#[subscription]`'s answer holds: a stream
/// sent to the task that polls it, and shared with no other.
type Feed = Box<dyn Iterator<Item = &'static str> + Send>;

async fn boot<M: Module + 'static>() -> HeadlessApp {
    HeadlessApp::builder()
        .module::<M>()
        .build_headless()
        .await
        .expect("the module boots")
}

fn parcels(app: &HeadlessApp) -> Arc<ParcelsService> {
    app.container()
        .get::<ParcelsService>()
        .expect("the module provides it")
}

/// One retry, no breaker: a row of a test sets what it is about.
fn retrying_once() -> ResilienceConfig {
    ResilienceConfig {
        retries: Some(1),
        breaker_window: None,
        ..ResilienceConfig::default()
    }
}

#[module(
    imports = [ResilienceModule::for_root((Courier, ResilienceConfig {
        retries: Some(2),
        ..retrying_once()
    }))],
    providers = [ParcelsService, FragmentService],
)]
struct RetryingModule;

#[tokio::test(start_paused = true)]
async fn a_method_is_sent_once_unless_it_declares_itself_idempotent() {
    let app = boot::<RetryingModule>().await;
    let parcels = parcels(&app);

    assert!(failed(&parcels.ship().await));
    assert_eq!(
        parcels.attempted(),
        1,
        "a call run once is never sent again"
    );

    assert!(failed(&parcels.track().await));
    assert_eq!(
        parcels.attempted(),
        3,
        "one call and the policy's two retries"
    );
}

/// A host whose decorated methods macros write, the attribute naming the field
/// and the method borrowing the host each from a context of its own.
#[injectable]
struct FragmentService {
    #[inject]
    courier: Arc<ResiliencePolicy<Courier>>,
    attempts: AtomicU32,
    hub: Hub,
}

impl FragmentService {
    fn attempted(&self) -> u32 {
        self.attempts.swap(0, Ordering::Relaxed)
    }
}

// A method and its `#[resilient]` come from two contexts, the caller's `tt` fragments and the
// macro's, either way round, so the field and the receiver are written in two.
macro_rules! declare_fragment_methods {
    (once: { $($once:tt)* } idempotent: { $($idempotent:tt)* }) => {
        impl FragmentService {
            #[resilient(courier)]
            $($once)*

            #[resilient(courier, idempotent)]
            $($idempotent)*
        }
    };
    ($(#[$($decorator:tt)*] $name:ident)*) => {
        impl FragmentService {
            $(
                #[$($decorator)*]
                async fn $name(&self) -> Answer<()> {
                    self.attempts.fetch_add(1, Ordering::Relaxed);
                    Err(CarrierDown)
                }
            )*
        }
    };
}

declare_fragment_methods! {
    once: {
        async fn ship(&self) -> Answer<()> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            Err(CarrierDown)
        }
    }
    idempotent: {
        async fn track(&self) -> Answer<()> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            Err(CarrierDown)
        }
    }
}

declare_fragment_methods! {
    #[resilient(courier)] deliver
    #[resilient(courier, idempotent)] trace
}

declare_fragment_methods! {
    once: {
        async fn origin<'a>(&'a self) -> Answer<&'a str> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            Ok(self.hub.0.as_str())
        }
    }
    idempotent: {
        async fn sorted<'a>(&'a self) -> Answer<&'a str> {
            if self.attempts.fetch_add(1, Ordering::Relaxed) == 0 {
                return Err(CarrierDown);
            }
            Ok(self.hub.0.as_str())
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_method_a_macro_fragment_writes_runs_under_its_policy_as_the_one_written_out() {
    let app = boot::<RetryingModule>().await;
    let fragments = app
        .container()
        .get::<FragmentService>()
        .expect("the module provides it");

    assert!(failed(&fragments.ship().await));
    assert_eq!(
        fragments.attempted(),
        1,
        "sent once, as `ParcelsService::ship`"
    );
    assert!(failed(&fragments.track().await));
    assert_eq!(
        fragments.attempted(),
        3,
        "and again, as `ParcelsService::track`"
    );

    assert!(failed(&fragments.deliver().await));
    assert_eq!(
        fragments.attempted(),
        1,
        "the caller's attribute, sent once"
    );
    assert!(failed(&fragments.trace().await));
    assert_eq!(
        fragments.attempted(),
        3,
        "the caller's attribute, and again"
    );
}

#[tokio::test(start_paused = true)]
async fn a_method_borrowing_through_the_lifetime_its_receiver_names_runs_under_its_policy() {
    let app = boot::<RetryingModule>().await;
    let parcels = parcels(&app);
    let fragments = app
        .container()
        .get::<FragmentService>()
        .expect("the module provides it");

    assert!(matches!(parcels.origin().await, Ok("the Lyon hub")));
    assert_eq!(parcels.attempted(), 1, "sent once");
    assert!(matches!(parcels.sorted().await, Ok("the Lyon hub")));
    assert_eq!(
        parcels.attempted(),
        2,
        "its first attempt failed, its retry answered"
    );

    assert!(matches!(fragments.origin().await, Ok("the Lyon hub")));
    assert_eq!(fragments.attempted(), 1, "through a fragment, sent once");
    assert!(matches!(fragments.sorted().await, Ok("the Lyon hub")));
    assert_eq!(
        fragments.attempted(),
        2,
        "through a fragment, answered on its retry"
    );
}

#[tokio::test(start_paused = true)]
async fn each_attempt_reads_the_arguments_the_method_was_called_with() {
    let app = boot::<RetryingModule>().await;
    let parcels = parcels(&app);

    let labelled = parcels.label("P1".to_owned(), "north").await;
    assert!(
        matches!(labelled.as_deref(), Ok("P1 to north")),
        "{labelled:?}"
    );
    assert_eq!(
        *parcels.seen.lock().expect("unpoisoned"),
        ["P1@north", "P1@north", "P1@north"],
        "two failed attempts and the one answering, each reading the same arguments"
    );

    let relabelled = parcels.relabel("P2".to_owned(), "-B".to_owned()).await;
    assert!(
        matches!(relabelled.as_deref(), Ok("P2-B")),
        "{relabelled:?}"
    );
}

#[module(
    imports = [ResilienceModule::for_root((Courier, quiet()))],
    providers = [ParcelsService],
)]
struct QuietModule;

#[tokio::test(start_paused = true)]
async fn a_question_mark_and_an_early_return_answer_the_attempt_as_written() {
    let app = boot::<QuietModule>().await;
    let parcels = parcels(&app);

    assert!(matches!(parcels.locate("").await, Ok("nowhere")));
    assert!(matches!(parcels.locate("P1").await, Ok("Lyon")));
    assert!(
        failed(&parcels.locate("P9").await),
        "`?` converted the lookup's error into the call's"
    );
}

/// The body is checked against the answer the method's own return type names,
/// so `Ok(&self.hub.0)` is a `&str` and `Ok(Box::new(…))` a `Box<dyn …>`, at
/// the tail and at an early `return`, run once and again, and `?` converts as
/// before.
#[tokio::test(start_paused = true)]
async fn an_attempt_makes_every_coercion_the_undecorated_method_makes() {
    let app = boot::<RetryingOnceModule>().await;
    let parcels = parcels(&app);
    let listed = |fed: Answer<Feed>| fed.map(Iterator::collect::<Vec<_>>);

    assert!(matches!(parcels.departure().await, Ok("the Lyon hub")));
    assert!(matches!(parcels.arrival().await, Ok("the Lyon hub")));
    assert_eq!(parcels.attempted(), 2, "a failed attempt and its retry");

    assert_eq!(listed(parcels.manifest("").await).ok(), Some(vec![]));
    assert_eq!(
        listed(parcels.manifest("P1").await).ok(),
        Some(vec!["Lyon", "Paris"])
    );
    assert!(
        matches!(
            parcels.manifest("P9").await,
            Err(ResilienceError::Failed(CarrierDown))
        ),
        "`?` converted the lookup's error into the call's"
    );

    assert_eq!(listed(parcels.returns("").await).ok(), Some(vec![]));
    assert_eq!(parcels.attempted(), 2, "a failed attempt and its retry");
    assert_eq!(listed(parcels.returns("P1").await).ok(), Some(vec!["Lyon"]));
}

#[tokio::test(start_paused = true)]
async fn a_pinned_timeout_ends_an_attempt_the_policys_would_not() {
    let app = boot::<QuietModule>().await;
    let parcels = parcels(&app);

    let started = tokio::time::Instant::now();
    assert!(matches!(parcels.slow().await, Ok("answered")));
    assert_eq!(
        started.elapsed(),
        Duration::from_secs(1),
        "within the policy's 5 s, its default"
    );

    let started = tokio::time::Instant::now();
    let hurried = parcels.hurried().await;
    assert!(
        matches!(
            hurried,
            Err(ResilienceError::TimedOut { timeout, .. }) if timeout == Duration::from_millis(100)
        ),
        "{hurried:?}"
    );
    assert_eq!(started.elapsed(), Duration::from_millis(100));
}

#[module(
    imports = [ResilienceModule::for_root((Courier, retrying_once()))],
    providers = [ParcelsService],
)]
struct RetryingOnceModule;

#[tokio::test(start_paused = true)]
async fn pinned_retries_send_the_call_as_often_as_they_say() {
    let app = boot::<RetryingOnceModule>().await;
    let parcels = parcels(&app);

    assert!(failed(&parcels.track().await));
    assert_eq!(parcels.attempted(), 2, "the policy's one retry");

    assert!(failed(&parcels.insist().await));
    assert_eq!(parcels.attempted(), 5, "the method's four retries");
}

#[tokio::test(start_paused = true)]
async fn a_retry_of_a_decorated_method_names_its_host_and_its_method() {
    let app = boot::<RetryingOnceModule>().await;
    let parcels = parcels(&app);
    let logs = LogCapture::install();

    assert!(matches!(parcels.flaky().await, Ok(())));
    let retried = logs.expect_one(
        nest_rs_resilience::TARGET,
        "attempt failed; retrying the call",
    );
    assert_eq!(retried.field("instance").as_deref(), Some("courier"));
    assert_eq!(retried.field("host").as_deref(), Some("ParcelsService"));
    assert_eq!(retried.field("method").as_deref(), Some("flaky"));

    let calls = AtomicU32::new(0);
    let answered = parcels
        .courier
        .run_idempotent(|_| async {
            match calls.fetch_add(1, Ordering::Relaxed) {
                0 => Err(CarrierDown),
                _ => Ok(()),
            }
        })
        .await;
    assert!(matches!(answered, Ok(())), "{answered:?}");
    let lines = logs.find(
        nest_rs_resilience::TARGET,
        "attempt failed; retrying the call",
    );
    assert_eq!(lines.len(), 2, "{lines:#?}");
    assert_eq!(
        (lines[1].field("host"), lines[1].field("method")),
        (None, None),
        "a call run through the policy is no method"
    );
}

/// A retry a decorated method's run gives up names its host and its method too:
/// one its dependency asked to wait past the backoff's ceiling, and one whose
/// backoff would end past the unit's deadline.
#[tokio::test(start_paused = true)]
async fn a_retry_a_decorated_method_gives_up_names_its_host_and_its_method() {
    let app = boot::<RetryingOnceModule>().await;
    let parcels = parcels(&app);
    let logs = LogCapture::install();

    let asked = parcels.reroute().await;
    assert!(
        matches!(asked, Err(ResilienceError::Failed(CarrierBusy(_)))),
        "{asked:?}"
    );
    assert_eq!(
        parcels.attempted(),
        1,
        "a wait past the ceiling is not waited"
    );
    let past = logs.expect_one(
        nest_rs_resilience::TARGET,
        "the dependency asked a wait past the backoff's ceiling; not attempted again",
    );

    let deadline = tokio::time::Instant::now() + Duration::from_millis(1);
    let spent = nest_rs_core::__private::with_deadline(Some(deadline), parcels.track()).await;
    assert!(failed(&spent), "{spent:?}");
    assert_eq!(
        parcels.attempted(),
        1,
        "no retry whose wait ends past the deadline"
    );
    let out_of_time = logs.expect_one(
        nest_rs_resilience::TARGET,
        "no time left before the unit's deadline; not attempted again",
    );

    for (line, method) in [(&past, "reroute"), (&out_of_time, "track")] {
        assert_eq!(
            line.field("instance").as_deref(),
            Some("courier"),
            "{line:?}"
        );
        assert_eq!(
            line.field("host").as_deref(),
            Some("ParcelsService"),
            "{line:?}"
        );
        assert_eq!(line.field("method").as_deref(), Some(method), "{line:?}");
    }
}

#[module(
    imports = [ResilienceModule::for_root((Courier, ResilienceConfig {
        concurrency: Some(1),
        ..quiet()
    }))],
    providers = [ParcelsService],
)]
struct NarrowModule;

#[tokio::test(start_paused = true)]
async fn a_decorated_method_refused_by_a_full_bulkhead_is_named_too() {
    let app = boot::<NarrowModule>().await;
    let parcels = parcels(&app);
    let logs = LogCapture::install();

    let (slow, refused) = tokio::join!(parcels.slow(), async {
        tokio::task::yield_now().await;
        parcels.quote().await
    });
    assert!(matches!(slow, Ok("answered")), "{slow:?}");
    assert!(
        matches!(refused, Err(ResilienceError::Full { .. })),
        "{refused:?}"
    );
    assert_eq!(parcels.attempted(), 0, "a refused call is never sent");
    let full = logs.expect_one(
        nest_rs_resilience::TARGET,
        "bulkhead full; the call is refused",
    );
    assert_eq!(full.field("host").as_deref(), Some("ParcelsService"));
    assert_eq!(full.field("method").as_deref(), Some("quote"));
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

#[module(
    imports = [ResilienceModule::for_root((Courier, opening_on_the_first_failure()))],
    providers = [ParcelsService],
)]
struct OpeningModule;

/// One breaker counts every method on its field, and its transition is the
/// policy's own: it names the instance, never the method whose failure moved it.
#[tokio::test(start_paused = true)]
async fn every_method_on_one_field_counts_in_one_breaker() {
    let app = boot::<OpeningModule>().await;
    let parcels = parcels(&app);
    let logs = LogCapture::install();

    assert!(failed(&parcels.ship().await));
    parcels.attempted();
    let opened = logs.expect_one(nest_rs_resilience::TARGET, "circuit breaker changed state");
    assert_eq!(opened.field("instance").as_deref(), Some("courier"));
    assert_eq!(opened.field("state").as_deref(), Some("open"));
    assert_eq!(opened.field("host"), None, "{opened:?}");
    assert_eq!(opened.field("method"), None, "{opened:?}");

    let refused = parcels.quote().await;
    let Err(ResilienceError::Open { retry_after, .. }) = refused else {
        panic!("the failure of another method opened the breaker: {refused:?}");
    };
    assert_eq!(retry_after, Duration::from_secs(30));
    assert_eq!(
        parcels.attempted(),
        0,
        "a call refused by an open breaker is never sent"
    );
}

/// A decorated idempotent method's attempt ends at its unit's deadline however
/// long its call takes to first wait: the policy gives it up there, before the
/// edge cutting the unit at that deadline, its breaker counts it, and it is not
/// sent again with no time left.
#[tokio::test(start_paused = true)]
async fn an_idempotent_methods_attempt_slow_to_first_wait_ends_at_its_units_deadline() {
    let app = boot::<OpeningModule>().await;
    let parcels = parcels(&app);
    let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
    let cut = tokio::time::timeout_at(
        deadline,
        nest_rs_core::__private::with_deadline(Some(deadline), parcels.stalled()),
    )
    .await;
    assert!(
        matches!(cut, Ok(Err(ResilienceError::TimedOut { timeout, .. }))
            if timeout == Duration::from_millis(300)),
        "the policy gave the attempt up at the deadline, before the edge cut it: {cut:?}"
    );
    assert_eq!(parcels.attempted(), 1, "not sent again, with no time left");
    assert!(
        matches!(
            held::<Courier>(&app).breaker_state(),
            Some(BreakerState::Open { .. })
        ),
        "the attempt counted in the breaker"
    );
}

/// A future polled at each wake, with a waker of its own, as a task another
/// worker runs may be polled before the code that woke it goes on.
struct PollsOnWake<T> {
    future: Mutex<Option<std::pin::Pin<Box<dyn Future<Output = T> + Send>>>>,
    answered: Mutex<Option<T>>,
}

impl<T: Send + 'static> PollsOnWake<T> {
    /// `future`, first polled at a wake.
    fn new(future: impl Future<Output = T> + Send + 'static) -> Arc<Self> {
        Arc::new(Self {
            future: Mutex::new(Some(Box::pin(future))),
            answered: Mutex::new(None),
        })
    }

    /// The future polled now, as a wake polls it.
    fn poll_now(self: &Arc<Self>) {
        std::task::Wake::wake_by_ref(self);
    }

    /// What the future answered, once it has.
    fn answered(&self) -> Option<T> {
        self.answered.lock().expect("one wake at a time").take()
    }
}

impl<T: Send + 'static> std::task::Wake for PollsOnWake<T> {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        let mut future = self.future.lock().expect("one wake at a time");
        let Some(polled) = future.as_mut() else {
            return;
        };
        let waker = std::task::Waker::from(Arc::clone(self));
        if let std::task::Poll::Ready(answer) = polled
            .as_mut()
            .poll(&mut std::task::Context::from_waker(&waker))
        {
            *future = None;
            *self.answered.lock().expect("one wake at a time") = Some(answer);
        }
    }
}

/// `future` polled once, with no task to wake: what it waits on from then, a
/// test polls it again for.
fn polled_once<F: Future + ?Sized>(future: std::pin::Pin<&mut F>) -> std::task::Poll<F::Output> {
    future.poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
}

#[module(
    imports = [ResilienceModule::for_root((Courier, ResilienceConfig {
        concurrency: Some(1),
        concurrency_wait: Some(Duration::from_secs(10)),
        breaker_window: Some(1),
        breaker_failures: 1,
        breaker_open: Duration::from_secs(1),
        ..quiet()
    }))],
    providers = [ParcelsService],
)]
struct ProbingModule;

/// A decorated method run once, dropped while its probe's call is out, hands
/// the probe's turn on before its slot of the bulkhead: the call waiting for
/// that slot, polled as soon as the freed slot wakes it — as a task another
/// worker runs may be, before the drop goes on — takes the turn the dropped
/// probe handed on.
#[tokio::test(start_paused = true)]
async fn a_method_run_once_dropped_while_its_probe_is_out_hands_the_turn_on_before_its_slot() {
    let app = boot::<ProbingModule>().await;
    let parcels = parcels(&app);
    let courier = held::<Courier>(&app);
    let mut holding = Box::pin(parcels.slow());
    assert!(
        polled_once(holding.as_mut()).is_pending(),
        "it takes the slot"
    );
    let mut probing = Box::pin(parcels.held_out());
    assert!(
        polled_once(probing.as_mut()).is_pending(),
        "it waits for the slot"
    );
    let waiting = PollsOnWake::new({
        let parcels = Arc::clone(&parcels);
        async move { parcels.quote().await }
    });
    waiting.poll_now();
    assert!(
        waiting.answered().is_none(),
        "it waits for the slot, behind the method that probes once it has it"
    );
    courier
        .open_breaker(Duration::from_secs(1))
        .expect("an open time the breaker takes");
    assert!(matches!(holding.await, Ok("answered")));
    assert!(polled_once(probing.as_mut()).is_pending());
    assert_eq!(
        courier.breaker_state(),
        Some(BreakerState::HalfOpen),
        "handed the slot, it probes, and its call never answers"
    );

    drop(probing);
    let answer = waiting.answered();
    assert!(
        matches!(answer, Some(Ok(42))),
        "woken by the slot the dropped method freed, it took the turn its probe handed on: \
         {answer:?}"
    );
    assert_eq!(
        courier.breaker_state(),
        Some(BreakerState::Closed),
        "and closed the breaker as the next probe"
    );
}

/// What a call to a dependency that refuses every connection fails with.
#[derive(Debug, thiserror::Error)]
#[error("the upstream failed")]
struct UpstreamFailed(#[source] reqwest::Error);

impl From<reqwest::Error> for UpstreamFailed {
    fn from(error: reqwest::Error) -> Self {
        Self(error.without_url())
    }
}

/// Where the docs' feature reaches its upstream: the address a deployment sets,
/// never one a caller passes.
#[nest_rs_config::config(namespace = "posts")]
#[derive(Clone, Default)]
struct PostsConfig {
    upstream_url: String,
}

/// The upstream's address in a test whose upstream is a peer, or none.
fn upstream_at(url: &str) -> PostsConfig {
    PostsConfig {
        upstream_url: url.to_owned(),
    }
}

#[injectable]
struct LinksService {
    #[inject]
    courier: Arc<ResiliencePolicy<Courier>>,
    #[inject]
    config: Arc<PostsConfig>,
}

impl LinksService {
    /// The docs' first call: the one host the config names, a post's id in
    /// its path.
    #[resilient(courier, idempotent)]
    async fn preview(&self, post: u64) -> Result<String, ResilienceError<UpstreamFailed>> {
        let url = format!("{}/previews/{post}", self.config.upstream_url);
        Ok(reqwest::get(url).await?.text().await?)
    }
}

/// The feature's module, importing its instance bare.
#[module(imports = [ResilienceModule<Courier>], providers = [LinksService])]
struct LinksModule;

/// The test's root pinning the feature's policy, as the docs' `Test it`
/// recipe writes it.
#[module(imports = [
    ResilienceModule::for_root((Courier, opening_on_the_first_failure())),
    LinksModule,
])]
struct OpeningOnOneFailure;

/// `M` booted with the docs' feature reaching its upstream at `url`.
async fn boot_reaching<M: Module + 'static>(url: &str) -> HeadlessApp {
    HeadlessApp::builder()
        .module::<M>()
        .provide(upstream_at(url))
        .build_headless()
        .await
        .expect("the module boots")
}

#[tokio::test]
async fn a_policy_pinned_to_open_on_one_failure_refuses_the_next_call_unsent() {
    let app = boot_reaching::<OpeningOnOneFailure>("https://127.0.0.1:1").await;
    let links = app.container().get::<LinksService>().expect("provided");

    let failed = links.preview(7).await;
    assert!(
        matches!(failed, Err(ResilienceError::Failed(_))),
        "{failed:?}"
    );
    let refused = links.preview(7).await;
    assert!(
        matches!(refused, Err(ResilienceError::Open { retry_after, .. })
            if retry_after <= Duration::from_secs(30)),
        "{refused:?}"
    );
}

/// The circuit breaker page's `Test it` recipe for a switch, as it runs: opened
/// by hand, the breaker refuses a call unsent; closed by hand, it sends the next.
#[tokio::test(start_paused = true)]
async fn a_method_is_refused_unsent_while_its_breaker_is_opened_by_hand_and_sent_once_closed() {
    let app = boot::<OpeningModule>().await;
    let parcels = parcels(&app);
    let courier = app
        .container()
        .get::<ResiliencePolicy<Courier>>()
        .expect("the module built the policy");

    courier
        .open_breaker(Duration::from_secs(60))
        .expect("an open time the breaker takes");
    let refused = parcels.quote().await;
    assert!(
        matches!(refused, Err(ResilienceError::Open { retry_after, .. })
            if retry_after == Duration::from_secs(60)),
        "{refused:?}"
    );
    assert_eq!(parcels.attempted(), 0, "refused unsent");

    courier.close_breaker().expect("the policy has a breaker");
    assert_eq!(courier.breaker_state(), Some(BreakerState::Closed));
    assert!(matches!(parcels.quote().await, Ok(42)));
    assert_eq!(parcels.attempted(), 1, "sent again");
}

/// The authority the previewer's peer chains to, and the client trusts.
static AUTHORITY: LazyLock<TestAuthority> = LazyLock::new(TestAuthority::new);

static CERTIFICATE: LazyLock<TestCertificate> = LazyLock::new(|| AUTHORITY.server(&["127.0.0.1"]));

/// The dependency the docs' failure recipes call, its namespace theirs alone.
#[instance]
struct Previewer;

/// The target the docs' feature files its lines on.
const POSTS: &str = "features::posts";

/// What a call to the previewer fails with: reqwest's error without the URL it
/// quotes, as the docs' error keeps it.
#[derive(Debug, thiserror::Error)]
#[error("the previewer failed")]
struct PreviewFailed(#[source] reqwest::Error);

impl From<reqwest::Error> for PreviewFailed {
    fn from(error: reqwest::Error) -> Self {
        Self(error.without_url())
    }
}

impl nest_rs_core::ToProblem for PreviewFailed {
    fn to_problem(&self) -> Option<nest_rs_core::Problem> {
        None
    }
}

impl PreviewFailed {
    /// Whether the previewer answered that it has no preview of the post.
    fn missing(&self) -> bool {
        self.0.status() == Some(reqwest::StatusCode::NOT_FOUND)
    }
}

/// A host the test builds itself: the previewer's policy, the upstream's
/// address the app's config holds, and a client trusting the peer's authority.
struct Previews {
    previewer: Arc<ResiliencePolicy<Previewer>>,
    config: Arc<PostsConfig>,
    http: reqwest::Client,
}

impl Previews {
    fn of(app: &HeadlessApp) -> Self {
        let authority =
            reqwest::Certificate::from_pem(AUTHORITY.pem().as_bytes()).expect("the authority");
        Self {
            previewer: held::<Previewer>(app),
            config: app
                .container()
                .get::<PostsConfig>()
                .expect("the test seeded the upstream's address"),
            http: reqwest::Client::builder()
                .tls_certs_only([authority])
                .build()
                .expect("a client"),
        }
    }

    /// The docs' answer that is no failure: a post the previewer has no
    /// preview of is its answer, `None`, and any other failing status an `Err`.
    #[resilient(previewer, idempotent)]
    async fn preview(&self, post: u64) -> Result<Option<String>, ResilienceError<PreviewFailed>> {
        let url = format!("{}/previews/{post}", self.config.upstream_url);
        let page = self.http.get(url).send().await?;
        if page.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let page = page.error_for_status()?;
        Ok(Some(page.text().await?))
    }

    /// The docs' fallback: no card while the policy refuses the call, the
    /// refusal filed at `warn` with its chain, and the previewer's own failure
    /// kept as the caller's error.
    async fn link_card(&self, post: u64) -> Result<Option<String>, PreviewFailed> {
        self.preview(post).await.or_else(|error| match error {
            ResilienceError::Failed(failed) => Err(failed),
            refused => {
                let error = error_message(&refused);
                tracing::warn!(
                    target: POSTS,
                    %error,
                    "link preview refused; answering no card",
                );
                Ok(None)
            }
        })
    }

    /// The docs' answer among the errors: a post the previewer has no preview
    /// of is `error_for_status`'s `Err`, which the predicates take for its
    /// answer — sent once, and no failure.
    #[resilient(
        previewer,
        idempotent,
        retry_if = |error| !error.missing(),
        record_if = |error| !error.missing()
    )]
    async fn page(&self, post: u64) -> Result<String, ResilienceError<PreviewFailed>> {
        let url = format!("{}/previews/{post}", self.config.upstream_url);
        let page = self.http.get(url).send().await?.error_for_status()?;
        Ok(page.text().await?)
    }

    /// The docs' fallback by key: `no_card` answers for the policy's refusals
    /// alone, and the previewer's own failure stays the method's error.
    #[resilient(
        previewer,
        fallback = no_card,
        fallback_if = |error| !matches!(error, ResilienceError::Failed(_))
    )]
    async fn card(&self, post: u64) -> Result<Option<String>, ResilienceError<PreviewFailed>> {
        let url = format!("{}/previews/{post}", self.config.upstream_url);
        let page = self.http.get(url).send().await?.error_for_status()?;
        Ok(Some(page.text().await?))
    }

    async fn no_card(
        &self,
        post: u64,
        refused: ResilienceError<PreviewFailed>,
    ) -> Result<Option<String>, ResilienceError<PreviewFailed>> {
        let error = error_message(&refused);
        tracing::warn!(
            target: POSTS,
            post,
            %error,
            "link preview refused; answering no card",
        );
        Ok(None)
    }
}

/// One retry, waiting a millisecond or two, and a breaker opening on two
/// failures among the latest two attempts.
#[module(imports = [ResilienceModule::for_root((Previewer, ResilienceConfig {
    retries: Some(1),
    backoff_first: Duration::from_millis(1),
    backoff_ceiling: Duration::from_millis(2),
    breaker_window: Some(2),
    breaker_failures: 2,
    ..ResilienceConfig::default()
}))])]
struct AnsweringModule;

/// The `What counts as a failure` recipe, as it runs: a `404` is the
/// previewer's answer, `Ok(None)`, sent once and counted as no failure, so two
/// in a row leave closed a breaker two failures open; a `503` is an `Err`,
/// attempted again and counted, its two failures opening the breaker.
#[tokio::test]
async fn an_answer_the_dependency_gave_is_the_calls_ok_never_attempted_again_nor_a_failure() {
    let peer = HttpsPeer::answering(
        &CERTIFICATE,
        [
            PeerAnswer::status(404),
            PeerAnswer::status(404),
            PeerAnswer::status(503),
        ],
    )
    .await;
    let app = boot_reaching::<AnsweringModule>(&peer.url("")).await;
    let previews = Previews::of(&app);

    for sent in 1..=2 {
        let missing = previews.preview(7).await;
        assert!(matches!(missing, Ok(None)), "{missing:?}");
        assert_eq!(
            peer.requests().len(),
            sent,
            "an answer is never attempted again"
        );
    }
    assert_eq!(
        peer.requests()[0].line,
        "GET /previews/7 HTTP/1.1",
        "the config's host, the caller's post in the path"
    );
    assert_eq!(
        previews.previewer.breaker_state(),
        Some(BreakerState::Closed),
        "nor counted as a failure: two would have opened the breaker"
    );

    let down = previews.preview(8).await;
    assert!(matches!(down, Err(ResilienceError::Failed(_))), "{down:?}");
    assert_eq!(
        peer.requests().len(),
        4,
        "a failing status is attempted again"
    );
    assert!(
        matches!(
            previews.previewer.breaker_state(),
            Some(BreakerState::Open { .. })
        ),
        "its two failures opened the breaker"
    );
}

#[module(imports = [ResilienceModule::for_root((Previewer, opening_on_the_first_failure()))])]
struct FallbackModule;

/// The fallback recipe, as it runs: the previewer's own failure is the caller's
/// error, and the call the breaker it opened refuses answers no card, unsent,
/// its refusal filed at `warn` with the chain the policy answered.
#[tokio::test]
async fn the_methods_result_is_its_fallback_a_refusal_answered_and_a_failure_kept() {
    let peer = HttpsPeer::answering(&CERTIFICATE, [PeerAnswer::status(503)]).await;
    let app = boot_reaching::<FallbackModule>(&peer.url("")).await;
    let previews = Previews::of(&app);
    let logs = LogCapture::install();

    let failed = previews.link_card(7).await;
    assert!(matches!(failed, Err(PreviewFailed(_))), "{failed:?}");
    logs.expect_none(POSTS, "link preview refused; answering no card");
    let refused = previews.link_card(7).await;
    assert!(
        matches!(refused, Ok(None)),
        "no card while the breaker refuses the call: {refused:?}"
    );
    assert_eq!(peer.requests().len(), 1, "the refused call is never sent");
    let said = logs.expect_one(POSTS, "link preview refused; answering no card");
    assert_eq!(said.level, "warn", "{said:?}");
    let open = previews
        .preview(7)
        .await
        .expect_err("the breaker still refuses the call");
    assert_eq!(
        said.field("error"),
        Some(error_message(&open)),
        "the refusal's whole chain"
    );
}

/// The same answer told by key, as it runs: a `404` stays the `Err` reqwest
/// makes of it, and `retry_if` and `record_if` take it for the previewer's
/// answer — sent once, and no failure in a breaker two failures open; a `503`
/// is attempted again and counted.
#[tokio::test]
async fn an_err_the_predicates_take_for_the_dependencys_answer_is_sent_once_and_no_failure() {
    let peer = HttpsPeer::answering(
        &CERTIFICATE,
        [
            PeerAnswer::status(404),
            PeerAnswer::status(404),
            PeerAnswer::status(503),
        ],
    )
    .await;
    let app = boot_reaching::<AnsweringModule>(&peer.url("")).await;
    let previews = Previews::of(&app);

    for sent in 1..=2 {
        let missing = previews.page(7).await;
        assert!(
            matches!(&missing, Err(ResilienceError::Failed(failed)) if failed.missing()),
            "{missing:?}"
        );
        assert_eq!(peer.requests().len(), sent, "its answer is sent once");
    }
    assert_eq!(
        previews.previewer.breaker_state(),
        Some(BreakerState::Closed),
        "nor counted as a failure: two would have opened the breaker"
    );

    let down = previews.page(8).await;
    assert!(matches!(down, Err(ResilienceError::Failed(_))), "{down:?}");
    assert_eq!(
        peer.requests().len(),
        4,
        "a failing status is attempted again"
    );
    assert!(
        matches!(
            previews.previewer.breaker_state(),
            Some(BreakerState::Open { .. })
        ),
        "its two failures opened the breaker"
    );
}

/// The fallback named by key, as it runs: the previewer's own failure is the
/// method's error, its fallback not asked, and the call the breaker it opened
/// refuses answers no card, unsent, the refusal filed at `warn` with the chain
/// the policy answered and the post the method was called with.
#[tokio::test]
async fn a_fallback_named_by_key_answers_no_card_for_a_refusal_and_keeps_the_previewers_failure() {
    let peer = HttpsPeer::answering(&CERTIFICATE, [PeerAnswer::status(503)]).await;
    let app = boot_reaching::<FallbackModule>(&peer.url("")).await;
    let previews = Previews::of(&app);
    let logs = LogCapture::install();

    let failed = previews.card(7).await;
    assert!(
        matches!(failed, Err(ResilienceError::Failed(PreviewFailed(_)))),
        "{failed:?}"
    );
    logs.expect_none(POSTS, "link preview refused; answering no card");
    let refused = previews.card(7).await;
    assert!(
        matches!(refused, Ok(None)),
        "no card while the breaker refuses the call: {refused:?}"
    );
    assert_eq!(peer.requests().len(), 1, "the refused call is never sent");
    let said = logs.expect_one(POSTS, "link preview refused; answering no card");
    assert_eq!(said.level, "warn", "{said:?}");
    assert_eq!(said.field("post").as_deref(), Some("7"), "{said:?}");
    let open = previews
        .preview(7)
        .await
        .expect_err("the breaker still refuses the call");
    assert_eq!(
        said.field("error"),
        Some(error_message(&open)),
        "the refusal's whole chain"
    );
}

/// A credential the previewed URL's query carries, which no line may quote.
const CREDENTIAL: &str = "zzqx-preview-token";

/// The URL the route below previews, set by the test serving it: the peer's,
/// its query carrying a credential.
static PREVIEWED: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The previewer behind a route, as an app serves it: its failure answers an
/// opaque `500`, which the edge files at `error` with the failure's chain.
#[nest_rs_http::controller(path = "/cards")]
struct CardsController {
    #[inject]
    previewer: Arc<ResiliencePolicy<Previewer>>,
}

#[nest_rs_http::routes]
impl CardsController {
    #[get("/")]
    #[public]
    #[resilient(previewer)]
    async fn card(&self) -> Result<String, ResilienceError<PreviewFailed>> {
        let authority =
            reqwest::Certificate::from_pem(AUTHORITY.pem().as_bytes()).expect("the authority");
        let http = reqwest::Client::builder()
            .tls_certs_only([authority])
            .build()
            .expect("a client");
        let page = http
            .get(PREVIEWED.get().expect("the test set the URL"))
            .send()
            .await?;
        Ok(page.error_for_status()?.text().await?)
    }
}

#[module(
    imports = [ResilienceModule::for_root((Previewer, quiet()))],
    providers = [CardsController],
)]
struct CardsModule;

/// The docs' error keeps reqwest's error without its URL, so the edge filing a
/// failed preview — the line and the span's status — quotes no credential its
/// query carries.
#[tokio::test]
async fn a_failed_preview_is_filed_without_the_url_it_requested() {
    let peer = HttpsPeer::answering(&CERTIFICATE, [PeerAnswer::status(503)]).await;
    PREVIEWED
        .set(peer.url(&format!("/down?api_key={CREDENTIAL}")))
        .expect("set once");
    let logs = LogCapture::install();
    let app = nest_rs_testing::TestApp::for_module::<CardsModule>()
        .await
        .expect("the route boots");

    let failed = app.http().get("/cards").send().await;
    failed.assert_status(http::StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(peer.requests().len(), 1, "the preview was sent");
    let filed = logs.expect_one(nest_rs_http::target::HTTP, "request failed");
    assert!(
        filed
            .field("error")
            .is_some_and(|error| error.contains("the previewer failed")),
        "the failure's chain is filed: {filed:?}"
    );
    assert!(
        !logs.quotes(CREDENTIAL),
        "no line quotes the URL: {:?}",
        logs.events()
    );
    assert!(
        logs.spans().iter().all(|span| span
            .fields
            .values()
            .all(|value| !value.contains(CREDENTIAL))),
        "no span records the URL: {:?}",
        logs.spans()
    );
}

#[tokio::test(start_paused = true)]
async fn a_decorated_call_runs_on_a_task_of_its_own() {
    let app = boot::<QuietModule>().await;
    let parcels = parcels(&app);

    let spawned = tokio::spawn(async move { parcels.quote().await });
    assert!(matches!(spawned.await.expect("the task ends"), Ok(42)));
}

/// One slot, which at most four calls wait for at once, 10 s each at most,
/// under a rate limit each run reads before it waits.
#[module(
    imports = [ResilienceModule::for_root((Courier, ResilienceConfig {
        concurrency: Some(1),
        concurrency_wait: Some(Duration::from_secs(10)),
        concurrency_waiters: Some(4),
        rate_limit: Some(100),
        ..retrying_once()
    }))],
    providers = [ParcelsService],
)]
struct WaitingModule;

/// A run whose answer is `Send` and not `Sync` — a `Cell`, a boxed stream — is
/// `Send` itself, run once or again, decorated or not, its wait before a retry
/// included, and its wait for a slot among a bounded number of waiters, the
/// rate read before it: each compiles on a task of its own, as an `#[sse]`
/// route's or a `#[subscription]`'s answer must.
#[tokio::test(start_paused = true)]
async fn a_run_answering_a_value_that_is_not_sync_runs_on_a_task_of_its_own() {
    let app = boot::<RetryingOnceModule>().await;
    let parcels = parcels(&app);
    let courier = held::<Courier>(&app);

    let once = tokio::spawn({
        let courier = Arc::clone(&courier);
        async move {
            courier
                .run(|_| async { Ok::<_, CarrierDown>(Cell::new(7)) })
                .await
        }
    });
    let calls = Arc::new(AtomicU32::new(0));
    let again = tokio::spawn({
        let calls = Arc::clone(&calls);
        async move {
            courier
                .run_idempotent(|_| {
                    let calls = Arc::clone(&calls);
                    async move {
                        if calls.fetch_add(1, Ordering::Relaxed) == 0 {
                            return Err(CarrierDown);
                        }
                        Ok::<Feed, _>(Box::new(["P3"].into_iter()))
                    }
                })
                .await
        }
    });
    assert_eq!(
        once.await
            .expect("the task ends")
            .ok()
            .map(Cell::into_inner),
        Some(7)
    );
    let streamed = again.await.expect("the task ends");
    assert_eq!(
        streamed.ok().map(Iterator::collect::<Vec<_>>),
        Some(vec!["P3"])
    );
    assert_eq!(
        calls.load(Ordering::Relaxed),
        2,
        "a failed attempt and its retry"
    );

    let tallied = tokio::spawn({
        let parcels = Arc::clone(&parcels);
        async move { parcels.tally().await }
    });
    assert_eq!(
        tallied
            .await
            .expect("the task ends")
            .ok()
            .map(Cell::into_inner),
        Some(1)
    );
    parcels.attempted();
    let fed = tokio::spawn({
        let parcels = Arc::clone(&parcels);
        async move { parcels.feed().await }
    });
    let fed = fed.await.expect("the task ends");
    assert_eq!(
        fed.ok().map(Iterator::collect::<Vec<_>>),
        Some(vec!["P1", "P2"])
    );
    assert_eq!(parcels.attempted(), 2, "a failed attempt and its retry");

    let app = boot::<WaitingModule>().await;
    let parcels = app
        .container()
        .get::<ParcelsService>()
        .expect("the module provides it");
    let courier = held::<Courier>(&app);
    let holding = tokio::spawn({
        let parcels = Arc::clone(&parcels);
        async move { parcels.slow().await }
    });
    tokio::task::yield_now().await;
    let once = tokio::spawn({
        let courier = Arc::clone(&courier);
        async move {
            courier
                .run(|_| async { Ok::<_, CarrierDown>(Cell::new(7)) })
                .await
        }
    });
    let calls = Arc::new(AtomicU32::new(0));
    let again = tokio::spawn({
        let calls = Arc::clone(&calls);
        async move {
            courier
                .run_idempotent(|_| {
                    let calls = Arc::clone(&calls);
                    async move {
                        if calls.fetch_add(1, Ordering::Relaxed) == 0 {
                            return Err(CarrierDown);
                        }
                        Ok::<Feed, _>(Box::new(["P3"].into_iter()))
                    }
                })
                .await
        }
    });
    let fed = tokio::spawn({
        let parcels = Arc::clone(&parcels);
        async move { parcels.feed().await }
    });
    let tallied = tokio::spawn({
        let parcels = Arc::clone(&parcels);
        async move { parcels.tally().await }
    });
    tokio::task::yield_now().await;
    assert!(
        !once.is_finished() && !again.is_finished() && !fed.is_finished() && !tallied.is_finished(),
        "each waits for the slot the slow call holds"
    );
    assert!(matches!(
        holding.await.expect("the task ends"),
        Ok("answered")
    ));
    assert_eq!(
        once.await
            .expect("the task ends")
            .ok()
            .map(Cell::into_inner),
        Some(7)
    );
    let streamed = again.await.expect("the task ends");
    assert_eq!(
        streamed.ok().map(Iterator::collect::<Vec<_>>),
        Some(vec!["P3"])
    );
    let fed = fed.await.expect("the task ends");
    assert_eq!(
        fed.ok().map(Iterator::collect::<Vec<_>>),
        Some(vec!["P1", "P2"])
    );
    assert!(tallied.await.expect("the task ends").is_ok());
    assert_eq!(
        parcels.attempted(),
        3,
        "the feed's failed attempt and its retry, then the tally"
    );
}

/// The default instance, a receiver lent through its `Arc` and a generic
/// method.
#[injectable]
struct PreviewService {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
}

impl PreviewService {
    #[resilient(upstream, idempotent)]
    async fn shared(self: &Arc<Self>, url: &str) -> Answer<String> {
        Ok(format!("{url}, held {} times", Arc::strong_count(self)))
    }

    #[resilient(upstream)]
    async fn parsed<T: FromStr + Send>(&self, raw: &str) -> Answer<Option<T>> {
        Ok(raw.parse().ok())
    }
}

#[module(imports = [ResilienceModule], providers = [PreviewService])]
struct PreviewsModule;

#[tokio::test]
async fn an_arc_receiver_a_generic_method_and_the_default_instance_run_alike() {
    let app = boot::<PreviewsModule>().await;
    let previews = app.container().get::<PreviewService>().expect("provided");

    let shared = previews.shared("https://example.com").await;
    assert!(
        shared
            .as_deref()
            .is_ok_and(|said| said.starts_with("https://example.com, held ")),
        "{shared:?}"
    );
    assert!(matches!(
        previews.parsed::<u16>("8080").await,
        Ok(Some(8080))
    ));
    assert!(matches!(previews.parsed::<u16>("port").await, Ok(None)));
}

/// A native `async fn` of a trait whose method promises a `Send` future.
trait Tracking {
    fn status(&self, parcel: &str) -> impl Future<Output = Answer<String>> + Send;
}

/// A host the app's code builds itself, holding a policy the container built.
struct HandBuilt {
    courier: Arc<ResiliencePolicy<Courier>>,
}

impl Tracking for HandBuilt {
    #[resilient(courier, idempotent)]
    async fn status(&self, parcel: &str) -> Answer<String> {
        let depot = depot(parcel)?;
        Ok(format!("{parcel} waits in {depot}"))
    }
}

#[tokio::test(start_paused = true)]
async fn a_trait_impl_on_a_struct_built_by_hand_runs_alike() {
    let app = boot::<QuietModule>().await;
    let tracking = HandBuilt {
        courier: held::<Courier>(&app),
    };
    let status = tracking.status("P1").await;
    assert!(
        matches!(status.as_deref(), Ok("P1 waits in Lyon")),
        "{status:?}"
    );
    assert!(failed(&tracking.status("P9").await));
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

/// A decorated handler and `#[sse]` route of a `#[routes]` block, failing, then
/// refused, and each attempted again within its one request.
mod routes {
    use std::sync::Arc;

    use http::StatusCode;
    use nest_rs_core::operation_log::TARGET as OPERATIONS;
    use nest_rs_core::{code, module};
    use nest_rs_http::futures_util::stream;
    use nest_rs_http::{SseEvent, SseStream, controller, routes};
    use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
    use nest_rs_testing::{LogCapture, TestApp};

    use super::{
        Attempts, UpstreamDown, opening_on_the_first_failure, retried_once, retrying_once,
    };

    #[controller(path = "/decorated")]
    struct DecoratedController {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
        #[inject]
        attempts: Arc<Attempts>,
    }

    #[routes]
    impl DecoratedController {
        #[get("/")]
        #[public]
        #[resilient(upstream)]
        async fn preview(&self) -> Result<String, ResilienceError<UpstreamDown>> {
            Err(UpstreamDown)
        }

        /// A stream whose opening calls the upstream.
        #[sse("/feed")]
        #[public]
        #[resilient(upstream)]
        async fn feed(&self) -> Result<SseStream, ResilienceError<UpstreamDown>> {
            if self.attempts.first() {
                return Err(UpstreamDown);
            }
            Ok(SseStream::new(stream::iter([SseEvent::message("moved")])))
        }
    }

    #[module(
        imports = [ResilienceModule::for_root(opening_on_the_first_failure())],
        providers = [DecoratedController, Attempts],
    )]
    struct DecoratedHttpModule;

    #[tokio::test(start_paused = true)]
    async fn a_route_answers_its_refusal_503_with_the_breakers_wait() {
        let app = TestApp::for_module::<DecoratedHttpModule>()
            .await
            .expect("a decorated route boots");

        let failed = app.http().get("/decorated").send().await;
        failed.assert_status(StatusCode::INTERNAL_SERVER_ERROR);

        let refused = app.http().get("/decorated").send().await;
        refused.assert_status(StatusCode::SERVICE_UNAVAILABLE);
        refused.assert_header("retry-after", "30");
        refused
            .json()
            .await
            .value()
            .object()
            .get("code")
            .assert_string(code::UNAVAILABLE.as_str());
    }

    #[tokio::test(start_paused = true)]
    async fn an_sse_route_answers_its_refusal_503_with_the_breakers_wait() {
        let app = TestApp::for_module::<DecoratedHttpModule>()
            .await
            .expect("a decorated stream boots");

        let failed = app.http().get("/decorated/feed").send().await;
        failed.assert_status(StatusCode::INTERNAL_SERVER_ERROR);

        let refused = app.http().get("/decorated/feed").send().await;
        refused.assert_status(StatusCode::SERVICE_UNAVAILABLE);
        refused.assert_header("retry-after", "30");
        refused
            .json()
            .await
            .value()
            .object()
            .get("code")
            .assert_string(code::UNAVAILABLE.as_str());
        let attempts = app.container().get::<Attempts>().expect("provided");
        assert_eq!(
            attempts.made(),
            1,
            "sent once, and the opening the breaker refused never"
        );
    }

    #[controller(path = "/retried")]
    struct RetriedController {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
        #[inject]
        attempts: Arc<Attempts>,
    }

    #[routes]
    impl RetriedController {
        #[get("/")]
        #[public]
        #[resilient(upstream, idempotent)]
        async fn preview(&self) -> Result<String, ResilienceError<UpstreamDown>> {
            if self.attempts.first() {
                return Err(UpstreamDown);
            }
            Ok("previewed".to_owned())
        }

        /// A stream whose opening calls the upstream, again after a failure.
        #[sse("/feed")]
        #[public]
        #[resilient(upstream, idempotent)]
        async fn feed(&self) -> Result<SseStream, ResilienceError<UpstreamDown>> {
            if self.attempts.first() {
                return Err(UpstreamDown);
            }
            Ok(SseStream::new(stream::iter([SseEvent::message("moved")])))
        }
    }

    #[module(
        imports = [ResilienceModule::for_root(retrying_once())],
        providers = [RetriedController, Attempts],
    )]
    struct RetriedHttpModule;

    #[tokio::test(start_paused = true)]
    async fn an_sse_route_failing_once_answers_on_its_retry() {
        let app = TestApp::for_module::<RetriedHttpModule>()
            .await
            .expect("a retried stream boots");

        let answered = app.http().get("/retried/feed").send().await;
        answered.assert_status_is_ok();
        let body = answered
            .0
            .into_body()
            .into_string()
            .await
            .expect("the stream is text");
        assert!(body.contains("data: moved"), "the stream opens: {body:?}");
        let attempts = app.container().get::<Attempts>().expect("provided");
        assert_eq!(attempts.made(), 2, "the failed attempt and its retry");
    }

    #[tokio::test(start_paused = true)]
    async fn a_route_failing_once_answers_on_its_retry_within_its_one_request() {
        let logs = LogCapture::install();
        let app = TestApp::for_module::<RetriedHttpModule>()
            .await
            .expect("a retried route boots");

        let answered = app.http().get("/retried").send().await;
        answered.assert_status_is_ok();
        answered.assert_text("previewed").await;

        let attempts = app.container().get::<Attempts>().expect("provided");
        assert_eq!(attempts.made(), 2, "the failed attempt and its retry");
        retried_once(&logs, "RetriedController", "preview");
        let request = logs.expect_one(OPERATIONS, nest_rs_http::unit::REQUEST.name());
        assert_eq!(
            request.field("status").as_deref(),
            Some("200"),
            "{request:?}"
        );
    }
}

/// A decorated root field and subscription of an `#[operations]` block, and its
/// query, mutation, field resolver, entity and subscription attempted again.
mod operations {
    use std::sync::Arc;

    use async_graphql::futures_util::StreamExt as _;
    use async_graphql::futures_util::stream::{self, BoxStream};
    use nest_rs_core::module;
    use nest_rs_core::operation_log::{OK, TARGET as OPERATIONS};
    use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
    use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
    use nest_rs_testing::{LogCapture, TestApp};
    use serde_json::{Value, json};

    use super::{
        Attempts, UpstreamDown, opening_on_the_first_failure, retried_once, retrying_once,
    };

    #[resolver]
    struct DecoratedResolver {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
        #[inject]
        attempts: Arc<Attempts>,
    }

    #[operations]
    impl DecoratedResolver {
        #[query]
        #[public]
        #[resilient(upstream)]
        async fn decorated(&self) -> Result<String, ResilienceError<UpstreamDown>> {
            Err(UpstreamDown)
        }

        #[subscription]
        #[public]
        #[resilient(upstream)]
        async fn movements(
            &self,
        ) -> Result<BoxStream<'static, i32>, ResilienceError<UpstreamDown>> {
            if self.attempts.first() {
                return Err(UpstreamDown);
            }
            Ok(stream::iter([1]).boxed())
        }
    }

    #[module(
        imports = [GraphqlModule, ResilienceModule::for_root(opening_on_the_first_failure())],
        providers = [DecoratedResolver, Attempts],
    )]
    struct DecoratedGraphqlModule;

    #[tokio::test(start_paused = true)]
    async fn a_field_answers_its_refusal_unavailable_with_the_breakers_wait() {
        let app = TestApp::for_module::<DecoratedGraphqlModule>()
            .await
            .expect("a decorated resolver boots");
        let query = || async {
            app.http()
                .post("/graphql")
                .body_json(&serde_json::json!({ "query": "{ decorated }" }))
                .send()
                .await
                .json()
                .await
                .value()
                .deserialize::<serde_json::Value>()
        };
        let failed = query().await;
        assert_eq!(
            failed["errors"][0]["extensions"]["code"], "INTERNAL",
            "{failed}"
        );

        let refused = query().await;
        let extensions = &refused["errors"][0]["extensions"];
        assert_eq!(extensions["code"], "UNAVAILABLE", "{refused}");
        assert_eq!(extensions["retryAfterSeconds"], 30, "{refused}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_subscription_answers_its_refusal_unavailable_with_the_breakers_wait() {
        let app = TestApp::for_module::<DecoratedGraphqlModule>()
            .await
            .expect("a decorated subscription boots");
        let mut socket = app.graphql_socket().open();
        socket.connect().await;

        socket.subscribe("failed", "subscription { movements }");
        let failed = socket
            .next_item("failed")
            .await
            .expect("the opening answers");
        assert_eq!(
            failed["errors"][0]["extensions"]["code"], "INTERNAL",
            "{failed}"
        );

        socket.subscribe("refused", "subscription { movements }");
        let refused = socket
            .next_item("refused")
            .await
            .expect("the opening answers");
        let extensions = &refused["errors"][0]["extensions"];
        assert_eq!(extensions["code"], "UNAVAILABLE", "{refused}");
        assert_eq!(extensions["retryAfterSeconds"], 30, "{refused}");
        let attempts = app.container().get::<Attempts>().expect("provided");
        assert_eq!(
            attempts.made(),
            1,
            "sent once, and the opening the breaker refused never"
        );
    }

    /// A parcel, which a field resolver extends and an entity resolves by its
    /// number.
    #[derive(async_graphql::SimpleObject)]
    #[graphql(complex)]
    struct Parcel {
        number: String,
    }

    #[resolver]
    struct RetriedResolver {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
        #[inject]
        attempts: Arc<Attempts>,
    }

    /// `#[operations]` reads a field's `Result` by its spelling, so each
    /// decorated role writes it out.
    #[operations]
    impl RetriedResolver {
        #[query]
        #[public]
        async fn parcel(&self) -> Parcel {
            Parcel {
                number: "P1".to_owned(),
            }
        }

        #[query]
        #[public]
        #[resilient(upstream, idempotent)]
        async fn tracked(&self) -> Result<String, ResilienceError<UpstreamDown>> {
            if self.attempts.first() {
                return Err(UpstreamDown);
            }
            Ok("in transit".to_owned())
        }

        #[mutation]
        #[public]
        #[resilient(upstream, idempotent)]
        async fn reroute(&self) -> Result<String, ResilienceError<UpstreamDown>> {
            if self.attempts.first() {
                return Err(UpstreamDown);
            }
            Ok("rerouted".to_owned())
        }

        #[field_resolver]
        #[resilient(upstream, idempotent)]
        async fn depot(&self, parent: &Parcel) -> Result<String, ResilienceError<UpstreamDown>> {
            if self.attempts.first() {
                return Err(UpstreamDown);
            }
            Ok(format!("{} waits in Lyon", parent.number))
        }

        #[entity]
        #[public]
        #[resilient(upstream, idempotent)]
        async fn find_parcel_by_number(
            &self,
            number: String,
        ) -> Result<Parcel, ResilienceError<UpstreamDown>> {
            if self.attempts.first() {
                return Err(UpstreamDown);
            }
            Ok(Parcel {
                number: number.clone(),
            })
        }

        /// The parcels arriving, the box unsized where the method answers it.
        #[subscription]
        #[public]
        #[resilient(upstream, idempotent)]
        async fn arrivals(&self) -> Result<BoxStream<'static, i32>, ResilienceError<UpstreamDown>> {
            if self.attempts.first() {
                return Err(UpstreamDown);
            }
            Ok(Box::pin(stream::iter([7])))
        }
    }

    #[module(
        imports = [
            GraphqlModule::for_root(GraphqlConfig {
                federation: true,
                federation_sdl: nest_rs_config::Surface::Open,
                ..GraphqlConfig::default()
            }),
            ResilienceModule::for_root(retrying_once()),
        ],
        providers = [RetriedResolver, Attempts],
    )]
    struct RetriedGraphqlModule;

    /// What the schema answers `body` with, and how many attempts the one
    /// decorated method it reached made.
    async fn retried(body: Value) -> (Value, u32) {
        let app = TestApp::for_module::<RetriedGraphqlModule>()
            .await
            .expect("a resolver decorating every role boots");
        let answer = app
            .http()
            .post("/graphql")
            .body_json(&body)
            .send()
            .await
            .json()
            .await
            .value()
            .deserialize::<Value>();
        let attempts = app.container().get::<Attempts>().expect("provided");
        (answer, attempts.made())
    }

    #[tokio::test(start_paused = true)]
    async fn a_query_failing_once_answers_on_its_retry_within_its_one_operation() {
        let logs = LogCapture::install();
        let (answer, attempts) = retried(json!({ "query": "{ tracked }" })).await;
        assert_eq!(answer["data"]["tracked"], "in transit", "{answer}");
        assert_eq!(answer.get("errors"), None, "{answer}");
        assert_eq!(attempts, 2, "the failed attempt and its retry");
        retried_once(&logs, "RetriedResolver", "tracked");
        let operation = logs.expect_one(OPERATIONS, nest_rs_graphql::unit::OPERATION.name());
        assert_eq!(
            operation.field("outcome").as_deref(),
            Some(OK),
            "{operation:?}"
        );
        logs.expect_one(OPERATIONS, nest_rs_http::unit::REQUEST.name());
    }

    #[tokio::test(start_paused = true)]
    async fn a_mutation_failing_once_answers_on_its_retry() {
        let (answer, attempts) = retried(json!({ "query": "mutation { reroute }" })).await;
        assert_eq!(answer["data"]["reroute"], "rerouted", "{answer}");
        assert_eq!(attempts, 2, "the failed attempt and its retry");
    }

    #[tokio::test(start_paused = true)]
    async fn a_field_resolver_failing_once_answers_on_its_retry() {
        let (answer, attempts) = retried(json!({ "query": "{ parcel { depot } }" })).await;
        assert_eq!(
            answer["data"]["parcel"]["depot"], "P1 waits in Lyon",
            "{answer}"
        );
        assert_eq!(attempts, 2, "the failed attempt and its retry");
    }

    #[tokio::test(start_paused = true)]
    async fn an_entity_failing_once_answers_its_reference_on_its_retry() {
        let (answer, attempts) = retried(json!({
            "query": "query($reps: [_Any!]!) { \
                      _entities(representations: $reps) { ... on Parcel { number } } }",
            "variables": { "reps": [{ "__typename": "Parcel", "number": "P7" }] },
        }))
        .await;
        assert_eq!(answer["data"]["_entities"][0]["number"], "P7", "{answer}");
        assert_eq!(attempts, 2, "the failed attempt and its retry");
    }

    #[tokio::test(start_paused = true)]
    async fn a_subscription_failing_once_answers_on_its_retry() {
        let app = TestApp::for_module::<RetriedGraphqlModule>()
            .await
            .expect("a resolver decorating every role boots");
        let mut socket = app.graphql_socket().open();
        socket.connect().await;

        socket.subscribe("retried", "subscription { arrivals }");
        let item = socket
            .next_item("retried")
            .await
            .expect("the opening answers");
        assert_eq!(item["data"]["arrivals"], 7, "{item}");
        let attempts = app.container().get::<Attempts>().expect("provided");
        assert_eq!(attempts.made(), 2, "the failed attempt and its retry");
    }
}

/// A decorated message of a `#[messages]` block, refused, and attempted again
/// within its one unit.
mod messages {
    use std::sync::Arc;

    use nest_rs_core::module;
    use nest_rs_core::operation_log::{OK, TARGET as OPERATIONS};
    use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
    use nest_rs_testing::{LogCapture, TestApp};
    use nest_rs_ws::{WsModule, gateway, messages};

    use super::{
        Attempts, UpstreamDown, opening_on_the_first_failure, retried_once, retrying_at_once,
    };

    #[gateway(path = "/ws/decorated")]
    struct DecoratedGateway {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
    }

    #[messages]
    impl DecoratedGateway {
        #[subscribe_message("decorated")]
        #[public]
        #[resilient(upstream)]
        async fn decorated(&self) -> Result<String, ResilienceError<UpstreamDown>> {
            Err(UpstreamDown)
        }
    }

    #[module(
        imports = [WsModule, ResilienceModule::for_root(opening_on_the_first_failure())],
        providers = [DecoratedGateway],
    )]
    struct DecoratedWsModule;

    #[tokio::test]
    async fn a_message_answers_its_refusal_unavailable_with_the_breakers_wait() {
        let app = TestApp::builder()
            .module::<DecoratedWsModule>()
            .build_ws()
            .await
            .expect("a decorated gateway boots on a real port");
        let upstream = app
            .container()
            .get::<ResiliencePolicy>()
            .expect("the module built the policy");
        let mut socket = app.socket("/ws/decorated").connect().await;

        socket.send("decorated", serde_json::Value::Null).await;
        let failed = socket.next_envelope().await;
        assert_eq!(failed["data"]["code"], "INTERNAL", "{failed}");

        let most = crate::breaker_wait(&upstream);
        socket.send("decorated", serde_json::Value::Null).await;
        let refused = socket.next_envelope().await;
        let least = crate::breaker_wait(&upstream);
        assert_eq!(refused["data"]["code"], "UNAVAILABLE", "{refused}");
        let told = refused["data"]["retryAfterSeconds"].as_u64();
        assert!(
            told.is_some_and(|told| (least..=most).contains(&told)),
            "the breaker's wait as it refused, {least} to {most} s: {refused}"
        );

        app.shutdown().await.expect("the transport stops cleanly");
    }

    #[gateway(path = "/ws/retried")]
    struct RetriedGateway {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
        #[inject]
        attempts: Arc<Attempts>,
    }

    #[messages]
    impl RetriedGateway {
        #[subscribe_message("track")]
        #[public]
        #[resilient(upstream, idempotent)]
        async fn track(&self) -> Result<String, ResilienceError<UpstreamDown>> {
            if self.attempts.first() {
                return Err(UpstreamDown);
            }
            Ok("in transit".to_owned())
        }
    }

    #[module(
        imports = [WsModule, ResilienceModule::for_root(retrying_at_once())],
        providers = [RetriedGateway, Attempts],
    )]
    struct RetriedWsModule;

    /// On the real clock, as the socket is: the retry comes a millisecond or
    /// two after the failure.
    #[tokio::test]
    async fn a_message_failing_once_answers_on_its_retry_within_its_one_unit() {
        let logs = LogCapture::install();
        let app = TestApp::builder()
            .module::<RetriedWsModule>()
            .build_ws()
            .await
            .expect("a retried gateway boots on a real port");
        let mut socket = app.socket("/ws/retried").connect().await;

        socket.send("track", serde_json::Value::Null).await;
        let answer = socket.next_envelope().await;
        assert_eq!(answer["data"], "in transit", "{answer}");

        let attempts = app.container().get::<Attempts>().expect("provided");
        assert_eq!(attempts.made(), 2, "the failed attempt and its retry");
        retried_once(&logs, "RetriedGateway", "track");
        let message = logs.expect_one(OPERATIONS, nest_rs_ws::unit::MESSAGE.name());
        assert_eq!(message.field("outcome").as_deref(), Some(OK), "{message:?}");

        app.shutdown().await.expect("the transport stops cleanly");
    }
}

/// A decorated tool and resource listing of a `#[tools]` block, and its tool,
/// prompt, resource and listing attempted again.
mod tools {
    use std::sync::Arc;

    use nest_rs_core::module;
    use nest_rs_core::operation_log::{OK, TARGET as OPERATIONS};
    use nest_rs_mcp::model::{GetPromptResult, PromptMessage, Resource, Role};
    use nest_rs_mcp::{McpAccess, McpError, McpModule, mcp, tools};
    use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
    use nest_rs_testing::mcp::{call_method, call_tool, open_session, result};
    use nest_rs_testing::{LogCapture, TestApp};
    use serde_json::{Value, json};

    use super::{
        Attempts, UpstreamDown, opening_on_the_first_failure, retried_once, retrying_once,
    };

    const PATH: &str = "/mcp/decorated";

    #[mcp(path = "/mcp/decorated")]
    #[derive(Clone)]
    struct DecoratedTools {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
        #[inject]
        attempts: Arc<Attempts>,
    }

    #[tools]
    impl DecoratedTools {
        /// Preview a link through the upstream, under its policy.
        #[tool]
        #[public]
        #[resilient(upstream)]
        async fn decorated(&self) -> Result<String, ResilienceError<UpstreamDown>> {
            Err(UpstreamDown)
        }

        #[resources]
        #[public]
        #[resilient(upstream)]
        async fn parcels(&self) -> Result<Vec<Resource>, ResilienceError<UpstreamDown>> {
            if self.attempts.first() {
                return Err(UpstreamDown);
            }
            Ok(vec![Resource::new("parcel://7", "parcel 7")])
        }

        /// Where a parcel the listing lists stands.
        #[resource(template = "parcel://{id}")]
        #[public]
        async fn parcel(&self, id: u32) -> Result<String, McpError> {
            Ok(format!("parcel {id} waits in Lyon"))
        }
    }

    #[module(
        imports = [
            McpModule::for_root(McpAccess::Public),
            ResilienceModule::for_root(opening_on_the_first_failure()),
        ],
        providers = [DecoratedTools, Attempts],
    )]
    struct DecoratedMcpModule;

    #[tokio::test(start_paused = true)]
    async fn a_tool_answers_its_refusal_unavailable_with_the_breakers_wait() {
        let app = TestApp::for_module::<DecoratedMcpModule>()
            .await
            .expect("a decorated #[mcp] host boots");

        let failed = call_tool(app.http(), PATH, "decorated", None).await;
        assert!(failed.contains("\"code\":\"INTERNAL\""), "{failed}");

        let refused = call_tool(app.http(), PATH, "decorated", None).await;
        assert!(refused.contains("\"code\":\"UNAVAILABLE\""), "{refused}");
        assert!(refused.contains("\"retryAfterSeconds\":30"), "{refused}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_resource_listing_answers_its_refusal_unavailable_with_the_breakers_wait() {
        let app = TestApp::for_module::<DecoratedMcpModule>()
            .await
            .expect("a decorated listing boots");
        let session = open_session(app.http(), PATH, None).await;

        let failed = call_method(
            app.http(),
            PATH,
            &session,
            None,
            "resources/list",
            json!({}),
        )
        .await;
        assert!(failed.contains("\"code\":\"INTERNAL\""), "{failed}");

        let refused = call_method(
            app.http(),
            PATH,
            &session,
            None,
            "resources/list",
            json!({}),
        )
        .await;
        assert!(refused.contains("\"code\":\"UNAVAILABLE\""), "{refused}");
        assert!(refused.contains("\"retryAfterSeconds\":30"), "{refused}");
        let attempts = app.container().get::<Attempts>().expect("provided");
        assert_eq!(
            attempts.made(),
            1,
            "sent once, and the listing the breaker refused never"
        );
    }

    const RETRIED: &str = "/mcp/retried";

    #[mcp(path = "/mcp/retried")]
    #[derive(Clone)]
    struct RetriedTools {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
        #[inject]
        attempts: Arc<Attempts>,
    }

    #[tools]
    impl RetriedTools {
        /// Track a parcel through the carrier, under its policy.
        #[tool]
        #[public]
        #[resilient(upstream, idempotent)]
        async fn track(&self) -> Result<String, ResilienceError<UpstreamDown>> {
            if self.attempts.first() {
                return Err(UpstreamDown);
            }
            Ok("in transit".to_owned())
        }

        /// Draft a note to the carrier, under its policy.
        #[prompt]
        #[public]
        #[resilient(upstream, idempotent)]
        async fn draft(&self) -> Result<GetPromptResult, ResilienceError<UpstreamDown>> {
            if self.attempts.first() {
                return Err(UpstreamDown);
            }
            Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                Role::User,
                "drafted",
            )]))
        }

        /// Where a parcel stands, read from the carrier under its policy.
        #[resource(template = "parcel://{id}")]
        #[public]
        #[resilient(upstream, idempotent)]
        async fn parcel(&self, id: u32) -> Result<String, ResilienceError<UpstreamDown>> {
            if self.attempts.first() {
                return Err(UpstreamDown);
            }
            Ok(format!("parcel {id} waits in Lyon"))
        }

        #[resources]
        #[public]
        #[resilient(upstream, idempotent)]
        async fn parcels(&self) -> Result<Vec<Resource>, ResilienceError<UpstreamDown>> {
            if self.attempts.first() {
                return Err(UpstreamDown);
            }
            Ok(vec![Resource::new("parcel://7", "parcel 7")])
        }
    }

    #[module(
        imports = [
            McpModule::for_root(McpAccess::Public),
            ResilienceModule::for_root(retrying_once()),
        ],
        providers = [RetriedTools, Attempts],
    )]
    struct RetriedMcpModule;

    /// What the host answers `method` with on a session of its own, and how
    /// many attempts the one decorated method it reached made.
    async fn retried(method: &str, params: Value) -> (Value, u32) {
        let app = TestApp::for_module::<RetriedMcpModule>()
            .await
            .expect("a host decorating its prompt, its resource and its listing boots");
        let session = open_session(app.http(), RETRIED, None).await;
        let body = call_method(app.http(), RETRIED, &session, None, method, params).await;
        let attempts = app.container().get::<Attempts>().expect("provided");
        (result(&body), attempts.made())
    }

    #[tokio::test(start_paused = true)]
    async fn a_tool_failing_once_answers_on_its_retry_within_its_one_call() {
        let logs = LogCapture::install();
        let app = TestApp::for_module::<RetriedMcpModule>()
            .await
            .expect("a host decorating its tool boots");

        let called = call_tool(app.http(), RETRIED, "track", None).await;
        let answer = result(&called);
        assert_eq!(
            answer["result"]["content"][0]["text"], "in transit",
            "{answer}"
        );
        assert_ne!(answer["result"]["isError"], true, "{answer}");

        let attempts = app.container().get::<Attempts>().expect("provided");
        assert_eq!(attempts.made(), 2, "the failed attempt and its retry");
        retried_once(&logs, "RetriedTools", "track");
        let mut calls = logs.find(OPERATIONS, nest_rs_mcp::unit::OPERATION.name());
        calls.retain(|line| line.field("tool").as_deref() == Some("track"));
        assert_eq!(calls.len(), 1, "one call of the tool: {calls:#?}");
        assert_eq!(calls[0].field("outcome").as_deref(), Some(OK));
    }

    #[tokio::test(start_paused = true)]
    async fn a_prompt_failing_once_answers_on_its_retry() {
        let (answer, attempts) =
            retried("prompts/get", json!({ "name": "draft", "arguments": {} })).await;
        assert_eq!(
            answer["result"]["messages"][0]["content"]["text"], "drafted",
            "{answer}"
        );
        assert_eq!(attempts, 2, "the failed attempt and its retry");
    }

    #[tokio::test(start_paused = true)]
    async fn a_resource_failing_once_answers_on_its_retry() {
        let (answer, attempts) = retried("resources/read", json!({ "uri": "parcel://7" })).await;
        assert_eq!(
            answer["result"]["contents"][0]["text"], "parcel 7 waits in Lyon",
            "{answer}"
        );
        assert_eq!(attempts, 2, "the failed attempt and its retry");
    }

    #[tokio::test(start_paused = true)]
    async fn a_resource_listing_failing_once_answers_on_its_retry() {
        let (answer, attempts) = retried("resources/list", json!({})).await;
        assert_eq!(
            answer["result"]["resources"][0]["uri"], "parcel://7",
            "{answer}"
        );
        assert_eq!(attempts, 2, "the failed attempt and its retry");
    }
}

/// A decorated job of a `#[processor]` block, whose refusal holds its retry
/// back the breaker's wait, and whose handler is sent again within the job's
/// one attempt.
mod processor {
    use std::sync::Arc;
    use std::time::Duration;

    use nest_rs_core::Container;
    use nest_rs_core::operation_log::{OK, TARGET as OPERATIONS};
    use nest_rs_queue::__private::consume::{self, AttemptOutcome, Delivery};
    use nest_rs_queue::{
        Capabilities, ProcessMethod, QueueBackend, QueueName, WIRE_FORMAT_VERSION, processor, queue,
    };
    use nest_rs_resilience::{ResilienceError, ResiliencePolicy, resilient};
    use nest_rs_testing::LogCapture;
    use serde::{Deserialize, Serialize};

    use super::{
        Attempts, UpstreamDown, opening_on_the_first_failure, retried_once, retrying_once,
    };

    static BARE: QueueBackend = QueueBackend::new("bare", Capabilities::NONE);

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct RefreshCommand {
        url: String,
    }

    #[queue(name = "decorated-refreshes", job = RefreshCommand)]
    struct RefreshesQueue;

    #[nest_rs_core::injectable]
    struct RefreshProcessor {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
    }

    #[processor]
    impl RefreshProcessor {
        #[process(queue = RefreshesQueue, retries = 3)]
        #[resilient(upstream)]
        async fn refresh(&self, job: RefreshCommand) -> Result<(), ResilienceError<UpstreamDown>> {
            drop(job);
            Err(UpstreamDown)
        }
    }

    async fn attempted(container: &Container) -> AttemptOutcome {
        let method = nest_rs_core::inventory::iter::<ProcessMethod>()
            .find(|method| method.name() == "RefreshProcessor::refresh")
            .expect("the method is submitted");
        let mut delivery = Delivery::new(
            &BARE,
            QueueName::new("decorated-refreshes").expect("a valid name"),
            serde_json::json!({
                "v": WIRE_FORMAT_VERSION,
                "payload": { "url": "https://example.com" },
            }),
        );
        consume::attempt(method, &mut delivery, container.clone(), None).await
    }

    #[tokio::test(start_paused = true)]
    async fn a_job_refused_by_an_open_breaker_waits_at_least_its_retry_after() {
        let (_app, upstream) = crate::policy(opening_on_the_first_failure()).await;
        let container = Container::builder()
            .provide(RefreshProcessor { upstream })
            .build()
            .expect("the container builds");

        let failed = attempted(&container).await;
        assert!(matches!(failed, AttemptOutcome::Retry { .. }), "{failed:?}");

        let AttemptOutcome::Retry { after } = attempted(&container).await else {
            panic!("a 503 another attempt may clear retries");
        };
        let asked = Duration::from_secs(30);
        assert!(
            after >= asked && after <= asked.mul_f64(1.2),
            "the breaker's wait, stretched by a fifth at most: {after:?}"
        );
    }

    #[queue(name = "retried-refreshes", job = RefreshCommand)]
    struct RetriedRefreshesQueue;

    #[nest_rs_core::injectable]
    struct RetriedProcessor {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
        #[inject]
        attempts: Arc<Attempts>,
    }

    #[processor]
    impl RetriedProcessor {
        #[process(queue = RetriedRefreshesQueue, retries = 3)]
        #[resilient(upstream, idempotent)]
        async fn refresh(&self, job: RefreshCommand) -> Result<(), ResilienceError<UpstreamDown>> {
            if self.attempts.first() {
                return Err(UpstreamDown);
            }
            assert_eq!(job.url, "https://example.com");
            Ok(())
        }
    }

    /// The handler is sent again inside the job's one attempt: the queue is
    /// answered done, and never asked for a second attempt of the job.
    #[tokio::test(start_paused = true)]
    async fn a_job_failing_once_is_done_on_its_retry_within_its_one_attempt() {
        let logs = LogCapture::install();
        let (_app, upstream) = crate::policy(retrying_once()).await;
        let attempts = Arc::new(Attempts::default());
        let container = Container::builder()
            .provide(RetriedProcessor {
                upstream,
                attempts: Arc::clone(&attempts),
            })
            .build()
            .expect("the container builds");
        let method = nest_rs_core::inventory::iter::<ProcessMethod>()
            .find(|method| method.name() == "RetriedProcessor::refresh")
            .expect("the method is submitted");
        let mut delivery = Delivery::new(
            &BARE,
            QueueName::new("retried-refreshes").expect("a valid name"),
            serde_json::json!({
                "v": WIRE_FORMAT_VERSION,
                "payload": { "url": "https://example.com" },
            }),
        );

        let outcome = consume::attempt(method, &mut delivery, container, None).await;
        assert!(matches!(outcome, AttemptOutcome::Ok), "{outcome:?}");
        assert_eq!(attempts.made(), 2, "the failed attempt and its retry");
        retried_once(&logs, "RetriedProcessor", "refresh");
        let job = logs.expect_one(OPERATIONS, nest_rs_queue::unit::JOB.name());
        assert_eq!(job.field("outcome").as_deref(), Some(OK), "{job:?}");
        assert_eq!(job.field("attempt").as_deref(), Some("1"), "{job:?}");
    }
}

/// A decorated tick of a `#[scheduled]` block, attempted again within the one
/// run its trigger fires, and refused: the occurrence filed failed, the one
/// after it run.
mod scheduled {
    use std::sync::Arc;
    use std::time::Duration;

    use nest_rs_core::operation_log::{ERROR, OK, TARGET as OPERATIONS};
    use nest_rs_core::{injectable, module};
    use nest_rs_resilience::{
        BreakerState, ResilienceError, ResilienceModule, ResiliencePolicy, resilient,
    };
    use nest_rs_schedule::{Scheduler, scheduled};
    use nest_rs_testing::{HeadlessApp, LogCapture};

    use super::{Attempts, Dependency, Refusing, UpstreamDown, refusing, retrying_once};

    #[injectable]
    struct RefreshTasks {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
        #[inject]
        attempts: Arc<Attempts>,
    }

    #[scheduled]
    impl RefreshTasks {
        #[after("1s")]
        #[resilient(upstream, idempotent)]
        async fn refresh(&self) -> Result<(), ResilienceError<UpstreamDown>> {
            if self.attempts.first() {
                return Err(UpstreamDown);
            }
            Ok(())
        }
    }

    #[module(
        imports = [ResilienceModule::for_root(retrying_once())],
        providers = [RefreshTasks, Attempts],
    )]
    struct DecoratedScheduleModule;

    #[tokio::test(start_paused = true)]
    async fn a_tick_failing_once_answers_on_its_retry_within_its_one_run() {
        let logs = LogCapture::install();
        let app = HeadlessApp::builder()
            .module::<DecoratedScheduleModule>()
            .build_headless()
            .await
            .expect("a decorated tick boots");
        let scheduler = app
            .spawn_transport(Scheduler::new())
            .await
            .expect("the scheduler configures against the app");
        tokio::time::sleep(Duration::from_secs(2)).await;
        scheduler.shutdown().await.expect("the scheduler stops");

        let attempts = app.container().get::<Attempts>().expect("provided");
        assert_eq!(attempts.made(), 2, "the failed attempt and its retry");
        let ran = logs.expect_one(OPERATIONS, nest_rs_schedule::unit::TICK.name());
        assert_eq!(ran.field("outcome").as_deref(), Some(OK));
        logs.expect_none(nest_rs_schedule::TARGET, "scheduled job failed");
    }

    #[injectable]
    struct SyncTasks {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
        #[inject]
        dependency: Arc<Dependency>,
    }

    #[scheduled]
    impl SyncTasks {
        #[every("1s")]
        #[resilient(upstream, timeout = "100ms")]
        async fn sync(&self) -> Result<(), ResilienceError<UpstreamDown>> {
            self.dependency.call().await
        }
    }

    #[module(
        imports = [ResilienceModule::for_root(refusing())],
        providers = [SyncTasks, Dependency],
    )]
    struct RefusedScheduleModule;

    /// No client reads a tick's `503`: its refusal is the occurrence failed,
    /// filed as any failure of a tick, and the schedule goes on.
    #[tokio::test(start_paused = true)]
    async fn a_tick_its_policy_refuses_is_filed_failed_and_its_next_occurrence_runs() {
        for refusing in Refusing::EACH {
            let logs = LogCapture::install();
            let app = HeadlessApp::builder()
                .module::<RefusedScheduleModule>()
                .build_headless()
                .await
                .expect("a decorated tick boots");
            let tasks = app.container().get::<SyncTasks>().expect("provided");
            // Open past the first occurrence, and no longer at the second.
            let (upstream, dependency) =
                refusing.arrange_for(app.container(), Duration::from_millis(1500));
            let mut events = upstream.events();
            let scheduler = app
                .spawn_transport(Scheduler::new())
                .await
                .expect("the scheduler configures against the app");

            tokio::time::sleep(Duration::from_millis(1500)).await;
            let mounted = logs.expect_one(nest_rs_schedule::TARGET, "mounted");
            let failed = logs.expect_one(nest_rs_schedule::TARGET, "scheduled job failed");
            assert_eq!(failed.level, "error", "{failed:?}");
            assert_eq!(logs.events_at(tracing::Level::ERROR).len(), 1);
            let ran = logs.expect_one(OPERATIONS, nest_rs_schedule::unit::TICK.name());
            assert_eq!(ran.field("outcome").as_deref(), Some(ERROR), "{ran:?}");
            for line in [&failed, &ran] {
                assert_eq!(line.field("host"), mounted.field("host"), "{line:?}");
                assert_eq!(line.field("method"), mounted.field("method"), "{line:?}");
            }
            assert_eq!(mounted.field("host").as_deref(), Some("SyncTasks"));
            assert_eq!(mounted.field("method").as_deref(), Some("sync"));
            refusing.told(&crate::told(&mut events), "SyncTasks", "sync");
            assert_eq!(dependency.called(), refusing.sent());
            assert_eq!(
                upstream.concurrency_running(),
                Some(0),
                "the refused run holds no slot of the bulkhead"
            );

            // The breaker is left to its open time: the occurrence after it
            // is the probe that closes it.
            if refusing == Refusing::TimedOut {
                refusing.lift(&upstream, &dependency);
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            scheduler.shutdown().await.expect("the scheduler stops");

            let ticks = logs.find(OPERATIONS, nest_rs_schedule::unit::TICK.name());
            assert_eq!(ticks.len(), 2, "the occurrence after it fired: {ticks:#?}");
            assert_eq!(ticks[1].field("outcome").as_deref(), Some(OK), "{ticks:#?}");
            assert_eq!(dependency.called(), refusing.sent() + 1);
            assert_eq!(upstream.breaker_state(), Some(BreakerState::Closed));
            assert_eq!(
                logs.find(nest_rs_schedule::TARGET, "scheduled job failed")
                    .len(),
                1,
                "the one refusal, filed once"
            );

            // The occurrence that ran ended the refusal, its open time over or its stall lifted.
            refusing.arrange(app.container());
            assert_eq!(
                failed.field("error"),
                Some(refusing.chain(tasks.sync().await)),
                "{failed:?}"
            );
        }
    }
}

/// A decorated listener of a `#[listeners]` block, attempted again within the
/// one call an emit makes, and refused: its unit filed failed, the emit going
/// on.
mod listeners {
    use std::sync::Arc;

    use nest_rs_core::operation_log::{ERROR, OK, TARGET as OPERATIONS};
    use nest_rs_core::{injectable, module};
    use nest_rs_events::{EventBus, EventsModule, listeners};
    use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
    use nest_rs_testing::{HeadlessApp, LogCapture};

    use super::{Attempts, Dependency, Refusing, UpstreamDown, refusing, retrying_once};

    /// A parcel left the depot, which the listener tells the carrier.
    #[derive(Clone)]
    struct Shipped;

    #[injectable]
    struct ShippedListener {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
        #[inject]
        attempts: Arc<Attempts>,
    }

    #[listeners]
    impl ShippedListener {
        #[on_event]
        #[resilient(upstream, idempotent)]
        async fn on_shipped(&self, _event: Shipped) -> Result<(), ResilienceError<UpstreamDown>> {
            if self.attempts.first() {
                return Err(UpstreamDown);
            }
            Ok(())
        }
    }

    #[module(
        imports = [EventsModule, ResilienceModule::for_root(retrying_once())],
        providers = [ShippedListener, Attempts],
    )]
    struct DecoratedEventsModule;

    #[tokio::test(start_paused = true)]
    async fn a_listener_failing_once_answers_on_its_retry_within_one_emit() {
        let logs = LogCapture::install();
        let app = HeadlessApp::builder()
            .module::<DecoratedEventsModule>()
            .build_headless()
            .await
            .expect("a decorated listener boots");
        let bus = app
            .container()
            .get::<EventBus>()
            .expect("the bus is provided");
        bus.emit(Shipped).await;

        let attempts = app.container().get::<Attempts>().expect("provided");
        assert_eq!(attempts.made(), 2, "the failed attempt and its retry");
        let ran = logs.expect_one(OPERATIONS, nest_rs_events::unit::DISPATCH.name());
        assert_eq!(ran.field("outcome").as_deref(), Some(OK));
    }

    /// Whom a returned parcel goes back to, which no line may say.
    const RECIPIENT: &str = "ada.lovelace@example.com";

    /// A parcel came back, which two listeners tell the dependency.
    #[derive(Clone)]
    struct Returned {
        recipient: &'static str,
    }

    #[injectable]
    struct ReturnsListener {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
        #[inject]
        dependency: Arc<Dependency>,
    }

    #[listeners]
    impl ReturnsListener {
        #[on_event]
        #[resilient(upstream, timeout = "100ms")]
        async fn refund(&self, event: Returned) -> Result<(), ResilienceError<UpstreamDown>> {
            assert_eq!(event.recipient, RECIPIENT);
            self.dependency.call().await
        }

        #[on_event]
        #[resilient(upstream, timeout = "100ms")]
        async fn restock(&self, event: Returned) -> Result<(), ResilienceError<UpstreamDown>> {
            assert_eq!(event.recipient, RECIPIENT);
            self.dependency.call().await
        }
    }

    #[module(
        imports = [EventsModule, ResilienceModule::for_root(refusing())],
        providers = [ReturnsListener, Dependency],
    )]
    struct RefusedEventsModule;

    /// No client reads a listener's `503`: its refusal fails its own unit,
    /// filed as any failure of a listener, the listener after it is still
    /// called — refused in its turn here, so whichever the bus calls first —
    /// and the emit returns to its caller.
    #[tokio::test(start_paused = true)]
    async fn a_listener_its_policy_refuses_fails_its_unit_and_the_emit_goes_on() {
        for refusing in Refusing::EACH {
            let logs = LogCapture::install();
            let app = HeadlessApp::builder()
                .module::<RefusedEventsModule>()
                .build_headless()
                .await
                .expect("a decorated listener boots");
            let listener = app.container().get::<ReturnsListener>().expect("provided");
            let bus = app
                .container()
                .get::<EventBus>()
                .expect("the bus is provided");
            let (upstream, dependency) = refusing.arrange(app.container());
            let mut events = upstream.events();

            bus.emit(Returned {
                recipient: RECIPIENT,
            })
            .await;
            let told = crate::told(&mut events);
            assert_eq!(dependency.called(), 2 * refusing.sent());

            let mounted = logs.find(nest_rs_events::TARGET, "mounted");
            let failed = logs.find(
                nest_rs_events::TARGET,
                "event listener failed — dispatch continues with the next listener",
            );
            let ran = logs.find(OPERATIONS, nest_rs_events::unit::DISPATCH.name());
            let chain = refusing.chain(
                listener
                    .refund(Returned {
                        recipient: RECIPIENT,
                    })
                    .await,
            );
            for method in ["refund", "restock"] {
                let named = |lines: &[nest_rs_testing::CapturedEvent]| {
                    let mut named = lines
                        .iter()
                        .filter(|line| line.field("method").as_deref() == Some(method));
                    let line = named.next().cloned();
                    assert!(
                        named.next().is_none(),
                        "one line for `{method}`: {lines:#?}"
                    );
                    line.unwrap_or_else(|| panic!("a line for `{method}`: {lines:#?}"))
                };
                let (mounted, failed, ran) = (named(&mounted), named(&failed), named(&ran));
                assert_eq!(failed.level, "error", "{failed:?}");
                assert_eq!(
                    failed.field("error").as_deref(),
                    Some(&*chain),
                    "{failed:?}"
                );
                assert_eq!(ran.field("outcome").as_deref(), Some(ERROR), "{ran:?}");
                for line in [&failed, &ran] {
                    assert_eq!(line.field("host"), mounted.field("host"), "{line:?}");
                    assert_eq!(line.field("event"), mounted.field("event"), "{line:?}");
                }
                assert_eq!(mounted.field("host").as_deref(), Some("ReturnsListener"));
                assert_eq!(mounted.field("event").as_deref(), Some("Returned"));
                refusing.told(&told, "ReturnsListener", method);
            }
            assert_eq!(
                failed.len(),
                2,
                "each listener's refusal, once: {failed:#?}"
            );
            assert_eq!(logs.events_at(tracing::Level::ERROR).len(), 2);
            assert!(
                !logs.quotes(RECIPIENT),
                "no line says what the event carried"
            );
        }
    }
}

/// A decorated indicator of an `#[indicators]` block, attempted again within
/// the one run its probe makes, and refused: its probe down, the others up.
mod indicators {
    use std::sync::Arc;

    use http::StatusCode;
    use nest_rs_core::{injectable, module};
    use nest_rs_health::{HealthModule, HealthService, IndicatorStatus, ProbeKind, indicators};
    use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
    use nest_rs_testing::{HeadlessApp, LogCapture, TestApp};
    use serde_json::Value;

    use super::{Attempts, Dependency, Refusing, UpstreamDown, refusing, retrying_once};

    #[injectable]
    struct UpstreamIndicator {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
        #[inject]
        attempts: Arc<Attempts>,
    }

    #[indicators]
    impl UpstreamIndicator {
        #[readiness]
        #[resilient(upstream, idempotent)]
        async fn upstream_reachable(&self) -> Result<(), ResilienceError<UpstreamDown>> {
            if self.attempts.first() {
                return Err(UpstreamDown);
            }
            Ok(())
        }
    }

    #[module(
        imports = [HealthModule, ResilienceModule::for_root(retrying_once())],
        providers = [UpstreamIndicator, Attempts],
    )]
    struct DecoratedHealthModule;

    #[tokio::test(start_paused = true)]
    async fn an_indicator_failing_once_answers_up_on_its_retry() {
        let app = HeadlessApp::builder()
            .module::<DecoratedHealthModule>()
            .build_headless()
            .await
            .expect("a decorated indicator boots");
        let health = app
            .container()
            .get::<HealthService>()
            .expect("HealthModule provides it");
        let ready = health.probe(ProbeKind::Readiness).await;

        assert_eq!(ready.status, IndicatorStatus::Up, "{:?}", ready.details);
        assert!(ready.details.contains_key("upstream_reachable"));
        let attempts = app.container().get::<Attempts>().expect("provided");
        assert_eq!(attempts.made(), 2, "the failed attempt and its retry");
    }

    #[injectable]
    struct DependencyIndicator {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
        #[inject]
        dependency: Arc<Dependency>,
    }

    #[indicators]
    impl DependencyIndicator {
        #[readiness]
        #[resilient(upstream, timeout = "100ms")]
        async fn dependency_reachable(&self) -> Result<(), ResilienceError<UpstreamDown>> {
            self.dependency.call().await
        }
    }

    #[module(
        imports = [HealthModule, ResilienceModule::for_root(refusing())],
        providers = [DependencyIndicator, Dependency],
    )]
    struct RefusedHealthModule;

    /// What the probe at `path` answers: its status and its report.
    async fn probed(app: &TestApp, path: &str) -> (StatusCode, Value) {
        let answered = app.http().get(path).send().await;
        let status = answered.0.status();
        (status, answered.json().await.value().deserialize::<Value>())
    }

    /// The `503` here is the probe's own, which an orchestrator reads: a
    /// refused indicator is down, its refusal in the log and never in the
    /// body, and the probe after the refusal lifted is up.
    #[tokio::test(start_paused = true)]
    async fn an_indicator_its_policy_refuses_answers_down_on_its_probe_alone() {
        for refusing in Refusing::EACH {
            let logs = LogCapture::install();
            let app = TestApp::for_module::<RefusedHealthModule>()
                .await
                .expect("a decorated indicator boots");
            let indicator = app
                .container()
                .get::<DependencyIndicator>()
                .expect("provided");
            let (upstream, dependency) = refusing.arrange(app.container());
            let mut events = upstream.events();

            let (status, ready) = probed(&app, "/health/ready").await;
            let told = crate::told(&mut events);
            assert_eq!(dependency.called(), refusing.sent());
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{ready}");
            assert_eq!(ready["status"], "down", "{ready}");
            assert_eq!(ready["error"]["dependency_reachable"]["status"], "down");
            let (status, live) = probed(&app, "/health/live").await;
            assert_eq!(status, StatusCode::OK, "{live}");
            assert_eq!(live["status"], "up", "{live}");

            let failed = logs.expect_one(nest_rs_health::TARGET, "health indicator failed");
            assert_eq!(failed.level, "warn", "{failed:?}");
            assert_eq!(
                failed.field("indicator").as_deref(),
                Some("dependency_reachable")
            );
            assert_eq!(
                failed.field("kind").as_deref(),
                Some(ProbeKind::Readiness.as_str())
            );
            refusing.told(&told, "DependencyIndicator", "dependency_reachable");
            let chain = refusing.chain(indicator.dependency_reachable().await);
            assert_eq!(
                failed.field("error").as_deref(),
                Some(&*chain),
                "{failed:?}"
            );
            assert!(
                !ready.to_string().contains(&chain),
                "the report says the indicator is down, never why: {ready}"
            );

            refusing.lift(&upstream, &dependency);
            let (status, ready) = probed(&app, "/health/ready").await;
            assert_eq!(status, StatusCode::OK, "{ready}");
            assert_eq!(ready["info"]["dependency_reachable"]["status"], "up");
        }
    }
}

/// A decorated hook of a `#[hooks]` block, attempted again within the one run
/// the app's init makes, and refused: an init hook failing the boot, a shutdown
/// hook filed with the hook after it run.
mod hooks {
    use std::sync::Arc;

    use std::sync::atomic::{AtomicU32, Ordering};

    use nest_rs_core::operation_log::{ERROR, OK, TARGET as OPERATIONS};
    use nest_rs_core::{BootStep, LifecyclePhase, hooks, injectable, module};
    use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
    use nest_rs_testing::{HeadlessApp, LogCapture};

    use super::{Attempts, Dependency, Refusing, UpstreamDown, refusing, retrying_once};

    #[injectable]
    struct CacheWarmer {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
        #[inject]
        attempts: Arc<Attempts>,
    }

    #[hooks]
    impl CacheWarmer {
        #[on_module_init]
        #[resilient(upstream, idempotent)]
        async fn warm(&self) -> Result<(), ResilienceError<UpstreamDown>> {
            if self.attempts.first() {
                return Err(UpstreamDown);
            }
            Ok(())
        }
    }

    #[module(
        imports = [ResilienceModule::for_root(retrying_once())],
        providers = [CacheWarmer, Attempts],
    )]
    struct DecoratedHooksModule;

    #[tokio::test(start_paused = true)]
    async fn a_hook_failing_once_answers_on_its_retry_and_the_init_completes() {
        let app = HeadlessApp::builder()
            .module::<DecoratedHooksModule>()
            .build_headless()
            .await
            .expect("a decorated hook boots");
        app.init().await.expect("the hook answered on its retry");

        let attempts = app.container().get::<Attempts>().expect("provided");
        assert_eq!(attempts.made(), 2, "the failed attempt and its retry");
    }

    #[injectable]
    struct WarmingHooks {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
        #[inject]
        dependency: Arc<Dependency>,
    }

    #[hooks]
    impl WarmingHooks {
        #[on_module_init]
        #[resilient(upstream, timeout = "100ms")]
        async fn warm(&self) -> Result<(), ResilienceError<UpstreamDown>> {
            self.dependency.call().await
        }
    }

    #[module(
        imports = [ResilienceModule::for_root(refusing())],
        providers = [WarmingHooks, Dependency],
    )]
    struct RefusedInitModule;

    /// No client reads an init hook's `503`: its refusal fails the boot, the
    /// error naming the hook over the refusal itself, which a `main` matches.
    #[tokio::test(start_paused = true)]
    async fn an_init_hook_its_policy_refuses_fails_the_boot_naming_it() {
        for refusing in Refusing::EACH {
            let logs = LogCapture::install();
            let app = HeadlessApp::builder()
                .module::<RefusedInitModule>()
                .build_headless()
                .await
                .expect("a decorated hook boots");
            let hooks = app.container().get::<WarmingHooks>().expect("provided");
            let (upstream, dependency) = refusing.arrange(app.container());
            let mut events = upstream.events();

            let failed = app
                .init()
                .await
                .expect_err("the hook's refusal fails the boot");
            let told = crate::told(&mut events);
            assert_eq!(dependency.called(), refusing.sent());

            let ran = logs.expect_one(OPERATIONS, nest_rs_core::unit::HOOK.name());
            assert_eq!(ran.field("outcome").as_deref(), Some(ERROR), "{ran:?}");
            assert_eq!(
                ran.field("phase").as_deref(),
                Some(LifecyclePhase::OnModuleInit.as_str())
            );
            assert_eq!(ran.field("host").as_deref(), Some("WarmingHooks"));
            assert_eq!(ran.field("method").as_deref(), Some("warm"));
            assert_eq!(ran.field("origin").as_deref(), Some(module_path!()));
            refusing.told(&told, "WarmingHooks", "warm");
            let torn_down = logs.expect_one(
                nest_rs_core::target::LIFECYCLE,
                "the boot failed after init hooks ran: running the shutdown hooks",
            );
            assert_eq!(torn_down.field("host"), ran.field("host"), "{torn_down:?}");
            assert_eq!(torn_down.field("method"), ran.field("method"));

            assert!(
                failed
                    .chain()
                    .any(|link| link.is::<ResilienceError<UpstreamDown>>()),
                "the refusal itself, for a `main` to match: {failed:?}"
            );
            let step = BootStep::InitHook {
                phase: LifecyclePhase::OnModuleInit,
                host: "WarmingHooks",
                method: "warm",
                origin: module_path!(),
            };
            let chain = refusing.chain(hooks.warm().await);
            assert_eq!(format!("{failed:#}"), format!("{step} failed: {chain}"));
        }
    }

    #[injectable]
    struct FlushingHooks {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
        #[inject]
        dependency: Arc<Dependency>,
        closed: AtomicU32,
    }

    #[hooks]
    impl FlushingHooks {
        #[on_module_destroy]
        #[resilient(upstream, timeout = "100ms")]
        async fn flush(&self) -> Result<(), ResilienceError<UpstreamDown>> {
            self.dependency.call().await
        }

        #[on_application_shutdown]
        async fn close(&self) -> anyhow::Result<()> {
            self.closed.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    #[module(
        imports = [ResilienceModule::for_root(refusing())],
        providers = [FlushingHooks, Dependency],
    )]
    struct RefusedShutdownModule;

    /// No client reads a shutdown hook's `503`: its refusal is filed as any
    /// failure of a shutdown hook, and the hook after it still runs.
    #[tokio::test(start_paused = true)]
    async fn a_shutdown_hook_its_policy_refuses_is_filed_and_the_hook_after_it_runs() {
        for refusing in Refusing::EACH {
            let logs = LogCapture::install();
            let app = HeadlessApp::builder()
                .module::<RefusedShutdownModule>()
                .build_headless()
                .await
                .expect("a decorated hook boots");
            app.init().await.expect("no init hook fails");
            let hooks = app.container().get::<FlushingHooks>().expect("provided");
            let (upstream, dependency) = refusing.arrange(app.container());
            let mut events = upstream.events();

            app.close().await;
            let told = crate::told(&mut events);
            assert_eq!(dependency.called(), refusing.sent());

            let failed = logs.expect_one(nest_rs_core::target::LIFECYCLE, "lifecycle hook failed");
            assert_eq!(failed.level, "error", "{failed:?}");
            assert_eq!(logs.events_at(tracing::Level::ERROR).len(), 1);
            let mut ran = logs.find(OPERATIONS, nest_rs_core::unit::HOOK.name());
            ran.retain(|line| line.field("host").as_deref() == Some("FlushingHooks"));
            let [flushed, closed] = &ran[..] else {
                panic!("the refused hook, then the one after it: {ran:#?}");
            };
            assert_eq!(flushed.field("outcome").as_deref(), Some(ERROR));
            for line in [&failed, flushed] {
                assert_eq!(
                    line.field("phase").as_deref(),
                    Some(LifecyclePhase::OnModuleDestroy.as_str()),
                    "{line:?}"
                );
                assert_eq!(line.field("host").as_deref(), Some("FlushingHooks"));
                assert_eq!(line.field("method").as_deref(), Some("flush"));
                assert_eq!(line.field("origin").as_deref(), Some(module_path!()));
            }
            assert_eq!(closed.field("method").as_deref(), Some("close"));
            assert_eq!(closed.field("outcome").as_deref(), Some(OK));
            assert_eq!(hooks.closed.load(Ordering::Relaxed), 1);
            refusing.told(&told, "FlushingHooks", "flush");

            let chain = refusing.chain(hooks.flush().await);
            assert_eq!(
                failed.field("error").as_deref(),
                Some(&*chain),
                "{failed:?}"
            );
        }
    }
}

/// What a decorated method's future holds, and the stack it is polled on: its
/// body's future and each argument once, beside what its run holds whatever its
/// body, so a method holding a large buffer runs where its undecorated twin
/// runs.
mod footprint {
    use std::sync::Arc;

    use nest_rs_core::{injectable, module};
    use nest_rs_resilience::{ResilienceModule, ResiliencePolicy, resilient};

    use super::{Answer, Courier, boot};
    use crate::quiet;

    /// What a body below holds across its `.await`, or takes by value: past
    /// anything a run holds, so a second copy of it shows.
    const HELD: usize = 16 * 1024;

    /// What the rows under tokio's threshold hold or take: a quarter of it.
    const UNDER: usize = 4 * 1024;

    /// What a run holds whatever its body: its slot, its attempt's pass and
    /// bounds, the timers it waits on.
    const RUN: usize = 1024;

    #[injectable]
    struct BuffersService {
        #[inject]
        courier: Arc<ResiliencePolicy<Courier>>,
    }

    impl BuffersService {
        async fn holding<const N: usize>(&self) -> Answer<usize> {
            let buffer = [7_u8; N];
            tokio::task::yield_now().await;
            Ok(std::hint::black_box(&buffer).len())
        }

        #[resilient(courier)]
        async fn holding_once<const N: usize>(&self) -> Answer<usize> {
            let buffer = [7_u8; N];
            tokio::task::yield_now().await;
            Ok(std::hint::black_box(&buffer).len())
        }

        #[resilient(courier, idempotent)]
        async fn holding_again<const N: usize>(&self) -> Answer<usize> {
            let buffer = [7_u8; N];
            tokio::task::yield_now().await;
            Ok(std::hint::black_box(&buffer).len())
        }

        async fn taking<const N: usize>(&self, buffer: [u8; N]) -> Answer<usize> {
            tokio::task::yield_now().await;
            Ok(std::hint::black_box(&buffer).len())
        }

        #[resilient(courier)]
        async fn taking_once<const N: usize>(&self, buffer: [u8; N]) -> Answer<usize> {
            tokio::task::yield_now().await;
            Ok(std::hint::black_box(&buffer).len())
        }

        #[resilient(courier, idempotent)]
        async fn taking_again<const N: usize>(&self, buffer: [u8; N]) -> Answer<usize> {
            tokio::task::yield_now().await;
            Ok(std::hint::black_box(&buffer).len())
        }

        #[resilient(courier)]
        async fn answering_once(&self) -> Answer<usize> {
            Ok(7)
        }

        #[resilient(courier, idempotent)]
        async fn answering_again(&self) -> Answer<usize> {
            Ok(7)
        }
    }

    #[module(
        imports = [ResilienceModule::for_root((Courier, quiet()))],
        providers = [BuffersService],
    )]
    struct BuffersModule;

    /// Each shape of a method holding or taking `N` bytes: the size of its
    /// future decorated and undecorated, and what a debug build holds outside
    /// its body's run — no buffer the body holds, the argument once that a
    /// method run once moves into its body's closure, and the arguments as the
    /// undecorated method holds them that a method run again lends each
    /// attempt.
    fn shapes<const N: usize>(
        buffers: &BuffersService,
    ) -> [(&'static str, usize, usize, usize, usize); 4] {
        let holding = size_of_val(&buffers.holding::<N>());
        let taking = size_of_val(&buffers.taking::<N>([7; N]));
        [
            (
                "holding a buffer across an `.await`, run once",
                N,
                size_of_val(&buffers.holding_once::<N>()),
                holding,
                0,
            ),
            (
                "holding a buffer across an `.await`, run again",
                N,
                size_of_val(&buffers.holding_again::<N>()),
                holding,
                0,
            ),
            (
                "taking a buffer by value, run once",
                N,
                size_of_val(&buffers.taking_once::<N>([7; N])),
                taking,
                N,
            ),
            (
                "taking a buffer by value, run again",
                N,
                size_of_val(&buffers.taking_again::<N>([7; N])),
                taking,
                taking,
            ),
        ]
    }

    /// A release build holds a decorated method's body and each argument once,
    /// in place, beside what its run holds whatever its body; a debug build
    /// boxes the body's closure and its future, so the method holds no more
    /// than its arguments.
    #[tokio::test]
    async fn a_decorated_methods_future_holds_its_body_and_each_argument_once() {
        let app = boot::<BuffersModule>().await;
        let buffers = app.container().get::<BuffersService>().expect("provided");
        for (shape, bytes, decorated, twin, own) in shapes::<HELD>(&buffers)
            .into_iter()
            .chain(shapes::<UNDER>(&buffers))
        {
            if cfg!(debug_assertions) {
                assert!(
                    decorated <= own + RUN,
                    "in a debug build, a method {shape}, of {bytes} B, is a future of {decorated} \
                     B decorated, holding {own} B outside its run"
                );
            } else {
                assert!(
                    twin <= decorated && decorated <= twin + RUN,
                    "in a release build, a method {shape}, of {bytes} B, is a future of \
                     {decorated} B decorated, of {twin} B undecorated"
                );
            }
        }
    }

    /// tokio's default worker stack, which `#[nest_rs::main]`'s runtime keeps.
    const WORKER_STACK: usize = 2 * 1024 * 1024;

    /// What the methods below hold across their `.await` on that stack: a
    /// third of it, which the undecorated method holds there too.
    const HELD_ON_A_WORKER: usize = WORKER_STACK / 3;

    /// A stack overflow aborts the process: nextest, which runs each test in a
    /// process of its own, reports this one failed on `SIGABRT` and runs the
    /// rest.
    #[test]
    fn a_decorated_method_holding_a_large_buffer_across_an_await_runs_on_a_worker_its_twin_runs_on()
    {
        // A runtime of the test's own, for its worker's stack.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_stack_size(WORKER_STACK)
            .enable_all()
            .build()
            .expect("the runtime builds");
        runtime.block_on(async {
            let app = boot::<BuffersModule>().await;
            let buffers = app.container().get::<BuffersService>().expect("provided");
            let twin = tokio::spawn({
                let buffers = Arc::clone(&buffers);
                async move { buffers.holding::<HELD_ON_A_WORKER>().await }
            });
            assert!(matches!(twin.await, Ok(Ok(HELD_ON_A_WORKER))));
            let once = tokio::spawn({
                let buffers = Arc::clone(&buffers);
                async move { buffers.holding_once::<HELD_ON_A_WORKER>().await }
            });
            assert!(matches!(once.await, Ok(Ok(HELD_ON_A_WORKER))));
            let again =
                tokio::spawn(async move { buffers.holding_again::<HELD_ON_A_WORKER>().await });
            assert!(matches!(again.await, Ok(Ok(HELD_ON_A_WORKER))));
        });
    }

    /// What the methods below take by value on that stack: three thirty-seconds
    /// of it. Run once, a decorated method moves what it takes through its
    /// body's closure and into its future, each copy a debug build's stack holds
    /// a moment, where its twin moves it once.
    const TAKEN_ON_A_WORKER: usize = WORKER_STACK * 3 / 32;

    /// A stack overflow aborts the process, as above.
    #[test]
    fn a_decorated_method_taking_a_large_buffer_by_value_runs_on_a_worker_of_tokios_default_stack()
    {
        // A runtime of the test's own, for its worker's stack.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_stack_size(WORKER_STACK)
            .enable_all()
            .build()
            .expect("the runtime builds");
        runtime.block_on(async {
            let app = boot::<BuffersModule>().await;
            let buffers = app.container().get::<BuffersService>().expect("provided");
            let once = tokio::spawn({
                let buffers = Arc::clone(&buffers);
                async move {
                    buffers
                        .taking_once::<TAKEN_ON_A_WORKER>([7; TAKEN_ON_A_WORKER])
                        .await
                }
            });
            assert!(matches!(once.await, Ok(Ok(TAKEN_ON_A_WORKER))));
            let again = tokio::spawn(async move {
                buffers
                    .taking_again::<TAKEN_ON_A_WORKER>([7; TAKEN_ON_A_WORKER])
                    .await
            });
            assert!(matches!(again.await, Ok(Ok(TAKEN_ON_A_WORKER))));
        });
    }

    /// As a run's under tokio's threshold, a decorated method's call allocates
    /// nothing of its own in a release build. A debug build boxes its future,
    /// and its body's closure, which allocates only when it holds something:
    /// run again it borrows the method's own.
    #[tokio::test]
    #[cfg_attr(
        not(target_os = "linux"),
        ignore = "pins the allocations a run makes on Linux, where the dev container and CI run it"
    )]
    async fn a_decorated_method_answering_at_once_allocates_nothing_but_its_token_in_a_release_build()
     {
        let app = boot::<BuffersModule>().await;
        let buffers = app.container().get::<BuffersService>().expect("provided");
        let token = crate::token_allocations();
        let debug = usize::from(cfg!(debug_assertions));
        for (shape, boxed, (answered, made)) in [
            (
                "run once",
                debug,
                crate::allocations(|| buffers.answering_once()),
            ),
            (
                "run again",
                2 * debug,
                crate::allocations(|| buffers.answering_again()),
            ),
        ] {
            assert!(matches!(answered, Ok(7)), "{shape}: {answered:?}");
            assert_eq!(
                made,
                token + boxed,
                "a method {shape} allocates its token, {token}, and in a debug build {boxed} box(es)"
            );
        }
    }
}

/// What the dependency answered is told where an operator reads it: the chain
/// of the error an attempt answered rides the retry's line and the breaker's
/// opening, so a run ending on the breaker's refusal still leaves its cause.
mod told {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("the label printer refused the job")]
    struct PrinterDown(#[source] std::io::Error);

    const CHAIN: &str = "the label printer refused the job: connection reset by peer";

    #[injectable]
    struct LabelsService {
        #[inject]
        courier: Arc<ResiliencePolicy<Courier>>,
    }

    impl LabelsService {
        #[resilient(courier, idempotent)]
        async fn print(&self) -> Result<(), ResilienceError<PrinterDown>> {
            Err(PrinterDown(std::io::Error::other(
                "connection reset by peer",
            )))
        }
    }

    #[module(
        imports = [ResilienceModule::for_root((Courier, ResilienceConfig {
            retries: Some(2),
            breaker_window: Some(20),
            breaker_failures: 10,
            ..ResilienceConfig::default()
        }))],
        providers = [LabelsService],
    )]
    struct LabelsModule;

    fn labels(app: &HeadlessApp) -> Arc<LabelsService> {
        app.container()
            .get::<LabelsService>()
            .expect("the module provides it")
    }

    #[tokio::test(start_paused = true)]
    async fn a_retrys_line_carries_the_chain_of_the_error_its_attempt_answered() {
        let app = boot::<LabelsModule>().await;
        let labels = labels(&app);
        let logs = LogCapture::install();

        let answered = labels.print().await;
        assert!(
            matches!(answered, Err(ResilienceError::Failed(_))),
            "{answered:?}"
        );
        let retried = logs.find(
            nest_rs_resilience::TARGET,
            "attempt failed; retrying the call",
        );
        assert_eq!(retried.len(), 2, "{retried:#?}");
        for line in &retried {
            assert_eq!(line.field("error").as_deref(), Some(CHAIN), "{line:?}");
        }
    }

    /// Fifty idempotent callers of a dependency that is down: the breaker
    /// opens on the first ten failures, and every caller whose retry it then
    /// refuses answers `Open`. The opening's `warn` carries the chain of the
    /// failure that opened it, the one line a production filter keeps.
    #[tokio::test(start_paused = true)]
    async fn the_breakers_opening_carries_the_chain_of_the_failure_that_opened_it() {
        let app = boot::<LabelsModule>().await;
        let labels = labels(&app);
        let logs = LogCapture::install();

        let callers: Vec<_> = (0..50)
            .map(|_| {
                let labels = Arc::clone(&labels);
                tokio::spawn(async move { labels.print().await })
            })
            .collect();
        let mut refused = 0;
        for caller in callers {
            match caller.await.expect("no caller panics") {
                Err(ResilienceError::Open { .. }) => refused += 1,
                Err(ResilienceError::Failed(_)) => {}
                other => panic!("a down dependency fails or is refused: {other:?}"),
            }
        }
        assert!(refused > 0, "the breaker opened under its callers");

        let opened = logs
            .find(nest_rs_resilience::TARGET, "circuit breaker changed state")
            .into_iter()
            .find(|line| line.field("state").as_deref() == Some("open"))
            .expect("the breaker's opening is filed");
        assert_eq!(opened.field("error").as_deref(), Some(CHAIN), "{opened:?}");
    }

    /// What the tracker's URL carries, in its path and in its query.
    const SENT: &str = "s3cr3t";

    /// An app's error holding reqwest's as it came, its URL not taken out.
    #[derive(Debug, thiserror::Error)]
    #[error("the tracker could not be reached")]
    struct TrackerDown(#[from] reqwest::Error);

    #[injectable]
    struct TrackingService {
        #[inject]
        courier: Arc<ResiliencePolicy<Courier>>,
    }

    impl TrackingService {
        /// A call to a port nothing listens on.
        #[resilient(courier, idempotent)]
        async fn track(&self) -> Result<String, ResilienceError<TrackerDown>> {
            let url = format!("https://127.0.0.1:1/parcels/{SENT}?token={SENT}");
            Ok(reqwest::get(url).await?.text().await?)
        }
    }

    #[module(
        imports = [ResilienceModule::for_root((Courier, ResilienceConfig {
            retries: Some(1),
            backoff_first: Duration::from_millis(1),
            backoff_ceiling: Duration::from_millis(2),
            breaker_window: Some(2),
            breaker_failures: 2,
            ..ResilienceConfig::default()
        }))],
        providers = [TrackingService],
    )]
    struct TrackingModule;

    /// reqwest's sentence quotes the URL a request was sent to, and an app's
    /// error may hold reqwest's as it came: the retry's line and the breaker's
    /// opening say its chain all the same, without that URL.
    #[tokio::test]
    async fn a_reqwest_error_an_app_wrapped_with_its_url_is_told_without_it() {
        let app = boot::<TrackingModule>().await;
        let tracking = app
            .container()
            .get::<TrackingService>()
            .expect("the module provides it");
        let logs = LogCapture::install();

        let answered = tracking.track().await;
        let Err(ResilienceError::Failed(TrackerDown(failed))) = &answered else {
            panic!("both attempts are refused the connection: {answered:?}");
        };
        assert!(
            failed.to_string().contains(SENT),
            "the error holds its URL: {failed}"
        );

        let retried = logs.expect_one(
            nest_rs_resilience::TARGET,
            "attempt failed; retrying the call",
        );
        let opened = logs
            .find(nest_rs_resilience::TARGET, "circuit breaker changed state")
            .into_iter()
            .find(|line| line.field("state").as_deref() == Some("open"))
            .expect("the second failure opened the breaker");
        for line in [&retried, &opened] {
            let error = line.field("error").unwrap_or_default();
            assert!(
                error.starts_with("the tracker could not be reached: error sending request: "),
                "the app's sentence, reqwest's, then its causes: {line:?}"
            );
            for quoted in [SENT, "/parcels", "for url"] {
                assert!(!line.quotes(quoted), "`{quoted}` in {line:?}");
            }
        }
        assert_eq!(
            retried.field("error"),
            opened.field("error"),
            "one failure, said the same in both"
        );
    }

    /// An app's error handing reqwest's sentence on as its own: reqwest's
    /// error is then no link of its chain, and no driver's `SafeLink` reads it.
    #[derive(Debug, thiserror::Error)]
    enum TrackError {
        #[error(transparent)]
        Http(#[from] reqwest::Error),
    }

    #[injectable]
    struct HandingOnService {
        #[inject]
        courier: Arc<ResiliencePolicy<Courier>>,
    }

    impl HandingOnService {
        /// A call to a port nothing listens on.
        #[resilient(courier, idempotent)]
        async fn track(&self) -> Result<String, ResilienceError<TrackError>> {
            let url = format!("https://user:{SENT}@127.0.0.1:1/parcels/{SENT}?token={SENT}");
            Ok(reqwest::get(url).await?.text().await?)
        }
    }

    #[module(
        imports = [ResilienceModule::for_root((Courier, ResilienceConfig {
            retries: Some(1),
            backoff_first: Duration::from_millis(1),
            backoff_ceiling: Duration::from_millis(2),
            breaker_window: Some(2),
            breaker_failures: 2,
            ..ResilienceConfig::default()
        }))],
        providers = [HandingOnService],
    )]
    struct HandingOnModule;

    /// Whatever wrote the sentence, the retry's line and the breaker's opening
    /// tell a URL by its origin alone: the dependency reached, never the path,
    /// the query or the userinfo its request carried.
    #[tokio::test]
    async fn a_url_an_apps_error_hands_on_as_its_own_sentence_is_told_by_its_origin() {
        let app = boot::<HandingOnModule>().await;
        let tracking = app
            .container()
            .get::<HandingOnService>()
            .expect("the module provides it");
        let logs = LogCapture::install();

        let answered = tracking.track().await;
        let Err(ResilienceError::Failed(failed)) = &answered else {
            panic!("both attempts are refused the connection: {answered:?}");
        };
        assert!(
            nest_rs_core::error_message(failed).contains(SENT),
            "the error's chain quotes its URL: {failed}"
        );

        let retried = logs.expect_one(
            nest_rs_resilience::TARGET,
            "attempt failed; retrying the call",
        );
        let opened = logs
            .find(nest_rs_resilience::TARGET, "circuit breaker changed state")
            .into_iter()
            .find(|line| line.field("state").as_deref() == Some("open"))
            .expect("the second failure opened the breaker");
        for line in [&retried, &opened] {
            let error = line.field("error").unwrap_or_default();
            assert!(
                error.starts_with("error sending request for url (https://127.0.0.1:1): "),
                "reqwest's sentence, its URL cut to its origin, then its causes: {line:?}"
            );
            for quoted in [SENT, "/parcels", "token", "user"] {
                assert!(!line.quotes(quoted), "`{quoted}` in {line:?}");
            }
        }
    }
}

/// `#[tracing::instrument]` beside `#[resilient]`, in either order: written
/// above, tracing expands first and its span covers one attempt; written
/// below, the span covers the whole run. Each flavour of the instrument that
/// changes what it expands to is run in both orders, held to the attempts it
/// makes, the spans it opens and what each records.
mod instrumented {
    use super::*;

    #[injectable]
    struct TracedService {
        #[inject]
        courier: Arc<ResiliencePolicy<Courier>>,
        attempts: AtomicU32,
        hub: Hub,
    }

    /// One method written twice, `$above` under an instrument written above
    /// its policy and `$below` under the same one written below it. The
    /// method's tokens reach both as written, so each is the method a
    /// developer types.
    macro_rules! in_both_orders {
        (
            #[tracing::instrument($($traced:tt)*)]
            #[resilient($($policy:tt)*)]
            async fn $above:ident, $below:ident $($method:tt)*
        ) => {
            #[tracing::instrument($($traced)*)]
            #[resilient($($policy)*)]
            async fn $above $($method)*

            #[resilient($($policy)*)]
            #[tracing::instrument($($traced)*)]
            async fn $below $($method)*
        };
    }

    impl TracedService {
        fn fails_first(&self) -> bool {
            self.attempts.fetch_add(1, Ordering::Relaxed) == 0
        }

        fn attempted(&self) -> u32 {
            self.attempts.swap(0, Ordering::Relaxed)
        }

        #[tracing::instrument(name = "above", skip_all)]
        #[resilient(courier, idempotent)]
        async fn above(&self, parcel: String) -> Answer<String> {
            if self.fails_first() {
                return Err(CarrierDown);
            }
            Ok(parcel.clone())
        }

        #[resilient(courier, idempotent)]
        #[tracing::instrument(name = "below", skip_all)]
        async fn below(&self, parcel: String) -> Answer<String> {
            if self.fails_first() {
                return Err(CarrierDown);
            }
            Ok(parcel.clone())
        }

        /// Run once, consuming its argument, its error recorded by the span.
        #[tracing::instrument(name = "above_once", skip_all, err)]
        #[resilient(courier)]
        async fn above_once(&self, parcel: String) -> Answer<String> {
            depot(&parcel)?;
            Ok(parcel)
        }

        /// Two instruments above the policy, one inside the other: each
        /// attempt opens both.
        #[tracing::instrument(name = "outer", skip_all)]
        #[tracing::instrument(name = "inner", skip_all, err)]
        #[resilient(courier, idempotent)]
        async fn twice_above(&self, parcel: String) -> Answer<String> {
            if self.fails_first() {
                return Err(CarrierDown);
            }
            Ok(parcel.clone())
        }

        in_both_orders! {
            #[tracing::instrument(skip_all, err)]
            #[resilient(courier, idempotent)]
            async fn err_above, err_below(&self, parcel: String) -> Answer<String> {
                if self.fails_first() {
                    return Err(CarrierDown);
                }
                depot(&parcel)?;
                Ok(parcel.clone())
            }
        }

        in_both_orders! {
            #[tracing::instrument(skip_all, ret)]
            #[resilient(courier, idempotent)]
            async fn ret_above, ret_below(&self, parcel: String) -> Answer<String> {
                if self.fails_first() {
                    return Err(CarrierDown);
                }
                Ok(parcel.clone())
            }
        }

        in_both_orders! {
            #[tracing::instrument(skip_all, err, ret)]
            #[resilient(courier, idempotent)]
            async fn both_above, both_below(&self, parcel: String) -> Answer<String> {
                if self.fails_first() {
                    return Err(CarrierDown);
                }
                Ok(parcel.clone())
            }
        }

        // An owned argument the span records and each attempt reads again,
        // beside a field computed from one.
        in_both_orders! {
            #[tracing::instrument(skip(self, zone), fields(weight = parcel.len()))]
            #[resilient(courier, idempotent)]
            async fn fields_above, fields_below(&self, parcel: String, zone: &str) -> Answer<String> {
                if self.fails_first() {
                    return Err(CarrierDown);
                }
                Ok(format!("{parcel} to {zone}"))
            }
        }

        // Run once: the one attempt consumes the argument it was moved.
        in_both_orders! {
            #[tracing::instrument(skip_all, err, ret)]
            #[resilient(courier)]
            async fn once_above, once_below(&self, parcel: String) -> Answer<String> {
                self.attempts.fetch_add(1, Ordering::Relaxed);
                depot(&parcel)?;
                Ok(parcel)
            }
        }

        in_both_orders! {
            #[tracing::instrument(skip_all, ret)]
            #[resilient(courier, idempotent)]
            async fn generic_above, generic_below<T>(&self, parcel: T) -> Answer<T>
            where
                T: std::fmt::Debug + Clone + Sync,
            {
                if self.fails_first() {
                    return Err(CarrierDown);
                }
                Ok(parcel.clone())
            }
        }

        // The answer borrows its host or its argument through a lifetime the
        // method names.
        in_both_orders! {
            #[tracing::instrument(skip_all, err)]
            #[resilient(courier, idempotent)]
            async fn borrowed_above, borrowed_below<'a>(&'a self, zone: &'a str) -> Answer<&'a str> {
                if self.fails_first() {
                    return Err(CarrierDown);
                }
                Ok(if zone.is_empty() { &self.hub.0 } else { zone })
            }
        }

        /// The parcel held at the hub, in place of the error its run ended on.
        async fn held(
            &self,
            parcel: String,
            _error: ResilienceError<CarrierDown>,
        ) -> Answer<String> {
            Ok(format!("{parcel} held"))
        }

        // Every other key: each predicate, a fallback handed the argument
        // each attempt read, and the attempt's token read in the body.
        in_both_orders! {
            #[tracing::instrument(skip_all, err)]
            #[resilient(
                courier,
                idempotent,
                retry_if = |_| true,
                retry_ok_if = String::is_empty,
                record_if = |_| true,
                fallback = held,
                fallback_if = |_| true,
                cancel = token
            )]
            async fn keyed_above, keyed_below(&self, parcel: String) -> Answer<String> {
                self.attempts.fetch_add(1, Ordering::Relaxed);
                if token.is_cancelled() {
                    return Ok(String::new());
                }
                depot(&parcel)?;
                Ok(parcel.clone())
            }
        }

        // Sent once, its one attempt lending the argument its fallback takes.
        in_both_orders! {
            #[tracing::instrument(skip_all, ret)]
            #[resilient(courier, fallback = held, cancel = token)]
            async fn lent_above, lent_below(&self, parcel: String) -> Answer<String> {
                let _attempt = token;
                depot(&parcel)?;
                Ok(parcel.clone())
            }
        }

        // Every predicate reading the argument the method owns, which its
        // fallback is then handed by value.
        in_both_orders! {
            #[tracing::instrument(skip_all)]
            #[resilient(
                courier,
                idempotent,
                retry_if = |_| parcel != "P0",
                retry_ok_if = |held: &String| held.len() > parcel.len(),
                record_if = |_| !parcel.is_empty(),
                fallback = held,
                fallback_if = |_| parcel != "P0",
                cancel = token
            )]
            async fn read_above, read_below(&self, parcel: String) -> Answer<String> {
                self.attempts.fetch_add(1, Ordering::Relaxed);
                drop(token);
                depot(&parcel)?;
                Ok(parcel.clone())
            }
        }

        // Sent once under a predicate reading the argument its one attempt
        // lends it.
        in_both_orders! {
            #[tracing::instrument(skip_all)]
            #[resilient(courier, record_if = |_| !parcel.is_empty(), cancel = token)]
            async fn read_once_above, read_once_below(&self, parcel: String) -> Answer<String> {
                drop(token);
                depot(&parcel)?;
                Ok(parcel.clone())
            }
        }

        // Sent once under a predicate reading no argument: its one attempt
        // takes the argument it consumes, and its token.
        in_both_orders! {
            #[tracing::instrument(skip_all)]
            #[resilient(courier, record_if = |_| false, cancel = token)]
            async fn owned_above, owned_below(&self, parcel: String) -> Answer<String> {
                drop(token);
                depot(&parcel)?;
                Ok(parcel)
            }
        }
    }

    #[module(
        imports = [ResilienceModule::for_root((Courier, retrying_once()))],
        providers = [TracedService],
    )]
    struct TracedModule;

    /// What the instruments of this module filed while `Opened` is the default
    /// subscriber: each span opened, by its name and the fields it declares,
    /// and each event an `err` or a `ret` recorded, as its field and its value.
    #[derive(Default)]
    struct Filed {
        spans: Vec<(&'static str, Vec<&'static str>)>,
        events: Vec<(&'static str, String)>,
    }

    #[derive(Clone, Default)]
    struct Opened(Arc<Mutex<Filed>>);

    impl Opened {
        fn named(&self, name: &str) -> usize {
            self.fields(name).len()
        }

        /// The fields the spans named `name` declare, one list a span.
        fn fields(&self, name: &str) -> Vec<Vec<&'static str>> {
            let filed = self.0.lock().expect("unpoisoned");
            let named = filed.spans.iter().filter(|(opened, _)| *opened == name);
            named.map(|(_, fields)| fields.clone()).collect()
        }

        /// The events recorded since the last read, oldest first.
        fn recorded(&self) -> Vec<(&'static str, String)> {
            std::mem::take(&mut self.0.lock().expect("unpoisoned").events)
        }
    }

    impl tracing::field::Visit for Opened {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            let mut filed = self.0.lock().expect("unpoisoned");
            filed.events.push((field.name(), format!("{value:?}")));
        }
    }

    impl tracing::Subscriber for Opened {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            let fields = span.metadata().fields().iter().map(|field| field.name());
            let mut filed = self.0.lock().expect("unpoisoned");
            filed.spans.push((span.metadata().name(), fields.collect()));
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

        /// The policy files lines of its own, on its own target: only what an
        /// instrument of this module records is kept.
        fn event(&self, event: &tracing::Event<'_>) {
            if event.metadata().target() == module_path!() {
                event.record(&mut self.clone());
            }
        }

        fn enter(&self, _: &tracing::span::Id) {}

        fn exit(&self, _: &tracing::span::Id) {}
    }

    async fn traced() -> (HeadlessApp, Arc<TracedService>) {
        let app = boot::<TracedModule>().await;
        let traced = app
            .container()
            .get::<TracedService>()
            .expect("the module provides it");
        (app, traced)
    }

    const DOWN: &str = "the carrier did not answer";

    #[tokio::test(start_paused = true)]
    async fn a_method_instrumented_above_or_below_its_policy_runs_under_it() {
        let (_app, traced) = traced().await;
        let opened = Opened::default();
        let _default = tracing::subscriber::set_default(opened.clone());

        let above = traced.above("P1".to_owned()).await;
        assert!(matches!(above.as_deref(), Ok("P1")), "{above:?}");
        assert_eq!(traced.attempted(), 2, "sent again after its failure");
        assert_eq!(opened.named("above"), 2, "a span an attempt");

        let below = traced.below("P1".to_owned()).await;
        assert!(matches!(below.as_deref(), Ok("P1")), "{below:?}");
        assert_eq!(traced.attempted(), 2, "sent again after its failure");
        assert_eq!(opened.named("below"), 1, "one span for the whole run");

        let once = traced.above_once("P1".to_owned()).await;
        assert!(matches!(once.as_deref(), Ok("P1")), "{once:?}");
        let unknown = traced.above_once("P2".to_owned()).await;
        assert!(failed(&unknown), "{unknown:?}");
        assert_eq!(opened.named("above_once"), 2);
        assert_eq!(traced.attempted(), 0, "`above_once` counts no attempt");
        assert_eq!(opened.recorded(), [("error", DOWN.to_owned())]);

        let twice = traced.twice_above("P1".to_owned()).await;
        assert!(matches!(twice.as_deref(), Ok("P1")), "{twice:?}");
        assert_eq!(traced.attempted(), 2, "sent again after its failure");
        assert_eq!(opened.named("outer"), 2, "a span an attempt");
        assert_eq!(opened.named("inner"), 2, "a span an attempt");
        assert_eq!(opened.recorded(), [("error", DOWN.to_owned())]);
    }

    /// Above, `err` records what an attempt answered, the dependency's own
    /// error; below, what the run answered, which here is no error.
    #[tokio::test(start_paused = true)]
    async fn an_err_above_records_each_attempts_error_and_below_the_runs() {
        let (_app, traced) = traced().await;
        let opened = Opened::default();
        let _default = tracing::subscriber::set_default(opened.clone());

        let above = traced.err_above("P1".to_owned()).await;
        assert!(matches!(above.as_deref(), Ok("P1")), "{above:?}");
        assert_eq!(traced.attempted(), 2);
        assert_eq!(opened.named("err_above"), 2, "a span an attempt");
        assert_eq!(opened.recorded(), [("error", DOWN.to_owned())]);

        let below = traced.err_below("P1".to_owned()).await;
        assert!(matches!(below.as_deref(), Ok("P1")), "{below:?}");
        assert_eq!(traced.attempted(), 2);
        assert_eq!(opened.named("err_below"), 1, "one span for the whole run");
        assert_eq!(opened.recorded(), [], "the run answered");

        // A `?` converts its error into the attempt's in both orders, and a
        // run ending on it is the one error the span below records.
        let above = traced.err_above("P2".to_owned()).await;
        assert!(failed(&above), "{above:?}");
        assert_eq!(traced.attempted(), 2);
        assert_eq!(
            opened.recorded(),
            [("error", DOWN.to_owned()), ("error", DOWN.to_owned())]
        );
        let below = traced.err_below("P2".to_owned()).await;
        assert!(failed(&below), "{below:?}");
        assert_eq!(traced.attempted(), 2);
        let run = below.expect_err("failed").to_string();
        assert_eq!(opened.recorded(), [("error", run)]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_ret_above_records_each_attempts_answer_and_below_the_runs() {
        let (_app, traced) = traced().await;
        let opened = Opened::default();
        let _default = tracing::subscriber::set_default(opened.clone());

        let above = traced.ret_above("P1".to_owned()).await;
        assert!(matches!(above.as_deref(), Ok("P1")), "{above:?}");
        assert_eq!(traced.attempted(), 2);
        assert_eq!(opened.named("ret_above"), 2, "a span an attempt");
        assert_eq!(
            opened.recorded(),
            [
                ("return", "Err(CarrierDown)".to_owned()),
                ("return", r#"Ok("P1")"#.to_owned()),
            ]
        );

        let below = traced.ret_below("P1".to_owned()).await;
        assert!(matches!(below.as_deref(), Ok("P1")), "{below:?}");
        assert_eq!(traced.attempted(), 2);
        assert_eq!(opened.named("ret_below"), 1, "one span for the whole run");
        assert_eq!(opened.recorded(), [("return", r#"Ok("P1")"#.to_owned())]);
    }

    #[tokio::test(start_paused = true)]
    async fn an_err_and_a_ret_above_record_each_attempt_and_below_the_run() {
        let (_app, traced) = traced().await;
        let opened = Opened::default();
        let _default = tracing::subscriber::set_default(opened.clone());

        let above = traced.both_above("P1".to_owned()).await;
        assert!(matches!(above.as_deref(), Ok("P1")), "{above:?}");
        assert_eq!(traced.attempted(), 2);
        assert_eq!(opened.named("both_above"), 2, "a span an attempt");
        assert_eq!(
            opened.recorded(),
            [("error", DOWN.to_owned()), ("return", r#""P1""#.to_owned())]
        );

        let below = traced.both_below("P1".to_owned()).await;
        assert!(matches!(below.as_deref(), Ok("P1")), "{below:?}");
        assert_eq!(traced.attempted(), 2);
        assert_eq!(opened.named("both_below"), 1, "one span for the whole run");
        assert_eq!(opened.recorded(), [("return", r#""P1""#.to_owned())]);
    }

    /// The span's fields are read off the arguments as each attempt holds
    /// them: skipped, recorded by name, or computed from one.
    #[tokio::test(start_paused = true)]
    async fn a_span_reads_the_arguments_it_records_at_each_attempt() {
        let (_app, traced) = traced().await;
        let opened = Opened::default();
        let _default = tracing::subscriber::set_default(opened.clone());

        let above = traced.fields_above("P1".to_owned(), "north").await;
        assert!(matches!(above.as_deref(), Ok("P1 to north")), "{above:?}");
        assert_eq!(traced.attempted(), 2);
        assert_eq!(
            opened.fields("fields_above"),
            [["parcel", "weight"], ["parcel", "weight"]],
            "a span an attempt"
        );

        let below = traced.fields_below("P1".to_owned(), "north").await;
        assert!(matches!(below.as_deref(), Ok("P1 to north")), "{below:?}");
        assert_eq!(traced.attempted(), 2);
        assert_eq!(
            opened.fields("fields_below"),
            [["parcel", "weight"]],
            "one span for the whole run"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_method_run_once_consumes_its_argument_under_an_instrument_in_either_order() {
        let (_app, traced) = traced().await;
        let opened = Opened::default();
        let _default = tracing::subscriber::set_default(opened.clone());

        let above = traced.once_above("P1".to_owned()).await;
        assert!(matches!(above.as_deref(), Ok("P1")), "{above:?}");
        let unknown = traced.once_above("P2".to_owned()).await;
        assert!(failed(&unknown), "{unknown:?}");
        assert_eq!(traced.attempted(), 2, "each call sent once");
        assert_eq!(opened.named("once_above"), 2, "a span a call");
        assert_eq!(
            opened.recorded(),
            [("return", r#""P1""#.to_owned()), ("error", DOWN.to_owned())]
        );

        let below = traced.once_below("P1".to_owned()).await;
        assert!(matches!(below.as_deref(), Ok("P1")), "{below:?}");
        let unknown = traced.once_below("P2".to_owned()).await;
        assert!(failed(&unknown), "{unknown:?}");
        assert_eq!(traced.attempted(), 2, "each call sent once");
        assert_eq!(opened.named("once_below"), 2, "a span a call");
        let run = unknown.expect_err("failed").to_string();
        assert_eq!(
            opened.recorded(),
            [("return", r#""P1""#.to_owned()), ("error", run)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_generic_method_and_one_borrowing_through_a_lifetime_run_in_either_order() {
        let (_app, traced) = traced().await;
        let opened = Opened::default();
        let _default = tracing::subscriber::set_default(opened.clone());

        assert!(matches!(traced.generic_above(7_u32).await, Ok(7)));
        assert_eq!(traced.attempted(), 2);
        assert_eq!(opened.named("generic_above"), 2, "a span an attempt");
        assert!(matches!(
            traced.generic_below(("P1", 7_u8)).await,
            Ok(("P1", 7))
        ));
        assert_eq!(traced.attempted(), 2);
        assert_eq!(
            opened.named("generic_below"),
            1,
            "one span for the whole run"
        );

        let zone = "north".to_owned();
        assert!(matches!(traced.borrowed_above(&zone).await, Ok("north")));
        assert_eq!(traced.attempted(), 2);
        assert_eq!(opened.named("borrowed_above"), 2, "a span an attempt");
        assert!(matches!(
            traced.borrowed_below("").await,
            Ok("the Lyon hub")
        ));
        assert_eq!(traced.attempted(), 2);
        assert_eq!(
            opened.named("borrowed_below"),
            1,
            "one span for the whole run"
        );
    }

    /// An instrumented attempt holds its span and its arguments across its
    /// waits as the method's own future does: it runs on another task.
    #[tokio::test(start_paused = true)]
    async fn a_method_instrumented_above_its_policy_runs_on_a_task_of_its_own() {
        let (_app, traced) = traced().await;
        let sent = Arc::clone(&traced);
        let above = tokio::spawn(async move { sent.err_above("P1".to_owned()).await });
        let above = above.await.expect("no attempt panics");
        assert!(matches!(above.as_deref(), Ok("P1")), "{above:?}");
        let sent = Arc::clone(&traced);
        let once = tokio::spawn(async move { sent.once_above("P1".to_owned()).await });
        let once = once.await.expect("the attempt does not panic");
        assert!(matches!(once.as_deref(), Ok("P1")), "{once:?}");
    }

    /// Both orders hold under every other key. Above, each attempt opens a
    /// span recording what that attempt answered, and the fallback runs
    /// outside them all; below, one span covers the run and its fallback, and
    /// records what the method answers.
    #[tokio::test(start_paused = true)]
    async fn a_method_carrying_every_other_key_runs_under_an_instrument_in_either_order() {
        let (_app, traced) = traced().await;
        let opened = Opened::default();
        let _default = tracing::subscriber::set_default(opened.clone());

        let answered = traced.keyed_above("P1".to_owned()).await;
        assert!(matches!(answered.as_deref(), Ok("P1")), "{answered:?}");
        assert_eq!(traced.attempted(), 1);
        let above = traced.keyed_above("P2".to_owned()).await;
        assert!(matches!(above.as_deref(), Ok("P2 held")), "{above:?}");
        assert_eq!(traced.attempted(), 2, "sent again, then its fallback");
        assert_eq!(opened.named("keyed_above"), 3, "a span an attempt");
        assert_eq!(
            opened.recorded(),
            [("error", DOWN.to_owned()), ("error", DOWN.to_owned())],
            "each attempt's own error"
        );

        let below = traced.keyed_below("P2".to_owned()).await;
        assert!(matches!(below.as_deref(), Ok("P2 held")), "{below:?}");
        assert_eq!(traced.attempted(), 2);
        assert_eq!(opened.named("keyed_below"), 1, "one span for the whole run");
        assert_eq!(opened.recorded(), [], "the method answered its fallback's");

        let above = traced.lent_above("P2".to_owned()).await;
        assert!(matches!(above.as_deref(), Ok("P2 held")), "{above:?}");
        assert_eq!(opened.named("lent_above"), 1);
        assert_eq!(
            opened.recorded(),
            [("return", "Err(CarrierDown)".to_owned())],
            "the attempt's answer, its fallback outside the span"
        );
        let below = traced.lent_below("P2".to_owned()).await;
        assert!(matches!(below.as_deref(), Ok("P2 held")), "{below:?}");
        assert_eq!(opened.named("lent_below"), 1);
        assert_eq!(
            opened.recorded(),
            [("return", r#"Ok("P2 held")"#.to_owned())],
            "the method's answer, its fallback's"
        );

        let above = traced.owned_above("P1".to_owned()).await;
        assert!(matches!(above.as_deref(), Ok("P1")), "{above:?}");
        let below = traced.owned_below("P2".to_owned()).await;
        assert!(failed(&below), "{below:?}");
        assert_eq!(
            (opened.named("owned_above"), opened.named("owned_below")),
            (1, 1)
        );
    }

    /// Both orders hold where a predicate reads an argument the method owns:
    /// the future an instrument written above builds borrows it as the
    /// attempt does, and the fallback takes it once the filter read it.
    #[tokio::test(start_paused = true)]
    async fn a_predicate_reads_an_owned_argument_under_an_instrument_in_either_order() {
        let (_app, traced) = traced().await;
        for (parcel, answers, sent) in [
            ("P1", Ok("P1"), 1),
            ("P2", Ok("P2 held"), 2),
            ("P0", Err(()), 1),
        ] {
            let above = traced.read_above(parcel.to_owned()).await;
            assert_eq!(above.as_deref().map_err(drop), answers, "above: {parcel}");
            assert_eq!(traced.attempted(), sent, "above: {parcel}");
            let below = traced.read_below(parcel.to_owned()).await;
            assert_eq!(below.as_deref().map_err(drop), answers, "below: {parcel}");
            assert_eq!(traced.attempted(), sent, "below: {parcel}");
        }

        let above = traced.read_once_above("P1".to_owned()).await;
        assert!(matches!(above.as_deref(), Ok("P1")), "{above:?}");
        let below = traced.read_once_below("P2".to_owned()).await;
        assert!(failed(&below), "{below:?}");
        let sent = Arc::clone(&traced);
        let spawned = tokio::spawn(async move { sent.read_once_above("P1".to_owned()).await });
        let spawned = spawned.await.expect("the attempt does not panic");
        assert!(matches!(spawned.as_deref(), Ok("P1")), "{spawned:?}");
    }
}

/// What the predicate keys make of an attempt's answer: `retry_if` and
/// `retry_ok_if` say what is sent again, `record_if` what counts as a failure
/// in the breaker, each alone and together, a closure reading the method's
/// arguments and its host where it names them.
mod judged {
    use super::*;

    /// What the carrier answers a quote with when it gives none: its own
    /// answer or its failure, each final or worth another attempt.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
    enum NoQuote {
        #[error("the carrier is busy")]
        Busy,
        #[error("the carrier declined the parcel")]
        Declined,
        #[error("the carrier did not answer")]
        Down,
        #[error("the carrier closed the route")]
        Gone,
    }

    impl NoQuote {
        /// Whether another attempt may be answered a quote.
        fn transient(&self) -> bool {
            matches!(self, Self::Busy | Self::Down)
        }

        /// Whether the carrier failed, rather than answered.
        fn failed(&self) -> bool {
            matches!(self, Self::Down | Self::Gone)
        }
    }

    type Quoted = Result<u32, ResilienceError<NoQuote>>;

    #[injectable]
    struct QuotesService {
        #[inject]
        courier: Arc<ResiliencePolicy<Courier>>,
        attempts: AtomicU32,
    }

    impl QuotesService {
        /// The attempt about to run, counted from one.
        fn attempt(&self) -> u32 {
            self.attempts.fetch_add(1, Ordering::Relaxed) + 1
        }

        fn attempted(&self) -> u32 {
            self.attempts.swap(0, Ordering::Relaxed)
        }

        #[resilient(courier, idempotent, retry_if = NoQuote::transient)]
        async fn retried(&self, failure: NoQuote) -> Quoted {
            self.attempt();
            Err(failure)
        }

        #[resilient(courier, record_if = NoQuote::failed)]
        async fn recorded(&self, failure: NoQuote) -> Quoted {
            self.attempt();
            Err(failure)
        }

        #[resilient(
            courier,
            idempotent,
            retry_if = NoQuote::transient,
            record_if = |error| error.failed()
        )]
        async fn judged(&self, failure: NoQuote) -> Quoted {
            self.attempt();
            Err(failure)
        }

        /// A quote growing by `step` an attempt, asked again while it is under
        /// thirty.
        #[resilient(courier, idempotent, retry_ok_if = |quote| *quote < 30)]
        async fn polled(&self, step: u32) -> Quoted {
            Ok(self.attempt() * step)
        }

        /// Every predicate at once, one reading an argument and one the host:
        /// a quote is asked again under `floor`, an error once.
        #[resilient(
            courier,
            idempotent,
            retry_if = |error| error.transient() && self.attempts.load(Ordering::Relaxed) < 2,
            retry_ok_if = |quote| *quote < floor,
            record_if = NoQuote::failed,
        )]
        async fn bid(&self, floor: u32, failure: Option<NoQuote>) -> Quoted {
            let quote = self.attempt() * 10;
            match failure {
                Some(failure) => Err(failure),
                None => Ok(quote),
            }
        }

        /// Run once, a failure counted outside the carrier's own zone:
        /// `record_if` reads an argument the method owns, a generic one and
        /// the host, each once the one attempt that read them answered.
        #[resilient(
            courier,
            record_if = |error| {
                error.failed() && zone != home.as_ref() && self.attempts.load(Ordering::Relaxed) > 0
            }
        )]
        async fn recorded_in<H>(&self, zone: String, home: H, failure: NoQuote) -> Quoted
        where
            H: AsRef<str> + Send + Sync,
        {
            self.attempt();
            if zone.is_empty() || home.as_ref().is_empty() {
                return Ok(0);
            }
            Err(failure)
        }

        /// Sent again as the zone allows, each retry predicate reading an
        /// argument the method owns: an error outside `home`, a quote in
        /// `slow` under thirty.
        #[resilient(
            courier,
            idempotent,
            retry_if = |error| error.transient() && zone != "home",
            retry_ok_if = |quote| zone == "slow" && *quote < 30,
        )]
        async fn routed(&self, zone: String, failure: Option<NoQuote>) -> Quoted {
            let quote = self.attempt() * 10;
            match failure {
                Some(failure) if !zone.is_empty() => Err(failure),
                _ => Ok(quote),
            }
        }
    }

    /// Two retries a millisecond or two apart, and no breaker.
    fn retrying_twice() -> ResilienceConfig {
        ResilienceConfig {
            retries: Some(2),
            backoff_first: Duration::from_millis(1),
            backoff_ceiling: Duration::from_millis(2),
            breaker_window: None,
            ..ResilienceConfig::default()
        }
    }

    #[module(
        imports = [ResilienceModule::for_root((Courier, retrying_twice()))],
        providers = [QuotesService],
    )]
    struct RetriedModule;

    /// The same retries, under a breaker its one failure opens.
    #[module(
        imports = [ResilienceModule::for_root((Courier, ResilienceConfig {
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: Duration::from_secs(30),
            ..retrying_twice()
        }))],
        providers = [QuotesService],
    )]
    struct CountedModule;

    /// `M` booted, the service it provides and the policy that service runs
    /// under.
    async fn quotes<M: Module + 'static>() -> (
        HeadlessApp,
        Arc<QuotesService>,
        Arc<ResiliencePolicy<Courier>>,
    ) {
        let app = boot::<M>().await;
        let quotes = app
            .container()
            .get::<QuotesService>()
            .expect("the module provides it");
        let courier = held::<Courier>(&app);
        (app, quotes, courier)
    }

    #[tokio::test(start_paused = true)]
    async fn an_err_retry_if_refuses_ends_the_run_and_one_it_takes_is_sent_again() {
        let (_app, quotes, _courier) = quotes::<RetriedModule>().await;
        for (failure, sent) in [
            (NoQuote::Declined, 1),
            (NoQuote::Gone, 1),
            (NoQuote::Busy, 3),
            (NoQuote::Down, 3),
        ] {
            let answered = quotes.retried(failure).await;
            assert!(
                matches!(answered, Err(ResilienceError::Failed(last)) if last == failure),
                "the last attempt's error is the method's: {answered:?}"
            );
            assert_eq!(quotes.attempted(), sent, "{failure:?}");
        }
    }

    /// Run once, `record_if` alone decides: an `Err` it refuses counts in
    /// neither direction. The window of a closed breaker holds nothing of it,
    /// and as a probe's answer it neither closes the breaker nor opens it
    /// again: the next call probes, until an `Ok` or an `Err` it takes.
    #[tokio::test(start_paused = true)]
    async fn an_err_record_if_refuses_counts_in_neither_direction_in_the_breaker() {
        let (_app, quotes, courier) = quotes::<CountedModule>().await;
        for answer in [NoQuote::Declined, NoQuote::Busy] {
            let answered = quotes.recorded(answer).await;
            assert!(
                matches!(answered, Err(ResilienceError::Failed(said)) if said == answer),
                "still the method's error: {answered:?}"
            );
            assert_eq!(courier.breaker_state(), Some(BreakerState::Closed));
            let counted = courier.breaker_stats().expect("the policy has a breaker");
            assert_eq!(
                (counted.outcomes, counted.failures),
                (0, 0),
                "no outcome of the window, a success no more than a failure"
            );
        }
        assert!(quotes.recorded(NoQuote::Down).await.is_err());
        assert!(
            matches!(courier.breaker_state(), Some(BreakerState::Open { .. })),
            "an `Err` it takes is a failure, and one opens this breaker"
        );

        tokio::time::advance(Duration::from_secs(31)).await;
        quotes.attempted();
        for probe in 0..2 {
            let answered = quotes.recorded(NoQuote::Declined).await;
            assert!(
                matches!(answered, Err(ResilienceError::Failed(NoQuote::Declined))),
                "probe {probe} is sent: {answered:?}"
            );
            assert_eq!(
                courier.breaker_state(),
                Some(BreakerState::HalfOpen),
                "probe {probe} neither closed the breaker nor opened it again"
            );
        }
        assert_eq!(
            quotes.attempted(),
            2,
            "each took the probe's turn in its own"
        );

        let quoted = quotes.polled(30).await;
        assert!(matches!(quoted, Ok(30)), "the next probe: {quoted:?}");
        assert_eq!(
            courier.breaker_state(),
            Some(BreakerState::Closed),
            "an `Ok` closes it"
        );
    }

    /// A probe answering an `Err` `record_if` refuses, no success yet in its
    /// series, is not one its caller gave up: the breaker moved as the first
    /// was let through, to half-open, and no answer moves it since — no line,
    /// no transition told, no count on any counter — the call after probing.
    #[tokio::test(start_paused = true)]
    async fn a_probe_answering_an_err_record_if_refuses_moves_the_breaker_nowhere() {
        use nest_rs_resilience::BreakerPhase;
        use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
        use opentelemetry_sdk::metrics::{
            InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
        };

        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        opentelemetry::global::set_meter_provider(provider.clone());
        // Every counter's points, as the latest export says them: a transition counts
        // on one, and an answer that moves nothing on none.
        let counted = || {
            provider.force_flush().expect("the export");
            let exported = exporter
                .get_finished_metrics()
                .expect("the exported metrics");
            let mut points: Vec<(String, String, u64)> = exported
                .last()
                .into_iter()
                .flat_map(|resource| resource.scope_metrics())
                .flat_map(|scope| scope.metrics())
                .flat_map(|metric| {
                    let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
                        return Vec::new();
                    };
                    sum.data_points()
                        .map(|point| {
                            let mut by: Vec<_> = point
                                .attributes()
                                .map(|kv| format!("{}={}", kv.key, kv.value))
                                .collect();
                            by.sort();
                            (metric.name().to_owned(), by.join(","), point.value())
                        })
                        .collect()
                })
                .collect();
            points.sort();
            points
        };
        let total = |points: &[(String, String, u64)]| -> u64 {
            points.iter().map(|(_, _, value)| value).sum()
        };

        let (_app, quotes, courier) = quotes::<CountedModule>().await;
        assert!(quotes.recorded(NoQuote::Down).await.is_err());
        assert!(
            matches!(courier.breaker_state(), Some(BreakerState::Open { .. })),
            "one failure opens this breaker"
        );
        tokio::time::advance(Duration::from_secs(31)).await;
        quotes.attempted();
        let opened = counted();
        assert_eq!(total(&opened), 1, "the opening, counted: {opened:?}");

        let logs = LogCapture::install();
        let mut events = courier.events();
        let moved = |logs: &LogCapture| -> Vec<Option<String>> {
            logs.find(nest_rs_resilience::TARGET, "circuit breaker changed state")
                .iter()
                .map(|line| line.field("state"))
                .collect()
        };
        let transitions = |events: &mut nest_rs_resilience::ResilienceEvents| {
            let mut told = crate::told(events);
            told.retain(|event| matches!(event, ResilienceEvent::Transition { .. }));
            told
        };

        let answered = quotes.recorded(NoQuote::Declined).await;
        assert!(
            matches!(answered, Err(ResilienceError::Failed(NoQuote::Declined))),
            "sent as the probe: {answered:?}"
        );
        assert_eq!(courier.breaker_state(), Some(BreakerState::HalfOpen));
        assert_eq!(
            moved(&logs),
            [Some("half_open".to_owned())],
            "the probe let through, and nothing its answer filed"
        );
        let told = transitions(&mut events);
        assert!(
            matches!(
                &told[..],
                [ResilienceEvent::Transition {
                    from: BreakerPhase::Open,
                    to: BreakerPhase::HalfOpen,
                    manual: false,
                    ..
                }]
            ),
            "{told:?}"
        );
        let probed = counted();
        assert_eq!(total(&probed), 2, "and the half-opening: {probed:?}");

        let answered = quotes.recorded(NoQuote::Declined).await;
        assert!(
            matches!(answered, Err(ResilienceError::Failed(NoQuote::Declined))),
            "the next call probes in its turn: {answered:?}"
        );
        assert_eq!(quotes.attempted(), 2, "each was sent");
        assert_eq!(courier.breaker_state(), Some(BreakerState::HalfOpen));
        assert_eq!(
            moved(&logs).len(),
            1,
            "a probe in a breaker already half-open"
        );
        let told = transitions(&mut events);
        assert!(told.is_empty(), "{told:?}");
        assert_eq!(counted(), probed);
    }

    /// The two axes are independent: the dependency's final answer is neither
    /// sent again nor a failure, its "busy" is sent again and no failure, and
    /// its final failure is counted and sent no more.
    #[tokio::test(start_paused = true)]
    async fn an_err_is_sent_again_and_counted_each_as_its_own_predicate_says() {
        let (_app, quotes, courier) = quotes::<CountedModule>().await;

        assert!(quotes.judged(NoQuote::Declined).await.is_err());
        assert_eq!(quotes.attempted(), 1, "its answer is sent once");
        assert_eq!(courier.breaker_state(), Some(BreakerState::Closed));

        assert!(quotes.judged(NoQuote::Busy).await.is_err());
        assert_eq!(quotes.attempted(), 3, "sent again up to the retries");
        assert_eq!(
            courier.breaker_state(),
            Some(BreakerState::Closed),
            "and none of the three a failure: one opens this breaker"
        );

        assert!(quotes.judged(NoQuote::Gone).await.is_err());
        assert_eq!(quotes.attempted(), 1, "a final failure is sent once");
        assert!(
            matches!(courier.breaker_state(), Some(BreakerState::Open { .. })),
            "and counted"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_ok_retry_ok_if_takes_is_sent_again_and_the_last_answers_once_the_retries_are_spent()
    {
        let (_app, quotes, courier) = quotes::<CountedModule>().await;

        let ready = quotes.polled(10).await;
        assert!(matches!(ready, Ok(30)), "{ready:?}");
        assert_eq!(quotes.attempted(), 3, "asked again under thirty");

        let pending = quotes.polled(1).await;
        assert!(
            matches!(pending, Ok(3)),
            "the last `Ok` answers once the retries are spent: {pending:?}"
        );
        assert_eq!(quotes.attempted(), 3);
        assert_eq!(
            courier.breaker_state(),
            Some(BreakerState::Closed),
            "an `Ok` sent again is no failure"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_predicates_of_one_method_read_its_arguments_and_its_host() {
        let (_app, quotes, courier) = quotes::<CountedModule>().await;

        let bid = quotes.bid(20, None).await;
        assert!(matches!(bid, Ok(20)), "{bid:?}");
        assert_eq!(quotes.attempted(), 2, "asked again under its floor");

        let busy = quotes.bid(0, Some(NoQuote::Busy)).await;
        assert!(
            matches!(busy, Err(ResilienceError::Failed(NoQuote::Busy))),
            "{busy:?}"
        );
        assert_eq!(
            quotes.attempted(),
            2,
            "sent again once, though two retries were left"
        );
        assert_eq!(courier.breaker_state(), Some(BreakerState::Closed));
    }

    /// A method run once owns its arguments, and its predicate reads them all
    /// the same: the attempt borrowed what `record_if` is then asked about.
    #[tokio::test(start_paused = true)]
    async fn a_predicate_of_a_method_run_once_reads_an_argument_the_method_owns() {
        let (_app, quotes, courier) = quotes::<CountedModule>().await;

        let own = quotes
            .recorded_in("home".to_owned(), "home", NoQuote::Down)
            .await;
        assert!(
            matches!(own, Err(ResilienceError::Failed(NoQuote::Down))),
            "{own:?}"
        );
        let counted = courier.breaker_stats().expect("the policy has a breaker");
        assert_eq!(
            (courier.breaker_state(), counted.outcomes, counted.failures),
            (Some(BreakerState::Closed), 0, 0),
            "a failure in the carrier's own zone counts in neither direction"
        );

        let abroad = quotes
            .recorded_in("abroad".to_owned(), String::from("home"), NoQuote::Down)
            .await;
        assert!(abroad.is_err(), "{abroad:?}");
        assert!(
            matches!(courier.breaker_state(), Some(BreakerState::Open { .. })),
            "the same failure elsewhere is counted, and one opens this breaker"
        );
        assert_eq!(quotes.attempted(), 2, "each sent once");
    }

    #[tokio::test(start_paused = true)]
    async fn the_retry_predicates_of_an_idempotent_method_read_an_argument_it_owns() {
        let (_app, quotes, _courier) = quotes::<RetriedModule>().await;
        for (zone, failure, sent) in [
            ("home", Some(NoQuote::Busy), 1),
            ("abroad", Some(NoQuote::Busy), 3),
            ("abroad", Some(NoQuote::Declined), 1),
        ] {
            let answered = quotes.routed(zone.to_owned(), failure).await;
            assert!(answered.is_err(), "{zone}: {answered:?}");
            assert_eq!(quotes.attempted(), sent, "{zone}, {failure:?}");
        }

        let slow = quotes.routed("slow".to_owned(), None).await;
        assert!(matches!(slow, Ok(30)), "asked again under thirty: {slow:?}");
        assert_eq!(quotes.attempted(), 3);
        let fast = quotes.routed("fast".to_owned(), None).await;
        assert!(matches!(fast, Ok(10)), "{fast:?}");
        assert_eq!(quotes.attempted(), 1, "its first quote is its answer");
    }
}

/// What `fallback` and `fallback_if` make of the error a run ends on: the
/// method naming a fallback answers what it answers, handed the arguments and
/// that error — the call's own or each refusal of the policy — unless its
/// filter refuses the error; and every attempt of such a method runs in a
/// savepoint of its unit's transaction, rolled back before the fallback runs.
mod fallback {
    use super::*;

    /// What a run ended on, as its fallback read it.
    fn ended_on(error: &ResilienceError<CarrierDown>) -> &'static str {
        match error {
            ResilienceError::Failed(CarrierDown) => "failed",
            ResilienceError::TimedOut { .. } => "timed out",
            ResilienceError::Open { .. } => "open",
            ResilienceError::Full { .. } => "full",
            ResilienceError::Limited { .. } => "limited",
            _ => "unsettled",
        }
    }

    #[injectable]
    struct RatesService {
        #[inject]
        courier: Arc<ResiliencePolicy<Courier>>,
        attempts: AtomicU32,
        /// What each fallback was handed and how each savepoint was settled,
        /// in the order they happened.
        journal: Mutex<Vec<String>>,
    }

    impl RatesService {
        fn note(&self, entry: impl Into<String>) {
            self.journal.lock().expect("unpoisoned").push(entry.into());
        }

        fn journal(&self) -> Vec<String> {
            std::mem::take(&mut *self.journal.lock().expect("unpoisoned"))
        }

        fn attempted(&self) -> u32 {
            self.attempts.swap(0, Ordering::Relaxed)
        }

        /// The rate last known for `zone`, twice the weight.
        async fn last_known(
            &self,
            zone: &str,
            weight: u32,
            error: ResilienceError<CarrierDown>,
        ) -> Answer<u32> {
            self.note(format!("{zone}/{weight} after {}", ended_on(&error)));
            Ok(weight * 2)
        }

        /// The carrier quotes its home zone alone.
        #[resilient(courier, fallback = last_known)]
        async fn rate(&self, zone: &str, weight: u32) -> Answer<u32> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            if zone == "home" {
                return Ok(weight);
            }
            Err(CarrierDown)
        }

        #[resilient(courier, timeout = "100ms", fallback = last_known)]
        async fn slow_rate(&self, zone: &str, weight: u32) -> Answer<u32> {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if zone == "home" {
                return Ok(weight);
            }
            Err(CarrierDown)
        }

        /// A fallback answering for the policy's refusals alone.
        #[resilient(
            courier,
            fallback = last_known,
            fallback_if = |error| !matches!(error, ResilienceError::Failed(_))
        )]
        async fn refused_rate(&self, zone: &str, weight: u32) -> Answer<u32> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            if zone == "home" {
                return Ok(weight);
            }
            Err(CarrierDown)
        }

        /// A fallback knowing no rate either.
        async fn none_known(&self, error: ResilienceError<CarrierDown>) -> Answer<u32> {
            self.note(format!("none after {}", ended_on(&error)));
            Err(ResilienceError::Failed(CarrierDown))
        }

        #[resilient(courier, fallback = none_known)]
        async fn unknown_rate(&self) -> Answer<u32> {
            Ok(7)
        }

        /// An owned argument each attempt reads and the fallback takes.
        async fn queued(
            &self,
            order: String,
            error: ResilienceError<CarrierDown>,
        ) -> Answer<String> {
            self.note(format!("{order} after {}", ended_on(&error)));
            Ok(format!("{order} queued"))
        }

        #[resilient(courier, idempotent, retries = 2, fallback = queued)]
        async fn ship(&self, order: String) -> Answer<String> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            if order.is_empty() {
                return Ok(order.clone());
            }
            Err(CarrierDown)
        }

        /// No fallback and sent once: its `Err` is its caller's.
        #[resilient(courier)]
        async fn listed_rate(&self) -> Answer<u32> {
            Err(CarrierDown)
        }

        /// The rate last known for a zone the method owned.
        async fn last_known_in(
            &self,
            zone: String,
            weight: u32,
            error: ResilienceError<CarrierDown>,
        ) -> Answer<u32> {
            self.note(format!("{zone}/{weight} after {}", ended_on(&error)));
            Ok(weight * 2)
        }

        /// Run once, the predicate and the filter both reading the zone the
        /// fallback is then handed by value: no rate is known for `closed`,
        /// and a failure in `quiet` is none in the breaker.
        #[resilient(
            courier,
            record_if = |_| zone != "quiet",
            fallback = last_known_in,
            fallback_if = |error| matches!(error, ResilienceError::Failed(_)) && zone != "closed"
        )]
        async fn owned_rate(&self, zone: String, weight: u32) -> Answer<u32> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            if zone == "home" {
                return Ok(weight);
            }
            Err(CarrierDown)
        }

        /// The same filter over a run sent again.
        #[resilient(
            courier,
            idempotent,
            fallback = last_known_in,
            fallback_if = |_| zone != "closed" && self.attempts.load(Ordering::Relaxed) > 0
        )]
        async fn owned_rate_again(&self, zone: String, weight: u32) -> Answer<u32> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            if zone == "home" {
                return Ok(weight);
            }
            Err(CarrierDown)
        }
    }

    /// One retry a millisecond or two later, and a breaker eight failures open.
    fn falling_back() -> ResilienceConfig {
        ResilienceConfig {
            retries: Some(1),
            backoff_first: Duration::from_millis(1),
            backoff_ceiling: Duration::from_millis(2),
            breaker_window: Some(8),
            breaker_failures: 8,
            ..ResilienceConfig::default()
        }
    }

    #[module(
        imports = [ResilienceModule::for_root((Courier, falling_back()))],
        providers = [RatesService],
    )]
    struct FallingBackModule;

    #[module(
        imports = [ResilienceModule::for_root((Courier, ResilienceConfig {
            concurrency: Some(1),
            ..quiet()
        }))],
        providers = [RatesService],
    )]
    struct OneAtATimeModule;

    #[module(
        imports = [ResilienceModule::for_root((Courier, ResilienceConfig {
            rate_limit: Some(1),
            rate_window: Duration::from_secs(30),
            ..quiet()
        }))],
        providers = [RatesService],
    )]
    struct LimitedModule;

    /// `M` booted, the service it provides and the policy that service runs
    /// under.
    async fn rates<M: Module + 'static>() -> (
        HeadlessApp,
        Arc<RatesService>,
        Arc<ResiliencePolicy<Courier>>,
    ) {
        let app = boot::<M>().await;
        let rates = app
            .container()
            .get::<RatesService>()
            .expect("the module provides it");
        let courier = held::<Courier>(&app);
        (app, rates, courier)
    }

    #[tokio::test(start_paused = true)]
    async fn a_fallback_is_handed_the_arguments_and_the_calls_error_and_answers_in_its_place() {
        let (_app, rates, _courier) = rates::<FallingBackModule>().await;
        let logs = LogCapture::install();

        let answered = rates.rate("home", 3).await;
        assert!(matches!(answered, Ok(3)), "{answered:?}");
        assert_eq!(rates.journal(), [""; 0], "a run that answered asks nothing");
        logs.expect_none(
            nest_rs_resilience::TARGET,
            "the call ended on an error; its fallback answers",
        );

        let fell_back = rates.rate("north", 3).await;
        assert!(matches!(fell_back, Ok(6)), "{fell_back:?}");
        assert_eq!(rates.journal(), ["north/3 after failed"]);
        assert_eq!(rates.attempted(), 2, "each call sent once");
        let line = logs.expect_one(
            nest_rs_resilience::TARGET,
            "the call ended on an error; its fallback answers",
        );
        assert_eq!(line.level, "debug", "{line:?}");
        assert_eq!(line.field("instance").as_deref(), Some("courier"));
        assert_eq!(line.field("host").as_deref(), Some("RatesService"));
        assert_eq!(line.field("method").as_deref(), Some("rate"));
        assert_eq!(
            line.field("error"),
            Some(error_message(&ResilienceError::Failed(CarrierDown))),
            "the whole chain of the error the run ended on"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_fallback_is_handed_a_timeout_and_an_open_breakers_refusal() {
        let (_app, rates, courier) = rates::<FallingBackModule>().await;

        let timed_out = rates.slow_rate("north", 3).await;
        assert!(matches!(timed_out, Ok(6)), "{timed_out:?}");
        assert_eq!(rates.journal(), ["north/3 after timed out"]);

        courier
            .open_breaker(Duration::from_secs(60))
            .expect("an open time the breaker takes");
        let refused = rates.rate("south", 4).await;
        assert!(matches!(refused, Ok(8)), "{refused:?}");
        assert_eq!(rates.journal(), ["south/4 after open"]);
        assert_eq!(rates.attempted(), 0, "refused unsent, and answered");
    }

    #[tokio::test(start_paused = true)]
    async fn a_fallback_is_handed_a_full_bulkheads_refusal() {
        let (_app, rates, _courier) = rates::<OneAtATimeModule>().await;

        let (held, refused) = tokio::join!(rates.slow_rate("north", 3), async {
            tokio::task::yield_now().await;
            rates.rate("south", 4).await
        });
        assert!(matches!(refused, Ok(8)), "{refused:?}");
        assert!(matches!(held, Ok(6)), "{held:?}");
        assert_eq!(
            rates.journal(),
            ["south/4 after full", "north/3 after timed out"]
        );
        assert_eq!(rates.attempted(), 0, "the refused call is never sent");
    }

    #[tokio::test(start_paused = true)]
    async fn a_fallback_is_handed_a_reached_rate_limits_refusal() {
        let (_app, rates, _courier) = rates::<LimitedModule>().await;

        let sent = rates.rate("home", 3).await;
        assert!(matches!(sent, Ok(3)), "{sent:?}");
        let refused = rates.rate("home", 4).await;
        assert!(matches!(refused, Ok(8)), "{refused:?}");
        assert_eq!(rates.journal(), ["home/4 after limited"]);
        assert_eq!(rates.attempted(), 1, "the refused call is never sent");
    }

    #[tokio::test(start_paused = true)]
    async fn an_error_fallback_if_refuses_is_the_methods_and_one_it_takes_is_answered() {
        let (_app, rates, courier) = rates::<FallingBackModule>().await;
        let logs = LogCapture::install();

        let failed = rates.refused_rate("north", 3).await;
        assert!(super::failed(&failed), "{failed:?}");
        assert_eq!(rates.journal(), [""; 0], "its fallback is not called");
        logs.expect_none(
            nest_rs_resilience::TARGET,
            "the call ended on an error; its fallback answers",
        );

        courier
            .open_breaker(Duration::from_secs(60))
            .expect("an open time the breaker takes");
        let refused = rates.refused_rate("north", 3).await;
        assert!(matches!(refused, Ok(6)), "{refused:?}");
        assert_eq!(rates.journal(), ["north/3 after open"]);
    }

    /// The filter is asked before the fallback takes the arguments: what it
    /// read by reference, the fallback is handed by value, run once or again.
    #[tokio::test(start_paused = true)]
    async fn a_filter_reads_an_argument_its_fallback_is_then_handed_by_value() {
        let (_app, rates, courier) = rates::<FallingBackModule>().await;

        let answered = rates.owned_rate("home".to_owned(), 3).await;
        assert!(matches!(answered, Ok(3)), "{answered:?}");
        let fell_back = rates.owned_rate("north".to_owned(), 3).await;
        assert!(matches!(fell_back, Ok(6)), "{fell_back:?}");
        let refused = rates.owned_rate("closed".to_owned(), 3).await;
        assert!(super::failed(&refused), "{refused:?}");
        assert_eq!(rates.journal(), ["north/3 after failed"]);
        assert_eq!(rates.attempted(), 3, "each sent once");
        let counted = courier.breaker_stats().expect("the policy has a breaker");
        assert_eq!(
            (counted.outcomes, counted.failures),
            (3, 2),
            "`record_if` read the zone too: both failures counted"
        );
        let quiet = rates.owned_rate("quiet".to_owned(), 4).await;
        assert!(matches!(quiet, Ok(8)), "{quiet:?}");
        let counted = courier.breaker_stats().expect("the policy has a breaker");
        assert_eq!(
            (counted.outcomes, counted.failures),
            (3, 2),
            "and none here"
        );
        assert_eq!(rates.journal(), ["quiet/4 after failed"]);
        rates.attempted();

        let fell_back = rates.owned_rate_again("north".to_owned(), 5).await;
        assert!(matches!(fell_back, Ok(10)), "{fell_back:?}");
        assert_eq!(rates.attempted(), 2, "sent again, then its fallback");
        let refused = rates.owned_rate_again("closed".to_owned(), 5).await;
        assert!(super::failed(&refused), "{refused:?}");
        assert_eq!(rates.journal(), ["north/5 after failed"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_fallbacks_own_err_is_the_methods_answer() {
        let (_app, rates, courier) = rates::<FallingBackModule>().await;

        let answered = rates.unknown_rate().await;
        assert!(matches!(answered, Ok(7)), "{answered:?}");
        courier
            .open_breaker(Duration::from_secs(60))
            .expect("an open time the breaker takes");
        let unknown = rates.unknown_rate().await;
        assert!(
            super::failed(&unknown),
            "what the fallback answered, in place of the refusal: {unknown:?}"
        );
        assert_eq!(rates.journal(), ["none after open"]);
    }

    #[tokio::test(start_paused = true)]
    async fn an_idempotent_methods_fallback_answers_after_its_last_attempt_and_takes_what_each_read()
     {
        let (_app, rates, _courier) = rates::<FallingBackModule>().await;

        let queued = rates.ship("P1".to_owned()).await;
        assert!(matches!(queued.as_deref(), Ok("P1 queued")), "{queued:?}");
        assert_eq!(rates.attempted(), 3, "one call and its two retries first");
        assert_eq!(rates.journal(), ["P1 after failed"]);

        let sent = rates.ship(String::new()).await;
        assert!(matches!(sent.as_deref(), Ok("")), "{sent:?}");
        assert_eq!(rates.journal(), [""; 0]);
    }

    /// A unit of work holding a transaction, each savepoint it opens noting in
    /// the service's journal how it was settled.
    struct Unit(Arc<RatesService>);

    impl nest_rs_database::Executor for Unit {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn savepoint(&self) -> Option<Arc<dyn nest_rs_database::AtomicBlock>> {
            Some(Arc::new(Savepoint(Arc::clone(&self.0))))
        }
    }

    struct Savepoint(Arc<RatesService>);

    impl nest_rs_database::Executor for Savepoint {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    impl nest_rs_database::AtomicBlock for Savepoint {
        fn settle(
            self: Arc<Self>,
            ok: bool,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), nest_rs_database::AtomicError>> + Send>>
        {
            self.0.note(if ok { "kept" } else { "rolled back" });
            Box::pin(std::future::ready(Ok(())))
        }
    }

    /// The savepoint an attempt runs in is settled before what follows it: kept
    /// when the method answers that attempt's `Ok`, rolled back before the
    /// fallback runs and before an error its filter refused is answered. A
    /// method sent once naming no fallback opens none.
    #[tokio::test(start_paused = true)]
    async fn under_a_fallback_each_attempts_savepoint_is_settled_before_the_fallback_or_the_answer()
    {
        use nest_rs_database::__private::with_request_executor;

        let (_app, rates, _courier) = rates::<FallingBackModule>().await;
        let unit = || Arc::new(Unit(Arc::clone(&rates))) as Arc<dyn nest_rs_database::Executor>;

        let fell_back = with_request_executor(unit(), rates.rate("north", 3)).await;
        assert!(matches!(fell_back, Ok(6)), "{fell_back:?}");
        assert_eq!(rates.journal(), ["rolled back", "north/3 after failed"]);

        let answered = with_request_executor(unit(), rates.rate("home", 3)).await;
        assert!(matches!(answered, Ok(3)), "{answered:?}");
        assert_eq!(rates.journal(), ["kept"]);

        let refused = with_request_executor(unit(), rates.refused_rate("north", 3)).await;
        assert!(super::failed(&refused), "{refused:?}");
        assert_eq!(rates.journal(), ["rolled back"]);

        let queued = with_request_executor(unit(), rates.ship("P1".to_owned())).await;
        assert!(queued.is_ok(), "{queued:?}");
        assert_eq!(
            rates.journal(),
            [
                "rolled back",
                "rolled back",
                "rolled back",
                "P1 after failed"
            ],
            "each of the three attempts, then the fallback"
        );

        let listed = with_request_executor(unit(), rates.listed_rate()).await;
        assert!(super::failed(&listed), "{listed:?}");
        assert_eq!(rates.journal(), [""; 0], "sent once with no fallback");
    }
}

/// What `cancel` binds in a body: its attempt's own token, cancelled when the
/// policy gives that attempt up, under whichever shape the attempt takes its
/// arguments in — owning them, or lending them to the next attempt or to a
/// fallback.
mod cancel {
    use super::*;
    use nest_rs_resilience::CancellationToken;

    #[injectable]
    struct DispatchService {
        #[inject]
        courier: Arc<ResiliencePolicy<Courier>>,
        attempts: AtomicU32,
        /// Each attempt's token, as its body read it.
        tokens: Mutex<Vec<CancellationToken>>,
        /// The work an attempt handed to another task.
        work: Mutex<Option<tokio::task::JoinHandle<&'static str>>>,
    }

    impl DispatchService {
        fn keep(&self, token: &CancellationToken) {
            self.tokens.lock().expect("unpoisoned").push(token.clone());
        }

        /// Whether each token a body read is cancelled, in the order read.
        fn cancelled(&self) -> Vec<bool> {
            let tokens = std::mem::take(&mut *self.tokens.lock().expect("unpoisoned"));
            tokens.iter().map(CancellationToken::is_cancelled).collect()
        }

        /// Hands its work to another task, which owns the token, and never
        /// answers.
        #[resilient(courier, timeout = "100ms", cancel = token)]
        async fn dispatch(&self) -> Answer<()> {
            let work = tokio::spawn(async move {
                token.cancelled().await;
                "stopped"
            });
            *self.work.lock().expect("unpoisoned") = Some(work);
            std::future::pending().await
        }

        /// Its first attempt never answers; its second hands its token on.
        #[resilient(courier, idempotent, timeout = "100ms", cancel = token)]
        async fn redispatch(&self, parcel: &str) -> Answer<String> {
            self.keep(&token);
            if self.attempts.fetch_add(1, Ordering::Relaxed) == 0 {
                std::future::pending::<()>().await;
            }
            let live = !token.is_cancelled();
            let work = tokio::spawn(async move {
                token.cancelled().await;
                "stopped"
            });
            *self.work.lock().expect("unpoisoned") = Some(work);
            Ok(format!("{parcel} dispatched while live: {live}"))
        }

        async fn parked(
            &self,
            parcel: &str,
            _error: ResilienceError<CarrierDown>,
        ) -> Answer<String> {
            Ok(format!("{parcel} parked"))
        }

        #[resilient(courier, fallback = parked, cancel = attempt)]
        async fn park(&self, parcel: &str) -> Answer<String> {
            self.keep(&attempt);
            if parcel.is_empty() {
                return Ok(String::new());
            }
            Err(CarrierDown)
        }
    }

    #[module(
        imports = [ResilienceModule::for_root((Courier, retrying_once()))],
        providers = [DispatchService],
    )]
    struct DispatchModule;

    async fn dispatches() -> (HeadlessApp, Arc<DispatchService>) {
        let app = boot::<DispatchModule>().await;
        let dispatches = app
            .container()
            .get::<DispatchService>()
            .expect("the module provides it");
        (app, dispatches)
    }

    #[tokio::test(start_paused = true)]
    async fn the_token_a_body_reads_is_cancelled_as_its_attempt_is_given_up_and_stops_work_handed_on()
     {
        let (_app, dispatches) = dispatches().await;

        let given_up = dispatches.dispatch().await;
        assert!(
            matches!(given_up, Err(ResilienceError::TimedOut { .. })),
            "{given_up:?}"
        );
        let work = dispatches.work.lock().expect("unpoisoned").take();
        let stopped = tokio::time::timeout(Duration::from_secs(1), work.expect("handed on"))
            .await
            .expect("the task the body spawned is told its attempt was given up");
        assert_eq!(stopped.ok(), Some("stopped"));
    }

    #[tokio::test(start_paused = true)]
    async fn each_attempt_reads_a_token_of_its_own() {
        let (_app, dispatches) = dispatches().await;

        let answered = dispatches.redispatch("P1").await;
        assert!(
            matches!(answered.as_deref(), Ok("P1 dispatched while live: true")),
            "{answered:?}"
        );
        assert_eq!(
            dispatches.cancelled(),
            [true, false],
            "the attempt given up is cancelled, the one that answered lives on"
        );
        let work = dispatches.work.lock().expect("unpoisoned").take();
        assert!(
            !work.expect("handed on").is_finished(),
            "work the answering attempt handed on is not stopped by its answer"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_attempt_lending_its_arguments_to_a_fallback_reads_its_token_too() {
        let (_app, dispatches) = dispatches().await;

        let parked = dispatches.park("P1").await;
        assert!(matches!(parked.as_deref(), Ok("P1 parked")), "{parked:?}");
        assert_eq!(
            dispatches.cancelled(),
            [false],
            "an attempt that answered was not given up"
        );
    }
}

/// The predicates, the fallback and the token on each receiver and host the
/// decorator takes: a generic method, a receiver lent through its `Arc`, a
/// method borrowing through a lifetime it names, a method of a trait impl, and
/// a handler of each impl half, called through the framework's own path.
mod keyed {
    use super::*;

    #[injectable]
    struct KeyedService {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
        attempts: AtomicU32,
        hub: Hub,
    }

    impl KeyedService {
        fn attempted(&self) -> u32 {
            self.attempts.swap(0, Ordering::Relaxed)
        }

        /// What `raw` parses to: nothing is asked again, an empty text fails.
        #[resilient(
            upstream,
            idempotent,
            retry_if = |_| true,
            retry_ok_if = Option::is_none,
            record_if = |_| false,
            fallback = unparsed,
            fallback_if = |_| true,
            cancel = token
        )]
        async fn parsed<T: FromStr + Send + Sync>(&self, raw: &str) -> Answer<Option<T>> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            if raw.is_empty() || token.is_cancelled() {
                return Err(CarrierDown);
            }
            Ok(raw.parse().ok())
        }

        async fn unparsed<T: Send>(
            &self,
            raw: &str,
            error: ResilienceError<CarrierDown>,
        ) -> Answer<Option<T>> {
            if raw.is_empty() {
                return Ok(None);
            }
            Err(error)
        }

        #[resilient(upstream, record_if = |_| false, fallback = held, cancel = token)]
        async fn shared(self: &Arc<Self>, url: &str) -> Answer<String> {
            drop(token);
            if url.is_empty() {
                return Err(CarrierDown);
            }
            Ok(format!("{url}, held {} times", Arc::strong_count(self)))
        }

        async fn held(
            self: &Arc<Self>,
            url: &str,
            _error: ResilienceError<CarrierDown>,
        ) -> Answer<String> {
            Ok(format!("{url}held {} times", Arc::strong_count(self)))
        }

        /// Where a parcel for `zone` is sorted: the zone, asked again while it
        /// has no name, or the hub once the carrier lost it.
        #[resilient(
            upstream,
            idempotent,
            retry_ok_if = |zone| zone.is_empty(),
            fallback = home,
            cancel = token
        )]
        async fn sorted<'a>(&'a self, zone: &'a str) -> Answer<&'a str> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            let _attempt = token;
            match zone {
                "lost" => Err(CarrierDown),
                zone => Ok(zone),
            }
        }

        async fn home<'a>(
            &'a self,
            _zone: &'a str,
            _error: ResilienceError<CarrierDown>,
        ) -> Answer<&'a str> {
            Ok(&self.hub.0)
        }
    }

    #[module(
        imports = [ResilienceModule::for_root(ResilienceConfig {
            retries: Some(1),
            breaker_window: Some(1),
            breaker_failures: 1,
            ..ResilienceConfig::default()
        })],
        providers = [KeyedService],
    )]
    struct KeyedModule;

    #[tokio::test(start_paused = true)]
    async fn a_generic_method_an_arc_receiver_and_a_named_lifetime_take_every_key() {
        let app = boot::<KeyedModule>().await;
        let keyed = app.container().get::<KeyedService>().expect("provided");
        let upstream = held::<nest_rs_core::Unnamed>(&app);

        assert!(matches!(keyed.parsed::<u16>("8080").await, Ok(Some(8080))));
        assert_eq!(keyed.attempted(), 1);
        assert!(matches!(keyed.parsed::<u16>("port").await, Ok(None)));
        assert_eq!(keyed.attempted(), 2, "nothing parsed is asked again");
        assert!(matches!(keyed.parsed::<u16>("").await, Ok(None)));
        assert_eq!(keyed.attempted(), 2, "sent again, then its fallback");
        assert_eq!(
            upstream.breaker_state(),
            Some(BreakerState::Closed),
            "no `Err` was a failure, and one opens this breaker"
        );

        let shared = keyed.shared("https://example.com").await;
        assert!(
            shared
                .as_deref()
                .is_ok_and(|said| said.starts_with("https://example.com, held ")),
            "{shared:?}"
        );
        let fell_back = keyed.shared("").await;
        assert!(
            fell_back
                .as_deref()
                .is_ok_and(|said| said.starts_with("held ")),
            "{fell_back:?}"
        );

        assert!(matches!(keyed.sorted("north").await, Ok("north")));
        assert_eq!(keyed.attempted(), 1);
        assert!(matches!(keyed.sorted("").await, Ok("")));
        assert_eq!(keyed.attempted(), 2, "a zone with no name is asked again");
        assert!(matches!(keyed.sorted("lost").await, Ok("the Lyon hub")));
    }

    /// What a carrier quotes for a zone, which a host built by hand asks.
    trait Quoting {
        fn quote(&self, zone: &str) -> impl Future<Output = Answer<Option<u32>>> + Send;
    }

    /// A host the app's code builds itself, holding a policy the container
    /// built.
    struct Quotes {
        upstream: Arc<ResiliencePolicy>,
        attempts: AtomicU32,
    }

    impl Quotes {
        /// The price listed for `zone`, where the carrier quoted none.
        async fn listed(
            &self,
            zone: &str,
            error: ResilienceError<CarrierDown>,
        ) -> Answer<Option<u32>> {
            match zone {
                "far" => Ok(Some(9)),
                _ => Err(error),
            }
        }
    }

    impl Quoting for Quotes {
        #[resilient(
            upstream,
            idempotent,
            retry_if = |_| true,
            retry_ok_if = Option::is_none,
            record_if = |_| false,
            fallback = listed,
            fallback_if = |error| matches!(error, ResilienceError::Failed(_)),
            cancel = token
        )]
        async fn quote(&self, zone: &str) -> Answer<Option<u32>> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            match zone {
                "home" if !token.is_cancelled() => Ok(Some(4)),
                "" => Ok(None),
                _ => Err(CarrierDown),
            }
        }
    }

    /// The fallback a method of a trait impl names is an inherent method of
    /// its host: `Self`'s, as the key says, whichever impl the method is in.
    #[tokio::test(start_paused = true)]
    async fn a_method_of_a_trait_impl_takes_every_key_its_fallback_an_inherent_method() {
        let app = boot::<KeyedModule>().await;
        let upstream = held::<nest_rs_core::Unnamed>(&app);
        let quotes = Quotes {
            upstream: Arc::clone(&upstream),
            attempts: AtomicU32::new(0),
        };
        let attempted = || quotes.attempts.swap(0, Ordering::Relaxed);

        assert!(matches!(quotes.quote("home").await, Ok(Some(4))));
        assert_eq!(attempted(), 1);
        assert!(matches!(quotes.quote("").await, Ok(None)));
        assert_eq!(attempted(), 2, "no quote is asked again");
        assert!(matches!(quotes.quote("far").await, Ok(Some(9))));
        assert_eq!(attempted(), 2, "sent again, then its fallback");
        assert!(
            failed(&quotes.quote("lost").await),
            "the fallback's own `Err`, the run's error handed back"
        );
        assert_eq!(
            upstream.breaker_state(),
            Some(BreakerState::Closed),
            "no `Err` was a failure, and one opens this breaker"
        );

        upstream
            .open_breaker(Duration::from_secs(30))
            .expect("the policy has a breaker");
        attempted();
        let refused = quotes.quote("far").await;
        assert!(
            matches!(refused, Err(ResilienceError::Open { .. })),
            "a refusal `fallback_if` leaves to the caller: {refused:?}"
        );
        assert_eq!(attempted(), 0);
    }

    /// The breaker a handler's one failure opens, were its `Err` one.
    fn counting() -> ResilienceConfig {
        opening_on_the_first_failure()
    }

    /// What the fallback of a unit no client waits on was handed, in order.
    #[injectable]
    #[derive(Default)]
    struct Journal {
        entries: Mutex<Vec<String>>,
    }

    impl Journal {
        /// Notes the error a run ended on, after `what` it was handed with it.
        fn note(&self, what: &str, error: &ResilienceError<UpstreamDown>) {
            let ended_on = match error {
                ResilienceError::Failed(UpstreamDown) => "failed".to_owned(),
                ResilienceError::Open { .. } => "open".to_owned(),
                ResilienceError::TimedOut { timeout, .. } => format!("timed out at {timeout:?}"),
                _ => "another refusal".to_owned(),
            };
            self.entries
                .lock()
                .expect("unpoisoned")
                .push(format!("{what}{ended_on}"));
        }

        fn entries(&self) -> Vec<String> {
            self.entries.lock().expect("unpoisoned").clone()
        }
    }

    /// The one line a fallback answering for `method` of `host` files each
    /// time, the last of `times`: a handler's, as a provider's method's.
    #[track_caller]
    fn fell_back(logs: &LogCapture, host: &str, method: &str, times: usize) {
        let lines = logs.find(
            nest_rs_resilience::TARGET,
            "the call ended on an error; its fallback answers",
        );
        assert_eq!(lines.len(), times, "{lines:#?}");
        for line in &lines {
            assert_eq!(line.field("host").as_deref(), Some(host), "{line:?}");
            assert_eq!(line.field("method").as_deref(), Some(method), "{line:?}");
        }
    }

    mod scheduled {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::time::Duration;

        use nest_rs_core::operation_log::{OK, TARGET as OPERATIONS};
        use nest_rs_core::{injectable, module};
        use nest_rs_resilience::{
            BreakerState, ResilienceError, ResilienceModule, ResiliencePolicy, resilient,
        };
        use nest_rs_schedule::{Scheduler, scheduled};
        use nest_rs_testing::{HeadlessApp, LogCapture};

        use super::super::UpstreamDown;
        use super::{Journal, counting, fell_back};

        #[injectable]
        struct KeyedTasks {
            #[inject]
            upstream: Arc<ResiliencePolicy>,
            #[inject]
            journal: Arc<Journal>,
            attempts: AtomicU32,
        }

        impl KeyedTasks {
            /// A refresh the upstream did not take is left to the occurrence
            /// after it.
            async fn skipped(
                &self,
                error: ResilienceError<UpstreamDown>,
            ) -> Result<(), ResilienceError<UpstreamDown>> {
                self.journal.note("", &error);
                Ok(())
            }
        }

        #[scheduled]
        impl KeyedTasks {
            #[every("1s")]
            #[resilient(
                upstream,
                idempotent,
                retries = 1,
                retry_if = |_| false,
                retry_ok_if = |_| false,
                record_if = |_| false,
                fallback = skipped,
                fallback_if = |error| !matches!(error, ResilienceError::TimedOut { .. }),
                cancel = token
            )]
            async fn refresh(&self) -> Result<(), ResilienceError<UpstreamDown>> {
                drop(token);
                self.attempts.fetch_add(1, Ordering::Relaxed);
                Err(UpstreamDown)
            }
        }

        #[module(
            imports = [ResilienceModule::for_root(counting())],
            providers = [KeyedTasks, Journal],
        )]
        struct KeyedScheduleModule;

        /// A tick that falls back instead of failing: its occurrence is filed
        /// `ok`, for its own `Err` and for the refusal of a breaker opened by
        /// hand between two occurrences.
        #[tokio::test(start_paused = true)]
        async fn a_tick_is_filed_ok_once_its_fallback_answered_and_its_err_opens_no_breaker() {
            let logs = LogCapture::install();
            let app = HeadlessApp::builder()
                .module::<KeyedScheduleModule>()
                .build_headless()
                .await
                .expect("a tick carrying the keys boots");
            let tasks = app.container().get::<KeyedTasks>().expect("provided");
            let upstream = app
                .container()
                .get::<ResiliencePolicy>()
                .expect("the module built the policy");
            let scheduler = app
                .spawn_transport(Scheduler::new())
                .await
                .expect("the scheduler configures against the app");

            tokio::time::sleep(Duration::from_millis(1500)).await;
            assert_eq!(
                tasks.attempts.load(Ordering::Relaxed),
                1,
                "an `Err` `retry_if` refuses is sent once"
            );
            assert_eq!(upstream.breaker_state(), Some(BreakerState::Closed));
            upstream
                .open_breaker(Duration::from_secs(30))
                .expect("the policy has a breaker");
            tokio::time::sleep(Duration::from_secs(1)).await;
            scheduler.shutdown().await.expect("the scheduler stops");

            assert_eq!(tasks.journal.entries(), ["failed", "open"]);
            assert_eq!(tasks.attempts.load(Ordering::Relaxed), 1, "refused unsent");
            let ticks = logs.find(OPERATIONS, nest_rs_schedule::unit::TICK.name());
            assert_eq!(ticks.len(), 2, "{ticks:#?}");
            for tick in &ticks {
                assert_eq!(tick.field("outcome").as_deref(), Some(OK), "{tick:?}");
            }
            logs.expect_none(nest_rs_schedule::TARGET, "scheduled job failed");
            fell_back(&logs, "KeyedTasks", "refresh", 2);
        }
    }

    mod listeners {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        use nest_rs_core::operation_log::{ERROR, OK, TARGET as OPERATIONS};
        use nest_rs_core::{injectable, module};
        use nest_rs_events::{EventBus, EventsModule, listeners};
        use nest_rs_resilience::{
            BreakerState, CancellationToken, ResilienceError, ResilienceModule, ResiliencePolicy,
            resilient,
        };
        use nest_rs_testing::{HeadlessApp, LogCapture};

        use super::super::UpstreamDown;
        use super::{Journal, counting, fell_back};

        /// A parcel left the depot, which the listener tells the carrier.
        #[derive(Clone)]
        struct Dispatched {
            parcel: &'static str,
        }

        /// How long the unit of the listener asking about a late parcel lasts,
        /// its `timeout = "50ms"`.
        const UNIT: Duration = Duration::from_millis(50);

        /// A parcel is late, which the listener asks the carrier about for as
        /// long as its unit lasts.
        #[derive(Clone)]
        struct Delayed {
            /// Whether the fallback waits before it answers.
            patient: bool,
        }

        #[injectable]
        struct KeyedListener {
            #[inject]
            upstream: Arc<ResiliencePolicy>,
            #[inject]
            journal: Arc<Journal>,
            attempts: AtomicU32,
            /// The token of each attempt at asking about a late parcel.
            tokens: Mutex<Vec<CancellationToken>>,
        }

        impl KeyedListener {
            /// The carrier is told later, of the parcel the event named.
            async fn queued(
                &self,
                event: Dispatched,
                error: ResilienceError<UpstreamDown>,
            ) -> Result<(), ResilienceError<UpstreamDown>> {
                self.journal
                    .note(&format!("{} after ", event.parcel), &error);
                Ok(())
            }

            /// The carrier is asked again at the next sweep.
            async fn asked_later(
                &self,
                event: Delayed,
                error: ResilienceError<UpstreamDown>,
            ) -> Result<(), ResilienceError<UpstreamDown>> {
                self.journal.note("late after ", &error);
                if event.patient {
                    tokio::task::yield_now().await;
                }
                Ok(())
            }
        }

        #[listeners]
        impl KeyedListener {
            /// The event is lent to each attempt and handed to the fallback.
            #[on_event]
            #[resilient(
                upstream,
                idempotent,
                retries = 1,
                retry_if = |_| true,
                retry_ok_if = |_| false,
                record_if = |_| false,
                fallback = queued,
                fallback_if = |_| true,
                cancel = token
            )]
            async fn on_dispatched(
                &self,
                event: Dispatched,
            ) -> Result<(), ResilienceError<UpstreamDown>> {
                drop(token);
                self.attempts.fetch_add(1, Ordering::Relaxed);
                if event.parcel.is_empty() {
                    return Ok(());
                }
                Err(UpstreamDown)
            }

            /// A carrier that never answers, asked for as long as the unit
            /// lasts.
            #[on_event(timeout = "50ms")]
            #[resilient(upstream, fallback = asked_later, cancel = token)]
            async fn on_delayed(
                &self,
                event: Delayed,
            ) -> Result<(), ResilienceError<UpstreamDown>> {
                let Delayed { .. } = &event;
                self.tokens.lock().expect("unpoisoned").push(token);
                std::future::pending().await
            }
        }

        #[module(
            imports = [EventsModule, ResilienceModule::for_root(counting())],
            providers = [KeyedListener, Journal],
        )]
        struct KeyedEventsModule;

        #[tokio::test(start_paused = true)]
        async fn a_listeners_unit_is_filed_ok_once_its_fallback_took_the_event() {
            let logs = LogCapture::install();
            let app = HeadlessApp::builder()
                .module::<KeyedEventsModule>()
                .build_headless()
                .await
                .expect("a listener carrying the keys boots");
            let listener = app.container().get::<KeyedListener>().expect("provided");
            let upstream = app
                .container()
                .get::<ResiliencePolicy>()
                .expect("the module built the policy");
            let bus = app
                .container()
                .get::<EventBus>()
                .expect("the bus is provided");

            bus.emit(Dispatched { parcel: "P7" }).await;
            assert_eq!(
                listener.attempts.load(Ordering::Relaxed),
                2,
                "sent again, then its fallback"
            );
            assert_eq!(upstream.breaker_state(), Some(BreakerState::Closed));
            upstream
                .open_breaker(Duration::from_secs(30))
                .expect("the policy has a breaker");
            bus.emit(Dispatched { parcel: "P8" }).await;

            assert_eq!(
                listener.journal.entries(),
                ["P7 after failed", "P8 after open"]
            );
            assert_eq!(
                listener.attempts.load(Ordering::Relaxed),
                2,
                "refused unsent"
            );
            let dispatched = logs.find(OPERATIONS, nest_rs_events::unit::DISPATCH.name());
            assert_eq!(dispatched.len(), 2, "{dispatched:#?}");
            for unit in &dispatched {
                assert_eq!(unit.field("outcome").as_deref(), Some(OK), "{unit:?}");
            }
            fell_back(&logs, "KeyedListener", "on_dispatched", 2);
        }

        /// What a handler adds to a key is its unit. The deadline its edge
        /// cuts it at ends the attempt before the policy's own timeout, five
        /// seconds here; the token is cancelled there, and the fallback is
        /// handed the time the unit had left and none of its own: one
        /// answering at once answers for the unit, one that waits is cut with
        /// it, its unit filed failed.
        #[tokio::test(start_paused = true)]
        async fn a_listeners_fallback_answers_at_its_units_deadline_and_has_no_time_past_it() {
            let logs = LogCapture::install();
            let app = HeadlessApp::builder()
                .module::<KeyedEventsModule>()
                .build_headless()
                .await
                .expect("a listener carrying the keys boots");
            let listener = app.container().get::<KeyedListener>().expect("provided");
            let upstream = app
                .container()
                .get::<ResiliencePolicy>()
                .expect("the module built the policy");
            let bus = app
                .container()
                .get::<EventBus>()
                .expect("the bus is provided");
            let asked = || logs.find(OPERATIONS, nest_rs_events::unit::DISPATCH.name());

            let sent = tokio::time::Instant::now();
            bus.emit(Delayed { patient: false }).await;
            assert_eq!(sent.elapsed(), UNIT, "given up at the unit's deadline");
            assert_eq!(
                listener.journal.entries(),
                [format!("late after timed out at {UNIT:?}")]
            );
            assert_eq!(asked()[0].field("outcome").as_deref(), Some(OK));
            fell_back(&logs, "KeyedListener", "on_delayed", 1);
            assert!(
                matches!(upstream.breaker_state(), Some(BreakerState::Open { .. })),
                "the attempt given up there is a failure, and one opens this breaker"
            );

            upstream.close_breaker().expect("the policy has a breaker");
            bus.emit(Delayed { patient: true }).await;
            assert_eq!(
                listener.journal.entries().len(),
                2,
                "the fallback was called for the second timeout too"
            );
            let cut = asked();
            assert_eq!(cut.len(), 2, "{cut:#?}");
            assert_eq!(cut[1].field("outcome").as_deref(), Some(ERROR), "{cut:#?}");
            let failed = logs.expect_one(
                nest_rs_events::TARGET,
                "event listener failed — dispatch continues with the next listener",
            );
            assert_eq!(failed.field("method").as_deref(), Some("on_delayed"));

            let tokens = listener.tokens.lock().expect("unpoisoned");
            assert_eq!(tokens.len(), 2);
            assert!(
                tokens.iter().all(CancellationToken::is_cancelled),
                "each attempt's token, cancelled as it was given up"
            );
        }
    }

    mod indicators {
        use std::sync::Arc;
        use std::time::Duration;

        use nest_rs_core::{injectable, module};
        use nest_rs_health::{HealthModule, HealthService, IndicatorStatus, ProbeKind, indicators};
        use nest_rs_resilience::{
            BreakerState, ResilienceError, ResilienceModule, ResiliencePolicy, resilient,
        };
        use nest_rs_testing::{HeadlessApp, LogCapture};

        use super::super::UpstreamDown;
        use super::{Journal, counting, fell_back};

        #[injectable]
        struct KeyedIndicator {
            #[inject]
            upstream: Arc<ResiliencePolicy>,
            #[inject]
            journal: Arc<Journal>,
        }

        impl KeyedIndicator {
            /// The app serves without the upstream while its policy holds
            /// calls back.
            async fn degraded(
                &self,
                error: ResilienceError<UpstreamDown>,
            ) -> Result<(), ResilienceError<UpstreamDown>> {
                self.journal.note("", &error);
                Ok(())
            }
        }

        #[indicators]
        impl KeyedIndicator {
            #[readiness]
            #[resilient(
                upstream,
                idempotent,
                retry_if = |_| false,
                retry_ok_if = |_| false,
                record_if = |_| false,
                fallback = degraded,
                fallback_if = |error| !matches!(error, ResilienceError::Failed(_)),
                cancel = token
            )]
            async fn upstream_answers(&self) -> Result<(), ResilienceError<UpstreamDown>> {
                drop(token);
                Err(UpstreamDown)
            }
        }

        #[module(
            imports = [HealthModule, ResilienceModule::for_root(counting())],
            providers = [KeyedIndicator, Journal],
        )]
        struct KeyedHealthModule;

        /// A probe that falls back instead of failing: down for the call's
        /// own `Err`, which `fallback_if` leaves it, and up once the policy
        /// refuses the call.
        #[tokio::test(start_paused = true)]
        async fn an_indicator_answers_up_once_its_fallback_answered_for_a_refusal() {
            let logs = LogCapture::install();
            let app = HeadlessApp::builder()
                .module::<KeyedHealthModule>()
                .build_headless()
                .await
                .expect("an indicator carrying the keys boots");
            let indicator = app.container().get::<KeyedIndicator>().expect("provided");
            let upstream = app
                .container()
                .get::<ResiliencePolicy>()
                .expect("the module built the policy");
            let health = app
                .container()
                .get::<HealthService>()
                .expect("HealthModule provides it");

            let failed = health.probe(ProbeKind::Readiness).await;
            assert_eq!(failed.status, IndicatorStatus::Down, "{:?}", failed.details);
            logs.expect_one(nest_rs_health::TARGET, "health indicator failed");
            assert_eq!(upstream.breaker_state(), Some(BreakerState::Closed));

            upstream
                .open_breaker(Duration::from_secs(30))
                .expect("the policy has a breaker");
            let degraded = health.probe(ProbeKind::Readiness).await;
            assert_eq!(
                degraded.status,
                IndicatorStatus::Up,
                "{:?}",
                degraded.details
            );
            assert!(degraded.info.contains_key("upstream_answers"));
            assert_eq!(indicator.journal.entries(), ["open"]);
            assert_eq!(
                logs.find(nest_rs_health::TARGET, "health indicator failed")
                    .len(),
                1,
                "the probe its fallback answered files no failure"
            );
            fell_back(&logs, "KeyedIndicator", "upstream_answers", 1);
        }
    }

    mod hooks {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        use nest_rs_core::operation_log::{OK, TARGET as OPERATIONS};
        use nest_rs_core::{hooks, injectable, module};
        use nest_rs_resilience::{
            BreakerState, ResilienceError, ResilienceModule, ResiliencePolicy, resilient,
        };
        use nest_rs_testing::{HeadlessApp, LogCapture};

        use super::super::UpstreamDown;
        use super::{Journal, counting, fell_back};

        #[injectable]
        struct KeyedHooks {
            #[inject]
            upstream: Arc<ResiliencePolicy>,
            #[inject]
            journal: Arc<Journal>,
            attempts: AtomicU32,
        }

        impl KeyedHooks {
            /// The cache starts cold, and fills as it is read.
            async fn cold(
                &self,
                error: ResilienceError<UpstreamDown>,
            ) -> Result<(), ResilienceError<UpstreamDown>> {
                self.journal.note("", &error);
                Ok(())
            }
        }

        #[hooks]
        impl KeyedHooks {
            #[on_module_init]
            #[resilient(
                upstream,
                idempotent,
                retries = 1,
                retry_if = |_| true,
                retry_ok_if = |_| false,
                record_if = |_| false,
                fallback = cold,
                fallback_if = |_| true,
                cancel = token
            )]
            async fn warm(&self) -> Result<(), ResilienceError<UpstreamDown>> {
                drop(token);
                self.attempts.fetch_add(1, Ordering::Relaxed);
                Err(UpstreamDown)
            }
        }

        #[module(
            imports = [ResilienceModule::for_root(counting())],
            providers = [KeyedHooks, Journal],
        )]
        struct KeyedHooksModule;

        /// An init hook that falls back instead of failing the boot.
        #[tokio::test(start_paused = true)]
        async fn an_init_hook_completes_the_boot_once_its_fallback_answered() {
            let logs = LogCapture::install();
            let app = HeadlessApp::builder()
                .module::<KeyedHooksModule>()
                .build_headless()
                .await
                .expect("a hook carrying the keys boots");
            let hooks = app.container().get::<KeyedHooks>().expect("provided");
            let upstream = app
                .container()
                .get::<ResiliencePolicy>()
                .expect("the module built the policy");

            app.init().await.expect("the hook's fallback answered");

            assert_eq!(
                hooks.attempts.load(Ordering::Relaxed),
                2,
                "sent again, then its fallback"
            );
            assert_eq!(hooks.journal.entries(), ["failed"]);
            assert_eq!(upstream.breaker_state(), Some(BreakerState::Closed));
            let mut ran = logs.find(OPERATIONS, nest_rs_core::unit::HOOK.name());
            ran.retain(|line| line.field("host").as_deref() == Some("KeyedHooks"));
            assert_eq!(ran.len(), 1, "{ran:#?}");
            assert_eq!(ran[0].field("outcome").as_deref(), Some(OK), "{ran:#?}");
            fell_back(&logs, "KeyedHooks", "warm", 1);
        }
    }

    /// The state of the breaker the handlers of `app` ran under: closed, since
    /// `record_if` took none of their `Err`s for a failure.
    fn closed(app: &nest_rs_testing::TestApp) -> Option<BreakerState> {
        app.container()
            .get::<ResiliencePolicy>()
            .expect("the module built the policy")
            .breaker_state()
    }

    mod routes {
        use std::sync::Arc;

        use nest_rs_core::module;
        use nest_rs_http::{Path, controller, routes};
        use nest_rs_resilience::{
            BreakerState, ResilienceError, ResilienceModule, ResiliencePolicy, resilient,
        };
        use nest_rs_testing::TestApp;

        use super::super::UpstreamDown;
        use super::{closed, counting};

        type Preview = Result<String, ResilienceError<UpstreamDown>>;

        #[controller(path = "/keyed")]
        struct KeyedController {
            #[inject]
            upstream: Arc<ResiliencePolicy>,
        }

        impl KeyedController {
            async fn cached(&self, error: ResilienceError<UpstreamDown>) -> Preview {
                Ok(format!("cached: {error}"))
            }

            async fn stored(
                &self,
                id: Path<u32>,
                _error: ResilienceError<UpstreamDown>,
            ) -> Preview {
                Ok(format!("parcel {} from the store", id.0))
            }
        }

        #[routes]
        impl KeyedController {
            #[get("/")]
            #[public]
            #[resilient(upstream, record_if = |_| false, fallback = cached, cancel = token)]
            async fn preview(&self) -> Preview {
                drop(token);
                Err(UpstreamDown)
            }

            /// An extractor named whole, as its fallback is handed it, and taken
            /// apart in the body.
            #[get("/{id}")]
            #[public]
            #[resilient(upstream, record_if = |_| false, fallback = stored)]
            async fn parcel(&self, id: Path<u32>) -> Preview {
                let Path(id) = &id;
                if *id == 0 {
                    return Ok("no parcel".to_owned());
                }
                Err(UpstreamDown)
            }
        }

        #[module(
            imports = [ResilienceModule::for_root(counting())],
            providers = [KeyedController],
        )]
        struct KeyedHttpModule;

        #[tokio::test(start_paused = true)]
        async fn a_route_answers_what_its_fallback_answers_and_its_err_opens_no_breaker() {
            let app = TestApp::for_module::<KeyedHttpModule>()
                .await
                .expect("a route carrying the keys boots");
            for _ in 0..2 {
                let answered = app.http().get("/keyed").send().await;
                answered.assert_status_is_ok();
                answered.assert_text("cached: the call failed").await;
            }
            let stored = app.http().get("/keyed/7").send().await;
            stored.assert_status_is_ok();
            stored.assert_text("parcel 7 from the store").await;
            assert_eq!(closed(&app), Some(BreakerState::Closed));
        }
    }

    mod operations {
        use std::sync::Arc;

        use nest_rs_core::module;
        use nest_rs_graphql::{GraphqlModule, operations, resolver};
        use nest_rs_resilience::{
            BreakerState, ResilienceError, ResilienceModule, ResiliencePolicy, resilient,
        };
        use nest_rs_testing::TestApp;
        use serde_json::{Value, json};

        use super::super::UpstreamDown;
        use super::{closed, counting};

        #[resolver]
        struct KeyedResolver {
            #[inject]
            upstream: Arc<ResiliencePolicy>,
        }

        impl KeyedResolver {
            async fn cached(
                &self,
                number: String,
                _error: ResilienceError<UpstreamDown>,
            ) -> Result<String, ResilienceError<UpstreamDown>> {
                Ok(format!("{number} cached"))
            }
        }

        #[operations]
        impl KeyedResolver {
            #[query]
            #[public]
            #[resilient(upstream, record_if = |_| false, fallback = cached, cancel = token)]
            async fn keyed(&self, number: String) -> Result<String, ResilienceError<UpstreamDown>> {
                drop(token);
                if number.is_empty() {
                    return Ok(number.clone());
                }
                Err(UpstreamDown)
            }
        }

        #[module(
            imports = [GraphqlModule, ResilienceModule::for_root(counting())],
            providers = [KeyedResolver],
        )]
        struct KeyedGraphqlModule;

        #[tokio::test(start_paused = true)]
        async fn a_field_answers_what_its_fallback_answers_and_its_err_opens_no_breaker() {
            let app = TestApp::for_module::<KeyedGraphqlModule>()
                .await
                .expect("a field carrying the keys boots");
            for _ in 0..2 {
                let answer = app
                    .http()
                    .post("/graphql")
                    .body_json(&json!({ "query": "{ keyed(number: \"P7\") }" }))
                    .send()
                    .await
                    .json()
                    .await
                    .value()
                    .deserialize::<Value>();
                assert_eq!(answer["data"]["keyed"], "P7 cached", "{answer}");
            }
            assert_eq!(closed(&app), Some(BreakerState::Closed));
        }
    }

    mod messages {
        use std::sync::Arc;

        use nest_rs_core::module;
        use nest_rs_resilience::{
            BreakerState, ResilienceError, ResilienceModule, ResiliencePolicy, resilient,
        };
        use nest_rs_testing::TestApp;
        use nest_rs_ws::{WsModule, gateway, messages};

        use super::super::UpstreamDown;
        use super::counting;

        type Preview = Result<String, ResilienceError<UpstreamDown>>;

        #[gateway(path = "/ws/keyed")]
        struct KeyedGateway {
            #[inject]
            upstream: Arc<ResiliencePolicy>,
        }

        impl KeyedGateway {
            async fn cached(&self, _error: ResilienceError<UpstreamDown>) -> Preview {
                Ok("cached".to_owned())
            }
        }

        #[messages]
        impl KeyedGateway {
            #[subscribe_message("keyed")]
            #[public]
            #[resilient(upstream, record_if = |_| false, fallback = cached, cancel = token)]
            async fn keyed(&self) -> Preview {
                drop(token);
                Err(UpstreamDown)
            }
        }

        #[module(
            imports = [WsModule, ResilienceModule::for_root(counting())],
            providers = [KeyedGateway],
        )]
        struct KeyedWsModule;

        #[tokio::test]
        async fn a_message_answers_what_its_fallback_answers_and_its_err_opens_no_breaker() {
            let app = TestApp::builder()
                .module::<KeyedWsModule>()
                .build_ws()
                .await
                .expect("a gateway carrying the keys boots on a real port");
            let mut socket = app.socket("/ws/keyed").connect().await;
            for _ in 0..2 {
                socket.send("keyed", serde_json::Value::Null).await;
                let answer = socket.next_envelope().await;
                assert_eq!(answer["data"], "cached", "{answer}");
            }
            let upstream = app
                .container()
                .get::<ResiliencePolicy>()
                .expect("the module built the policy");
            assert_eq!(upstream.breaker_state(), Some(BreakerState::Closed));
            app.shutdown().await.expect("the transport stops cleanly");
        }
    }

    mod tools {
        use std::sync::Arc;

        use nest_rs_core::module;
        use nest_rs_mcp::{McpAccess, McpModule, mcp, tools};
        use nest_rs_resilience::{
            BreakerState, ResilienceError, ResilienceModule, ResiliencePolicy, resilient,
        };
        use nest_rs_testing::TestApp;
        use nest_rs_testing::mcp::{call_method, call_tool, open_session, result};
        use serde_json::json;

        use super::super::UpstreamDown;
        use super::{closed, counting};

        const PATH: &str = "/mcp/keyed";

        #[mcp(path = "/mcp/keyed")]
        #[derive(Clone)]
        struct KeyedTools {
            #[inject]
            upstream: Arc<ResiliencePolicy>,
        }

        impl KeyedTools {
            async fn cached(
                &self,
                _error: ResilienceError<UpstreamDown>,
            ) -> Result<String, ResilienceError<UpstreamDown>> {
                Ok("a cached preview".to_owned())
            }

            async fn last_seen(
                &self,
                id: u32,
                _error: ResilienceError<UpstreamDown>,
            ) -> Result<String, ResilienceError<UpstreamDown>> {
                Ok(format!("parcel {id} was last seen in Lyon"))
            }
        }

        #[tools]
        impl KeyedTools {
            /// Preview a link through the upstream, or from the cache.
            #[tool]
            #[public]
            #[resilient(upstream, record_if = |_| false, fallback = cached, cancel = token)]
            async fn keyed(&self) -> Result<String, ResilienceError<UpstreamDown>> {
                drop(token);
                Err(UpstreamDown)
            }

            /// Where a parcel stands, or where it was last seen.
            #[resource(template = "keyed-parcel://{id}")]
            #[public]
            #[resilient(upstream, record_if = |_| false, fallback = last_seen)]
            async fn parcel(&self, id: u32) -> Result<String, ResilienceError<UpstreamDown>> {
                if id == 0 {
                    return Ok("no parcel".to_owned());
                }
                Err(UpstreamDown)
            }
        }

        #[module(
            imports = [
                McpModule::for_root(McpAccess::Public),
                ResilienceModule::for_root(counting()),
            ],
            providers = [KeyedTools],
        )]
        struct KeyedMcpModule;

        #[tokio::test(start_paused = true)]
        async fn a_tool_and_a_resource_answer_what_their_fallbacks_answer() {
            let app = TestApp::for_module::<KeyedMcpModule>()
                .await
                .expect("an #[mcp] host carrying the keys boots");

            let called = call_tool(app.http(), PATH, "keyed", None).await;
            assert!(called.contains("a cached preview"), "{called}");

            let session = open_session(app.http(), PATH, None).await;
            let read = call_method(
                app.http(),
                PATH,
                &session,
                None,
                "resources/read",
                json!({ "uri": "keyed-parcel://7" }),
            )
            .await;
            assert_eq!(
                result(&read)["result"]["contents"][0]["text"],
                "parcel 7 was last seen in Lyon",
                "the resource's argument reached its fallback: {read}"
            );
            assert_eq!(closed(&app), Some(BreakerState::Closed));
        }
    }

    mod processor {
        use std::sync::Arc;

        use nest_rs_core::Container;
        use nest_rs_queue::__private::consume::{self, AttemptOutcome, Delivery};
        use nest_rs_queue::{
            Capabilities, ProcessMethod, QueueBackend, QueueName, WIRE_FORMAT_VERSION, processor,
            queue,
        };
        use nest_rs_resilience::{BreakerState, ResilienceError, ResiliencePolicy, resilient};
        use serde::{Deserialize, Serialize};

        use super::super::UpstreamDown;
        use super::counting;

        static BARE: QueueBackend = QueueBackend::new("bare", Capabilities::NONE);

        #[derive(Debug, Clone, Serialize, Deserialize)]
        struct RefreshCommand {
            url: String,
        }

        #[queue(name = "keyed-refreshes", job = RefreshCommand)]
        struct KeyedRefreshesQueue;

        #[nest_rs_core::injectable]
        struct KeyedProcessor {
            #[inject]
            upstream: Arc<ResiliencePolicy>,
        }

        impl KeyedProcessor {
            /// A refresh the upstream refused is left for the next sweep.
            async fn skipped(
                &self,
                job: RefreshCommand,
                error: ResilienceError<UpstreamDown>,
            ) -> Result<(), ResilienceError<UpstreamDown>> {
                if job.url.is_empty() {
                    return Err(error);
                }
                Ok(())
            }
        }

        #[processor]
        impl KeyedProcessor {
            #[process(queue = KeyedRefreshesQueue, retries = 3)]
            #[resilient(upstream, record_if = |_| false, fallback = skipped, cancel = token)]
            async fn refresh(
                &self,
                job: RefreshCommand,
            ) -> Result<(), ResilienceError<UpstreamDown>> {
                drop(token);
                if job.url.is_empty() {
                    return Ok(());
                }
                Err(UpstreamDown)
            }
        }

        #[tokio::test(start_paused = true)]
        async fn a_job_is_done_once_its_fallback_answered_for_it() {
            let (_app, upstream) = crate::policy(counting()).await;
            let container = Container::builder()
                .provide(KeyedProcessor {
                    upstream: Arc::clone(&upstream),
                })
                .build()
                .expect("the container builds");
            let method = nest_rs_core::inventory::iter::<ProcessMethod>()
                .find(|method| method.name() == "KeyedProcessor::refresh")
                .expect("the method is submitted");
            for _ in 0..2 {
                let mut delivery = Delivery::new(
                    &BARE,
                    QueueName::new("keyed-refreshes").expect("a valid name"),
                    serde_json::json!({
                        "v": WIRE_FORMAT_VERSION,
                        "payload": { "url": "https://example.com" },
                    }),
                );
                let outcome =
                    consume::attempt(method, &mut delivery, container.clone(), None).await;
                assert!(matches!(outcome, AttemptOutcome::Ok), "{outcome:?}");
            }
            assert_eq!(upstream.breaker_state(), Some(BreakerState::Closed));
        }
    }
}

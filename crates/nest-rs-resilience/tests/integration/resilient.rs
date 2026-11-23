//! Covers `src/resilient.rs`, what `#[resilient]` expands to: a method's body
//! sent once or again as its attribute declares, written out or by a macro,
//! its answer borrowed through the lifetime its receiver names, its arguments
//! read again at each attempt, `?` and `return` answering one attempt, each
//! answer coerced as the undecorated method's is, the `timeout` and `retries`
//! a method pins, the lines naming it, the docs' recipes as they run (an answer
//! that is no failure, a fallback, a breaker opened by hand), each receiver and
//! host it takes, a refusal answered `503` by every edge serving a decorated
//! handler, a method of every impl half that takes one, called through the
//! framework's own path, attempted again within that one call, and what a
//! decorated method's future holds (`footprint`): its size against its
//! undecorated twin's, and a large one run on a worker of tokio's default stack.

use std::cell::Cell;
use std::future::Future;
use std::str::FromStr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use nest_rs_core::{Module, error_message, injectable, instance, module};
use nest_rs_resilience::{
    BreakerState, ResilienceConfig, ResilienceError, ResilienceModule, ResiliencePolicy, resilient,
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

// The methods come through `tt` fragments, keeping the caller's spans, and their
// `#[resilient]` the macro's — or the attribute the caller's and the method the
// macro's: either way the field and the receiver are written in two contexts.
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

/// A decorated handler and `#[sse]` route of a `#[routes]` block, failing, then
/// refused, and an `#[sse]` route attempted again.
mod routes {
    use std::sync::Arc;

    use http::StatusCode;
    use nest_rs_core::{code, module};
    use nest_rs_http::futures_util::stream;
    use nest_rs_http::{SseEvent, SseStream, controller, routes};
    use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
    use nest_rs_testing::TestApp;

    use super::{Attempts, UpstreamDown, opening_on_the_first_failure, retrying_once};

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

    #[tokio::test]
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

    #[tokio::test]
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
}

/// A decorated root field and subscription of an `#[operations]` block, and its
/// mutation, field resolver, entity and subscription attempted again.
mod operations {
    use std::sync::Arc;

    use async_graphql::futures_util::StreamExt as _;
    use async_graphql::futures_util::stream::{self, BoxStream};
    use nest_rs_core::module;
    use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
    use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
    use nest_rs_testing::TestApp;
    use serde_json::{Value, json};

    use super::{Attempts, UpstreamDown, opening_on_the_first_failure, retrying_once};

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

    #[tokio::test]
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

    #[tokio::test]
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

/// A decorated message of a `#[messages]` block.
mod messages {
    use std::sync::Arc;

    use nest_rs_core::module;
    use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
    use nest_rs_testing::TestApp;
    use nest_rs_ws::{WsModule, gateway, messages};

    use super::{UpstreamDown, opening_on_the_first_failure};

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
        let mut socket = app.socket("/ws/decorated").connect().await;

        socket.send("decorated", serde_json::Value::Null).await;
        let failed = socket.next_envelope().await;
        assert_eq!(failed["data"]["code"], "INTERNAL", "{failed}");

        socket.send("decorated", serde_json::Value::Null).await;
        let refused = socket.next_envelope().await;
        assert_eq!(refused["data"]["code"], "UNAVAILABLE", "{refused}");
        assert_eq!(refused["data"]["retryAfterSeconds"], 30, "{refused}");

        app.shutdown().await.expect("the transport stops cleanly");
    }
}

/// A decorated tool and resource listing of a `#[tools]` block, and its prompt,
/// resource and listing attempted again.
mod tools {
    use std::sync::Arc;

    use nest_rs_core::module;
    use nest_rs_mcp::model::{GetPromptResult, PromptMessage, Resource, Role};
    use nest_rs_mcp::{McpAccess, McpError, McpModule, mcp, tools};
    use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
    use nest_rs_testing::TestApp;
    use nest_rs_testing::mcp::{call_method, call_tool, open_session, result};
    use serde_json::{Value, json};

    use super::{Attempts, UpstreamDown, opening_on_the_first_failure, retrying_once};

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

    #[tokio::test]
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

    #[tokio::test]
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
/// back the breaker's wait.
mod processor {
    use std::sync::Arc;
    use std::time::Duration;

    use nest_rs_core::Container;
    use nest_rs_queue::__private::consume::{self, AttemptOutcome, Delivery};
    use nest_rs_queue::{
        Capabilities, ProcessMethod, QueueBackend, QueueName, WIRE_FORMAT_VERSION, processor, queue,
    };
    use nest_rs_resilience::{ResilienceError, ResiliencePolicy, resilient};
    use serde::{Deserialize, Serialize};

    use super::{UpstreamDown, opening_on_the_first_failure};

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

    #[tokio::test]
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
}

/// A decorated tick of a `#[scheduled]` block, attempted again within the one
/// run its trigger fires.
mod scheduled {
    use std::sync::Arc;
    use std::time::Duration;

    use nest_rs_core::operation_log::{OK, TARGET as OPERATIONS};
    use nest_rs_core::{injectable, module};
    use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
    use nest_rs_schedule::{Scheduler, scheduled};
    use nest_rs_testing::{HeadlessApp, LogCapture};

    use super::{Attempts, UpstreamDown, retrying_once};

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
}

/// A decorated listener of a `#[listeners]` block, attempted again within the
/// one call an emit makes.
mod listeners {
    use std::sync::Arc;

    use nest_rs_core::operation_log::{OK, TARGET as OPERATIONS};
    use nest_rs_core::{injectable, module};
    use nest_rs_events::{EventBus, EventsModule, listeners};
    use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
    use nest_rs_testing::{HeadlessApp, LogCapture};

    use super::{Attempts, UpstreamDown, retrying_once};

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
}

/// A decorated indicator of an `#[indicators]` block, attempted again within
/// the one run its probe makes.
mod indicators {
    use std::sync::Arc;

    use nest_rs_core::{injectable, module};
    use nest_rs_health::{HealthModule, HealthService, IndicatorStatus, ProbeKind, indicators};
    use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
    use nest_rs_testing::HeadlessApp;

    use super::{Attempts, UpstreamDown, retrying_once};

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
}

/// A decorated hook of a `#[hooks]` block, attempted again within the one run
/// the app's init makes.
mod hooks {
    use std::sync::Arc;

    use nest_rs_core::{hooks, injectable, module};
    use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
    use nest_rs_testing::HeadlessApp;

    use super::{Attempts, UpstreamDown, retrying_once};

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
    }

    #[module(
        imports = [ResilienceModule::for_root((Courier, quiet()))],
        providers = [BuffersService],
    )]
    struct BuffersModule;

    #[tokio::test]
    async fn a_decorated_methods_future_holds_its_body_and_each_argument_once() {
        let app = boot::<BuffersModule>().await;
        let buffers = app.container().get::<BuffersService>().expect("provided");
        let profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        let holding = size_of_val(&buffers.holding::<HELD>());
        let taking = size_of_val(&buffers.taking::<HELD>([7; HELD]));
        for (shape, decorated, twin) in [
            (
                "holding a buffer across an `.await`, run once",
                size_of_val(&buffers.holding_once::<HELD>()),
                holding,
            ),
            (
                "holding a buffer across an `.await`, run again",
                size_of_val(&buffers.holding_again::<HELD>()),
                holding,
            ),
            (
                "taking a buffer by value, run once",
                size_of_val(&buffers.taking_once::<HELD>([7; HELD])),
                taking,
            ),
            (
                "taking a buffer by value, run again",
                size_of_val(&buffers.taking_again::<HELD>([7; HELD])),
                taking,
            ),
        ] {
            assert!(
                decorated <= twin + RUN,
                "in a {profile} build, a method {shape} is a future of {decorated} B decorated, \
                 of {twin} B undecorated"
            );
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
}

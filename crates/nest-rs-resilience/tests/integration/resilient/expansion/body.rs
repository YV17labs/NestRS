//! What `#[resilient]` expands to: a method's body sent once or again as its
//! attribute declares, written out or by a macro, its answer borrowed through
//! the lifetime its receiver names, its arguments read again at each attempt,
//! `?` and `return` answering one attempt, each answer coerced as the
//! undecorated method's is, the `timeout` and `retries` a method pins, an
//! attempt given up at its unit's deadline, a probe dropped mid-call handing
//! its turn on before its slot of the bulkhead, the lines naming it, the docs'
//! recipes as they run (an answer that is no failure, a fallback, a breaker
//! opened by hand), and each receiver and host it takes.

use std::cell::Cell;
use std::sync::LazyLock;

use nest_rs_testing::{HttpsPeer, PeerAnswer, TestAuthority, TestCertificate};

use super::*;

/// What the carrier answers when it asks to be called again later: a `Problem`
/// in its chain asking the wait.
#[derive(Debug, thiserror::Error)]
#[error("the carrier asked to be called again later")]
struct CarrierBusy(#[source] nest_rs_core::Problem);

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

fn parcels(app: &HeadlessApp) -> Arc<ParcelsService> {
    app.container()
        .get::<ParcelsService>()
        .expect("the module provides it")
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

// The answer is named from the macro's own crate: `$crate` resolves by the context it is written in.
macro_rules! declare_crate_rooted_methods {
    ($once:ident, $again:ident, $fallen:ident) => {
        impl FragmentService {
            #[resilient(courier)]
            async fn $once(&self) -> $crate::resilient::Answer<()> {
                self.attempts.fetch_add(1, Ordering::Relaxed);
                Err(CarrierDown)
            }

            #[resilient(courier, idempotent)]
            async fn $again(&self) -> Result<(), ResilienceError<$crate::resilient::CarrierDown>> {
                self.attempts.fetch_add(1, Ordering::Relaxed);
                Err(CarrierDown)
            }

            #[resilient(courier, fallback = unrouted)]
            async fn $fallen(&self) -> $crate::resilient::Answer<()> {
                self.attempts.fetch_add(1, Ordering::Relaxed);
                Err(CarrierDown)
            }

            async fn unrouted(
                &self,
                error: ResilienceError<CarrierDown>,
            ) -> $crate::resilient::Answer<()> {
                Err(error)
            }
        }
    };
}

declare_crate_rooted_methods!(route, reroute, route_or_not);

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

    assert!(failed(&fragments.route().await));
    assert!(failed(&fragments.reroute().await));
    assert!(failed(&fragments.route_or_not().await));
    assert_eq!(
        fragments.attempted(),
        5,
        "an answer named through `$crate`: once, again, and once under a fallback"
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
            Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. }) if timeout == Duration::from_millis(100)
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
        .run_idempotent(|| async {
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
        matches!(
            refused,
            Err(ResilienceError::Refused {
                refusal: ResilienceRefusal::Full,
                ..
            })
        ),
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
    let Err(ResilienceError::Refused {
        refusal: ResilienceRefusal::Open { retry_after, .. },
        ..
    }) = refused
    else {
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
        matches!(cut, Ok(Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. }))
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
        matches!(refused, Err(ResilienceError::Refused { refusal: ResilienceRefusal::Open { retry_after, .. }, .. })
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
        matches!(refused, Err(ResilienceError::Refused { refusal: ResilienceRefusal::Open { retry_after, .. }, .. })
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
        fallback_if = |error| matches!(error, ResilienceError::Refused { .. })
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
                .run(|| async { Ok::<_, CarrierDown>(Cell::new(7)) })
                .await
        }
    });
    let calls = Arc::new(AtomicU32::new(0));
    let again = tokio::spawn({
        let calls = Arc::clone(&calls);
        async move {
            courier
                .run_idempotent(|| {
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
                .run(|| async { Ok::<_, CarrierDown>(Cell::new(7)) })
                .await
        }
    });
    let calls = Arc::new(AtomicU32::new(0));
    let again = tokio::spawn({
        let calls = Arc::clone(&calls);
        async move {
            courier
                .run_idempotent(|| {
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

/// A host whose decorated method calls another of its own, under one policy.
#[injectable]
struct QuotesService {
    #[inject]
    courier: Arc<ResiliencePolicy<Courier>>,
}

impl QuotesService {
    #[resilient(courier)]
    async fn quoted(&self) -> Result<u32, ResilienceError<ResilienceError<CarrierDown>>> {
        self.priced().await
    }

    #[resilient(courier)]
    async fn priced(&self) -> Answer<u32> {
        Ok(42)
    }
}

#[module(
    imports = [ResilienceModule::for_root((Courier, ResilienceConfig {
        concurrency: Some(1),
        ..opening_on_the_first_failure()
    }))],
    providers = [QuotesService],
)]
struct NestingModule;

/// A decorated method calling another of its own policy is one attempt of
/// it, the inner call let through by nothing again: it takes no second slot
/// of a bulkhead of one, and the probe it makes closes the breaker, where the
/// inner call took the probe's turn from it, was refused, and kept the
/// breaker open for good.
#[tokio::test(start_paused = true)]
async fn a_method_calling_another_of_its_own_policy_is_one_attempt_of_it() {
    let app = boot::<NestingModule>().await;
    let quotes = app
        .container()
        .get::<QuotesService>()
        .expect("the module provides it");
    let courier = held::<Courier>(&app);
    let quoted = quotes.quoted().await;
    assert!(matches!(quoted, Ok(42)), "one slot, taken once: {quoted:?}");

    courier
        .open_breaker(Duration::from_secs(1))
        .expect("an open time the breaker takes");
    tokio::time::advance(Duration::from_secs(1)).await;
    let probed = quotes.quoted().await;
    assert!(matches!(probed, Ok(42)), "{probed:?}");
    assert_eq!(
        courier.breaker_state(),
        Some(BreakerState::Closed),
        "the probe closed the breaker"
    );
}

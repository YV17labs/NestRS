//! The predicates, the fallback and the token on each receiver and host the
//! decorator takes: a generic method, a receiver lent through its `Arc`, a
//! method borrowing through a lifetime it names, a method of a trait impl, and
//! a handler of each impl half, called through the framework's own path.

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
    async fn listed(&self, zone: &str, error: ResilienceError<CarrierDown>) -> Answer<Option<u32>> {
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
        async fn on_delayed(&self, event: Delayed) -> Result<(), ResilienceError<UpstreamDown>> {
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

        async fn stored(&self, id: Path<u32>, _error: ResilienceError<UpstreamDown>) -> Preview {
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
        Capabilities, ProcessMethod, QueueBackend, QueueName, WIRE_FORMAT_VERSION, processor, queue,
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
        async fn refresh(&self, job: RefreshCommand) -> Result<(), ResilienceError<UpstreamDown>> {
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
            let outcome = consume::attempt(method, &mut delivery, container.clone(), None).await;
            assert!(matches!(outcome, AttemptOutcome::Ok), "{outcome:?}");
        }
        assert_eq!(upstream.breaker_state(), Some(BreakerState::Closed));
    }
}

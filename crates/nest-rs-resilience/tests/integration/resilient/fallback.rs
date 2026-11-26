//! What `fallback` and `fallback_if` make of the error a run ends on: the
//! method naming a fallback answers what it answers, handed the arguments and
//! that error — the call's own or each refusal of the policy — unless its
//! filter refuses the error; and every attempt of such a method runs in a
//! savepoint of its unit's transaction, rolled back before the fallback runs.

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

    /// As `last_known`, two of its parameters under a `#[cfg]`: one no
    /// build compiles, of a type none declares, and one every build does.
    async fn last_known_where_compiled(
        &self,
        #[cfg(any())] absent: Absent,
        zone: &str,
        #[cfg_attr(test, cfg(test))] weight: u32,
        error: ResilienceError<CarrierDown>,
    ) -> Answer<u32> {
        self.note(format!("{zone}/{weight} after {}", ended_on(&error)));
        Ok(weight * 2)
    }

    #[resilient(courier, fallback = last_known_where_compiled)]
    async fn rate_where_compiled(
        &self,
        #[cfg(any())] absent: Absent,
        zone: &str,
        #[cfg_attr(test, cfg(test))] weight: u32,
    ) -> Answer<u32> {
        if zone == "home" {
            return Ok(weight);
        }
        Err(CarrierDown)
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
    async fn queued(&self, order: String, error: ResilienceError<CarrierDown>) -> Answer<String> {
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

/// A parameter under a `#[cfg]` is handed in the build that compiles it,
/// under that condition, and named in no other.
#[tokio::test]
async fn a_fallback_is_handed_each_parameter_its_build_compiles() {
    let (_app, rates, _courier) = rates::<FallingBackModule>().await;
    let answered = rates.rate_where_compiled("home", 3).await;
    assert!(matches!(answered, Ok(3)), "{answered:?}");
    let fell_back = rates.rate_where_compiled("north", 3).await;
    assert!(matches!(fell_back, Ok(6)), "{fell_back:?}");
    assert_eq!(rates.journal(), ["north/3 after failed"]);
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
async fn an_idempotent_methods_fallback_answers_after_its_last_attempt_and_takes_what_each_read() {
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
    fn holds(&self) -> nest_rs_database::Holds<'_> {
        nest_rs_database::Holds::Transaction(self)
    }
}

impl nest_rs_database::HeldTransaction for Unit {
    fn after_commit(&self, _work: nest_rs_database::Deferred) {
        unreachable!("no test holds work on this unit")
    }

    fn savepoint(&self) -> Arc<dyn nest_rs_database::AtomicBlock> {
        Arc::new(Savepoint(Arc::clone(&self.0)))
    }

    fn outbox(&self) -> Option<&dyn nest_rs_database::Outbox> {
        None
    }
}

struct Savepoint(Arc<RatesService>);

impl nest_rs_database::Executor for Savepoint {
    fn holds(&self) -> nest_rs_database::Holds<'_> {
        nest_rs_database::Holds::Transaction(self)
    }
}

impl nest_rs_database::HeldTransaction for Savepoint {
    fn after_commit(&self, _work: nest_rs_database::Deferred) {
        unreachable!("no test holds work on this savepoint")
    }

    fn savepoint(&self) -> Arc<dyn nest_rs_database::AtomicBlock> {
        Arc::new(Self(Arc::clone(&self.0)))
    }

    fn outbox(&self) -> Option<&dyn nest_rs_database::Outbox> {
        None
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
/// fallback runs — the attempt failed or given up at its bound — and before
/// an error its filter refused is answered. A method sent once naming no
/// fallback opens none.
#[tokio::test(start_paused = true)]
async fn under_a_fallback_each_attempts_savepoint_is_settled_before_the_fallback_or_the_answer() {
    use nest_rs_database::__private::with_request_executor;

    let (_app, rates, _courier) = rates::<FallingBackModule>().await;
    let unit = || Arc::new(Unit(Arc::clone(&rates))) as Arc<dyn nest_rs_database::Executor>;

    let fell_back = with_request_executor(unit(), rates.rate("north", 3)).await;
    assert!(matches!(fell_back, Ok(6)), "{fell_back:?}");
    assert_eq!(rates.journal(), ["rolled back", "north/3 after failed"]);

    let timed_out = with_request_executor(unit(), rates.slow_rate("north", 3)).await;
    assert!(matches!(timed_out, Ok(6)), "{timed_out:?}");
    assert_eq!(
        rates.journal(),
        ["rolled back", "north/3 after timed out"],
        "an attempt given up at its bound is rolled back as one that failed"
    );

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

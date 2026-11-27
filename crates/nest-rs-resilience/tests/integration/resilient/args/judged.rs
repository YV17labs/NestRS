//! What the predicate keys make of an attempt's answer: `retry_if` and
//! `retry_ok_if` say what is sent again, `record_if` what counts as a failure
//! in the breaker, each alone and together, a closure reading the method's
//! arguments and its host where it names them; and the savepoint an
//! idempotent method's last attempt runs in: kept for the `Ok` it answers,
//! though `retry_ok_if` still takes it, and rolled back for an `Err`, however
//! they judge it.

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
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

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
async fn an_ok_retry_ok_if_takes_is_sent_again_and_the_last_answers_once_the_retries_are_spent() {
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

/// A unit of work holding a transaction, noting how each savepoint it opens
/// is settled.
#[derive(Clone, Default)]
struct Unit(Arc<Mutex<Vec<&'static str>>>);

impl Unit {
    /// How each savepoint was settled since the last read, in order.
    fn settled(&self) -> Vec<&'static str> {
        std::mem::take(&mut *self.0.lock().expect("unpoisoned"))
    }
}

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
        Arc::new(self.clone())
    }

    fn outbox(&self) -> Option<&dyn nest_rs_database::Outbox> {
        None
    }
}

impl nest_rs_database::AtomicBlock for Unit {
    fn settle(
        self: Arc<Self>,
        ok: bool,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), nest_rs_database::AtomicError>> + Send>>
    {
        let settled = if ok { "kept" } else { "rolled back" };
        self.0.lock().expect("unpoisoned").push(settled);
        Box::pin(std::future::ready(Ok(())))
    }
}

/// An `Ok` `retry_ok_if` still takes once the retries are spent is the
/// method's answer, so the savepoint its attempt ran in is kept: each one
/// sent again before it was rolled back.
#[tokio::test(start_paused = true)]
async fn the_last_ok_of_an_idempotent_method_is_kept_though_retry_ok_if_still_takes_it() {
    use nest_rs_database::__private::with_request_executor;

    let (_app, quotes, _courier) = quotes::<RetriedModule>().await;
    let unit = Unit::default();

    let pending = with_request_executor(Arc::new(unit.clone()), quotes.polled(1)).await;
    assert!(
        matches!(pending, Ok(3)),
        "the last `Ok`, the retries spent: {pending:?}"
    );
    assert_eq!(quotes.attempted(), 3);
    assert_eq!(unit.settled(), ["rolled back", "rolled back", "kept"]);
}

/// An `Err` an idempotent method ends on is rolled back whichever way its
/// predicates judge it: sent again or not by `retry_if`, a failure or not by
/// `record_if`, its last attempt's savepoint as each before it.
#[tokio::test(start_paused = true)]
async fn the_last_err_of_an_idempotent_method_is_rolled_back_however_its_predicates_judge_it() {
    use nest_rs_database::__private::with_request_executor;

    let (_app, quotes, _courier) = quotes::<RetriedModule>().await;
    let unit = Unit::default();
    for (failure, judged, attempts) in [
        (NoQuote::Declined, "neither sent again nor a failure", 1),
        (NoQuote::Gone, "a failure sent no more", 1),
        (NoQuote::Busy, "sent again and no failure", 3),
        (NoQuote::Down, "sent again and a failure", 3),
    ] {
        let answered = with_request_executor(Arc::new(unit.clone()), quotes.judged(failure)).await;
        assert!(
            matches!(answered, Err(ResilienceError::Failed(last)) if last == failure),
            "{judged}: {answered:?}"
        );
        assert_eq!(
            unit.settled(),
            ["rolled back"].repeat(attempts),
            "{judged}: each attempt rolled back, the last included"
        );
    }
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

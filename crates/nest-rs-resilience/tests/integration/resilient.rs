//! Covers `src/resilient.rs`, what `#[resilient]` expands to: a method's body
//! sent once or again as its attribute declares, its arguments read again at
//! each attempt, `?` and `return` answering one attempt, the `timeout` and
//! `retries` a method pins, the lines naming it, each receiver and host it
//! takes, and a refusal answered `503` by every edge serving a decorated
//! handler.

use std::cell::Cell;
use std::future::Future;
use std::str::FromStr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nest_rs_core::{Module, injectable, instance, module};
use nest_rs_resilience::{
    BreakerState, ResilienceConfig, ResilienceError, ResilienceModule, ResiliencePolicy, resilient,
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
    providers = [ParcelsService],
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

#[tokio::test(start_paused = true)]
async fn every_method_on_one_field_counts_in_one_breaker() {
    let app = boot::<OpeningModule>().await;
    let parcels = parcels(&app);

    assert!(failed(&parcels.ship().await));
    parcels.attempted();

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

#[injectable]
struct LinksService {
    #[inject]
    courier: Arc<ResiliencePolicy<Courier>>,
}

impl LinksService {
    #[resilient(courier, idempotent)]
    async fn preview(&self, url: &str) -> Result<String, ResilienceError<UpstreamFailed>> {
        let page = reqwest::get(url).await.map_err(UpstreamFailed)?;
        page.text().await.map_err(UpstreamFailed)
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

#[tokio::test]
async fn a_policy_pinned_to_open_on_one_failure_refuses_the_next_call_unsent() {
    let app = boot::<OpeningOnOneFailure>().await;
    let links = app.container().get::<LinksService>().expect("provided");
    let nowhere = "https://127.0.0.1:1/";

    let failed = links.preview(nowhere).await;
    assert!(
        matches!(failed, Err(ResilienceError::Failed(_))),
        "{failed:?}"
    );
    let refused = links.preview(nowhere).await;
    assert!(
        matches!(refused, Err(ResilienceError::Open { retry_after, .. })
            if retry_after <= Duration::from_secs(30)),
        "{refused:?}"
    );
}

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

#[tokio::test(start_paused = true)]
async fn a_decorated_call_runs_on_a_task_of_its_own() {
    let app = boot::<QuietModule>().await;
    let parcels = parcels(&app);

    let spawned = tokio::spawn(async move { parcels.quote().await });
    assert!(matches!(spawned.await.expect("the task ends"), Ok(42)));
}

/// A run whose answer is `Send` and not `Sync` — a `Cell`, a boxed stream — is
/// `Send` itself, run once or again, decorated or not, its wait before a retry
/// included: each compiles on a task of its own, as an `#[sse]` route's or a
/// `#[subscription]`'s answer must.
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

/// A decorated handler of a `#[routes]` block, failing, then refused.
mod routes {
    use std::sync::Arc;

    use http::StatusCode;
    use nest_rs_core::{code, module};
    use nest_rs_http::{controller, routes};
    use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
    use nest_rs_testing::TestApp;

    use super::{UpstreamDown, opening_on_the_first_failure};

    #[controller(path = "/decorated")]
    struct DecoratedController {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
    }

    #[routes]
    impl DecoratedController {
        #[get("/")]
        #[public]
        #[resilient(upstream)]
        async fn preview(&self) -> Result<String, ResilienceError<UpstreamDown>> {
            Err(UpstreamDown)
        }
    }

    #[module(
        imports = [ResilienceModule::for_root(opening_on_the_first_failure())],
        providers = [DecoratedController],
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
}

/// A decorated root field of an `#[operations]` block.
mod operations {
    use std::sync::Arc;

    use nest_rs_core::module;
    use nest_rs_graphql::{GraphqlModule, operations, resolver};
    use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
    use nest_rs_testing::TestApp;

    use super::{UpstreamDown, opening_on_the_first_failure};

    #[resolver]
    struct DecoratedResolver {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
    }

    #[operations]
    impl DecoratedResolver {
        #[query]
        #[public]
        #[resilient(upstream)]
        async fn decorated(&self) -> Result<String, ResilienceError<UpstreamDown>> {
            Err(UpstreamDown)
        }
    }

    #[module(
        imports = [GraphqlModule, ResilienceModule::for_root(opening_on_the_first_failure())],
        providers = [DecoratedResolver],
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

/// A decorated tool of a `#[tools]` block.
mod tools {
    use std::sync::Arc;

    use nest_rs_core::module;
    use nest_rs_mcp::{McpAccess, McpModule, mcp, tools};
    use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
    use nest_rs_testing::TestApp;
    use nest_rs_testing::mcp::call_tool;

    use super::{UpstreamDown, opening_on_the_first_failure};

    const PATH: &str = "/mcp/decorated";

    #[mcp(path = "/mcp/decorated")]
    #[derive(Clone)]
    struct DecoratedTools {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
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
    }

    #[module(
        imports = [
            McpModule::for_root(McpAccess::Public),
            ResilienceModule::for_root(opening_on_the_first_failure()),
        ],
        providers = [DecoratedTools],
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

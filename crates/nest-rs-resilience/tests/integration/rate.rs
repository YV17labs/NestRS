//! Covers `src/rate.rs` through `#[resilient]`: every attempt a decorated
//! method sends takes a token of its policy's rate, a retry included,
//! whichever method on the field sends it; methods finding none free wait in
//! one line, each for its own token; a method the rate refuses is never
//! attempted again and is named by the lines its refusal and its wait file;
//! and the refusal is answered `503` with the time to the next token by a
//! route, and waited out by a job's retry.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use nest_rs_core::{Module, injectable, instance, module};
use nest_rs_resilience::{
    ResilienceConfig, ResilienceError, ResilienceModule, ResiliencePolicy, TARGET, resilient,
};
use nest_rs_testing::{HeadlessApp, LogCapture};

use crate::quiet;

#[instance]
struct Forwarder;

#[derive(Debug, thiserror::Error)]
#[error("the forwarder did not answer")]
struct ForwarderDown;

impl nest_rs_core::ToProblem for ForwarderDown {
    fn to_problem(&self) -> Option<nest_rs_core::Problem> {
        None
    }
}

type Answer<T = ()> = Result<T, ResilienceError<ForwarderDown>>;

#[injectable]
struct ShipmentsService {
    #[inject]
    forwarder: Arc<ResiliencePolicy<Forwarder>>,
    attempts: AtomicU32,
}

impl ShipmentsService {
    fn attempted(&self) -> u32 {
        self.attempts.load(Ordering::Relaxed)
    }

    /// Fails its first attempt, and answers the next.
    #[resilient(forwarder, idempotent)]
    async fn track(&self) -> Answer<&'static str> {
        if self.attempts.fetch_add(1, Ordering::Relaxed) == 0 {
            return Err(ForwarderDown);
        }
        Ok("in transit")
    }

    #[resilient(forwarder)]
    async fn ship(&self) -> Answer {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Answers the instant its body ran at.
    #[resilient(forwarder)]
    async fn weigh(&self) -> Answer<tokio::time::Instant> {
        Ok(tokio::time::Instant::now())
    }

    #[resilient(forwarder, idempotent)]
    async fn insist(&self) -> Answer {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        Err(ForwarderDown)
    }
}

async fn boot<M: Module + 'static>() -> HeadlessApp {
    HeadlessApp::builder()
        .module::<M>()
        .build_headless()
        .await
        .expect("the module boots")
}

fn shipments(app: &HeadlessApp) -> Arc<ShipmentsService> {
    app.container()
        .get::<ShipmentsService>()
        .expect("the module provides it")
}

#[module(
    imports = [ResilienceModule::for_root((Forwarder, ResilienceConfig {
        retries: Some(1),
        rate_limit: Some(2),
        ..quiet()
    }))],
    providers = [ShipmentsService],
)]
struct TwoAMinuteModule;

#[tokio::test(start_paused = true)]
async fn every_attempt_a_decorated_method_sends_takes_a_token_its_retry_included() {
    let app = boot::<TwoAMinuteModule>().await;
    let shipments = shipments(&app);

    assert!(matches!(shipments.track().await, Ok("in transit")));
    assert_eq!(shipments.attempted(), 2, "a failed attempt and its retry");

    let refused = shipments.ship().await;
    assert!(
        matches!(refused, Err(ResilienceError::Limited { .. })),
        "the other method's retry took the bucket's last token: {refused:?}"
    );
    assert_eq!(
        shipments.attempted(),
        2,
        "a call the rate refuses is never sent"
    );
}

#[module(
    imports = [ResilienceModule::for_root((Forwarder, ResilienceConfig {
        retries: Some(3),
        rate_limit: Some(1),
        rate_window: Duration::from_secs(10),
        rate_wait: Some(Duration::from_secs(5)),
        ..quiet()
    }))],
    providers = [ShipmentsService],
)]
struct OneInTenSecondsModule;

#[tokio::test(start_paused = true)]
async fn a_decorated_method_the_rate_refuses_ends_its_run_and_its_lines_name_it() {
    let app = boot::<OneInTenSecondsModule>().await;
    let shipments = shipments(&app);
    let logs = LogCapture::install();

    let refused = shipments.insist().await;
    assert!(
        matches!(refused, Err(ResilienceError::Limited { .. })),
        "its first retry found no token free, the next past the 5 s wait: {refused:?}"
    );
    assert_eq!(
        shipments.attempted(),
        1,
        "the retry the rate refused ends the run, with none after it"
    );
    let retried = logs.find(TARGET, "attempt failed; retrying the call");
    assert_eq!(retried.len(), 1, "{retried:#?}");

    tokio::time::advance(Duration::from_secs(6)).await;
    assert!(
        matches!(shipments.ship().await, Ok(())),
        "a token under 4 s away, within the wait"
    );
    assert_eq!(shipments.attempted(), 2);

    for (message, method) in [
        ("rate limit reached; the call is refused", "insist"),
        ("rate limit reached; waiting for a token", "ship"),
    ] {
        let line = logs.expect_one(TARGET, message);
        assert_eq!(line.level, "debug", "{line:?}");
        assert_eq!(line.field("instance").as_deref(), Some("forwarder"));
        assert_eq!(line.field("host").as_deref(), Some("ShipmentsService"));
        assert_eq!(line.field("method").as_deref(), Some(method), "{line:?}");
    }
}

#[module(
    imports = [ResilienceModule::for_root((Forwarder, ResilienceConfig {
        rate_limit: Some(1),
        rate_window: Duration::from_secs(10),
        rate_wait: Some(Duration::from_secs(20)),
        ..quiet()
    }))],
    providers = [ShipmentsService],
)]
struct WaitingInLineModule;

/// One token every 10 s, taken: two methods on the field, called together,
/// wait in the order they were called, the first for the token at 10 s and
/// the second for the one after, and a third, whose token lies past the 20 s
/// wait, is refused at once.
#[tokio::test(start_paused = true)]
async fn decorated_methods_finding_no_token_free_wait_in_one_line_each_for_its_own_token() {
    let app = boot::<WaitingInLineModule>().await;
    let shipments = shipments(&app);
    let started = tokio::time::Instant::now();
    assert!(matches!(shipments.ship().await, Ok(())), "the one token");

    let (weighed, shipped, refused) = tokio::join!(shipments.weigh(), shipments.ship(), async {
        let refused = shipments.weigh().await;
        (refused, started.elapsed())
    });
    assert!(
        matches!(weighed, Ok(at) if at - started == Duration::from_secs(10)),
        "the first in line, sent at the next token: {weighed:?}"
    );
    assert!(matches!(shipped, Ok(())), "{shipped:?}");
    assert_eq!(
        started.elapsed(),
        Duration::from_secs(20),
        "the second in line, sent at the token after"
    );
    assert!(
        matches!(refused, (Err(ResilienceError::Limited { retry_after, .. }), at)
            if retry_after == Duration::from_secs(30) && at.is_zero()),
        "the third, told at once the 30 s to the token it would be due: {refused:?}"
    );
}

/// A decorated route over a policy sending one attempt per 30 s.
mod routes {
    use std::sync::Arc;
    use std::time::Duration;

    use http::StatusCode;
    use nest_rs_core::{code, module};
    use nest_rs_http::{controller, routes};
    use nest_rs_resilience::{
        ResilienceConfig, ResilienceError, ResilienceModule, ResiliencePolicy, resilient,
    };
    use nest_rs_testing::TestApp;

    use super::ForwarderDown;
    use crate::quiet;

    #[controller(path = "/quotes")]
    struct QuotesController {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
    }

    #[routes]
    impl QuotesController {
        #[get("/")]
        #[public]
        #[resilient(upstream)]
        async fn quote(&self) -> Result<String, ResilienceError<ForwarderDown>> {
            Ok("12 EUR".to_owned())
        }
    }

    #[module(
        imports = [ResilienceModule::for_root(ResilienceConfig {
            rate_limit: Some(1),
            rate_window: Duration::from_secs(30),
            ..quiet()
        })],
        providers = [QuotesController],
    )]
    struct QuotesHttpModule;

    #[tokio::test(start_paused = true)]
    async fn a_route_the_rate_refuses_answers_503_with_the_time_to_the_next_token() {
        let app = TestApp::for_module::<QuotesHttpModule>()
            .await
            .expect("a decorated route boots");

        app.http()
            .get("/quotes")
            .send()
            .await
            .assert_status(StatusCode::OK);

        tokio::time::advance(Duration::from_secs(12)).await;
        let refused = app.http().get("/quotes").send().await;
        refused.assert_status(StatusCode::SERVICE_UNAVAILABLE);
        refused.assert_header("retry-after", "18");
        refused
            .json()
            .await
            .value()
            .object()
            .get("code")
            .assert_string(code::UNAVAILABLE.as_str());
    }
}

/// A decorated job, whose refusal by the rate holds its retry back until the
/// next token.
mod processor {
    use std::sync::Arc;
    use std::time::Duration;

    use nest_rs_core::Container;
    use nest_rs_queue::__private::consume::{self, AttemptOutcome, Delivery};
    use nest_rs_queue::{
        Capabilities, ProcessMethod, QueueBackend, QueueName, WIRE_FORMAT_VERSION, processor, queue,
    };
    use nest_rs_resilience::{ResilienceConfig, ResilienceError, ResiliencePolicy, resilient};
    use serde::{Deserialize, Serialize};

    use super::ForwarderDown;
    use crate::quiet;

    static BARE: QueueBackend = QueueBackend::new("bare", Capabilities::NONE);

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct QuoteCommand {
        parcel: String,
    }

    #[queue(name = "limited-quotes", job = QuoteCommand)]
    struct QuotesQueue;

    #[nest_rs_core::injectable]
    struct QuoteProcessor {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
    }

    #[processor]
    impl QuoteProcessor {
        #[process(queue = QuotesQueue, retries = 3)]
        #[resilient(upstream)]
        async fn quote(&self, job: QuoteCommand) -> Result<(), ResilienceError<ForwarderDown>> {
            drop(job);
            Ok(())
        }
    }

    async fn attempted(container: &Container) -> AttemptOutcome {
        let method = nest_rs_core::inventory::iter::<ProcessMethod>()
            .find(|method| method.name() == "QuoteProcessor::quote")
            .expect("the method is submitted");
        let mut delivery = Delivery::new(
            &BARE,
            QueueName::new("limited-quotes").expect("a valid name"),
            serde_json::json!({
                "v": WIRE_FORMAT_VERSION,
                "payload": { "parcel": "P1" },
            }),
        );
        consume::attempt(method, &mut delivery, container.clone(), None).await
    }

    #[tokio::test(start_paused = true)]
    async fn a_job_the_rate_refuses_waits_at_least_the_time_to_the_next_token_before_its_retry() {
        let (_app, upstream) = crate::policy(ResilienceConfig {
            rate_limit: Some(1),
            rate_window: Duration::from_secs(30),
            ..quiet()
        })
        .await;
        let container = Container::builder()
            .provide(QuoteProcessor { upstream })
            .build()
            .expect("the container builds");

        let sent = attempted(&container).await;
        assert!(matches!(sent, AttemptOutcome::Ok), "{sent:?}");

        let AttemptOutcome::Retry { after } = attempted(&container).await else {
            panic!("a 503 another attempt may clear retries");
        };
        let to_the_token = Duration::from_secs(30);
        assert!(
            after >= to_the_token && after <= to_the_token.mul_f64(1.2),
            "the time to the next token, stretched by a fifth at most: {after:?}"
        );
    }
}

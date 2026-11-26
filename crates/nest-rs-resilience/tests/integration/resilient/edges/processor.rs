//! A decorated job of a `#[processor]` block, whose refusal holds its retry
//! back the breaker's wait, and whose handler is sent again within the job's
//! one attempt.

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

use super::{Attempts, UpstreamDown, opening_on_the_first_failure, retried_once, retrying_once};

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

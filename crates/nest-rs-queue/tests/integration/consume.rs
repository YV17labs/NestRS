//! One attempt at a job through the port — the envelope, the handler, the
//! budget — and discovery, whose refusals every backend owes: a queue name
//! outside the rule, two methods on one queue, and each `#[process]` declaration
//! a backend without its capability cannot honour.

use std::any::TypeId;
use std::future::Future;
use std::num::NonZeroU32;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nest_rs_core::Container;
use nest_rs_queue::__private::consume::{self, AttemptOutcome, Delivery};
use nest_rs_queue::__private::{HandlerContext, nest_rs_worker};
use nest_rs_queue::{
    JobError, JobId, ProcessMethod, ProcessOptions, QueueBackend, QueueName, Throttle,
    WIRE_FORMAT_VERSION, processor, queue,
};
use serde_json::json;

/// A job id as a push mints it: a UUID v7.
const JOB_ID: &str = "01890a5d-ac96-774b-bcce-b302099a8057";

use crate::{
    BARE, FULL, TRANSCODED, TranscodeCommand, TranscodeProcessor, TranscodeQueue, method, reaching,
};

fn transcode_delivery(file: &str) -> Delivery {
    Delivery::new(
        &BARE,
        QueueName::new("transcode").expect("a valid name"),
        json!({ "v": WIRE_FORMAT_VERSION, "payload": { "file": file } }),
    )
}

#[tokio::test]
async fn an_enveloped_job_reaches_its_method_through_an_attempt() {
    let container = Container::builder().provide(TranscodeProcessor).build();
    let mut delivery = transcode_delivery("drained.wav");

    let outcome = consume::attempt(
        method("TranscodeProcessor::transcode"),
        &mut delivery,
        container,
    )
    .await;

    assert!(matches!(outcome, AttemptOutcome::Ok), "{outcome:?}");
    assert_eq!(
        TRANSCODED.lock().expect("lock").as_slice(),
        &["drained.wav"]
    );
}

#[tokio::test]
async fn an_unversioned_payload_runs_and_warns_naming_its_queue() {
    let logs = nest_rs_testing::LogCapture::install();
    let container = Container::builder().provide(TranscodeProcessor).build();
    let mut delivery = Delivery::new(
        &BARE,
        QueueName::new("transcode").expect("a valid name"),
        json!({ "file": "legacy.wav" }),
    );

    let outcome = consume::attempt(
        method("TranscodeProcessor::transcode"),
        &mut delivery,
        container,
    )
    .await;

    assert!(matches!(outcome, AttemptOutcome::Ok), "{outcome:?}");
    assert_eq!(TRANSCODED.lock().expect("lock").as_slice(), &["legacy.wav"]);
    let event = logs.expect_one(
        nest_rs_queue::TARGET,
        "processed an unversioned job payload",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("queue").as_deref(), Some("transcode"));
    assert!(
        event.field("hint").is_some(),
        "a bare warn is the defect — the hint says what to do about it: {:?}",
        event.fields,
    );

    let container = Container::builder().provide(TranscodeProcessor).build();
    consume::attempt(
        method("TranscodeProcessor::transcode"),
        &mut delivery,
        container,
    )
    .await;
    assert_eq!(
        logs.find(
            nest_rs_queue::TARGET,
            "processed an unversioned job payload"
        )
        .len(),
        1,
        "one warn per delivery, not one per attempt",
    );
}

#[tokio::test]
async fn a_missing_provider_and_an_undecodable_payload_dead_letter_without_panicking() {
    let missing = consume::attempt(
        method("TranscodeProcessor::transcode"),
        &mut transcode_delivery("x.wav"),
        Container::builder().build(),
    )
    .await;
    let AttemptOutcome::DeadLetter(error) = missing else {
        panic!("a missing provider stays missing on every attempt: {missing:?}");
    };
    assert!(
        error.to_string().contains("not registered"),
        "names the wiring defect: {error}"
    );

    let mut drifted = Delivery::new(
        &BARE,
        QueueName::new("transcode").expect("a valid name"),
        json!({ "v": WIRE_FORMAT_VERSION, "payload": { "wrong_field": "nope" } }),
    );
    let undecodable = consume::attempt(
        method("TranscodeProcessor::transcode"),
        &mut drifted,
        Container::builder().provide(TranscodeProcessor).build(),
    )
    .await;
    let AttemptOutcome::DeadLetter(error) = undecodable else {
        panic!("the same bytes never decode on a retry: {undecodable:?}");
    };
    assert!(
        error.to_string().contains("failed to deserialize job"),
        "names the decode failure: {error}",
    );
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
enum CardKind {
    Visa,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct ChargeCommand {
    amount: u64,
    card: CardKind,
}

#[queue(name = "charges", job = ChargeCommand)]
struct ChargeQueue;

struct ChargeProcessor;

impl nest_rs_core::ProviderResidency for ChargeProcessor {
    const SINGLETON: bool = true;
}

#[processor]
impl ChargeProcessor {
    #[process(queue = ChargeQueue)]
    async fn charge(&self, _job: ChargeCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn an_undecodable_payload_is_dead_lettered_without_its_values() {
    for (payload, said) in [
        (
            json!({ "amount": "sk_live_51HsecretTOKEN", "card": "Visa" }),
            "failed to deserialize job for queue `charges`: invalid type: a string, expected u64",
        ),
        (
            json!({ "amount": 1, "card": "4242424242424242" }),
            "failed to deserialize job for queue `charges`: unknown variant, expected `Visa`",
        ),
    ] {
        let logs = nest_rs_testing::LogCapture::install();
        let mut delivery = Delivery::new(
            &BARE,
            QueueName::new("charges").expect("a valid name"),
            json!({ "v": WIRE_FORMAT_VERSION, "payload": payload }),
        );
        let outcome = consume::attempt(
            method("ChargeProcessor::charge"),
            &mut delivery,
            Container::builder().provide(ChargeProcessor).build(),
        )
        .await;
        let AttemptOutcome::DeadLetter(error) = outcome else {
            panic!("the same bytes never decode on a retry: {outcome:?}");
        };
        assert_eq!(error.to_string(), said, "the dead-letter record's reason");
        let logged = logs.expect_one(
            nest_rs_queue::TARGET,
            "job dead-lettered: non-retryable failure",
        );
        assert_eq!(logged.field("error").as_deref(), Some(said));
    }
}

/// What every receipt handler below decodes its body as: a number, sent a secret.
const SECRET_BODY: &str = r#""sk_live_51HsecretTOKEN""#;

/// serde's sentence for that body, as the report every line and record carries.
const BODY_REPORT: &str = "invalid type: a string, expected u64 at line 1 column 24";

#[queue(name = "receipts-anyhow", job = String)]
struct ReceiptsAnyhowQueue;

#[queue(name = "receipts-serde", job = String)]
struct ReceiptsSerdeQueue;

#[queue(name = "receipts-context", job = String)]
struct ReceiptsContextQueue;

struct ReceiptProcessor;

impl nest_rs_core::ProviderResidency for ReceiptProcessor {
    const SINGLETON: bool = true;
}

/// A body decoding something of its own, in the three shapes a handler returns
/// it: the documented `anyhow::Result` with `?`, serde's error itself, and a
/// `.context(…)` over it.
#[processor]
impl ReceiptProcessor {
    #[process(queue = ReceiptsAnyhowQueue)]
    async fn with_question_mark(&self, body: String) -> anyhow::Result<()> {
        let _amount: u64 = serde_json::from_str(&body)?;
        Ok(())
    }

    #[process(queue = ReceiptsSerdeQueue, retries = 1)]
    async fn returning_serde(&self, body: String) -> Result<(), serde_json::Error> {
        serde_json::from_str::<u64>(&body).map(drop)
    }

    #[process(queue = ReceiptsContextQueue)]
    async fn with_context(&self, body: String) -> anyhow::Result<()> {
        use anyhow::Context;
        let _amount: u64 = serde_json::from_str(&body).context("upstream reply")?;
        Ok(())
    }
}

/// No event this capture holds carries `needle` in any field.
fn assert_never_quoted(logs: &nest_rs_testing::LogCapture, needle: &str) {
    let quoting: Vec<String> = logs
        .events()
        .into_iter()
        .filter(|event| event.fields.values().any(|value| value.contains(needle)))
        .map(|event| format!("{} {:?}", event.message, event.fields))
        .collect();
    assert!(quoting.is_empty(), "lines quoting the value: {quoting:#?}");
}

async fn receipt_attempt(
    method_name: &str,
    queue: &str,
) -> (AttemptOutcome, nest_rs_testing::LogCapture) {
    let logs = nest_rs_testing::LogCapture::install();
    let mut delivery = Delivery::new(
        &BARE,
        QueueName::new(queue.to_owned()).expect("a valid name"),
        json!({ "v": WIRE_FORMAT_VERSION, "payload": SECRET_BODY }),
    );
    let outcome = consume::attempt(
        method(method_name),
        &mut delivery,
        Container::builder().provide(ReceiptProcessor).build(),
    )
    .await;
    (outcome, logs)
}

#[tokio::test]
async fn a_decode_failure_a_handler_returns_is_said_without_its_value_in_every_shape() {
    for (method_name, queue, said) in [
        (
            "ReceiptProcessor::with_question_mark",
            "receipts-anyhow",
            BODY_REPORT.to_owned(),
        ),
        (
            "ReceiptProcessor::with_context",
            "receipts-context",
            format!("upstream reply: {BODY_REPORT}"),
        ),
    ] {
        let (outcome, logs) = receipt_attempt(method_name, queue).await;
        let AttemptOutcome::DeadLetter(error) = outcome else {
            panic!("no retry is declared, so the failure dead-letters: {outcome:?}");
        };
        assert_eq!(
            nest_rs_core::error_message(&error),
            said,
            "the record an adapter keeps"
        );
        let line = logs.expect_one(
            nest_rs_queue::TARGET,
            "job dead-lettered: retry budget spent",
        );
        assert_eq!(line.field("error").as_deref(), Some(said.as_str()));
        assert_never_quoted(&logs, "sk_live");
    }

    let (outcome, logs) =
        receipt_attempt("ReceiptProcessor::returning_serde", "receipts-serde").await;
    assert!(
        matches!(outcome, AttemptOutcome::Retry { .. }),
        "{outcome:?}"
    );
    let warned = logs.expect_one(
        nest_rs_queue::TARGET,
        "job failed; will retry within the budget",
    );
    assert_eq!(
        warned.field("error").as_deref(),
        Some(BODY_REPORT),
        "said once, as its report — not serde's sentence and then the report",
    );
    assert_never_quoted(&logs, "sk_live");
}

static TRIMMED: Mutex<Option<String>> = Mutex::new(None);

// The pipe carrier is the handler's type; the queue's `job` is the wire payload
// the pipe runs on.
#[queue(name = "trimmed", job = String)]
struct TrimmedQueue;

struct TrimProcessor;

impl nest_rs_core::ProviderResidency for TrimProcessor {
    const SINGLETON: bool = true;
}

#[processor]
impl TrimProcessor {
    #[process(queue = TrimmedQueue)]
    async fn trim(
        &self,
        name: nest_rs_pipes::Piped<nest_rs_pipes::Trim, String>,
    ) -> anyhow::Result<()> {
        *TRIMMED.lock().expect("lock") = Some(name.into_inner());
        Ok(())
    }
}

#[tokio::test]
async fn a_piped_job_argument_runs_its_pipe_before_the_handler() {
    let mut delivery = Delivery::new(
        &BARE,
        QueueName::new("trimmed").expect("a valid name"),
        json!({ "v": WIRE_FORMAT_VERSION, "payload": "  hi  " }),
    );

    let outcome = consume::attempt(
        method("TrimProcessor::trim"),
        &mut delivery,
        Container::builder().provide(TrimProcessor).build(),
    )
    .await;

    assert!(matches!(outcome, AttemptOutcome::Ok), "{outcome:?}");
    assert_eq!(
        TRIMMED.lock().expect("lock").as_deref(),
        Some("hi"),
        "the handler saw the trimmed value, not the raw payload",
    );
}

#[queue(name = "id-lists", job = String)]
struct IdListsQueue;

struct IdListProcessor;

impl nest_rs_core::ProviderResidency for IdListProcessor {
    const SINGLETON: bool = true;
}

#[processor]
impl IdListProcessor {
    #[process(queue = IdListsQueue)]
    async fn ids(
        &self,
        _ids: nest_rs_pipes::Piped<nest_rs_pipes::ParseArray<u64>, String>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn a_list_a_pipe_refuses_is_dead_lettered_without_the_refused_item() {
    let logs = nest_rs_testing::LogCapture::install();
    let mut delivery = Delivery::new(
        &BARE,
        QueueName::new("id-lists").expect("a valid name"),
        json!({ "v": WIRE_FORMAT_VERSION, "payload": "1,sk_live_51HsecretTOKEN,3" }),
    );

    let outcome = consume::attempt(
        method("IdListProcessor::ids"),
        &mut delivery,
        Container::builder().provide(IdListProcessor).build(),
    )
    .await;

    let AttemptOutcome::DeadLetter(error) = outcome else {
        panic!("a refusal repeats on every attempt, so it dead-letters: {outcome:?}");
    };
    let recorded = format!("{} {error:?}", nest_rs_core::error_message(&error));
    assert!(
        !recorded.contains("sk_live"),
        "the dead-letter record quotes the item: {recorded}",
    );
    logs.expect_one(
        nest_rs_queue::TARGET,
        "job dead-lettered: non-retryable failure",
    );
    assert_never_quoted(&logs, "sk_live");
}

/// A signup the processor validates itself, the way its service does.
#[nest_rs_core::input]
struct Signup {
    #[validate(length(min = 32))]
    password: String,
}

/// `ServiceError::Validation`'s shape: a constant sentence, the validation
/// failure kept as its source.
#[derive(Debug, thiserror::Error)]
enum SignupError {
    #[error("validation failed")]
    Validation(#[from] nest_rs_core::__private::validator::ValidationErrors),
}

#[queue(name = "signups", job = String)]
struct SignupsQueue;

struct SignupProcessor;

impl nest_rs_core::ProviderResidency for SignupProcessor {
    const SINGLETON: bool = true;
}

#[processor]
impl SignupProcessor {
    #[process(queue = SignupsQueue, retries = 1)]
    async fn sign_up(&self, password: String) -> Result<(), SignupError> {
        use nest_rs_core::__private::validator::Validate;
        Signup { password }.validate()?;
        Ok(())
    }
}

/// validator's own `Display` prints every rule's parameters, the submitted value among them.
#[tokio::test]
async fn a_validation_failure_a_handler_returns_is_said_without_the_submitted_value() {
    let logs = nest_rs_testing::LogCapture::install();
    let container = Container::builder().provide(SignupProcessor).build();
    let sign_up = method("SignupProcessor::sign_up");
    let mut delivery = Delivery::new(
        &BARE,
        QueueName::new("signups").expect("a valid name"),
        json!({ "v": WIRE_FORMAT_VERSION, "payload": "sk_live_51HsecretTOKEN" }),
    );

    let first = consume::attempt(sign_up, &mut delivery, container.clone()).await;
    assert!(matches!(first, AttemptOutcome::Retry { .. }), "{first:?}");
    let last = consume::attempt(sign_up, &mut delivery, container).await;

    let AttemptOutcome::DeadLetter(error) = last else {
        panic!("the second attempt spends the budget: {last:?}");
    };
    let record = nest_rs_core::error_message(&error);
    assert!(
        !record.contains("sk_live"),
        "the dead-letter record quotes the submitted value: {record}",
    );
    logs.expect_one(
        nest_rs_queue::TARGET,
        "job failed; will retry within the budget",
    );
    logs.expect_one(
        nest_rs_queue::TARGET,
        "job dead-lettered: retry budget spent",
    );
    assert_never_quoted(&logs, "sk_live");
}

/// A context that reports it could not honour the attempt, carrying the
/// classification a `WorkerDbContext` reaches from the database's own error.
struct Unsettleable(nest_rs_worker::Unhonoured);

impl nest_rs_worker::JobContext for Unsettleable {
    fn scope<'a>(
        &'a self,
        _transaction: nest_rs_worker::JobTransaction,
        inner: Pin<Box<dyn Future<Output = bool> + Send + 'a>>,
    ) -> Pin<Box<dyn Future<Output = nest_rs_worker::JobSettlement> + Send + 'a>> {
        Box::pin(async move {
            inner.await;
            nest_rs_worker::JobSettlement::Unhonoured(self.0)
        })
    }
}

async fn settle(why: nest_rs_worker::Unhonoured) -> AttemptOutcome {
    let container = Container::builder()
        .provide(TranscodeProcessor)
        .provide_dyn::<dyn nest_rs_worker::JobContext>(Arc::new(Unsettleable(why)))
        .build();
    consume::attempt(
        method("TranscodeProcessor::transcode"),
        &mut transcode_delivery("drained.wav"),
        container,
    )
    .await
}

#[tokio::test]
async fn an_attempt_its_context_could_not_settle_carries_the_classification() {
    let transient = settle(nest_rs_worker::Unhonoured::retryable(
        "the job's transaction could not be committed",
    ))
    .await;
    assert!(
        matches!(transient, AttemptOutcome::Retry { .. }),
        "a serialization conflict is what the retry budget is for: {transient:?}",
    );

    let deterministic = settle(nest_rs_worker::Unhonoured::deterministic(
        "the job's transaction could not be committed",
    ))
    .await;
    let AttemptOutcome::DeadLetter(error) = deterministic else {
        panic!("a failure that repeats identically dead-letters at once: {deterministic:?}");
    };
    assert_eq!(
        error.to_string(),
        "the job's transaction could not be committed",
        "the sentence the context wrote is what the dead-letter record carries",
    );
}

static FLAKY_RUNS: AtomicU32 = AtomicU32::new(0);

struct FlakyProcessor;

impl nest_rs_core::ProviderResidency for FlakyProcessor {
    const SINGLETON: bool = true;
}

#[processor]
impl FlakyProcessor {
    #[process(queue = TranscodeQueue, retries = 2)]
    async fn flaky(&self, _job: TranscodeCommand) -> anyhow::Result<()> {
        FLAKY_RUNS.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("the upstream API timed out")
    }
}

#[tokio::test]
async fn a_retryable_failure_runs_again_while_the_budget_lasts_then_dead_letters() {
    let container = Container::builder().provide(FlakyProcessor).build();
    let flaky = method("FlakyProcessor::flaky");
    let mut delivery = transcode_delivery("x.wav");

    let mut outcomes = Vec::new();
    let mut attempts = Vec::new();
    for _ in 1..=3 {
        attempts.push(delivery.attempt());
        outcomes.push(consume::attempt(flaky, &mut delivery, container.clone()).await);
    }
    assert_eq!(
        attempts,
        [1, 2, 3],
        "the delivery counts, so no adapter does"
    );

    assert!(
        matches!(outcomes[0], AttemptOutcome::Retry { .. }),
        "{outcomes:?}"
    );
    assert!(
        matches!(outcomes[1], AttemptOutcome::Retry { .. }),
        "{outcomes:?}"
    );
    assert!(
        matches!(outcomes[2], AttemptOutcome::DeadLetter(_)),
        "{outcomes:?}"
    );
    assert_eq!(
        FLAKY_RUNS.load(Ordering::SeqCst),
        3,
        "three attempts ran, and the stored job survived the two that were followed by another",
    );
}

/// Told when the stuck method has started, so its attempt is dropped mid-run.
static STUCK_STARTED: tokio::sync::Notify = tokio::sync::Notify::const_new();

struct StuckProcessor;

impl nest_rs_core::ProviderResidency for StuckProcessor {
    const SINGLETON: bool = true;
}

#[processor]
impl StuckProcessor {
    #[process(queue = TranscodeQueue)]
    async fn stuck(&self, _job: TranscodeCommand) -> anyhow::Result<()> {
        STUCK_STARTED.notify_one();
        std::future::pending::<()>().await;
        Ok(())
    }
}

#[tokio::test]
async fn an_attempt_its_driver_drops_files_its_line_cancelled_in_the_jobs_trace() {
    let logs = nest_rs_testing::LogCapture::install();
    let container = Container::builder().provide(StuckProcessor).build();
    let mut delivery = transcode_delivery("stuck.wav");

    {
        let attempt = consume::attempt(method("StuckProcessor::stuck"), &mut delivery, container);
        tokio::select! {
            outcome = attempt => panic!("the stuck method settled: {outcome:?}"),
            () = STUCK_STARTED.notified() => {}
        }
    }

    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_queue::unit::JOB.name(),
    );
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
        "{line:#?}",
    );
    assert_eq!(line.field("attempt").as_deref(), Some("1"));
    assert_eq!(line.field("queue").as_deref(), Some("transcode"));
    assert!(line.field("duration_ms").is_some(), "{line:#?}");
    let span = logs.expect_span(nest_rs_queue::TARGET, nest_rs_queue::unit::JOB.name());
    assert!(
        line.trace_id.is_some(),
        "the line carries the job's trace: {line:#?}"
    );
    assert_eq!(line.trace_id, span.field("trace_id"), "{line:#?}");
    assert_eq!(
        span.field("error.type").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
        "{:?}",
        span.fields,
    );
    assert_eq!(span.field("otel.status_code").as_deref(), Some("error"));
    assert!(
        logs.events()
            .iter()
            .all(|event| !event.message.starts_with("job dead-lettered")
                && !event.message.starts_with("job failed")),
        "a dropped attempt is neither failed nor dead-lettered here",
    );
}

/// The `queue.job` spans an attempt opened, in creation order.
fn job_spans(logs: &nest_rs_testing::LogCapture) -> Vec<nest_rs_testing::CapturedSpan> {
    logs.spans()
        .into_iter()
        .filter(|span| {
            span.target == nest_rs_queue::TARGET && span.name == nest_rs_queue::unit::JOB.name()
        })
        .collect()
}

/// Two attempts at one delivery of `value`, both failing retryably.
async fn two_attempts(value: serde_json::Value) -> nest_rs_testing::LogCapture {
    let logs = nest_rs_testing::LogCapture::install();
    let container = Container::builder().provide(FlakyProcessor).build();
    let flaky = method("FlakyProcessor::flaky");
    let mut delivery = Delivery::new(
        &BARE,
        QueueName::new("transcode").expect("a valid name"),
        value,
    );
    for _ in 1..=2 {
        let outcome = consume::attempt(flaky, &mut delivery, container.clone()).await;
        assert!(
            matches!(outcome, AttemptOutcome::Retry { .. }),
            "{outcome:?}"
        );
    }
    logs
}

#[tokio::test]
async fn every_attempt_at_a_delivery_without_a_usable_trace_runs_in_the_first_attempts_trace() {
    let cases = [
        ("no envelope", json!({ "file": "x.wav" }), None),
        (
            "an actor alone",
            json!({ "v": WIRE_FORMAT_VERSION, "payload": { "file": "x.wav" }, "actor_id": "user-7" }),
            Some("user-7"),
        ),
        (
            "a corrupt traceparent",
            json!({ "v": WIRE_FORMAT_VERSION, "payload": { "file": "x.wav" }, "traceparent": "00-zz" }),
            None,
        ),
    ];
    for (case, value, actor) in cases {
        let logs = two_attempts(value).await;
        let spans = job_spans(&logs);
        assert_eq!(spans.len(), 2, "{case}: {spans:#?}");
        let (first, second) = (&spans[0], &spans[1]);
        assert!(first.field("trace_id").is_some(), "{case}: {first:#?}");
        assert_eq!(
            first.field("trace_id"),
            second.field("trace_id"),
            "{case}: the attempts at one delivery share one trace",
        );
        assert_eq!(
            second.field("parent_span_id"),
            first.field("span_id"),
            "{case}: a later attempt is a child of the first",
        );
        for span in &spans {
            assert_eq!(
                span.field("continued_trace").as_deref(),
                Some("false"),
                "{case}: nothing was continued from the producer",
            );
            assert_eq!(span.field("actor_id").as_deref(), actor, "{case}");
        }
        let started = logs.find(nest_rs_queue::TARGET, "job started");
        assert_eq!(started.len(), 2, "{case}: {started:#?}");
        for (line, span) in started.iter().zip(&spans) {
            assert_eq!(line.trace_id, span.field("trace_id"), "{case}: {line:#?}");
            assert_eq!(line.span_id, span.field("span_id"), "{case}: {line:#?}");
            assert_eq!(line.actor_id.as_deref(), actor, "{case}: {line:#?}");
        }
        if case == "no envelope" {
            let warned = logs.expect_one(
                nest_rs_queue::TARGET,
                "processed an unversioned job payload",
            );
            assert_eq!(
                warned.trace_id,
                spans[0].field("trace_id"),
                "{case}: the warn is filed inside the first attempt's trace: {warned:#?}",
            );
        }
    }
}

#[tokio::test]
async fn every_attempt_at_a_continued_delivery_is_a_child_of_the_enqueue() {
    let enqueue = nest_rs_core::Correlation::minted(None);
    let logs = two_attempts(json!({
        "v": WIRE_FORMAT_VERSION,
        "payload": { "file": "x.wav" },
        "traceparent": enqueue.traceparent().to_string(),
    }))
    .await;

    let spans = job_spans(&logs);
    assert_eq!(spans.len(), 2, "{spans:#?}");
    for span in &spans {
        assert_eq!(span.field("trace_id"), Some(enqueue.trace_id().to_string()));
        assert_eq!(
            span.field("parent_span_id"),
            Some(enqueue.span_id().to_string()),
        );
        assert_eq!(span.field("continued_trace").as_deref(), Some("true"));
    }
    logs.expect_none(
        nest_rs_queue::TARGET,
        "job envelope carries keys this consumer cannot use",
    );
}

#[tokio::test]
async fn an_envelope_carrying_an_unusable_trace_context_runs_and_says_so_once() {
    let logs = two_attempts(json!({
        "v": WIRE_FORMAT_VERSION,
        "id": JOB_ID,
        "payload": { "file": "x.wav" },
        "traceparent": "garbage",
        "actor_id": 7,
    }))
    .await;

    let event = logs.expect_one(
        nest_rs_queue::TARGET,
        "job envelope carries keys this consumer cannot use",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("queue").as_deref(), Some("transcode"));
    assert_eq!(
        event.field("job_id").as_deref(),
        Some(JOB_ID),
        "the id the envelope carries"
    );
    assert_eq!(
        event.field("unusable").as_deref(),
        Some("traceparent, actor_id")
    );
    let hint = event.field("hint").unwrap_or_default();
    assert!(
        hint.contains("runs under a trace of its own") && hint.contains("runs for no actor"),
        "{event:#?}"
    );
    assert!(
        !event.fields.contains_key("actor_id"),
        "a field named `actor_id` would read as the actor the line names: {event:#?}",
    );
    let spans = job_spans(&logs);
    assert_eq!(
        event.trace_id,
        spans[0].field("trace_id"),
        "the warn is filed inside the first attempt's trace: {event:#?}",
    );
    assert_eq!(spans[0].field("actor_id"), None, "a number names no actor");
}

#[tokio::test]
async fn a_usable_trace_beside_an_actor_naming_nobody_says_only_what_the_job_lost() {
    for (case, actor) in [("a number", json!(7)), ("an empty string", json!(""))] {
        let enqueue = nest_rs_core::Correlation::minted(None);
        let logs = two_attempts(json!({
            "v": WIRE_FORMAT_VERSION,
            "payload": { "file": "x.wav" },
            "traceparent": enqueue.traceparent().to_string(),
            "actor_id": actor,
        }))
        .await;

        let event = logs.expect_one(
            nest_rs_queue::TARGET,
            "job envelope carries keys this consumer cannot use",
        );
        assert_eq!(
            event.field("unusable").as_deref(),
            Some("actor_id"),
            "{case}"
        );
        let hint = event.field("hint").unwrap_or_default();
        assert!(
            hint.contains("runs for no actor") && !hint.contains("trace of its own"),
            "{case}: {hint}"
        );
        let spans = job_spans(&logs);
        assert_eq!(
            spans[0].field("continued_trace").as_deref(),
            Some("true"),
            "{case}"
        );
        assert_eq!(spans[0].field("actor_id"), None, "{case}");
    }
}

#[tokio::test]
async fn an_unusable_tracestate_beside_a_usable_trace_is_dropped_and_said() {
    let enqueue = nest_rs_core::Correlation::minted(None);
    let logs = two_attempts(json!({
        "v": WIRE_FORMAT_VERSION,
        "payload": { "file": "x.wav" },
        "traceparent": enqueue.traceparent().to_string(),
        "tracestate": 7,
    }))
    .await;

    let event = logs.expect_one(
        nest_rs_queue::TARGET,
        "job envelope carries keys this consumer cannot use",
    );
    assert_eq!(event.field("unusable").as_deref(), Some("tracestate"));
    assert!(
        event
            .field("hint")
            .is_some_and(|hint| hint.contains("without the vendor state")),
        "{event:#?}"
    );
}

/// Two attempts at `value` the way a backend declaring `DelayedPush` runs them:
/// the first on the stored value, the second on the record the first re-filed —
/// each a delivery of its own, as Redis makes them.
async fn two_refiled_attempts(
    value: serde_json::Value,
) -> (nest_rs_testing::LogCapture, serde_json::Value) {
    let logs = nest_rs_testing::LogCapture::install();
    let container = Container::builder().provide(FlakyProcessor).build();
    let flaky = method("FlakyProcessor::flaky");
    let queue = || QueueName::new("transcode").expect("a valid name");
    let mut first = Delivery::new(&FULL, queue(), value);
    let outcome = consume::attempt(flaky, &mut first, container.clone()).await;
    assert!(
        matches!(outcome, AttemptOutcome::Retry { .. }),
        "{outcome:?}"
    );
    let refiled = first.retry_envelope().into_json();
    let mut second = Delivery::new(&FULL, queue(), refiled.clone());
    let outcome = consume::attempt(flaky, &mut second, container).await;
    assert!(
        matches!(outcome, AttemptOutcome::Retry { .. }),
        "{outcome:?}"
    );
    (logs, refiled)
}

#[tokio::test]
async fn a_minted_trace_refiled_for_a_later_attempt_is_never_read_as_the_producers() {
    for (case, value) in [
        ("no envelope", json!({ "file": "x.wav" })),
        (
            "a corrupt traceparent",
            json!({ "v": WIRE_FORMAT_VERSION, "payload": { "file": "x.wav" }, "traceparent": "00-zz" }),
        ),
    ] {
        let (logs, refiled) = two_refiled_attempts(value).await;
        assert_eq!(refiled["trace_minted"], json!(true), "{case}: {refiled}");
        let spans = job_spans(&logs);
        assert_eq!(spans.len(), 2, "{case}: {spans:#?}");
        let (first, second) = (&spans[0], &spans[1]);
        assert_eq!(
            first.field("trace_id"),
            second.field("trace_id"),
            "{case}: one trace per job"
        );
        assert_eq!(
            second.field("parent_span_id"),
            first.field("span_id"),
            "{case}: a later attempt is a child of the first",
        );
        for span in &spans {
            assert_eq!(
                span.field("continued_trace").as_deref(),
                Some("false"),
                "{case}: nothing was continued from the producer: {span:#?}",
            );
        }
    }

    let enqueue = nest_rs_core::Correlation::minted(None);
    let (logs, refiled) = two_refiled_attempts(json!({
        "v": WIRE_FORMAT_VERSION,
        "payload": { "file": "x.wav" },
        "traceparent": enqueue.traceparent().to_string(),
    }))
    .await;
    assert!(refiled.get("trace_minted").is_none(), "{refiled}");
    for span in job_spans(&logs) {
        assert_eq!(span.field("continued_trace").as_deref(), Some("true"));
        assert_eq!(
            span.field("parent_span_id"),
            Some(enqueue.span_id().to_string())
        );
    }
}

#[tokio::test]
async fn the_keys_a_delivery_could_not_use_are_said_once_per_job_not_per_attempt() {
    let enqueue = nest_rs_core::Correlation::minted(None);
    let (logs, refiled) = two_refiled_attempts(json!({
        "v": WIRE_FORMAT_VERSION,
        "id": JOB_ID,
        "payload": { "file": "x.wav" },
        "traceparent": enqueue.traceparent().to_string(),
        "tracestate": 7,
        "actor_id": 7,
        "unique_key": "a\nb",
    }))
    .await;

    let warned = logs.find(
        nest_rs_queue::TARGET,
        "job envelope carries keys this consumer cannot use",
    );
    assert_eq!(warned.len(), 1, "{warned:#?}");
    assert_eq!(
        warned[0].field("unusable").as_deref(),
        Some("tracestate, actor_id, unique_key")
    );
    for dropped in ["tracestate", "actor_id", "unique_key"] {
        assert!(refiled.get(dropped).is_none(), "`{dropped}`: {refiled}");
    }
    assert_eq!(
        refiled["traceparent"],
        json!(enqueue.traceparent().to_string()),
        "what was usable travels on",
    );
}

/// A W3C `traceparent` a newer release's producer sealed, spelled as this
/// release spells it.
const NEWER_TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

/// What a newer release's push stored: an envelope of the next version, under
/// `id`, carrying the producer's trace.
fn sealed_by_a_newer_release(id: &str) -> serde_json::Value {
    let newer = u64::from(WIRE_FORMAT_VERSION) + 1;
    json!({
        "v": newer,
        "id": id,
        "attempt": 3,
        "traceparent": NEWER_TRACEPARENT,
        "payload": { "file": "x.wav" },
    })
}

/// A UUID v7 minted now — a job pushed a moment ago.
fn fresh_job_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// One attempt at `delivery` of the flaky method, which a newer release's job
/// never reaches.
async fn attempt_newer(delivery: &mut Delivery) -> AttemptOutcome {
    let container = Container::builder().provide(FlakyProcessor).build();
    consume::attempt(method("FlakyProcessor::flaky"), delivery, container).await
}

#[tokio::test]
async fn a_newer_releases_job_carrying_a_key_this_release_never_wrote_is_handed_back() {
    let id = fresh_job_id();
    let mut stored = sealed_by_a_newer_release(&id);
    stored["priority"] = json!(5);
    let mut delivery = Delivery::new(
        &FULL,
        QueueName::new("transcode").expect("a valid name"),
        stored.clone(),
    );
    assert_eq!(delivery.id().to_string(), id, "its id is read");
    let runs = FLAKY_RUNS.load(Ordering::SeqCst);
    let outcome = attempt_newer(&mut delivery).await;
    assert!(
        matches!(outcome, AttemptOutcome::Defer { .. }),
        "handed back unread: {outcome:?}"
    );
    assert_eq!(FLAKY_RUNS.load(Ordering::SeqCst), runs, "nothing ran");
    assert_eq!(delivery.retry_envelope().into_json(), stored, "as stored");
}

#[tokio::test]
async fn a_job_a_newer_release_sealed_is_handed_back_unread_within_the_patience() {
    let logs = nest_rs_testing::LogCapture::install();
    let stored = sealed_by_a_newer_release(JOB_ID);
    let mut delivery = Delivery::new(
        &FULL,
        QueueName::new("transcode").expect("a valid name"),
        stored.clone(),
    )
    .with_deferred_for(Duration::from_secs(60 * 60));
    assert_eq!(
        delivery.id().to_string(),
        JOB_ID,
        "the id the push returned finds the job"
    );

    let runs = FLAKY_RUNS.load(Ordering::SeqCst);
    for _ in 1..=2 {
        let outcome = attempt_newer(&mut delivery).await;
        let AttemptOutcome::Defer { after } = outcome else {
            panic!("a job this release cannot read is handed back: {outcome:?}");
        };
        assert_eq!(after, nest_rs_queue::NEWER_RELEASE_WAIT);
    }
    assert_eq!(FLAKY_RUNS.load(Ordering::SeqCst), runs, "nothing ran");
    assert_eq!(delivery.attempt(), 1, "no attempt is spent");
    assert_eq!(
        delivery.retry_envelope().into_json(),
        stored,
        "the record goes back as it was stored",
    );

    let warned = logs.expect_one(
        nest_rs_queue::TARGET,
        "job sealed by a newer release handed back unread",
    );
    assert_eq!(warned.level, "warn");
    assert_eq!(warned.field("job_id").as_deref(), Some(JOB_ID));
    assert_eq!(
        warned.field("version"),
        Some((u64::from(WIRE_FORMAT_VERSION) + 1).to_string())
    );
    assert_eq!(
        warned.field("supported"),
        Some(WIRE_FORMAT_VERSION.to_string())
    );
    assert_eq!(warned.field("waited_ms").as_deref(), Some("3600000"));
    assert_eq!(
        warned.field("patience_ms"),
        Some(
            nest_rs_queue::NEWER_RELEASE_PATIENCE
                .as_millis()
                .to_string()
        )
    );
    assert_eq!(
        warned.trace_id.as_deref(),
        Some("4bf92f3577b34da6a3ce929d0e0e4736"),
        "the line is filed in the trace the newer producer sealed",
    );
    assert!(
        job_spans(&logs).is_empty(),
        "no attempt ran, so none is reported"
    );
}

#[tokio::test]
async fn a_job_a_newer_release_sealed_is_dead_lettered_once_it_waited_unread_past_the_patience() {
    let logs = nest_rs_testing::LogCapture::install();
    let mut delivery = Delivery::new(
        &FULL,
        QueueName::new("transcode").expect("a valid name"),
        sealed_by_a_newer_release(&fresh_job_id()),
    )
    .with_deferred_for(nest_rs_queue::NEWER_RELEASE_PATIENCE);

    let runs = FLAKY_RUNS.load(Ordering::SeqCst);
    let outcome = attempt_newer(&mut delivery).await;
    let AttemptOutcome::DeadLetter(error) = outcome else {
        panic!("past the patience, a newer release's job is dead-lettered: {outcome:?}");
    };
    assert_eq!(FLAKY_RUNS.load(Ordering::SeqCst), runs, "nothing ran");
    assert!(!error.retryable);
    let newer = (u64::from(WIRE_FORMAT_VERSION) + 1).to_string();
    let sentence = error.to_string();
    for named in [
        format!("wire-format version {newer}"),
        format!("(version {WIRE_FORMAT_VERSION})"),
        "kept with the dead letters".to_owned(),
    ] {
        assert!(sentence.contains(&named), "{named:?} in {sentence}");
    }

    let said = logs.expect_one(
        nest_rs_queue::TARGET,
        "job dead-lettered: a newer release sealed it, and none of its consumers ran it in time",
    );
    assert_eq!(said.level, "error");
    assert_eq!(said.field("version"), Some(newer));
    assert_eq!(
        said.field("waited_ms"),
        Some(
            nest_rs_queue::NEWER_RELEASE_PATIENCE
                .as_millis()
                .to_string()
        )
    );
    assert_eq!(
        said.trace_id.as_deref(),
        Some("4bf92f3577b34da6a3ce929d0e0e4736")
    );
    assert!(
        logs.find(
            nest_rs_queue::TARGET,
            "job sealed by a newer release handed back unread"
        )
        .is_empty()
    );
    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_queue::unit::JOB.name(),
    );
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::ERROR)
    );
    assert_eq!(
        line.trace_id.as_deref(),
        Some("4bf92f3577b34da6a3ce929d0e0e4736")
    );
}

#[tokio::test]
async fn a_newer_release_job_on_a_backend_keeping_no_record_is_aged_from_its_push() {
    let queue = || QueueName::new("transcode").expect("a valid name");
    let mut fresh = Delivery::new(&FULL, queue(), sealed_by_a_newer_release(&fresh_job_id()));
    assert!(matches!(
        attempt_newer(&mut fresh).await,
        AttemptOutcome::Defer { .. }
    ));
    let mut old = Delivery::new(&FULL, queue(), sealed_by_a_newer_release(JOB_ID));
    assert!(matches!(
        attempt_newer(&mut old).await,
        AttemptOutcome::DeadLetter(_)
    ));
}

#[tokio::test]
async fn a_newer_release_job_naming_no_id_is_dead_lettered_at_once() {
    let logs = nest_rs_testing::LogCapture::install();
    let mut stored = sealed_by_a_newer_release("not-a-job-id");
    stored
        .as_object_mut()
        .expect("an envelope")
        .remove("traceparent");
    let mut delivery = Delivery::new(
        &FULL,
        QueueName::new("transcode").expect("a valid name"),
        stored,
    )
    .with_deferred_for(Duration::ZERO);
    let outcome = attempt_newer(&mut delivery).await;
    let AttemptOutcome::DeadLetter(error) = outcome else {
        panic!("a job no wait can be counted for is dead-lettered: {outcome:?}");
    };
    assert!(error.to_string().contains("names no id"), "{error}");
    let said = logs.expect_one(
        nest_rs_queue::TARGET,
        "job dead-lettered: a newer release sealed it, and none of its consumers ran it in time",
    );
    assert_eq!(said.field("waited_ms"), None, "no wait was counted");
}

type Handled<'a> = Pin<Box<dyn Future<Output = Result<(), JobError>> + Send + 'a>>;

fn never_runs(
    _payload: std::borrow::Cow<'_, serde_json::Value>,
    _context: HandlerContext,
) -> Handled<'_> {
    Box::pin(async { Ok(()) })
}

struct ThrottledHost;
struct CheckpointHost;
struct FirstClaimant;
struct SecondClaimant;
struct BadlyNamedHost;
struct ZeroWindowHost;
struct SubMillisecondWindowHost;

nest_rs_core::inventory::submit! {
    nest_rs_queue::__private::process_method(
        module_path!(), "ThrottledHost::run", "throttled",
        ProcessOptions::DEFAULT.with_throttle(Throttle::new(NonZeroU32::MIN, Duration::from_secs(1))),
        TypeId::of::<ThrottledHost>, never_runs,
    )
}

nest_rs_core::inventory::submit! {
    nest_rs_queue::__private::process_method(
        module_path!(), "CheckpointHost::run", "resumable",
        ProcessOptions::DEFAULT.with_checkpoint(true),
        TypeId::of::<CheckpointHost>, never_runs,
    )
}

nest_rs_core::inventory::submit! {
    nest_rs_queue::__private::process_method(
        module_path!(), "FirstClaimant::drain", "contested",
        ProcessOptions::DEFAULT,
        TypeId::of::<FirstClaimant>, never_runs,
    )
}

nest_rs_core::inventory::submit! {
    nest_rs_queue::__private::process_method(
        module_path!(), "SecondClaimant::drain", "contested",
        ProcessOptions::DEFAULT.with_retries(9),
        TypeId::of::<SecondClaimant>, never_runs,
    )
}

nest_rs_core::inventory::submit! {
    nest_rs_queue::__private::process_method(
        module_path!(), "BadlyNamedHost::run", "nestrs:queue:dead",
        ProcessOptions::DEFAULT,
        TypeId::of::<BadlyNamedHost>, never_runs,
    )
}

nest_rs_core::inventory::submit! {
    nest_rs_queue::__private::process_method(
        module_path!(), "SubMillisecondWindowHost::run", "half-a-millisecond",
        ProcessOptions::DEFAULT.with_throttle(Throttle::new(NonZeroU32::MIN, Duration::from_micros(500))),
        TypeId::of::<SubMillisecondWindowHost>, never_runs,
    )
}

nest_rs_core::inventory::submit! {
    nest_rs_queue::__private::process_method(
        module_path!(), "ZeroWindowHost::run", "unwindowed",
        ProcessOptions::DEFAULT.with_throttle(Throttle::new(NonZeroU32::MIN, Duration::ZERO)),
        TypeId::of::<ZeroWindowHost>, never_runs,
    )
}

/// What discovery answers for an app reaching only `host`, on `backend`.
fn discover_only<H: 'static>(
    backend: &QueueBackend,
) -> anyhow::Result<Vec<&'static ProcessMethod>> {
    consume::discover(&reaching(&[TypeId::of::<H>()]), backend)
}

/// The refusal a backend without `capability` owes a method declaring it, at boot.
fn assert_refused_at_boot<H: 'static>(site: &str, capability: &str) {
    let refusal = discover_only::<H>(&BARE)
        .expect_err("a backend without the capability refuses the declaration at boot")
        .to_string();
    for part in [site, capability, "`bare`"] {
        assert!(
            refusal.contains(part),
            "the refusal names {part}: {refusal}"
        );
    }
    assert!(
        discover_only::<H>(&FULL).is_ok(),
        "a backend declaring the capability serves the method",
    );
}

#[test]
fn throttle_is_refused_at_boot_by_a_backend_without_it() {
    assert_refused_at_boot::<ThrottledHost>("ThrottledHost::run", "throttling");
}

#[test]
fn checkpoint_is_refused_at_boot_by_a_backend_without_it() {
    assert_refused_at_boot::<CheckpointHost>("CheckpointHost::run", "checkpoints");
}

#[test]
fn two_methods_on_one_queue_fail_the_boot_naming_both() {
    let refusal = consume::discover(
        &reaching(&[
            TypeId::of::<FirstClaimant>(),
            TypeId::of::<SecondClaimant>(),
        ]),
        &FULL,
    )
    .expect_err("two claimants on one queue must not boot")
    .to_string();
    for part in ["contested", "FirstClaimant::drain", "SecondClaimant::drain"] {
        assert!(refusal.contains(part), "{refusal}");
    }
}

#[test]
fn an_entry_draining_a_name_outside_the_rule_fails_the_boot() {
    let refusal = discover_only::<BadlyNamedHost>(&FULL)
        .expect_err("a hand-built entry reaches the boot unchecked, and the boot checks it")
        .to_string();
    assert!(
        refusal.contains("BadlyNamedHost::run") && refusal.contains("nestrs:queue:dead"),
        "{refusal}",
    );
}

#[test]
fn an_entry_with_a_throttle_window_under_a_millisecond_fails_the_boot() {
    for (site, refusal) in [
        (
            "ZeroWindowHost::run",
            discover_only::<ZeroWindowHost>(&FULL),
        ),
        (
            "SubMillisecondWindowHost::run",
            discover_only::<SubMillisecondWindowHost>(&FULL),
        ),
    ] {
        let refusal = refusal
            .expect_err("a window under a millisecond limits nothing")
            .to_string();
        assert!(
            refusal.contains(site) && refusal.contains("under a millisecond"),
            "{refusal}",
        );
    }
}

#[test]
fn a_method_another_app_owns_is_not_this_apps_to_refuse() {
    // Every entry above is linked into this binary; none is reachable here, so
    // none of their refusals is this app's.
    let methods =
        consume::discover(&reaching(&[]), &BARE).expect("nothing reachable, nothing refused");
    assert!(methods.is_empty());
}

#[tokio::test]
async fn an_attempt_is_named_for_its_queue() {
    let logs = nest_rs_testing::LogCapture::install();
    let container = Container::builder().provide(TranscodeProcessor).build();
    let mut delivery = transcode_delivery("named.wav");

    consume::attempt(
        method("TranscodeProcessor::transcode"),
        &mut delivery,
        container,
    )
    .await;

    let span = logs.expect_span(nest_rs_queue::TARGET, nest_rs_queue::unit::JOB.name());
    assert_eq!(
        span.field("otel.name").as_deref(),
        Some("process transcode")
    );
    assert_eq!(
        span.field("messaging.destination.name").as_deref(),
        Some("transcode"),
    );
}

#[tokio::test]
async fn an_undeliverable_record_is_one_unit_of_work_the_port_reports() {
    let logs = nest_rs_testing::LogCapture::install();

    let error = consume::refuse(
        &BARE,
        &QueueName::new("reports").expect("a queue name"),
        Some("backend-9"),
        JobError::abort("the stored entry is not one a producer files"),
    )
    .await;

    assert!(!error.retryable);
    let event = logs.expect_one(nest_rs_queue::TARGET, "job dead-lettered: undeliverable");
    assert_eq!(event.level, "error");
    assert_eq!(event.field("queue").as_deref(), Some("reports"));
    assert_eq!(
        event.field("job_id"),
        None,
        "a record not read that far names no job"
    );
    assert_eq!(event.field("backend_id").as_deref(), Some("backend-9"));
    assert!(
        event
            .field("error")
            .is_some_and(|error| error.contains("not one a producer files")),
        "{event:?}"
    );
    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_queue::unit::JOB.name(),
    );
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::ERROR)
    );
    assert!(line.trace_id.is_some(), "the line carries its unit's trace");
    logs.expect_span(nest_rs_queue::TARGET, nest_rs_queue::unit::JOB.name());
}

#[tokio::test]
async fn the_job_id_is_the_envelopes_and_the_backends_own_id_rides_beside_it() {
    let logs = nest_rs_testing::LogCapture::install();
    let container = Container::builder().provide(TranscodeProcessor).build();
    let mut delivery = Delivery::new(
        &BARE,
        QueueName::new("transcode").expect("a valid name"),
        json!({ "v": WIRE_FORMAT_VERSION, "id": JOB_ID, "payload": { "file": "id.wav" } }),
    )
    .with_backend_id("1767225600000-0");
    assert_eq!(delivery.id().to_string(), JOB_ID);

    let outcome = consume::attempt(
        method("TranscodeProcessor::transcode"),
        &mut delivery,
        container,
    )
    .await;
    assert!(matches!(outcome, AttemptOutcome::Ok), "{outcome:?}");

    let span = logs.expect_span(nest_rs_queue::TARGET, nest_rs_queue::unit::JOB.name());
    assert_eq!(span.field("messaging.message.id").as_deref(), Some(JOB_ID));
    assert_eq!(span.field("backend_id").as_deref(), Some("1767225600000-0"));
    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_queue::unit::JOB.name(),
    );
    assert_eq!(line.field("job_id").as_deref(), Some(JOB_ID));
    assert_eq!(line.field("backend_id").as_deref(), Some("1767225600000-0"));
}

#[tokio::test]
async fn a_record_naming_no_job_id_runs_under_one_minted_for_its_delivery() {
    let container = Container::builder().provide(FlakyProcessor).build();
    let mut delivery = transcode_delivery("x.wav");
    let minted = delivery.id().clone();
    assert!(
        JobId::parse(&minted.to_string()).is_ok(),
        "a v7, as a push mints"
    );

    let outcome = consume::attempt(method("FlakyProcessor::flaky"), &mut delivery, container).await;
    assert!(
        matches!(outcome, AttemptOutcome::Retry { .. }),
        "{outcome:?}"
    );
    assert_eq!(delivery.id(), &minted);
    assert_eq!(delivery.retry_envelope().id(), &minted);
}

#[tokio::test]
async fn a_retry_says_how_long_to_wait_before_the_next_attempt() {
    let logs = nest_rs_testing::LogCapture::install();
    let container = Container::builder().provide(FlakyProcessor).build();
    let flaky = method("FlakyProcessor::flaky");
    let mut delivery = transcode_delivery("x.wav");

    let mut waits = Vec::new();
    for _ in 1..=2 {
        let AttemptOutcome::Retry { after } =
            consume::attempt(flaky, &mut delivery, container.clone()).await
        else {
            panic!("the budget holds two retries");
        };
        waits.push(after);
    }
    assert!(
        waits[0] >= Duration::from_millis(800) && waits[0] <= Duration::from_millis(1200),
        "one second after the first failure, jittered: {waits:?}"
    );
    assert!(
        waits[1] >= Duration::from_millis(1600) && waits[1] <= Duration::from_millis(2400),
        "twice that after the second: {waits:?}"
    );
    let warned = logs.find(
        nest_rs_queue::TARGET,
        "job failed; will retry within the budget",
    );
    assert_eq!(
        warned[0].field("retry_after_ms"),
        Some(waits[0].as_millis().to_string()),
        "{warned:#?}"
    );
}

#[tokio::test]
async fn an_envelope_re_filed_for_a_later_attempt_spends_the_budget_from_there() {
    let logs = nest_rs_testing::LogCapture::install();
    let container = Container::builder().provide(FlakyProcessor).build();
    let mut delivery = Delivery::new(
        &BARE,
        QueueName::new("transcode").expect("a valid name"),
        json!({
            "v": WIRE_FORMAT_VERSION,
            "id": JOB_ID,
            "attempt": 3,
            "payload": { "file": "x.wav" },
        }),
    );
    assert_eq!(delivery.attempt(), 3);

    let outcome = consume::attempt(method("FlakyProcessor::flaky"), &mut delivery, container).await;
    assert!(
        matches!(outcome, AttemptOutcome::DeadLetter(_)),
        "{outcome:?}"
    );
    let spent = logs.expect_one(
        nest_rs_queue::TARGET,
        "job dead-lettered: retry budget spent",
    );
    assert_eq!(spent.field("attempts").as_deref(), Some("3"));
    let span = logs.expect_span(nest_rs_queue::TARGET, nest_rs_queue::unit::JOB.name());
    assert_eq!(span.field("attempt").as_deref(), Some("3"));
}

#[tokio::test]
async fn the_record_re_filed_after_a_retry_is_delivered_as_the_next_attempt() {
    let logs = nest_rs_testing::LogCapture::install();
    let enqueue = nest_rs_core::Correlation::minted(None);
    let container = Container::builder().provide(FlakyProcessor).build();
    let flaky = method("FlakyProcessor::flaky");
    let mut first = Delivery::new(
        &BARE,
        QueueName::new("transcode").expect("a valid name"),
        json!({
            "v": WIRE_FORMAT_VERSION,
            "id": JOB_ID,
            "attempt": 1,
            "payload": { "file": "x.wav" },
            "traceparent": enqueue.traceparent().to_string(),
        }),
    );
    let outcome = consume::attempt(flaky, &mut first, container.clone()).await;
    assert!(
        matches!(outcome, AttemptOutcome::Retry { .. }),
        "{outcome:?}"
    );

    let re_filed = first.retry_envelope();
    assert_eq!(re_filed.id().to_string(), JOB_ID);
    let mut second = Delivery::new(
        &BARE,
        QueueName::new("transcode").expect("a valid name"),
        re_filed.into_json(),
    );
    assert_eq!(second.attempt(), 2);
    assert_eq!(second.id().to_string(), JOB_ID);
    consume::attempt(flaky, &mut second, container).await;

    let spans = job_spans(&logs);
    assert_eq!(spans.len(), 2, "{spans:#?}");
    assert_eq!(spans[1].field("attempt").as_deref(), Some("2"));
    for span in &spans {
        assert_eq!(
            span.field("parent_span_id"),
            Some(enqueue.span_id().to_string()),
            "each attempt is a child of the enqueue, re-filed or not",
        );
    }
}

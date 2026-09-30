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
use nest_rs_queue::consume::{self, AttemptOutcome, Delivery};
use nest_rs_queue::{
    HandlerContext, JobError, JobId, ProcessMethod, ProcessOptions, QueueBackend, QueueName,
    Throttle, WIRE_FORMAT_VERSION, nest_rs_worker, processor, queue,
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

/// A value with no envelope — a pre-envelope producer's, or a foreign one's —
/// still runs, and says so: running it quietly is the same defect as dropping
/// it, one incident later, and the warn names the queue left to drain.
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

    // A retry reopens the same stored value; it is not a second legacy job.
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

/// Two deterministic failures the handler reports rather than panics on, so the
/// job reaches the dead list and the worker keeps draining: a provider no
/// reachable module registers, and a payload that no longer decodes as the job.
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

/// A payload that does not decode is dead-lettered naming the queue, where the
/// decode failed and what kind of value it found — never the value. serde's own
/// sentence quoted it, into the error-level dead-letter line and the backend's
/// dead-letter record: a field whose type changed between a producer and an
/// older worker leaked whatever it held.
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

/// `Piped<P, T>` on a job argument: the queue analog of the HTTP and GraphQL
/// form, stripped by the decorator, run by the attempt before the body.
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

/// The classification reaches the retry budget. It used to not: every
/// unsettleable attempt was retried, so a commit failing identically on every
/// attempt burned the whole budget replaying the job body before dead-lettering.
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

/// The budget is the port's: `retries = 2` is two re-runs after the first
/// attempt, and the third failure dead-letters — on every backend alike, since
/// no adapter counts.
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

/// The `queue.job` spans an attempt opened, in creation order.
fn job_spans(logs: &nest_rs_testing::LogCapture) -> Vec<nest_rs_testing::CapturedSpan> {
    logs.spans()
        .into_iter()
        .filter(|span| {
            span.target == nest_rs_queue::TARGET && span.name == nest_rs_queue::unit::JOB
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

/// An operator follows a retried job by its `trace_id`. A value carrying no
/// usable trace context — no envelope, an envelope naming only its actor, a
/// corrupt `traceparent` — has its trace minted at the first attempt, and every
/// later attempt runs as a child of that one: one trace per delivery, never one
/// per attempt. Every line an attempt files reads the same ids, the early ones
/// included.
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

/// The contrast: a job whose envelope carried the producer's trace continues it
/// at every attempt, each a child of the enqueue — never of an earlier attempt —
/// and warns about no key of it.
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

/// Our push never writes a `traceparent` that does not parse or an `actor_id`
/// that names nobody, so an envelope carrying one was written around the producer
/// or damaged. The job still runs, under a trace of its own and for no actor, and
/// the line saying so — naming the keys in a field of its own, never one called
/// `actor_id` — is filed once per delivery, inside the attempt's trace.
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

/// An envelope continuing its trace with an actor that names nobody — not a
/// string, or empty, which the kernel refuses to record — runs as a child of the
/// enqueue and for no actor, and the line says only that.
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

/// A vendor state that cannot be adopted beside a usable trace is dropped, as the
/// specification allows — and said: the job continues its trace without it.
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

/// A job whose trace its first attempt minted keeps that trace on a backend
/// that re-files every attempt — and the later attempts still say they were not
/// continued from the producer, and are children of the first attempt, exactly
/// as on a backend running the budget inside one delivery. They said
/// `continued_trace=true` from attempt 2: the re-filed `traceparent` could not be
/// told from a producer's.
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

    // The contrast: a producer's trace re-filed stays the producer's.
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

/// The keys a delivery said it could not use are not re-filed for the next
/// attempt, so the warn naming them is filed once per job — on a backend that
/// makes each attempt a delivery of its own, it was filed at every attempt.
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

/// A job a newer release sealed runs nothing here and spends no attempt: it is
/// handed back as it was stored, for a consumer of that release, under the id
/// its push returned — and the delivery says so once, naming both versions. It
/// used to be dead-lettered, under an id minted for the delivery, so a rolling
/// deploy lost every such job an old replica fetched.
#[tokio::test]
async fn a_job_a_newer_release_sealed_is_handed_back_unread_and_never_dead_lettered() {
    let logs = nest_rs_testing::LogCapture::install();
    let newer = u64::from(WIRE_FORMAT_VERSION) + 1;
    let stored = json!({ "v": newer, "id": JOB_ID, "attempt": 3, "payload": { "file": "x.wav" } });
    let mut delivery = Delivery::new(
        &FULL,
        QueueName::new("transcode").expect("a valid name"),
        stored.clone(),
    );
    assert_eq!(
        delivery.id().to_string(),
        JOB_ID,
        "the id the push returned finds the job"
    );

    let container = Container::builder().provide(FlakyProcessor).build();
    let runs = FLAKY_RUNS.load(Ordering::SeqCst);
    for _ in 1..=2 {
        let outcome = consume::attempt(
            method("FlakyProcessor::flaky"),
            &mut delivery,
            container.clone(),
        )
        .await;
        let AttemptOutcome::Defer { after } = outcome else {
            panic!("a job this release cannot read is handed back: {outcome:?}");
        };
        assert_eq!(after, consume::NEWER_RELEASE_WAIT);
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
    assert_eq!(warned.field("version"), Some(newer.to_string()));
    assert_eq!(
        warned.field("supported"),
        Some(WIRE_FORMAT_VERSION.to_string())
    );
    assert!(
        job_spans(&logs).is_empty(),
        "no attempt ran, so none is reported"
    );
}

// --- discovery -----------------------------------------------------------------

type Handled = Pin<Box<dyn Future<Output = Result<(), JobError>> + Send>>;

fn never_runs(_payload: serde_json::Value, _context: HandlerContext) -> Handled {
    Box::pin(async { Ok(()) })
}

struct ThrottledHost;
struct CheckpointHost;
struct FirstClaimant;
struct SecondClaimant;
struct BadlyNamedHost;
struct ZeroWindowHost;

nest_rs_core::inventory::submit! {
    ProcessMethod::new(
        module_path!(), "ThrottledHost::run", "throttled",
        ProcessOptions::DEFAULT.with_throttle(Throttle::new(NonZeroU32::MIN, Duration::from_secs(1))),
        TypeId::of::<ThrottledHost>, never_runs,
    )
}

nest_rs_core::inventory::submit! {
    ProcessMethod::new(
        module_path!(), "CheckpointHost::run", "resumable",
        ProcessOptions::DEFAULT.with_checkpoint(true),
        TypeId::of::<CheckpointHost>, never_runs,
    )
}

nest_rs_core::inventory::submit! {
    ProcessMethod::new(
        module_path!(), "FirstClaimant::drain", "contested",
        ProcessOptions::DEFAULT,
        TypeId::of::<FirstClaimant>, never_runs,
    )
}

nest_rs_core::inventory::submit! {
    ProcessMethod::new(
        module_path!(), "SecondClaimant::drain", "contested",
        ProcessOptions::DEFAULT.with_retries(9),
        TypeId::of::<SecondClaimant>, never_runs,
    )
}

nest_rs_core::inventory::submit! {
    ProcessMethod::new(
        module_path!(), "BadlyNamedHost::run", "nestrs:queue:dead",
        ProcessOptions::DEFAULT,
        TypeId::of::<BadlyNamedHost>, never_runs,
    )
}

nest_rs_core::inventory::submit! {
    ProcessMethod::new(
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

/// The refusal a backend without `capability` owes a method declaring it: at
/// boot, naming the declaration, the method, the capability and the backend —
/// and nothing refused on a backend that declares it.
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
    // Naming both is the point: the loser would take a share of the jobs, so an
    // error naming only one sends the reader to the wrong file half the time.
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

/// `#[process]` refuses `window = "0s"` at compile time; an entry built by hand
/// says nothing until the boot, which is the one place left to say it — on
/// every backend, since a window of zero limits nothing on any of them.
#[test]
fn an_entry_with_a_zero_throttle_window_fails_the_boot() {
    let refusal = discover_only::<ZeroWindowHost>(&FULL)
        .expect_err("a zero window limits nothing")
        .to_string();
    assert!(
        refusal.contains("ZeroWindowHost::run") && refusal.contains("window of zero"),
        "{refusal}",
    );
}

#[test]
fn a_method_another_app_owns_is_not_this_apps_to_refuse() {
    // Every entry above is linked into this binary; none is reachable here, so
    // none of their refusals is this app's.
    let methods =
        consume::discover(&reaching(&[]), &BARE).expect("nothing reachable, nothing refused");
    assert!(methods.is_empty());
}

/// An attempt's span is named for the queue it drains, as OpenTelemetry's
/// messaging conventions name a `process` span.
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

    let span = logs.expect_span(nest_rs_queue::TARGET, nest_rs_queue::unit::JOB);
    assert_eq!(
        span.field("otel.name").as_deref(),
        Some("process transcode")
    );
    assert_eq!(
        span.field("messaging.destination.name").as_deref(),
        Some("transcode"),
    );
}

/// A record no method can take is still a unit of work: the port files its
/// dead-letter event and its operation line inside a `queue.job` span, with the
/// adapter's sentence as the error and the queue the record named.
#[tokio::test]
async fn an_undeliverable_record_is_one_unit_of_work_the_port_reports() {
    let logs = nest_rs_testing::LogCapture::install();

    let error = consume::refuse(
        &BARE,
        Some("reports"),
        None,
        Some("backend-9"),
        JobError::abort("no #[process] method serves queue `reports`"),
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
            .is_some_and(|error| error.contains("no #[process] method serves queue")),
        "{event:?}"
    );
    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_queue::unit::JOB,
    );
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::ERROR)
    );
    assert!(line.trace_id.is_some(), "the line carries its unit's trace");
    logs.expect_span(nest_rs_queue::TARGET, nest_rs_queue::unit::JOB);
}

/// A record that could be read as far as its envelope continues the trace the
/// push sealed, so following that trace reaches the dead-letter; and a queue name
/// outside the rule names no span, while the event still carries it.
#[tokio::test]
async fn an_undeliverable_record_continues_its_trace_and_names_no_span_after_a_bad_name() {
    let logs = nest_rs_testing::LogCapture::install();
    let enqueue = nest_rs_core::Correlation::minted(None);
    let sealed = json!({
        "v": WIRE_FORMAT_VERSION,
        "id": JOB_ID,
        "payload": {},
        "traceparent": enqueue.traceparent().to_string(),
    });

    consume::refuse(
        &BARE,
        Some("nobody"),
        Some(&sealed),
        None,
        JobError::abort("no #[process] method serves queue `nobody`"),
    )
    .await;
    consume::refuse(
        &BARE,
        Some("bad key\nFORGED"),
        Some(&sealed),
        None,
        JobError::abort("its queue name is outside the rule"),
    )
    .await;

    let spans = job_spans(&logs);
    assert_eq!(spans.len(), 2, "{spans:#?}");
    assert_eq!(
        spans[0].field("trace_id"),
        Some(enqueue.trace_id().to_string())
    );
    assert_eq!(
        spans[0].field("otel.name").as_deref(),
        Some("process nobody")
    );
    assert_eq!(
        spans[1].field("otel.name").as_deref(),
        Some("process unknown")
    );
    assert_eq!(spans[1].field("messaging.destination.name"), None);
    assert_eq!(
        spans[0].field("messaging.message.id").as_deref(),
        Some(JOB_ID),
        "the job the push's receipt names is the one reported dead",
    );
    let events = logs.find(nest_rs_queue::TARGET, "job dead-lettered: undeliverable");
    assert_eq!(events[1].field("queue").as_deref(), Some("bad key\nFORGED"));
    assert_eq!(events[0].field("job_id").as_deref(), Some(JOB_ID));
}

// --- the job's identity and its retries ------------------------------------------

/// The job's id is the one its push sealed — never the backend's — and rides
/// the span as `messaging.message.id` and the line as `job_id`; the backend's
/// own id rides beside it as `backend_id`.
#[tokio::test]
async fn the_job_id_is_the_envelopes_and_the_backends_own_id_rides_beside_it() {
    let logs = nest_rs_testing::LogCapture::install();
    let container = Container::builder().provide(TranscodeProcessor).build();
    let mut delivery = Delivery::new(
        &BARE,
        QueueName::new("transcode").expect("a valid name"),
        json!({ "v": WIRE_FORMAT_VERSION, "id": JOB_ID, "payload": { "file": "id.wav" } }),
    )
    .with_backend_id("apalis-7");
    assert_eq!(delivery.id().to_string(), JOB_ID);

    let outcome = consume::attempt(
        method("TranscodeProcessor::transcode"),
        &mut delivery,
        container,
    )
    .await;
    assert!(matches!(outcome, AttemptOutcome::Ok), "{outcome:?}");

    let span = logs.expect_span(nest_rs_queue::TARGET, nest_rs_queue::unit::JOB);
    assert_eq!(span.field("messaging.message.id").as_deref(), Some(JOB_ID));
    assert_eq!(span.field("backend_id").as_deref(), Some("apalis-7"));
    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_queue::unit::JOB,
    );
    assert_eq!(line.field("job_id").as_deref(), Some(JOB_ID));
    assert_eq!(line.field("backend_id").as_deref(), Some("apalis-7"));
}

/// A record no push of this release wrote names no id: the delivery mints one,
/// and every attempt at the delivery — and the record re-filed for the next —
/// carries that one.
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

/// A retry says how long to wait — the port's backoff, the same on every
/// backend — and says so on its `warn`, so an operator reading the line knows
/// when the next attempt is due.
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

/// The attempt number travels in the envelope, so a job re-filed for its third
/// attempt spends the budget from there: on a method with `retries = 2`, that
/// attempt is the last, and its failure dead-letters.
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
    let span = logs.expect_span(nest_rs_queue::TARGET, nest_rs_queue::unit::JOB);
    assert_eq!(span.field("attempt").as_deref(), Some("3"));
}

/// What a backend declaring `DelayedPush` re-files after a `Retry`: the same
/// job, one attempt on — which a later delivery runs as that attempt, still a
/// child of the enqueue.
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

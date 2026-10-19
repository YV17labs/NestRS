//! The push and cancel surface through a backend that records what it is
//! handed, and the net under every call, on a paused clock.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use futures_util::future::BoxFuture;
use nest_rs_database::{Executor as DatabaseExecutor, OutboxError, OutboxRecord};
use nest_rs_http::HttpConfig;
use nest_rs_queue::{
    BACKEND_TIMEOUT, Capabilities, Capability, ENQUEUE_BATCH, Envelope, JobId, JobProducer,
    JobProducerExt, PushOptions, PushReceipt, QueueBackend, QueueError, QueueName,
    WIRE_FORMAT_VERSION, async_trait,
};
use serde_json::{Value, json};

use crate::{BARE, FULL, TranscodeCommand, TranscodeQueue, within_twice_the_net};

/// One call the port made to the backend: the queue, the envelopes as stored,
/// the options.
type Filed = (String, Vec<Value>, PushOptions);

/// The unique key the tests hold a job under.
const HELD_KEY: &str = "song-1";

/// A backend declaring unique jobs and no way to remove one.
static UNIQUE_ONLY: QueueBackend = QueueBackend::new(
    "unique-only",
    Capabilities::NONE.with(Capability::UniquePush),
);

/// A backend that records every call and keeps what it filed in memory: a job
/// waits until it is removed, and a unique key is held by the job that waits
/// under it.
struct RecordingProducer {
    backend: &'static QueueBackend,
    filed: Mutex<Vec<Filed>>,
    /// The jobs still waiting: queue, id, unique key.
    waiting: Mutex<Vec<(String, JobId, Option<String>)>>,
    /// What the port asked it to remove: `queue:id`, or `queue:key`.
    removed: Mutex<Vec<String>>,
}

impl RecordingProducer {
    fn on(backend: &'static QueueBackend) -> Self {
        Self {
            backend,
            filed: Mutex::new(Vec::new()),
            waiting: Mutex::new(Vec::new()),
            removed: Mutex::new(Vec::new()),
        }
    }

    fn filed(&self) -> Vec<Filed> {
        self.filed.lock().expect("lock").clone()
    }

    fn removed(&self) -> Vec<String> {
        self.removed.lock().expect("lock").clone()
    }
}

#[async_trait]
impl JobProducer for RecordingProducer {
    fn backend(&self) -> &'static QueueBackend {
        self.backend
    }

    async fn enqueue(
        &self,
        queue: &QueueName,
        envelopes: Vec<Envelope>,
        options: &PushOptions,
    ) -> Result<(), QueueError> {
        let mut waiting = self.waiting.lock().expect("lock");
        for envelope in &envelopes {
            if let Some(key) = envelope.unique_key()
                && let Some((_, holder, _)) = waiting
                    .iter()
                    .find(|(on, _, held)| on == queue.as_str() && held.as_deref() == Some(key))
            {
                return Err(QueueError::UniqueKeyHeld {
                    queue: queue.clone(),
                    key: key.to_owned(),
                    holder: holder.clone(),
                });
            }
            waiting.push((
                queue.to_string(),
                envelope.id().clone(),
                envelope.unique_key().map(str::to_owned),
            ));
        }
        self.filed.lock().expect("lock").push((
            queue.to_string(),
            envelopes.into_iter().map(Envelope::into_json).collect(),
            options.clone(),
        ));
        Ok(())
    }

    async fn remove(&self, queue: &QueueName, id: &JobId) -> Result<bool, QueueError> {
        self.removed
            .lock()
            .expect("lock")
            .push(format!("{queue}:{id}"));
        let mut waiting = self.waiting.lock().expect("lock");
        let before = waiting.len();
        waiting.retain(|(on, waiting_id, _)| !(on == queue.as_str() && waiting_id == id));
        Ok(waiting.len() < before)
    }

    async fn remove_unique(&self, queue: &QueueName, key: &str) -> Result<bool, QueueError> {
        self.removed
            .lock()
            .expect("lock")
            .push(format!("{queue}:{key}"));
        let mut waiting = self.waiting.lock().expect("lock");
        let before = waiting.len();
        waiting.retain(|(on, _, held)| !(on == queue.as_str() && held.as_deref() == Some(key)));
        Ok(waiting.len() < before)
    }
}

/// A receipt for a job no backend ever filed.
fn stray_receipt() -> PushReceipt {
    PushReceipt::new(
        QueueName::new("transcode").expect("a valid name"),
        JobId::parse("01890a5d-ac96-774b-bcce-b302099a8057").expect("a v7"),
    )
}

fn song() -> TranscodeCommand {
    TranscodeCommand {
        file: "song.wav".into(),
    }
}

#[tokio::test]
async fn a_push_names_its_queue_seals_its_payload_and_returns_the_id_it_sealed() {
    let producer = RecordingProducer::on(&BARE);
    let receipt = producer
        .push(TranscodeQueue, song(), None)
        .await
        .expect("a plain push needs no capability");

    let filed = producer.filed();
    assert_eq!(filed.len(), 1);
    let (queue, envelopes, options) = &filed[0];
    assert_eq!(
        queue, "transcode",
        "the name came from the marker, not a literal"
    );
    assert_eq!(options, &PushOptions::default());
    let envelope = &envelopes[0];
    assert_eq!(envelope["v"], json!(WIRE_FORMAT_VERSION));
    assert_eq!(envelope["payload"], json!({ "file": "song.wav" }));
    assert_eq!(
        envelope["attempt"],
        json!(1),
        "a push files the first attempt"
    );
    assert!(
        envelope["traceparent"].is_string(),
        "the port sealed the trace context before the backend saw the job: {envelope}",
    );
    assert_eq!(
        envelope["id"],
        json!(receipt.id().to_string()),
        "the receipt names the id the port sealed, not one the backend chose",
    );
    assert_eq!(receipt.queue().as_str(), "transcode");
}

#[tokio::test]
async fn a_push_of_many_files_every_job_in_one_call_with_a_receipt_each() {
    let producer = RecordingProducer::on(&BARE);
    let receipts = producer
        .push_many(
            TranscodeQueue,
            ["a.wav", "b.wav", "c.wav"].map(|file| TranscodeCommand { file: file.into() }),
            None,
        )
        .await
        .expect("a bulk push needs no capability");

    assert_eq!(receipts.len(), 3);
    let filed = producer.filed();
    assert_eq!(filed.len(), 1, "one call to the backend, not three");
    assert_eq!(filed[0].1.len(), 3);
    assert_eq!(filed[0].1[2]["payload"], json!({ "file": "c.wav" }));
    let sealed: Vec<&Value> = filed[0].1.iter().map(|envelope| &envelope["id"]).collect();
    let receipted: Vec<Value> = receipts
        .iter()
        .map(|receipt| json!(receipt.id().to_string()))
        .collect();
    assert_eq!(
        sealed,
        receipted.iter().collect::<Vec<_>>(),
        "one receipt per job, in input order, each naming its own sealed id",
    );
    assert!(
        receipts[0].id() < receipts[1].id() && receipts[1].id() < receipts[2].id(),
        "v7 ids sort in push order",
    );
}

#[tokio::test]
async fn a_push_of_nothing_reaches_no_backend() {
    let producer = RecordingProducer::on(&BARE);
    let receipts = producer
        .push_many(TranscodeQueue, Vec::<TranscodeCommand>::new(), None)
        .await
        .expect("nothing to push");
    assert!(receipts.is_empty());
    assert!(producer.filed().is_empty());
}

#[tokio::test]
async fn a_push_of_nothing_is_still_refused_what_the_backend_cannot_honour() {
    let producer = RecordingProducer::on(&BARE);
    let refused = producer
        .push_many(
            TranscodeQueue,
            Vec::<TranscodeCommand>::new(),
            PushOptions::default().with_delay(Duration::from_secs(60)),
        )
        .await
        .expect_err("a bare backend honours no delay, batch or not");
    assert!(
        matches!(
            refused,
            QueueError::Unsupported {
                capability: Capability::DelayedPush,
                ..
            }
        ),
        "{refused:?}"
    );
    assert!(producer.filed().is_empty(), "nothing reached the backend");
}

#[tokio::test]
async fn the_raw_hatch_takes_a_runtime_name_and_checks_it() {
    let producer = RecordingProducer::on(&FULL);
    producer
        .push_json("billing.invoices", json!({ "invoice": 7 }), None)
        .await
        .expect("a runtime name inside the rule");
    assert_eq!(producer.filed()[0].0, "billing.invoices");

    for refused in ["nestrs:queue:dead", "", "with space", "tenant#acme"] {
        let error = producer
            .push_json(refused, json!({}), None)
            .await
            .expect_err("a name outside the rule is refused");
        assert!(
            matches!(error, QueueError::InvalidQueueName { .. }),
            "{refused:?}: {error}"
        );
    }
    assert_eq!(
        producer.filed().len(),
        1,
        "no refused name reached the backend"
    );
}

/// The refusal a backend without `capability` owes a push declaring it.
fn assert_refused_at_the_push(
    result: Result<PushReceipt, QueueError>,
    capability: Capability,
    producer: &RecordingProducer,
) {
    let Err(QueueError::Unsupported {
        capability: refused,
        backend,
    }) = result
    else {
        panic!("a backend without {capability:?} refuses the push: {result:?}");
    };
    assert_eq!(refused, capability);
    assert_eq!(backend, "bare");
    assert!(producer.filed().is_empty(), "nothing reached the backend");
}

#[tokio::test]
async fn delayed_push_is_refused_at_the_push_by_a_backend_without_it() {
    let producer = RecordingProducer::on(&BARE);
    let result = producer
        .push(
            TranscodeQueue,
            song(),
            PushOptions::default().with_delay(Duration::from_secs(60)),
        )
        .await;
    assert_refused_at_the_push(result, Capability::DelayedPush, &producer);
}

#[tokio::test]
async fn unique_push_is_refused_at_the_push_by_a_backend_without_it() {
    let producer = RecordingProducer::on(&BARE);
    let result = producer
        .push(
            TranscodeQueue,
            song(),
            PushOptions::default().with_unique("song-1"),
        )
        .await;
    assert_refused_at_the_push(result, Capability::UniquePush, &producer);
}

#[tokio::test]
async fn a_backend_declaring_the_options_receives_them_as_declared() {
    let producer = RecordingProducer::on(&FULL);
    let options = PushOptions::default()
        .with_delay(Duration::from_secs(60))
        .with_unique("song-1");
    producer
        .push(TranscodeQueue, song(), options.clone())
        .await
        .expect("declared, so served");
    let (_, envelopes, filed_options) = &producer.filed()[0];
    assert_eq!(filed_options, &options);
    assert_eq!(
        envelopes[0]["unique_key"],
        json!("song-1"),
        "the key travels with the job, for whoever settles it to release",
    );
}

#[tokio::test]
async fn a_unique_push_under_a_held_key_is_refused_naming_the_holder() {
    let producer = RecordingProducer::on(&FULL);
    let unique = PushOptions::default().with_unique("song-1");
    let first = producer
        .push(TranscodeQueue, song(), unique.clone())
        .await
        .expect("the key is free");

    let refused = producer
        .push(TranscodeQueue, song(), unique.clone())
        .await
        .expect_err("the key is held");
    let QueueError::UniqueKeyHeld { queue, key, holder } = &refused else {
        panic!("{refused:?}");
    };
    assert_eq!((queue.as_str(), key.as_str()), ("transcode", "song-1"));
    assert_eq!(holder, first.id());
    let sentence = refused.to_string();
    assert!(
        sentence.contains(&first.id().to_string()) && sentence.contains("song-1"),
        "{sentence}"
    );

    assert!(producer.cancel(&first).await.expect("declared, so served"));
    producer
        .push(TranscodeQueue, song(), unique)
        .await
        .expect("a cancelled job's key is free again");
}

#[tokio::test]
async fn a_unique_key_is_refused_on_a_push_of_many_and_an_invalid_one_anywhere() {
    let producer = RecordingProducer::on(&FULL);
    let many = producer
        .push_many(
            TranscodeQueue,
            [song(), song()],
            PushOptions::default().with_unique("song-1"),
        )
        .await
        .expect_err("one key cannot name several jobs");
    assert!(matches!(many, QueueError::InvalidOptions { .. }), "{many}");

    let invalid = producer
        .push(
            TranscodeQueue,
            song(),
            PushOptions::default().with_unique("song\n1"),
        )
        .await
        .expect_err("a key no backend could file");
    assert!(
        matches!(invalid, QueueError::InvalidUniqueKey { .. }),
        "{invalid}"
    );
    assert!(producer.filed().is_empty());
}

#[tokio::test]
async fn a_receipt_kept_as_data_still_names_its_job() {
    let producer = RecordingProducer::on(&FULL);
    let receipt = producer
        .push(TranscodeQueue, song(), None)
        .await
        .expect("a plain push");

    let kept = serde_json::to_value(&receipt).expect("a receipt serializes");
    assert_eq!(
        kept,
        json!({ "queue": "transcode", "id": receipt.id().to_string() })
    );
    let read_back: PushReceipt = serde_json::from_value(kept).expect("and reads back");
    assert!(producer.cancel(&read_back).await.expect("served"));

    for forged in [
        json!({ "queue": "a:b", "id": receipt.id().to_string() }),
        json!({ "queue": "transcode", "id": "job-0" }),
    ] {
        assert!(
            serde_json::from_value::<PushReceipt>(forged.clone()).is_err(),
            "{forged}"
        );
    }
}

/// The refusal a backend lacking what a cancel needs owes it.
fn assert_cancel_refused(
    result: Result<bool, QueueError>,
    capability: Capability,
    backend: &str,
    producer: &RecordingProducer,
) {
    let Err(QueueError::Unsupported {
        capability: refused,
        backend: named,
    }) = result
    else {
        panic!("a backend without {capability:?} refuses the cancel: {result:?}");
    };
    assert_eq!((refused, named), (capability, backend));
    assert!(producer.removed().is_empty(), "nothing reached the backend");
}

#[tokio::test]
async fn cancellation_is_refused_by_a_backend_without_what_it_needs() {
    let bare = RecordingProducer::on(&BARE);
    assert_cancel_refused(
        bare.cancel(&stray_receipt()).await,
        Capability::Cancellation,
        "bare",
        &bare,
    );
    assert_cancel_refused(
        bare.cancel_unique(TranscodeQueue, HELD_KEY).await,
        Capability::UniquePush,
        "bare",
        &bare,
    );

    let unique_only = RecordingProducer::on(&UNIQUE_ONLY);
    assert_cancel_refused(
        unique_only.cancel_unique(TranscodeQueue, HELD_KEY).await,
        Capability::Cancellation,
        "unique-only",
        &unique_only,
    );
}

#[tokio::test]
async fn a_cancel_reaches_the_backend_and_says_what_it_cancelled() {
    let logs = nest_rs_testing::LogCapture::install();
    let producer = RecordingProducer::on(&FULL);
    let receipt = producer
        .push(TranscodeQueue, song(), None)
        .await
        .expect("a plain push");
    assert!(
        producer
            .cancel(&receipt)
            .await
            .expect("declared, so served"),
        "the job waited, so it never starts"
    );
    assert!(
        !producer.cancel(&receipt).await.expect("served"),
        "a job already cancelled is no longer the backend's to cancel"
    );
    assert!(
        !producer
            .cancel(&stray_receipt())
            .await
            .expect("a job nothing filed is no error")
    );
    let named = format!("transcode:{}", receipt.id());
    assert_eq!(
        producer.removed(),
        [
            named.clone(),
            named,
            format!("transcode:{}", stray_receipt().id())
        ]
    );

    let cancelled = logs.expect_one(nest_rs_queue::TARGET, "queued job cancelled");
    assert_eq!(cancelled.level, "info");
    assert_eq!(
        cancelled.field("job_id"),
        Some(receipt.id().to_string()),
        "{cancelled:#?}"
    );
    assert_eq!(cancelled.field("queue").as_deref(), Some("transcode"));
}

#[tokio::test]
async fn a_unique_cancel_names_its_queue_and_checks_its_key_before_the_backend_sees_it() {
    let logs = nest_rs_testing::LogCapture::install();
    let producer = RecordingProducer::on(&FULL);
    producer
        .push(
            TranscodeQueue,
            song(),
            PushOptions::default().with_unique(HELD_KEY),
        )
        .await
        .expect("the key is free");
    assert!(
        producer
            .cancel_unique(TranscodeQueue, HELD_KEY)
            .await
            .expect("declared, so served")
    );
    assert!(
        !producer
            .cancel_unique(TranscodeQueue, "song-2")
            .await
            .expect("a key holding nothing is no error")
    );
    let invalid = producer
        .cancel_unique(TranscodeQueue, "song\n1")
        .await
        .expect_err("a key no push could file");
    assert!(
        matches!(invalid, QueueError::InvalidUniqueKey { .. }),
        "{invalid}"
    );
    assert_eq!(
        producer.removed(),
        ["transcode:song-1", "transcode:song-2"],
        "the queue came from the marker, and the refused key never reached the backend",
    );

    let cancelled = logs.expect_one(nest_rs_queue::TARGET, "unique job cancelled");
    assert_eq!(cancelled.level, "info");
    assert_eq!(cancelled.field("queue").as_deref(), Some("transcode"));
    assert_eq!(cancelled.field("unique_key").as_deref(), Some(HELD_KEY));
}

/// A backend declaring cancellation and overriding neither removal.
struct ForgetfulProducer;

#[async_trait]
impl JobProducer for ForgetfulProducer {
    fn backend(&self) -> &'static QueueBackend {
        &FULL
    }

    async fn enqueue(
        &self,
        _queue: &QueueName,
        _envelopes: Vec<Envelope>,
        _options: &PushOptions,
    ) -> Result<(), QueueError> {
        Ok(())
    }
}

#[tokio::test]
async fn cancellation_declared_and_not_implemented_is_named_a_driver_defect() {
    let outcomes = [
        (
            ForgetfulProducer.cancel(&stray_receipt()).await,
            "`JobProducer::remove`",
        ),
        (
            ForgetfulProducer
                .cancel_unique(TranscodeQueue, HELD_KEY)
                .await,
            "`JobProducer::remove_unique`",
        ),
    ];
    for (outcome, method) in outcomes {
        let error = outcome.expect_err("nothing implements the removal");
        assert!(
            matches!(error, QueueError::Backend(_)),
            "a driver defect, not a refusal: {error}"
        );
        let cause = std::error::Error::source(&error)
            .map(ToString::to_string)
            .unwrap_or_default();
        assert!(
            cause.contains("declares job cancellation") && cause.contains(method),
            "{cause}"
        );
    }
}

#[tokio::test]
async fn a_push_of_more_than_a_batch_reaches_the_backend_a_batch_at_a_time_in_order() {
    let producer = RecordingProducer::on(&BARE);
    let jobs = ENQUEUE_BATCH * 2 + 1;
    let receipts = producer
        .push_many(
            TranscodeQueue,
            (0..jobs).map(|n| TranscodeCommand {
                file: format!("{n}.wav"),
            }),
            None,
        )
        .await
        .expect("a bulk push needs no capability");

    let filed = producer.filed();
    assert_eq!(
        filed
            .iter()
            .map(|(_, envelopes, _)| envelopes.len())
            .collect::<Vec<_>>(),
        [ENQUEUE_BATCH, ENQUEUE_BATCH, 1],
        "full batches first, the remainder last",
    );
    let sealed: Vec<Value> = filed
        .iter()
        .flat_map(|(_, envelopes, _)| envelopes.iter().map(|envelope| envelope["id"].clone()))
        .collect();
    let receipted: Vec<Value> = receipts
        .iter()
        .map(|receipt| json!(receipt.id().to_string()))
        .collect();
    assert_eq!(
        sealed, receipted,
        "the batches keep the input order, and so do the receipts"
    );
    assert_eq!(
        filed[2].1[0]["payload"],
        json!({ "file": format!("{}.wav", jobs - 1) }),
    );
}

/// A backend that answers its first `answered` calls to `enqueue` after `delay`,
/// then no call at all — a store gone silent behind a network that still takes
/// the dial. Its cancels never answer.
struct SilentProducer {
    answered: usize,
    delay: Duration,
    /// How many envelopes each call to `enqueue` carried, in order.
    calls: Mutex<Vec<usize>>,
}

impl SilentProducer {
    fn answering(answered: usize, delay: Duration) -> Self {
        Self {
            answered,
            delay,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<usize> {
        self.calls.lock().expect("lock").clone()
    }
}

#[async_trait]
impl JobProducer for SilentProducer {
    fn backend(&self) -> &'static QueueBackend {
        &FULL
    }

    async fn enqueue(
        &self,
        _queue: &QueueName,
        envelopes: Vec<Envelope>,
        _options: &PushOptions,
    ) -> Result<(), QueueError> {
        let call = {
            let mut calls = self.calls.lock().expect("lock");
            calls.push(envelopes.len());
            calls.len()
        };
        if call > self.answered {
            std::future::pending::<()>().await;
        }
        tokio::time::sleep(self.delay).await;
        Ok(())
    }

    async fn remove(&self, _queue: &QueueName, _id: &JobId) -> Result<bool, QueueError> {
        std::future::pending().await
    }

    async fn remove_unique(&self, _queue: &QueueName, _key: &str) -> Result<bool, QueueError> {
        std::future::pending().await
    }
}

/// Whether `error` is the net's answer to `call` on the queue named `queue`,
/// and says so naming the queue.
fn unanswered(error: &QueueError, queue: &str, call: &str) -> bool {
    matches!(
        error,
        QueueError::Unanswered { queue: named, call: said } if named.as_str() == queue && *said == call
    ) && error.to_string().contains(&format!("on queue `{queue}`"))
}

#[tokio::test(start_paused = true)]
async fn a_push_the_backend_never_answers_fails_at_the_net_naming_the_queue() {
    let producer = SilentProducer::answering(0, Duration::ZERO);
    let started = tokio::time::Instant::now();
    let refused = within_twice_the_net(producer.push(TranscodeQueue, song(), None))
        .await
        .expect_err("the backend never answered");
    let waited = started.elapsed();

    assert!(
        unanswered(&refused, "transcode", "JobProducer::enqueue"),
        "{refused:?}"
    );
    assert!(
        waited >= BACKEND_TIMEOUT && waited < BACKEND_TIMEOUT + Duration::from_secs(1),
        "the push waited the net and no longer, not {waited:?}",
    );
}

#[tokio::test(start_paused = true)]
async fn a_cancel_the_backend_never_answers_fails_at_the_net_naming_the_queue() {
    let producer = SilentProducer::answering(1, Duration::ZERO);
    let receipt = producer
        .push(TranscodeQueue, song(), None)
        .await
        .expect("the push is answered");

    let refused = within_twice_the_net(producer.cancel(&receipt))
        .await
        .expect_err("the backend never answered the cancel");
    assert!(
        unanswered(&refused, "transcode", "JobProducer::remove"),
        "{refused:?}"
    );
    let refused = within_twice_the_net(producer.cancel_unique(TranscodeQueue, HELD_KEY))
        .await
        .expect_err("the backend never answered the cancel");
    assert!(
        unanswered(&refused, "transcode", "JobProducer::remove_unique"),
        "{refused:?}"
    );
}

/// A backend whose every call unwinds — an `unwrap` on its client's answer.
struct PanickingProducer;

#[async_trait]
impl JobProducer for PanickingProducer {
    fn backend(&self) -> &'static QueueBackend {
        &FULL
    }

    async fn enqueue(
        &self,
        _queue: &QueueName,
        _envelopes: Vec<Envelope>,
        _options: &PushOptions,
    ) -> Result<(), QueueError> {
        panic!("the backend's client unwound")
    }

    async fn remove(&self, _queue: &QueueName, _id: &JobId) -> Result<bool, QueueError> {
        panic!("the backend's client unwound")
    }

    async fn remove_unique(&self, _queue: &QueueName, _key: &str) -> Result<bool, QueueError> {
        panic!("the backend's client unwound")
    }
}

/// A push to a backend that unwinds is an error to its caller naming the queue
/// and the call — never the panic, which the port files once — where it used to
/// unwind into the caller's handler.
#[tokio::test]
async fn a_push_the_backend_panics_answering_is_an_error_naming_the_queue() {
    let logs = nest_rs_testing::LogCapture::install();
    let refused = PanickingProducer
        .push(TranscodeQueue, song(), None)
        .await
        .expect_err("the backend unwound");

    assert!(
        matches!(
            &refused,
            QueueError::Panicked { queue, call }
                if queue.as_str() == "transcode" && *call == "JobProducer::enqueue"
        ),
        "{refused:?}"
    );
    let said = refused.to_string();
    assert!(
        said.contains("on queue `transcode`") && !said.contains("the backend's client"),
        "{said}"
    );
    let filed = logs.expect_one(
        nest_rs_queue::TARGET,
        "queue backend panicked answering a call; the call is abandoned",
    );
    assert_eq!(filed.level, "error");
    assert_eq!(filed.field("queue").as_deref(), Some("transcode"));
    assert_eq!(filed.field("call").as_deref(), Some("JobProducer::enqueue"));
    assert_eq!(
        filed.field(nest_rs_core::panic::FIELD).as_deref(),
        Some("the backend's client unwound"),
    );
}

#[tokio::test(start_paused = true)]
async fn a_backend_answering_inside_the_net_is_waited_for() {
    let producer = SilentProducer::answering(1, BACKEND_TIMEOUT - Duration::from_millis(1));
    producer
        .push(TranscodeQueue, song(), None)
        .await
        .expect("answered inside the net");
}

/// The net under the HTTP edge's request timeout, both read from the constants
/// that set them: a handler's push to a backend that never answers fails with
/// the port's error naming the queue, not the edge's `503` naming nothing.
#[test]
fn the_bound_answers_before_the_http_edge_times_a_request_out() {
    let timeout = HttpConfig::default()
        .request_timeout
        .expect("the HTTP edge bounds a request by default");
    assert!(
        BACKEND_TIMEOUT < timeout,
        "{BACKEND_TIMEOUT:?} vs {timeout:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_push_of_many_the_net_cuts_short_stops_at_the_batch_never_answered() {
    let producer = SilentProducer::answering(1, Duration::ZERO);
    let refused = within_twice_the_net(producer.push_many(
        TranscodeQueue,
        (0..=ENQUEUE_BATCH).map(|n| TranscodeCommand {
            file: format!("{n}.wav"),
        }),
        None,
    ))
    .await
    .expect_err("the second batch is never answered");

    let QueueError::PartiallyQueued { receipts, source } = &refused else {
        panic!("the first batch was accepted, so the push was partly queued: {refused:?}");
    };
    assert_eq!(
        receipts.len(),
        ENQUEUE_BATCH,
        "one receipt per accepted job"
    );
    assert!(
        receipts
            .iter()
            .all(|receipt| receipt.queue().as_str() == "transcode"),
        "{receipts:?}"
    );
    assert!(
        unanswered(source, "transcode", "JobProducer::enqueue"),
        "{source:?}"
    );
    assert!(
        nest_rs_core::error_message(&refused).contains("did not answer `JobProducer::enqueue`"),
        "the cause is said with the error: {refused}"
    );
    assert_eq!(
        producer.calls(),
        [ENQUEUE_BATCH, 1],
        "the first batch was answered, the second never was, and nothing followed it",
    );

    let silent = SilentProducer::answering(0, Duration::ZERO);
    let refused = within_twice_the_net(silent.push_many(
        TranscodeQueue,
        (0..=ENQUEUE_BATCH).map(|n| TranscodeCommand {
            file: format!("{n}.wav"),
        }),
        None,
    ))
    .await
    .expect_err("nothing is answered");
    assert!(
        unanswered(&refused, "transcode", "JobProducer::enqueue"),
        "{refused:?}"
    );
}

/// A unit's handle keeping an outbox: it records what the port writes there
/// and removes, and fails a write when told to.
#[derive(Default)]
struct Keeping {
    written: Mutex<Vec<Vec<OutboxRecord>>>,
    /// What the port asked it to remove: `queue:id`, or `queue:key`.
    removed: Mutex<Vec<String>>,
    refuses: bool,
}

impl Keeping {
    fn refusing() -> Self {
        Self {
            refuses: true,
            ..Self::default()
        }
    }

    fn written(&self) -> Vec<Vec<OutboxRecord>> {
        self.written.lock().expect("lock").clone()
    }

    /// Whether a record of job `id` on `queue` is still kept.
    fn keeps(&self, queue: &str, id: uuid::Uuid) -> bool {
        self.written
            .lock()
            .expect("lock")
            .iter()
            .flatten()
            .any(|record| record.queue == queue && record.id == id)
    }
}

impl DatabaseExecutor for Keeping {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn write_outbox(
        &self,
        records: &mut dyn Iterator<Item = OutboxRecord>,
    ) -> Option<BoxFuture<'_, Result<(), OutboxError>>> {
        let answer = if self.refuses {
            Err(OutboxError::Database("the transaction is aborted".into()))
        } else {
            self.written.lock().expect("lock").push(records.collect());
            Ok(())
        };
        Some(Box::pin(std::future::ready(answer)))
    }

    fn remove_outbox<'a>(
        &'a self,
        queue: &'a str,
        id: uuid::Uuid,
    ) -> Option<BoxFuture<'a, Result<bool, OutboxError>>> {
        self.removed
            .lock()
            .expect("lock")
            .push(format!("{queue}:{id}"));
        let mut written = self.written.lock().expect("lock");
        let kept = written.iter().flatten().count();
        for records in written.iter_mut() {
            records.retain(|record| !(record.queue == queue && record.id == id));
        }
        let removed = written.iter().flatten().count() < kept;
        Some(Box::pin(std::future::ready(Ok(removed))))
    }

    fn remove_outbox_unique<'a>(
        &'a self,
        queue: &'a str,
        key: &'a str,
    ) -> Option<BoxFuture<'a, Result<bool, OutboxError>>> {
        self.removed
            .lock()
            .expect("lock")
            .push(format!("{queue}:{key}"));
        let mut written = self.written.lock().expect("lock");
        let kept = written.iter().flatten().count();
        for records in written.iter_mut() {
            records.retain(|record| {
                !(record.queue == queue && record.unique_key.as_deref() == Some(key))
            });
        }
        let removed = written.iter().flatten().count() < kept;
        Some(Box::pin(std::future::ready(Ok(removed))))
    }
}

/// A unit's handle keeping no outbox, as a pool's.
struct OnThePool;

impl DatabaseExecutor for OnThePool {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `push` run with `executor` as the ambient one.
async fn inside<F: Future>(executor: Arc<dyn DatabaseExecutor>, push: F) -> F::Output {
    nest_rs_database::with_request_executor(executor, push).await
}

#[tokio::test]
async fn a_push_inside_a_unit_keeping_an_outbox_is_written_there_and_never_reaches_the_backend() {
    let producer = RecordingProducer::on(&FULL);
    let keeping = Arc::new(Keeping::default());
    let receipt = inside(keeping.clone(), producer.push(TranscodeQueue, song(), None))
        .await
        .expect("written to the outbox");

    assert!(
        producer.filed().is_empty(),
        "the backend sees it after the commit, from the relay"
    );
    let written = keeping.written();
    assert_eq!(written.len(), 1, "one write");
    let [record] = written[0].as_slice() else {
        panic!("one record for one push: {written:?}");
    };
    assert_eq!(record.queue, "transcode");
    assert_eq!(
        record.id.to_string(),
        receipt.id().to_string(),
        "the receipt names the record's job"
    );
    assert_eq!(record.unique_key, None);
    assert_eq!(record.delay, Duration::ZERO);
    let sealed: Value = serde_json::from_str(&record.sealed).expect("the sealed job is JSON");
    assert_eq!(sealed["payload"], json!({ "file": "song.wav" }));
    assert_eq!(sealed["id"], json!(receipt.id().to_string()));
    assert_eq!(sealed["v"], json!(WIRE_FORMAT_VERSION));
}

#[tokio::test]
async fn a_push_of_many_inside_such_a_unit_is_one_write_with_a_record_per_receipt() {
    let producer = RecordingProducer::on(&FULL);
    let keeping = Arc::new(Keeping::default());
    let receipts = inside(
        keeping.clone(),
        producer.push_many(
            TranscodeQueue,
            (0..=ENQUEUE_BATCH).map(|n| TranscodeCommand {
                file: format!("{n}.wav"),
            }),
            None,
        ),
    )
    .await
    .expect("written to the outbox");

    assert!(producer.filed().is_empty());
    let written = keeping.written();
    assert_eq!(
        written.len(),
        1,
        "one write, so the whole push commits or rolls back with its unit"
    );
    let ids: Vec<String> = written[0]
        .iter()
        .map(|record| record.id.to_string())
        .collect();
    let receipted: Vec<String> = receipts
        .iter()
        .map(|receipt| receipt.id().to_string())
        .collect();
    assert_eq!(ids, receipted, "one record per receipt, in input order");
}

#[tokio::test]
async fn a_push_on_a_handle_keeping_no_outbox_is_filed_at_once() {
    let producer = RecordingProducer::on(&FULL);
    let receipt = inside(
        Arc::new(OnThePool),
        producer.push(TranscodeQueue, song(), None),
    )
    .await
    .expect("filed");
    let filed = producer.filed();
    assert_eq!(
        filed.len(),
        1,
        "a pool keeps no outbox: the backend files it now"
    );
    assert_eq!(filed[0].1[0]["id"], json!(receipt.id().to_string()));
}

#[tokio::test]
async fn a_push_keeps_its_options_on_the_record_and_is_refused_what_the_backend_lacks_first() {
    let producer = RecordingProducer::on(&FULL);
    let keeping = Arc::new(Keeping::default());
    let at = SystemTime::now() + Duration::from_secs(600);
    inside(keeping.clone(), async {
        producer
            .push(
                TranscodeQueue,
                song(),
                PushOptions::default()
                    .with_delay(Duration::from_secs(60))
                    .with_unique(HELD_KEY),
            )
            .await
            .expect("a delay and a key the backend declares");
        producer
            .push(
                TranscodeQueue,
                song(),
                PushOptions::default().with_delay(at),
            )
            .await
            .expect("an instant the backend declares");
    })
    .await;
    let written: Vec<OutboxRecord> = keeping.written().into_iter().flatten().collect();
    assert_eq!(written[0].delay, Duration::from_secs(60));
    assert_eq!(written[0].unique_key.as_deref(), Some(HELD_KEY));
    assert!(
        written[1].delay <= Duration::from_secs(600) && written[1].delay > Duration::from_secs(590),
        "an instant is kept as what is left of the wait: {:?}",
        written[1].delay,
    );

    let bare = RecordingProducer::on(&BARE);
    let keeping = Arc::new(Keeping::default());
    let refused = inside(
        keeping.clone(),
        bare.push(
            TranscodeQueue,
            song(),
            PushOptions::default().with_delay(Duration::from_secs(60)),
        ),
    )
    .await;
    assert!(
        matches!(refused, Err(QueueError::Unsupported { .. })),
        "refused at the push, before the outbox: {refused:?}"
    );
    assert!(keeping.written().is_empty());
}

#[tokio::test]
async fn a_key_another_job_holds_is_not_refused_at_a_push_the_outbox_keeps() {
    let producer = RecordingProducer::on(&FULL);
    producer
        .push(
            TranscodeQueue,
            song(),
            PushOptions::default().with_unique(HELD_KEY),
        )
        .await
        .expect("the first job holds the key");
    let keeping = Arc::new(Keeping::default());
    inside(
        keeping.clone(),
        producer.push(
            TranscodeQueue,
            song(),
            PushOptions::default().with_unique(HELD_KEY),
        ),
    )
    .await
    .expect("the relay checks the key once the unit committed");
    assert_eq!(keeping.written().len(), 1);
}

#[tokio::test]
async fn a_write_the_outbox_refuses_fails_the_push() {
    let producer = RecordingProducer::on(&FULL);
    let refused = inside(
        Arc::new(Keeping::refusing()),
        producer.push(TranscodeQueue, song(), None),
    )
    .await
    .expect_err("nothing was kept");
    assert!(matches!(refused, QueueError::Outbox(_)), "{refused:?}");
    assert!(
        producer.filed().is_empty(),
        "never filed behind its unit's back"
    );
}

#[tokio::test]
async fn a_cancel_inside_a_unit_removes_a_record_not_yet_filed_then_asks_the_backend() {
    let producer = RecordingProducer::on(&FULL);
    let keeping = Arc::new(Keeping::default());
    let cancelled = inside(keeping.clone(), async {
        let receipt = producer
            .push(TranscodeQueue, song(), None)
            .await
            .expect("written to the outbox");
        assert!(keeping.keeps(
            "transcode",
            receipt.id().to_string().parse().expect("a uuid")
        ));
        producer.cancel(&receipt).await
    })
    .await;
    assert!(
        cancelled.expect("cancelled"),
        "it never reached the backend"
    );
    assert!(keeping.written().iter().flatten().next().is_none());
    assert_eq!(producer.removed().len(), 1, "the backend is asked too");

    let keeping = Arc::new(Keeping::default());
    let cancelled = inside(keeping.clone(), async {
        producer
            .push(
                TranscodeQueue,
                song(),
                PushOptions::default().with_unique(HELD_KEY),
            )
            .await
            .expect("written to the outbox");
        producer.cancel_unique(TranscodeQueue, HELD_KEY).await
    })
    .await;
    assert!(cancelled.expect("cancelled"));
    assert!(keeping.written().iter().flatten().next().is_none());
    assert_eq!(
        keeping.removed.lock().expect("lock").as_slice(),
        [format!("transcode:{HELD_KEY}")],
    );
}

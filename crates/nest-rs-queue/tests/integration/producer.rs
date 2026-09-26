//! The push surface through a backend that records what it is handed: the
//! destination names the queue, the port checks and seals before any backend
//! sees a job, and each push-time declaration a backend without its capability
//! cannot honour is refused at the push — as a cancel is refused before the
//! backend sees it, and says what it removed.

use std::sync::Mutex;
use std::time::Duration;

use nest_rs_queue::{
    Capabilities, Capability, Envelope, JobId, JobProducer, JobProducerExt, PushOptions,
    PushReceipt, QueueBackend, QueueError, QueueName, WIRE_FORMAT_VERSION, async_trait,
};
use serde_json::{Value, json};

use crate::{BARE, FULL, TranscodeCommand, TranscodeQueue};

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

/// An empty batch is refused on the same grounds a full one is.
///
/// The short-circuit used to return before the capability check, so the answer
/// depended on the **runtime length** of the iterator: a filter that matched
/// nothing made a delayed push on a backend without `DelayedPush` read as
/// honoured, and the same call turned into `Unsupported` the first time the
/// filter matched — in production, at the one site nothing tested.
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

/// The refusal a backend without `capability` owes a push declaring it:
/// `Unsupported`, naming the capability and the backend, before the backend sees
/// anything.
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

/// A unique push under a key another job still holds is refused, naming the
/// job holding it — the caller learns which job to wait for, or to cancel.
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

/// A receipt is data a caller keeps — in a column, a message, a file — and
/// reads back to cancel with later; both halves are checked on the way back.
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

// --- a cancel -------------------------------------------------------------------

/// The refusal a backend lacking what a cancel needs owes it: `Unsupported`,
/// naming the first capability missing and the backend, before the backend sees
/// the call.
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

/// `Ok(true)` only for a job that had not started and now never will;
/// `Ok(false)` for one that is gone or was never known — and the cancel says on
/// `nest_rs::queue` which job it cancelled.
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

/// A backend declaring job cancellation and implementing neither removal is a
/// driver defect, and is told so: the refusal a backend without the capability
/// owes would tell it the capability was missing.
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

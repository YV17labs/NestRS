//! Integration coverage for the **typed queue handle** surface: a `QueueName`
//! type links a producer's `push_to::<Q>` and a consumer's
//! `#[process(queue = Q)]` to one wire name and one payload type. The round-trip
//! here stays in-process — a fake `JobProducer` records pushes, and the
//! `#[process]`-emitted handler is drained straight from the link-time inventory
//! and invoked with an envelope payload. Live-Redis coverage is the worker
//! app's e2e suite.

use std::any::TypeId;
use std::sync::{Arc, Mutex};

use nest_rs_core::Container;
use nest_rs_queue::{
    Job, JobProducer, JobProducerExt, ProcessMethod, QueueError, QueueName, WIRE_FORMAT_VERSION,
    processor, queue,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct TranscodeCommand {
    file: String,
}

// The single artifact both sides import: name + payload type in one place.
#[queue(name = "transcode", job = TranscodeCommand)]
struct TranscodeQueue;

#[test]
fn queue_name_carries_the_wire_name_and_payload_type() {
    assert_eq!(<TranscodeQueue as QueueName>::NAME, "transcode");
    // `Self::Job` is the payload type — assert it via a job round-trip.
    fn round_trip<Q: QueueName>(job: Q::Job) -> Q::Job
    where
        Q::Job: Job,
    {
        job
    }
    let job = round_trip::<TranscodeQueue>(TranscodeCommand {
        file: "a.wav".into(),
    });
    assert_eq!(job.file, "a.wav");
}

/// A `JobProducer` that records every `(queue, payload)` it is handed — enough
/// to prove `push_to::<Q>` routes to `Q::NAME` with the JSON of `Q::Job`.
#[derive(Default, Clone)]
struct RecordingProducer {
    pushed: Arc<Mutex<Vec<(String, serde_json::Value)>>>,
}

#[nest_rs_queue::async_trait]
impl JobProducer for RecordingProducer {
    async fn push_json(&self, queue: &str, payload: serde_json::Value) -> Result<(), QueueError> {
        self.pushed
            .lock()
            .expect("lock")
            .push((queue.to_string(), payload));
        Ok(())
    }
}

#[tokio::test]
async fn push_to_routes_by_the_typed_handle() {
    let producer = RecordingProducer::default();
    producer
        .push_to::<TranscodeQueue>(TranscodeCommand {
            file: "song.wav".into(),
        })
        .await
        .expect("typed push succeeds");

    let pushed = producer.pushed.lock().expect("lock").clone();
    assert_eq!(pushed.len(), 1);
    let (queue_name, payload) = &pushed[0];
    // The name came from `TranscodeQueue::NAME`, not a hand-typed literal.
    assert_eq!(queue_name, <TranscodeQueue as QueueName>::NAME);
    assert_eq!(payload, &json!({ "file": "song.wav" }));
}

#[tokio::test]
async fn push_dynamic_name_still_works_as_the_escape_hatch() {
    let producer = RecordingProducer::default();
    producer
        .push("dynamic-name", TranscodeCommand { file: "x.wav".into() })
        .await
        .expect("dynamic push succeeds");
    let pushed = producer.pushed.lock().expect("lock").clone();
    assert_eq!(pushed[0].0, "dynamic-name");
}

/// Shared sink so the invoked handler can be observed from the test.
#[derive(Default, Clone)]
struct Sink {
    seen: Arc<Mutex<Vec<String>>>,
}

struct TranscodeProcessor {
    sink: Sink,
}

#[processor]
impl TranscodeProcessor {
    // The type-path form: the macro reads `TranscodeQueue::NAME` into the
    // inventory entry and asserts this method's argument is
    // `<TranscodeQueue as QueueName>::Job` (compiling this test proves it).
    #[process(queue = TranscodeQueue, concurrency = 2, retries = 1)]
    async fn transcode(&self, job: TranscodeCommand) -> anyhow::Result<()> {
        self.sink.seen.lock().expect("lock").push(job.file);
        Ok(())
    }
}

fn transcode_method() -> &'static ProcessMethod {
    nest_rs_core::inventory::iter::<ProcessMethod>()
        .find(|m| {
            (m.provider_type_id)() == TypeId::of::<TranscodeProcessor>()
                && m.name == "TranscodeProcessor::transcode"
        })
        .expect("the typed #[process] method is discovered through the inventory")
}

#[test]
fn typed_process_populates_the_inventory_entry_from_the_queue_type() {
    let method = transcode_method();
    // Queue name resolved from `TranscodeQueue::NAME`, not a string literal.
    assert_eq!(method.queue, <TranscodeQueue as QueueName>::NAME);
    assert_eq!(method.concurrency, 2);
    assert_eq!(method.retries, 1);
}

#[tokio::test]
async fn typed_process_handler_round_trips_an_enveloped_payload() {
    let sink = Sink::default();
    let container = Container::builder()
        .provide(TranscodeProcessor { sink: sink.clone() })
        .build();

    let method = transcode_method();
    let payload = json!({
        "v": WIRE_FORMAT_VERSION,
        "payload": { "file": "drained.wav" },
    });

    (method.handler)(payload, container)
        .await
        .expect("handler dispatches the job to the provider");

    assert_eq!(sink.seen.lock().expect("lock").as_slice(), &["drained.wav"]);
}

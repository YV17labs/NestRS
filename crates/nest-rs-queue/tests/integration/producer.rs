//! Producer side: `push_to::<Q>` routes to `Q::NAME` with the JSON of
//! `Q::Job`; string-named `push` stays the escape hatch.

use std::sync::{Arc, Mutex};

use nest_rs_queue::{JobProducer, JobProducerExt, QueueError, QueueName};
use serde_json::json;

use crate::{TranscodeCommand, TranscodeQueue};

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
        .push(
            "dynamic-name",
            TranscodeCommand {
                file: "x.wav".into(),
            },
        )
        .await
        .expect("dynamic push succeeds");
    let pushed = producer.pushed.lock().expect("lock").clone();
    assert_eq!(pushed[0].0, "dynamic-name");
}

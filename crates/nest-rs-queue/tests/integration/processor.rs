//! Consumer side: the `#[process(queue = Q)]` type-path form populates the
//! link-time `ProcessMethod` entry from the queue type, and the emitted
//! handler dispatches an enveloped payload to the provider.

use std::any::TypeId;
use std::sync::{Arc, Mutex};

use nest_rs_core::Container;
use nest_rs_queue::{ProcessMethod, QueueName, WIRE_FORMAT_VERSION, processor};
use serde_json::json;

use crate::{TranscodeCommand, TranscodeQueue};

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
    #[process(queue = TranscodeQueue, retries = 1)]
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

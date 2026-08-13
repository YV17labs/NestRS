//! Consumer side: the `#[process(queue = Q)]` type-path form populates the
//! link-time `ProcessMethod` entry from the queue type, and the emitted
//! handler dispatches an enveloped payload to the provider.

use std::any::TypeId;
use std::sync::{Arc, Mutex};

use nest_rs_core::Container;
use nest_rs_queue::nest_rs_worker;
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

/// A context that reports it could not honour the attempt, carrying the
/// classification a `WorkerDbContext` reaches from the database's own error.
struct Unsettleable(nest_rs_worker::Unhonoured);

impl nest_rs_worker::JobContext for Unsettleable {
    fn scope<'a>(
        &'a self,
        _transaction: nest_rs_worker::JobTransaction,
        inner: std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = nest_rs_worker::JobSettlement> + Send + 'a>,
    > {
        Box::pin(async move {
            inner.await;
            nest_rs_worker::JobSettlement::Unhonoured(self.0)
        })
    }
}

async fn settle(why: nest_rs_worker::Unhonoured) -> nest_rs_queue::JobError {
    let container = Container::builder()
        .provide(TranscodeProcessor {
            sink: Sink::default(),
        })
        .provide_dyn::<dyn nest_rs_worker::JobContext>(Arc::new(Unsettleable(why)))
        .build();
    let payload = json!({
        "v": WIRE_FORMAT_VERSION,
        "payload": { "file": "drained.wav" },
    });

    (transcode_method().handler)(payload, container)
        .await
        .expect_err("a success the context could not honour is not a success")
}

/// The classification reaches the backend's retry policy. It used to not: every
/// unsettleable attempt was `JobError::retry`, so a commit that fails
/// identically on every attempt — a deferred constraint violation — burned the
/// whole retry budget replaying the job body, side effects included, before
/// dead-lettering.
#[tokio::test]
async fn an_attempt_the_context_could_not_settle_carries_its_classification() {
    let transient = settle(nest_rs_worker::Unhonoured::retryable(
        "the job's transaction could not be committed",
    ))
    .await;
    assert!(
        transient.retryable,
        "a serialization conflict is what the retry budget is for",
    );

    let deterministic = settle(nest_rs_worker::Unhonoured::deterministic(
        "the job's transaction could not be committed",
    ))
    .await;
    assert!(
        !deterministic.retryable,
        "and a failure that repeats identically aborts at once, like the three \
         other deterministic failures `#[process]` already aborts on",
    );
    assert_eq!(
        deterministic.to_string(),
        "the job's transaction could not be committed",
        "the sentence the context wrote is what the dead-letter record carries",
    );
}

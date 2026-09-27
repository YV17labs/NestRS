//! A `Checkpoint<S>` parameter through an attempt: what one attempt saves, the
//! next attempt at the same job reads, and the job's end clears it — the port's
//! promise, on every backend.
//!
//! A store that never answers meets the port's net: a read or a save fails the
//! attempt, retryably, naming the queue and the call, and a clear at the job's
//! end is said while the outcome stands — on a paused clock, so the suite never
//! waits the net out.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use nest_rs_core::Container;
use nest_rs_queue::consume::{self, AttemptOutcome, Delivery};
use nest_rs_queue::{
    Checkpoint, CheckpointStore, QueueError, QueueName, WIRE_FORMAT_VERSION, async_trait,
    processor, queue,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{FULL, method, within_twice_the_net};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ImportCommand {
    rows: u32,
}

#[queue(name = "imports", job = ImportCommand)]
struct ImportQueue;

#[queue(name = "one-shot-imports", job = ImportCommand)]
struct OneShotImportQueue;

/// Where each attempt started, as its checkpoint said.
static RESUMED_AT: Mutex<Vec<u32>> = Mutex::new(Vec::new());

struct ImportProcessor;

impl nest_rs_core::ProviderResidency for ImportProcessor {
    const SINGLETON: bool = true;
}

#[processor]
impl ImportProcessor {
    #[process(queue = ImportQueue, retries = 1, transactional = false)]
    async fn import(
        &self,
        job: ImportCommand,
        mut checkpoint: Checkpoint<u32>,
    ) -> anyhow::Result<()> {
        let start = checkpoint.get().copied().unwrap_or(0);
        RESUMED_AT.lock().expect("lock").push(start);
        if start == 0 {
            checkpoint.save(job.rows / 2).await?;
            anyhow::bail!("the import stopped halfway");
        }
        Ok(())
    }

    /// No retry to resume in: the first failure is the job's last.
    #[process(queue = OneShotImportQueue, transactional = false)]
    async fn import_once(
        &self,
        job: ImportCommand,
        mut checkpoint: Checkpoint<u32>,
    ) -> anyhow::Result<()> {
        checkpoint.save(job.rows / 2).await?;
        anyhow::bail!("the import stopped halfway")
    }
}

/// A backend's store, in memory — optionally one that cannot clear.
#[derive(Default)]
struct MemoryStore {
    saved: Mutex<Option<Value>>,
    clear_fails: AtomicBool,
}

impl MemoryStore {
    fn saved(&self) -> Option<Value> {
        self.saved.lock().expect("lock").clone()
    }
}

#[async_trait]
impl CheckpointStore for MemoryStore {
    async fn load(&self) -> Result<Option<Value>, QueueError> {
        Ok(self.saved())
    }

    async fn save(&self, state: Value) -> Result<(), QueueError> {
        *self.saved.lock().expect("lock") = Some(state);
        Ok(())
    }

    async fn clear(&self) -> Result<(), QueueError> {
        if self.clear_fails.load(Ordering::SeqCst) {
            return Err(QueueError::backend(std::io::Error::other(
                "the store went away",
            )));
        }
        *self.saved.lock().expect("lock") = None;
        Ok(())
    }
}

fn delivery(queue: &str, store: &Arc<MemoryStore>) -> Delivery {
    Delivery::new(
        &FULL,
        QueueName::new(queue.to_owned()).expect("a valid name"),
        json!({ "v": WIRE_FORMAT_VERSION, "payload": { "rows": 10 } }),
    )
    .with_checkpoint(store.clone())
}

#[tokio::test]
async fn a_retry_resumes_from_what_the_failed_attempt_saved_and_the_end_clears_it() {
    let store = Arc::new(MemoryStore::default());
    let container = Container::builder().provide(ImportProcessor).build();
    let import = method("ImportProcessor::import");
    assert!(import.options().checkpoint(), "the parameter declared it");
    let mut delivery = delivery("imports", &store);

    let first = consume::attempt(import, &mut delivery, container.clone()).await;
    assert!(matches!(first, AttemptOutcome::Retry { .. }), "{first:?}");
    assert_eq!(
        store.saved(),
        Some(json!(5)),
        "the save reached the backend's store, where a redelivery would read it",
    );
    let second = consume::attempt(import, &mut delivery, container).await;
    assert!(matches!(second, AttemptOutcome::Ok), "{second:?}");

    assert_eq!(
        RESUMED_AT.lock().expect("lock").as_slice(),
        &[0, 5],
        "the retry read the save the failed attempt made",
    );
    assert_eq!(
        store.saved(),
        None,
        "the job completed, so its checkpoint went"
    );
}

#[tokio::test]
async fn a_checkpoint_is_cleared_when_its_job_dead_letters() {
    let store = Arc::new(MemoryStore::default());
    let container = Container::builder().provide(ImportProcessor).build();
    let mut delivery = delivery("one-shot-imports", &store);

    let outcome = consume::attempt(
        method("ImportProcessor::import_once"),
        &mut delivery,
        container,
    )
    .await;
    assert!(
        matches!(outcome, AttemptOutcome::DeadLetter(_)),
        "{outcome:?}"
    );
    assert_eq!(store.saved(), None, "a dead letter is the job's end too");
}

/// The outcome is decided before the clear runs: a store that cannot clear is
/// said, at `warn`, and the job still completes.
#[tokio::test]
async fn a_checkpoint_the_store_cannot_clear_is_said_and_the_outcome_stands() {
    let logs = nest_rs_testing::LogCapture::install();
    let store = Arc::new(MemoryStore::default());
    store.clear_fails.store(true, Ordering::SeqCst);
    let container = Container::builder().provide(ImportProcessor).build();
    let mut delivery = delivery("one-shot-imports", &store);

    let outcome = consume::attempt(
        method("ImportProcessor::import_once"),
        &mut delivery,
        container,
    )
    .await;
    assert!(
        matches!(outcome, AttemptOutcome::DeadLetter(_)),
        "{outcome:?}"
    );
    let left = logs.expect_one(
        nest_rs_queue::TARGET,
        "job checkpoint not cleared at its terminal outcome",
    );
    assert_eq!(left.level, "warn");
    assert_eq!(
        left.field("job_id").as_deref(),
        Some(delivery.id().to_string().as_str())
    );
    assert!(
        left.field("error")
            .is_some_and(|error| error.contains("the store went away")),
        "{left:#?}"
    );
}

/// A store answering from memory, but for `silent`, the one call it never
/// answers — a store gone quiet behind a network that still takes the dial.
struct SilentStore {
    silent: &'static str,
    saved: Mutex<Option<Value>>,
}

impl SilentStore {
    fn silent_on(silent: &'static str) -> Arc<Self> {
        Arc::new(Self {
            silent,
            saved: Mutex::new(None),
        })
    }

    async fn answer(&self, call: &str) {
        if self.silent == call {
            std::future::pending::<()>().await;
        }
    }
}

#[async_trait]
impl CheckpointStore for SilentStore {
    async fn load(&self) -> Result<Option<Value>, QueueError> {
        self.answer("load").await;
        Ok(self.saved.lock().expect("lock").clone())
    }

    async fn save(&self, state: Value) -> Result<(), QueueError> {
        self.answer("save").await;
        *self.saved.lock().expect("lock") = Some(state);
        Ok(())
    }

    async fn clear(&self) -> Result<(), QueueError> {
        self.answer("clear").await;
        *self.saved.lock().expect("lock") = None;
        Ok(())
    }
}

fn silent_delivery(queue: &str, store: &Arc<SilentStore>) -> Delivery {
    Delivery::new(
        &FULL,
        QueueName::new(queue.to_owned()).expect("a valid name"),
        json!({ "v": WIRE_FORMAT_VERSION, "payload": { "rows": 10 } }),
    )
    .with_checkpoint(store.clone())
}

/// A store that never answers the read, or the save, fails the attempt at the
/// net, retryably, and the retry's line names the call and the queue.
#[tokio::test(start_paused = true)]
async fn a_checkpoint_the_store_never_reads_or_saves_fails_the_attempt_retryably() {
    for call in ["load", "save"] {
        let logs = nest_rs_testing::LogCapture::install();
        let store = SilentStore::silent_on(call);
        let container = Container::builder().provide(ImportProcessor).build();
        let mut delivery = silent_delivery("imports", &store);

        let outcome = within_twice_the_net(consume::attempt(
            method("ImportProcessor::import"),
            &mut delivery,
            container,
        ))
        .await;
        assert!(
            matches!(outcome, AttemptOutcome::Retry { .. }),
            "{call}: {outcome:?}"
        );
        let retried = logs.expect_one(
            nest_rs_queue::TARGET,
            "job failed; will retry within the budget",
        );
        let error = retried.field("error").unwrap_or_default();
        assert!(
            error.contains(&format!("`CheckpointStore::{call}` on queue `imports`")),
            "{call}: {error}"
        );
    }
}

/// A store that never answers the clear at the job's end is said at `warn`,
/// naming the call and the queue, and the outcome stands.
#[tokio::test(start_paused = true)]
async fn a_checkpoint_the_store_never_clears_is_said_and_the_outcome_stands() {
    let logs = nest_rs_testing::LogCapture::install();
    let store = SilentStore::silent_on("clear");
    let container = Container::builder().provide(ImportProcessor).build();
    let mut delivery = silent_delivery("one-shot-imports", &store);

    let outcome = within_twice_the_net(consume::attempt(
        method("ImportProcessor::import_once"),
        &mut delivery,
        container,
    ))
    .await;
    assert!(
        matches!(outcome, AttemptOutcome::DeadLetter(_)),
        "the outcome was decided before the clear: {outcome:?}"
    );
    let left = logs.expect_one(
        nest_rs_queue::TARGET,
        "job checkpoint not cleared at its terminal outcome",
    );
    assert_eq!(left.level, "warn");
    let error = left.field("error").unwrap_or_default();
    assert!(
        error.contains("`CheckpointStore::clear` on queue `one-shot-imports`"),
        "{error}"
    );
}

//! A `Checkpoint<S>` parameter through an attempt: what one attempt saves, the
//! next attempt at the same job reads; a store that never answers meets the net,
//! or the attempt's deadline when that comes first.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nest_rs_core::Container;
use nest_rs_queue::__private::consume::{self, AttemptOutcome, Delivery};
use nest_rs_queue::__private::nest_rs_database;
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

#[queue(name = "timed-imports", job = ImportCommand)]
struct TimedImportQueue;

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

    /// Bound well under the store's own net.
    #[process(queue = TimedImportQueue, retries = 1, timeout = "1s", transactional = false)]
    async fn import_in_a_second(
        &self,
        _job: ImportCommand,
        _checkpoint: Checkpoint<u32>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

/// A backend's store, in memory.
#[derive(Default)]
struct MemoryStore {
    saved: Mutex<Option<Value>>,
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
async fn a_retry_resumes_from_what_the_failed_attempt_saved() {
    let store = Arc::new(MemoryStore::default());
    let container = Container::builder()
        .provide(ImportProcessor)
        .build()
        .expect("the container builds");
    let import = method("ImportProcessor::import");
    assert!(import.options().checkpoint(), "the parameter declared it");
    let mut delivery = delivery("imports", &store);

    let first = consume::attempt(import, &mut delivery, container.clone(), None).await;
    assert!(matches!(first, AttemptOutcome::Retry { .. }), "{first:?}");
    assert_eq!(
        store.saved(),
        Some(json!(5)),
        "the save reached the backend's store, where a redelivery would read it",
    );
    let second = consume::attempt(import, &mut delivery, container, None).await;
    assert!(matches!(second, AttemptOutcome::Ok), "{second:?}");

    assert_eq!(
        RESUMED_AT.lock().expect("lock").as_slice(),
        &[0, 5],
        "the retry read the save the failed attempt made",
    );
}

#[tokio::test]
async fn a_checkpoint_that_does_not_decode_is_reported_without_its_value() {
    let store = Arc::new(MemoryStore::default());
    *store.saved.lock().expect("lock") = Some(json!("sk_live_51HsecretTOKEN"));
    let container = Container::builder()
        .provide(ImportProcessor)
        .build()
        .expect("the container builds");
    let mut delivery = delivery("one-shot-imports", &store);

    let outcome = consume::attempt(
        method("ImportProcessor::import_once"),
        &mut delivery,
        container,
        None,
    )
    .await;
    let AttemptOutcome::DeadLetter(error) = outcome else {
        panic!("the same bytes never decode on a retry: {outcome:?}");
    };
    let said = error.to_string();
    assert!(
        said.ends_with("does not decode: invalid type: a string, expected u32"),
        "{said}"
    );
    assert!(!said.contains("sk_live"), "{said}");
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
}

fn silent_delivery(queue: &str, store: &Arc<SilentStore>) -> Delivery {
    Delivery::new(
        &FULL,
        QueueName::new(queue.to_owned()).expect("a valid name"),
        json!({ "v": WIRE_FORMAT_VERSION, "payload": { "rows": 10 } }),
    )
    .with_checkpoint(store.clone())
}

#[tokio::test(start_paused = true)]
async fn a_checkpoint_the_store_never_reads_or_saves_fails_the_attempt_retryably() {
    for call in ["load", "save"] {
        let logs = nest_rs_testing::LogCapture::install();
        let store = SilentStore::silent_on(call);
        let container = Container::builder()
            .provide(ImportProcessor)
            .build()
            .expect("the container builds");
        let mut delivery = silent_delivery("imports", &store);

        let outcome = within_twice_the_net(consume::attempt(
            method("ImportProcessor::import"),
            &mut delivery,
            container,
            None,
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

/// A unit of work that settles at once, declaring a one-second settle net.
struct Bound;

struct BoundUnit;

/// The handle a bound unit installs.
struct Nowhere;

impl nest_rs_database::Executor for Nowhere {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl nest_rs_database::UnitOfWork for Bound {
    fn open(
        &self,
        _: &nest_rs_database::UnitSpec,
    ) -> Result<Box<dyn nest_rs_database::OpenUnit>, nest_rs_database::Unhonoured> {
        Ok(Box::new(BoundUnit))
    }

    fn settle_net(&self) -> Duration {
        Duration::from_secs(1)
    }
}

impl nest_rs_database::OpenUnit for BoundUnit {
    fn executor(&self) -> Arc<dyn nest_rs_database::Executor> {
        Arc::new(Nowhere)
    }

    fn settle(
        self: Box<Self>,
        _ok: bool,
    ) -> Pin<Box<dyn Future<Output = nest_rs_database::Settlement> + Send>> {
        Box::pin(async { nest_rs_database::Settlement::Settled })
    }
}

/// The checkpoint is read under the attempt's deadline: a store silent past it
/// cut a body that committed nothing, never a commit that may have landed.
#[tokio::test(start_paused = true)]
async fn a_checkpoint_the_store_never_reads_within_the_timeout_is_a_retryable_timeout() {
    let logs = nest_rs_testing::LogCapture::install();
    let store = SilentStore::silent_on("load");
    let container = Container::builder()
        .provide(ImportProcessor)
        .build()
        .expect("the container builds");
    let mut delivery = silent_delivery("timed-imports", &store);

    let outcome = within_twice_the_net(consume::attempt(
        method("ImportProcessor::import_in_a_second"),
        &mut delivery,
        container,
        Some(Arc::new(Bound)),
    ))
    .await;
    assert!(
        matches!(outcome, AttemptOutcome::Retry { .. }),
        "{outcome:?}"
    );
    let retried = logs.expect_one(
        nest_rs_queue::TARGET,
        "job failed; will retry within the budget",
    );
    let error = retried.field("error").unwrap_or_default();
    assert!(
        error.contains("did not end within its 1s timeout"),
        "{error}"
    );
}

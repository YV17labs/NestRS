//! [`Checkpoint`] — progress a job keeps across its retries and redeliveries —
//! and [`CheckpointStore`], what a backend implements to keep it.
//!
//! A backend reads a job's state when the delivery starts; this crate remembers
//! every save made through the delivery, so an attempt run again inside one
//! delivery reads what the attempt before it saved.
//!
//! The backend lets a job's checkpoint go in the step that ends the job
//! ([`Disposition`](crate::Disposition)); a retry keeps it.

use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::backend::bounded;
use crate::{JobError, QueueError, QueueName};

/// Where a backend keeps one job's checkpoint — keyed by the job's
/// [`JobId`](crate::JobId), so every delivery of the job reads the same one.
///
/// The port waits [`BACKEND_TIMEOUT`](crate::BACKEND_TIMEOUT) for each call's
/// answer, then drops the call: bound each round trip well inside it.
#[async_trait]
pub trait CheckpointStore: Send + Sync + 'static {
    /// The state saved for the job before this delivery started, if any.
    async fn load(&self) -> Result<Option<Value>, QueueError>;

    /// Replace the job's saved state. It lasts through the job's retries and a
    /// redelivery after a crash, and goes when the job does.
    async fn save(&self, state: Value) -> Result<(), QueueError>;
}

/// One delivery's checkpoint: the backend's store, and the latest state read or
/// saved through it. Internal ABI between a delivery and the decorator-emitted
/// handler.
#[doc(hidden)]
pub struct CheckpointCell {
    store: Arc<dyn CheckpointStore>,
    /// The job's queue, which a store silent past the net is reported on.
    queue: QueueName,
    /// `None` until the store was read; then the latest state, saved or loaded.
    latest: Mutex<Option<Option<Value>>>,
}

impl CheckpointCell {
    pub(crate) fn new(store: Arc<dyn CheckpointStore>, queue: QueueName) -> Self {
        Self {
            store,
            queue,
            latest: Mutex::new(None),
        }
    }

    async fn load(&self) -> Result<Option<Value>, QueueError> {
        let cached = self
            .latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(latest) = cached {
            return Ok(latest);
        }
        let loaded = bounded(&self.queue, "CheckpointStore::load", self.store.load()).await?;
        *self.latest.lock().unwrap_or_else(PoisonError::into_inner) = Some(loaded.clone());
        Ok(loaded)
    }

    async fn save(&self, state: Value) -> Result<(), QueueError> {
        bounded(
            &self.queue,
            "CheckpointStore::save",
            self.store.save(state.clone()),
        )
        .await?;
        *self.latest.lock().unwrap_or_else(PoisonError::into_inner) = Some(Some(state));
        Ok(())
    }
}

/// A job's progress: what earlier attempts saved, and a save for this one.
///
/// A `#[process]` parameter, recognised by its type:
///
/// ```
/// # use anyhow::Result;
/// # use nest_rs_core::injectable;
/// # use nest_rs_queue::{Checkpoint, ProcessMethod, input, processor, queue};
/// # #[input]
/// # #[derive(Clone)]
/// # pub struct ImportCommand { pub rows: u32 }
/// # #[input]
/// # #[derive(Clone)]
/// # pub struct ImportProgress { pub row: u32 }
/// # #[queue(name = "imports", job = ImportCommand)]
/// # pub struct ImportQueue;
/// # #[injectable]
/// # #[derive(Default)]
/// # pub struct ImportProcessor;
/// # #[processor]
/// # impl ImportProcessor {
/// #[process(queue = ImportQueue, transactional = false)]
/// async fn import(&self, job: ImportCommand, mut checkpoint: Checkpoint<ImportProgress>) -> Result<()> {
///     let start = checkpoint.get().map_or(0, |progress| progress.row);
///     // … import rows from `start`, saving every batch:
/// #   let row = start + job.rows;
///     checkpoint.save(ImportProgress { row }).await?;
/// #   Ok(())
/// }
/// # }
/// # let import = nest_rs_core::inventory::iter::<ProcessMethod>()
/// #     .find(|method| method.name() == "ImportProcessor::import");
/// # assert_eq!(import.map(|method| method.options().checkpoint()), Some(true));
/// ```
///
/// It lasts through the job's retries and through a redelivery after the
/// replica holding the job died, and is cleared when the job completes,
/// dead-letters or is cancelled. The method must declare
/// `transactional = false`: a checkpoint is saved at once, so a rolled-back
/// attempt's retry would resume past work that was undone.
pub struct Checkpoint<S> {
    cell: Arc<CheckpointCell>,
    state: Option<S>,
}

impl<S> Checkpoint<S>
where
    S: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    /// Read the delivery's latest checkpoint for the handler — emitted by the
    /// decorator, which runs it before the method.
    ///
    /// A backend that could not be read is retryable, like any transient fault;
    /// a saved state that no longer decodes as `S` is not, since every attempt
    /// would read the same bytes.
    #[doc(hidden)]
    pub async fn open(cell: Option<&Arc<CheckpointCell>>, queue: &str) -> Result<Self, JobError> {
        let Some(cell) = cell else {
            return Err(JobError::abort(format!(
                "a job from queue `{queue}` reached a method taking a `Checkpoint`, and its \
                 delivery carries no checkpoint store"
            )));
        };
        let state = match cell.load().await {
            // Said without the value: a checkpoint is the job's own data.
            Ok(Some(saved)) => Some(serde_json::from_value(saved).map_err(|error| {
                JobError::abort(format!(
                    "the checkpoint saved for a job from queue `{queue}` does not decode: {}",
                    nest_rs_core::DecodeError::new(&error),
                ))
            })?),
            Ok(None) => None,
            Err(error) => return Err(JobError::retry(error)),
        };
        Ok(Self {
            cell: Arc::clone(cell),
            state,
        })
    }

    /// The latest state saved for this job — by this attempt, an earlier one,
    /// or a replica that died holding it.
    pub fn get(&self) -> Option<&S> {
        self.state.as_ref()
    }

    /// Save `state` for this job, at once and outside any transaction. A later
    /// [`get`](Self::get) returns it, and so does the job's next attempt.
    ///
    /// Fails with [`QueueError::Unanswered`] when the store does not answer
    /// within [`BACKEND_TIMEOUT`](crate::BACKEND_TIMEOUT); returned with `?`,
    /// that fails the attempt, retryably, like any error the method returns.
    pub async fn save(&mut self, state: S) -> Result<(), QueueError> {
        let value = serde_json::to_value(&state)?;
        self.cell.save(value).await?;
        self.state = Some(state);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    /// A store answering from memory, counting how often it is read.
    #[derive(Default)]
    struct MemoryStore {
        saved: Mutex<Option<Value>>,
        loads: AtomicUsize,
    }

    #[async_trait]
    impl CheckpointStore for MemoryStore {
        async fn load(&self) -> Result<Option<Value>, QueueError> {
            self.loads.fetch_add(1, Ordering::SeqCst);
            Ok(self.saved.lock().expect("lock").clone())
        }

        async fn save(&self, state: Value) -> Result<(), QueueError> {
            *self.saved.lock().expect("lock") = Some(state);
            Ok(())
        }
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Progress {
        row: u32,
    }

    fn imports() -> QueueName {
        QueueName::new("imports").expect("a valid name")
    }

    #[tokio::test]
    async fn a_second_attempt_in_one_delivery_reads_the_first_attempts_save() {
        let store = Arc::new(MemoryStore::default());
        *store.saved.lock().expect("lock") = Some(json!({ "row": 1 }));
        let cell = Arc::new(CheckpointCell::new(store.clone(), imports()));

        let mut first = Checkpoint::<Progress>::open(Some(&cell), "imports")
            .await
            .expect("opens");
        assert_eq!(first.get(), Some(&Progress { row: 1 }));
        first.save(Progress { row: 7 }).await.expect("saves");

        let second = Checkpoint::<Progress>::open(Some(&cell), "imports")
            .await
            .expect("opens");
        assert_eq!(second.get(), Some(&Progress { row: 7 }));
        assert_eq!(
            store.loads.load(Ordering::SeqCst),
            1,
            "the store is read once per delivery"
        );
    }

    #[tokio::test]
    async fn a_saved_state_that_no_longer_decodes_aborts_rather_than_retries() {
        let store = Arc::new(MemoryStore::default());
        *store.saved.lock().expect("lock") = Some(json!("not a progress"));
        let cell = Arc::new(CheckpointCell::new(store, imports()));

        let Err(error) = Checkpoint::<Progress>::open(Some(&cell), "imports").await else {
            panic!("a state that does not decode must not open");
        };
        assert!(!error.retryable, "every attempt would read the same bytes");
    }

    #[tokio::test]
    async fn a_method_taking_a_checkpoint_without_a_store_is_refused_by_name() {
        let Err(error) = Checkpoint::<Progress>::open(None, "imports").await else {
            panic!("no store, no checkpoint");
        };
        assert!(!error.retryable);
        assert!(error.to_string().contains("imports"), "{error}");
    }
}

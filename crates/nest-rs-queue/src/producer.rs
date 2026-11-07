//! The producer seam: [`JobProducer`], what a backend implements to enqueue jobs,
//! and the path every push and cancel of [`Jobs`](crate::Jobs) takes to it.
//!
//! **The port pushes; a backend enqueues.** Every push goes through
//! [`Jobs`](crate::Jobs), which checks the queue's name and the options, refuses an
//! option the backend does not declare, mints each job's [`JobId`] and seals the
//! [`Envelope`] — then hands the backend sealed jobs and answers the caller with
//! receipts it wrote itself. A cancel takes the same path back.
//!
//! **Every call is waited on for [`BACKEND_TIMEOUT`](crate::BACKEND_TIMEOUT) at
//! most**, then answers [`QueueError::Unanswered`], naming the queue — or
//! [`QueueError::Panicked`] for a backend that panicked answering.
//!
//! **A push inside a unit of work holding a transaction is written into it.**
//! Where the data driver keeps an outbox
//! ([`Executor::write_outbox`](nest_rs_database::Executor::write_outbox)), the
//! sealed jobs become records of the caller's transaction: they are filed
//! after its commit by the [`OutboxRelay`](crate::OutboxRelay), and never on
//! its rollback. Anywhere else — a pool, no data driver — the backend files
//! them at once.

use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use nest_rs_database::OutboxRecord;
use serde_json::Value;

use crate::backend::bounded;
use crate::error::Unimplemented;
use crate::push_options::check_unique_key;
use crate::{
    Capabilities, Capability, Delay, Envelope, JobId, PushOptions, PushReceipt, QueueBackend,
    QueueError, QueueName, TARGET, envelope,
};

/// The most jobs the port hands a backend's [`JobProducer::enqueue`] in one
/// call. A push of more is filed in calls of this many, in order, each under
/// [`BACKEND_TIMEOUT`](crate::BACKEND_TIMEOUT).
pub const ENQUEUE_BATCH: usize = 100;

/// What a queue backend implements to enqueue jobs. Its binding registers it
/// as the port's [`Jobs`](crate::Jobs), which a feature injects and pushes
/// through.
///
/// **On failure**, the port answers the caller: a backend's `Err` as it is,
/// a call silent past [`BACKEND_TIMEOUT`](crate::BACKEND_TIMEOUT) as
/// [`QueueError::Unanswered`], and a panic as [`QueueError::Panicked`], filed
/// once at `error`. A backend returns its error rather than logging it.
#[async_trait]
pub trait JobProducer: Send + Sync + 'static {
    /// Which backend this is, and what it honours beyond the contract every
    /// backend owes — read before every push, to refuse what it lacks.
    fn backend(&self) -> &'static QueueBackend;

    /// File `envelopes` on `queue`, honouring `options`.
    ///
    /// Called by the port only, after it checked `queue` and `options`, refused
    /// any option this backend's capabilities do not name, and sealed every
    /// envelope under a [`JobId`] of its own minting — which the backend keys the
    /// job by ([`Envelope::id`]) and never replaces with one of its own. Not
    /// atomic across envelopes: when it fails, some prefix of them may already be
    /// enqueued, and the error does not say how many.
    ///
    /// **An envelope whose id the backend still holds is not filed again**: while
    /// its job waits, is held back by a delay or a retry, or runs, the envelope is
    /// skipped and the call answers `Ok` for it, its options unread — so a call
    /// sent again after an answer that never came files nothing twice. An id the
    /// backend let go of, its job ended, is filed again as a new job. Held by
    /// `nest_rs_testing::queue_kit!`.
    ///
    /// `envelopes` holds [`ENQUEUE_BATCH`] at most, and the port waits
    /// [`BACKEND_TIMEOUT`](crate::BACKEND_TIMEOUT) for the answer before dropping the
    /// call: bound each round trip well inside it, and leave nothing a dropped call
    /// would have had to undo.
    ///
    /// A backend declaring [`Capability::UniquePush`] refuses an envelope whose
    /// key another job on `queue` still holds with [`QueueError::UniqueKeyHeld`],
    /// naming that job, and files nothing for it; the job holding it is never
    /// the envelope's own, which is skipped as held.
    async fn enqueue(
        &self,
        queue: &QueueName,
        envelopes: Vec<Envelope>,
        options: &PushOptions,
    ) -> Result<(), QueueError>;

    /// Cancel the job `id` on `queue`: `Ok(true)` when it had not started and
    /// now never will, `Ok(false)` when it already started, already finished, or
    /// is unknown.
    ///
    /// `true` is a promise, and only a job that was still waiting earns it — on
    /// its queue, or on the schedule a delay or a retry holds it on. Cancelling is
    /// the job's terminal outcome, so the backend releases its unique key and its
    /// checkpoint. A backend declaring [`Capability::Cancellation`] overrides this
    /// body, which answers the driver defect when it does not.
    async fn remove(&self, queue: &QueueName, id: &JobId) -> Result<bool, QueueError> {
        let _ = (queue, id);
        Err(unimplemented(self.backend(), "remove"))
    }

    /// Cancel the job holding the unique key `key` on `queue` — [`remove`](Self::remove),
    /// naming the job by its key: `Ok(true)` when it had not started and now never
    /// will, which frees the key; `Ok(false)` when it already started, already
    /// finished, or no job holds the key.
    ///
    /// A backend declaring [`Capability::UniquePush`] and
    /// [`Capability::Cancellation`] overrides this body, which answers the driver
    /// defect when it does not.
    async fn remove_unique(&self, queue: &QueueName, key: &str) -> Result<bool, QueueError> {
        let _ = (queue, key);
        Err(unimplemented(self.backend(), "remove_unique"))
    }
}

/// Cancel the job `receipt` names on `producer`'s backend: the outbox of the
/// unit the cancel runs in first, then the backend, under the net.
pub(crate) async fn cancel(
    producer: &dyn JobProducer,
    receipt: &PushReceipt,
) -> Result<bool, QueueError> {
    let queue = receipt.queue();
    producer
        .backend()
        .check(Capabilities::NONE.with(Capability::Cancellation))?;
    let unkept = unkeep(queue, Unkeep::Job(receipt.id())).await?;
    let removed = bounded(queue, "JobProducer::remove", async {
        producer.remove(queue, receipt.id()).await
    })
    .await?
        || unkept;
    if removed {
        tracing::info!(
            target: TARGET,
            queue = %queue,
            job_id = %receipt.id(),
            "queued job cancelled",
        );
    }
    Ok(removed)
}

/// Cancel the job holding the unique key `key` on `queue`, as [`cancel`] does a
/// receipt's.
pub(crate) async fn cancel_unique(
    producer: &dyn JobProducer,
    queue: QueueName,
    key: &str,
) -> Result<bool, QueueError> {
    check_unique_key(key)?;
    producer.backend().check(
        Capabilities::NONE
            .with(Capability::UniquePush)
            .with(Capability::Cancellation),
    )?;
    let unkept = unkeep(&queue, Unkeep::Key(key)).await?;
    let removed = bounded(&queue, "JobProducer::remove_unique", async {
        producer.remove_unique(&queue, key).await
    })
    .await?
        || unkept;
    if removed {
        tracing::info!(target: TARGET, queue = %queue, "unique job cancelled");
    }
    Ok(removed)
}

/// The one path every push takes to its backend: check, refuse, mint, seal,
/// file — and answer with receipts naming the ids the port minted.
pub(crate) async fn push_values(
    producer: &dyn JobProducer,
    queue: &QueueName,
    payloads: Vec<Value>,
    options: &PushOptions,
) -> Result<Vec<PushReceipt>, QueueError> {
    refuse_what_no_push_may_carry(producer.backend(), options)?;

    // Refused above, never short-circuited before: an empty batch owes the same
    // refusals as a full one.
    if payloads.is_empty() {
        return Ok(Vec::new());
    }

    let envelopes: Vec<Envelope> = payloads
        .into_iter()
        .map(|payload| envelope::seal(payload, JobId::mint(), options.unique_key()))
        .collect();
    let mut receipts: Vec<PushReceipt> = envelopes
        .iter()
        .map(|envelope| PushReceipt::new(queue.clone(), envelope.id().clone()))
        .collect();
    if let Some(kept) = keep(queue, &envelopes, options).await {
        return kept.map(|()| receipts);
    }
    let mut envelopes = envelopes.into_iter();
    let mut accepted = 0;
    loop {
        let batch: Vec<Envelope> = envelopes.by_ref().take(ENQUEUE_BATCH).collect();
        if batch.is_empty() {
            return Ok(receipts);
        }
        let size = batch.len();
        if let Err(failed) = enqueue(producer, queue, batch, options).await {
            if accepted == 0 {
                return Err(failed);
            }
            receipts.truncate(accepted);
            return Err(QueueError::PartiallyQueued {
                receipts,
                source: Box::new(failed),
            });
        }
        accepted += size;
    }
}

/// [`push_values`] for one job, answering its one receipt.
pub(crate) async fn push_value(
    producer: &dyn JobProducer,
    queue: &QueueName,
    payload: Value,
    options: &PushOptions,
) -> Result<PushReceipt, QueueError> {
    refuse_what_no_push_may_carry(producer.backend(), options)?;
    let envelope = envelope::seal(payload, JobId::mint(), options.unique_key());
    let receipt = PushReceipt::new(queue.clone(), envelope.id().clone());
    let envelopes = vec![envelope];
    match keep(queue, &envelopes, options).await {
        Some(kept) => kept?,
        None => enqueue(producer, queue, envelopes, options).await?,
    }
    Ok(receipt)
}

/// Write `envelopes` into the outbox of the unit the push runs in, so they are
/// filed after its commit and never on its rollback — `None` when the unit
/// keeps none, and the backend files them now. Each record is sealed only once
/// the handle reads it, so a push filed at once seals nothing twice. A delay is
/// kept as what is left of it, which the relay hands the backend once the unit
/// committed.
async fn keep(
    queue: &QueueName,
    envelopes: &[Envelope],
    options: &PushOptions,
) -> Option<Result<(), QueueError>> {
    let executor = nest_rs_database::current_executor()?;
    let delay = options.delay().map_or(Duration::ZERO, |delay| match delay {
        Delay::For(wait) => wait,
        Delay::Until(at) => at.duration_since(SystemTime::now()).unwrap_or_default(),
    });
    let written = executor.write_outbox(&mut envelopes.iter().map(|envelope| {
        OutboxRecord::new(
            queue.as_str(),
            envelope.id().uuid(),
            envelope.sealed(),
            envelope.unique_key().map(str::to_owned),
            delay,
        )
    }))?;
    Some(written.await.map_err(QueueError::Outbox))
}

/// Remove the record of a job not yet filed from the outbox of the unit the
/// cancel runs in, answering whether there was one; `false` where the unit
/// keeps no outbox.
async fn unkeep(queue: &QueueName, which: Unkeep<'_>) -> Result<bool, QueueError> {
    let Some(executor) = nest_rs_database::current_executor() else {
        return Ok(false);
    };
    let removing = match which {
        Unkeep::Job(id) => executor.remove_outbox(queue.as_str(), id.uuid()),
        Unkeep::Key(key) => executor.remove_outbox_unique(queue.as_str(), key),
    };
    match removing {
        Some(removing) => removing.await.map_err(QueueError::Outbox),
        None => Ok(false),
    }
}

/// The record a cancel names: by its job, or by its unique key.
enum Unkeep<'a> {
    Job(&'a JobId),
    Key(&'a str),
}

/// One call to the backend's [`JobProducer::enqueue`], under the net.
async fn enqueue(
    producer: &dyn JobProducer,
    queue: &QueueName,
    envelopes: Vec<Envelope>,
    options: &PushOptions,
) -> Result<(), QueueError> {
    bounded(queue, "JobProducer::enqueue", async {
        producer.enqueue(queue, envelopes, options).await
    })
    .await
}

/// Refuse options no backend could honour, and those `backend` does not
/// declare — before anything reaches it.
pub(crate) fn refuse_what_no_push_may_carry(
    backend: &QueueBackend,
    options: &PushOptions,
) -> Result<(), QueueError> {
    options.check()?;
    backend.check(options.required_capabilities())
}

/// What a backend's default removal body answers: `Unsupported` without
/// cancellation, or the driver defect of a capability declared and not implemented.
fn unimplemented(backend: &QueueBackend, method: &'static str) -> QueueError {
    if backend.capabilities().contains(Capability::Cancellation) {
        QueueError::backend(Unimplemented {
            backend: backend.name(),
            method,
        })
    } else {
        QueueError::Unsupported {
            capability: Capability::Cancellation,
            backend: backend.name(),
        }
    }
}

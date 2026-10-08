//! The producer seam: [`JobProducer`], what a backend implements to enqueue jobs,
//! and [`JobProducerExt`], what a feature calls to push them — and to cancel one
//! that has not started.
//!
//! **The port pushes; a backend enqueues.** Every push goes through
//! [`JobProducerExt`], which checks the queue's name and the options, refuses an
//! option the backend does not declare, mints each job's [`JobId`] and seals the
//! [`Envelope`] — then hands the backend sealed jobs and answers the caller with
//! receipts it wrote itself. A cancel takes the same path back.
//!
//! **Every call is waited on for [`BACKEND_TIMEOUT`](crate::BACKEND_TIMEOUT) at
//! most**, then answers [`QueueError::Unanswered`], naming the queue.

use async_trait::async_trait;
use serde_json::Value;

use crate::backend::bounded;
use crate::error::Unimplemented;
use crate::push_options::check_unique_key;
use crate::{
    Capabilities, Capability, Destination, Envelope, JobId, PushOptions, PushReceipt, QueueBackend,
    QueueError, QueueName, TARGET, envelope,
};

/// The most jobs the port hands a backend's [`JobProducer::enqueue`] in one
/// call. A push of more is filed in calls of this many, in order, each under
/// [`BACKEND_TIMEOUT`](crate::BACKEND_TIMEOUT).
pub const ENQUEUE_BATCH: usize = 100;

/// What a queue backend implements to enqueue jobs. Inject it as
/// `Arc<dyn JobProducer>` and push through [`JobProducerExt`].
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
    /// `envelopes` holds [`ENQUEUE_BATCH`] at most, and the port waits
    /// [`BACKEND_TIMEOUT`](crate::BACKEND_TIMEOUT) for the answer before dropping the
    /// call: bound each round trip well inside it, and leave nothing a dropped call
    /// would have had to undo.
    ///
    /// A backend declaring [`Capability::UniquePush`] refuses an envelope whose
    /// key another job on `queue` still holds with [`QueueError::UniqueKeyHeld`],
    /// naming that job, and files nothing for it.
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

/// Push jobs through any [`JobProducer`] — `Arc<dyn JobProducer>` included.
///
/// An extension trait so the producer stays object-safe; import it beside
/// `JobProducer` and every method below is on the handle you injected.
#[async_trait]
pub trait JobProducerExt: JobProducer {
    /// Push `job` onto `destination`, a queue's marker. `options` is `None` for
    /// an immediate push, or a [`PushOptions`].
    ///
    /// The queue and the payload type both come from `destination`, so a push
    /// onto the wrong queue, or with the wrong payload, does not compile:
    ///
    /// ```compile_fail,E0308
    /// use nest_rs_queue::{queue, JobProducer, JobProducerExt};
    ///
    /// #[queue(name = "transcode", job = String)]
    /// struct TranscodeQueue;
    ///
    /// async fn demo<P: JobProducer>(producer: &P) {
    ///     // `TranscodeQueue`'s job is `String`; a `u32` does not compile.
    ///     producer.push(TranscodeQueue, 42u32, None).await.unwrap();
    /// }
    /// ```
    ///
    /// The receipt names the queue and the [`JobId`] the port minted for the job.
    ///
    /// Fails with [`QueueError::Unsupported`] for an option the backend does not
    /// declare, [`QueueError::InvalidQueueName`] or
    /// [`QueueError::InvalidUniqueKey`] for a value no backend could file,
    /// [`QueueError::Serialize`] for a payload that does not serialize,
    /// [`QueueError::UniqueKeyHeld`] for a unique key another job still holds,
    /// [`QueueError::Unanswered`], naming the queue, for a backend silent past
    /// [`BACKEND_TIMEOUT`](crate::BACKEND_TIMEOUT), and with whatever the backend returns otherwise.
    ///
    /// A backend's failure is a failure to *answer*: the write may have reached
    /// the backend and its reply not come back, in which case the job is enqueued
    /// and will run. A caller that retries such a failure enqueues it twice, unless
    /// the push carries a unique key — the same at-least-once the worker's side
    /// has, met at the push.
    async fn push<D, O>(
        &self,
        destination: D,
        job: D::Job,
        options: O,
    ) -> Result<PushReceipt, QueueError>
    where
        D: Destination + Send,
        O: Into<Option<PushOptions>> + Send,
    {
        let options = options.into().unwrap_or_default();
        let queue = destination.queue_name()?;
        let payload = serde_json::to_value(&job)?;
        push_value(self, &queue, payload, &options).await
    }

    /// Push every job of `jobs` onto `destination`, in calls to the backend of
    /// [`ENQUEUE_BATCH`] jobs at most, in order, returning one receipt per job in
    /// input order.
    ///
    /// Not atomic. A failure after the backend accepted some of its calls is
    /// [`QueueError::PartiallyQueued`], carrying the receipts of the jobs of
    /// those calls — queued, and to be left out of a retry — beside the failure
    /// that stopped the push; the jobs of the failing call may be queued too,
    /// and no receipt says which, so retrying them can queue one twice. Each
    /// call is waited on for [`BACKEND_TIMEOUT`](crate::BACKEND_TIMEOUT) at most. A delay applies to
    /// every job; a unique key names one job and is refused here with
    /// [`QueueError::InvalidOptions`] — push jobs that each need one with
    /// [`push`](Self::push).
    async fn push_many<D, I, O>(
        &self,
        destination: D,
        jobs: I,
        options: O,
    ) -> Result<Vec<PushReceipt>, QueueError>
    where
        D: Destination + Send,
        I: IntoIterator<Item = D::Job> + Send,
        I::IntoIter: Send,
        O: Into<Option<PushOptions>> + Send,
    {
        let options = options.into().unwrap_or_default();
        if options.unique_key().is_some() {
            return Err(QueueError::InvalidOptions {
                reason: "a unique key names one job, so a push of many cannot carry one",
            });
        }
        let queue = destination.queue_name()?;
        let payloads = jobs
            .into_iter()
            .map(|job| serde_json::to_value(&job))
            .collect::<Result<Vec<_>, _>>()?;
        push_values(self, &queue, payloads, &options).await
    }

    /// Push a JSON `payload` onto the queue named `queue`, for the one case no
    /// marker type is in reach: a queue **this binary does not declare**, drained
    /// by another deployment whose `#[queue]` types it does not link.
    ///
    /// This hatch checks neither the name against a declared queue nor the
    /// payload against its job type, so prefer [`push`](Self::push) wherever a
    /// marker exists.
    async fn push_json<O>(
        &self,
        queue: &str,
        payload: Value,
        options: O,
    ) -> Result<PushReceipt, QueueError>
    where
        O: Into<Option<PushOptions>> + Send,
    {
        let options = options.into().unwrap_or_default();
        let queue = QueueName::new(queue.to_owned())?;
        push_value(self, &queue, payload, &options).await
    }

    /// Cancel the job `receipt` names, so it never starts.
    ///
    /// `Ok(true)`: the job had not started, and now never will — whether it was
    /// waiting on its queue, or on the schedule a delay or a retry holds it on.
    /// Cancelling is the job's terminal outcome, so its unique key, if it held
    /// one, is free for the next push. `Ok(false)`: it already started — it then
    /// runs to its own outcome — already finished, or is unknown to the backend.
    ///
    /// Fails with [`QueueError::Unsupported`] on a backend without
    /// [`Capability::Cancellation`], before the backend sees the call, and with
    /// [`QueueError::Unanswered`] when the backend does not answer within
    /// [`BACKEND_TIMEOUT`](crate::BACKEND_TIMEOUT) — the cancel may then still
    /// land.
    async fn cancel(&self, receipt: &PushReceipt) -> Result<bool, QueueError> {
        let queue = receipt.queue();
        self.backend()
            .check(Capabilities::NONE.with(Capability::Cancellation))?;
        let removed = bounded(
            queue,
            "JobProducer::remove",
            self.remove(queue, receipt.id()),
        )
        .await?;
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

    /// Cancel the job holding the unique key `key` on `destination` —
    /// [`cancel`](Self::cancel), naming the job by its key rather than its
    /// receipt, with the same answer: `Ok(true)` when it had not started and now
    /// never will, which frees the key; `Ok(false)` when it already started,
    /// already finished, or no job holds the key.
    ///
    /// Fails with [`QueueError::InvalidUniqueKey`] for a key no push could file,
    /// and with [`QueueError::Unsupported`] on a backend without unique jobs or
    /// without cancellation, before the backend sees the call — and with
    /// [`QueueError::Unanswered`] as [`cancel`](Self::cancel) does.
    async fn cancel_unique<D>(&self, destination: D, key: &str) -> Result<bool, QueueError>
    where
        D: Destination + Send,
    {
        let queue = destination.queue_name()?;
        check_unique_key(key)?;
        self.backend().check(
            Capabilities::NONE
                .with(Capability::UniquePush)
                .with(Capability::Cancellation),
        )?;
        let removed = bounded(
            &queue,
            "JobProducer::remove_unique",
            self.remove_unique(&queue, key),
        )
        .await?;
        if removed {
            tracing::info!(
                target: TARGET,
                queue = %queue,
                unique_key = key,
                "unique job cancelled",
            );
        }
        Ok(removed)
    }
}

impl<T: JobProducer + ?Sized> JobProducerExt for T {}

/// The one path every push takes to its backend: check, refuse, mint, seal,
/// file — and answer with receipts naming the ids the port minted.
async fn push_values<P: JobProducer + ?Sized>(
    producer: &P,
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
async fn push_value<P: JobProducer + ?Sized>(
    producer: &P,
    queue: &QueueName,
    payload: Value,
    options: &PushOptions,
) -> Result<PushReceipt, QueueError> {
    refuse_what_no_push_may_carry(producer.backend(), options)?;
    let envelope = envelope::seal(payload, JobId::mint(), options.unique_key());
    let receipt = PushReceipt::new(queue.clone(), envelope.id().clone());
    enqueue(producer, queue, vec![envelope], options).await?;
    Ok(receipt)
}

/// One call to the backend's [`JobProducer::enqueue`], under the net.
async fn enqueue<P: JobProducer + ?Sized>(
    producer: &P,
    queue: &QueueName,
    envelopes: Vec<Envelope>,
    options: &PushOptions,
) -> Result<(), QueueError> {
    bounded(
        queue,
        "JobProducer::enqueue",
        producer.enqueue(queue, envelopes, options),
    )
    .await
}

/// Refuse options no backend could honour, and those `backend` does not
/// declare — before anything reaches it.
fn refuse_what_no_push_may_carry(
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

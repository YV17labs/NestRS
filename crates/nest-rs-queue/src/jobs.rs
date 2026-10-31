//! [`Jobs`] — the handle a feature pushes jobs through, and cancels them.

use std::fmt;
use std::sync::Arc;

use crate::producer::{cancel, cancel_unique, push_value, push_values};
use crate::{Destination, JobProducer, PushOptions, PushReceipt, QueueBackend, QueueError};

/// The queue port's one handle: what a feature injects as `Arc<Jobs>` to push a
/// job onto a queue its marker names, and to cancel one that has not started.
///
/// The backend's binding registers it over its [`JobProducer`], so a feature
/// reaches no backend's contract; with no binding imported, a provider
/// injecting it fails the boot naming the remedy
/// ([`BACKEND_REMEDY`](crate::BACKEND_REMEDY)).
///
/// The queue and the payload type both come from the destination, so a push
/// onto the wrong queue, or with the wrong payload, does not compile:
///
/// ```compile_fail,E0308
/// use nest_rs_queue::{Jobs, queue};
///
/// #[queue(name = "transcode", job = String)]
/// struct TranscodeQueue;
///
/// async fn transcode(jobs: &Jobs) {
///     // `TranscodeQueue`'s job is `String`; a `u32` does not compile.
///     jobs.push(TranscodeQueue, 42u32).await.unwrap();
/// }
/// ```
///
/// **Every call is waited on for [`BACKEND_TIMEOUT`](crate::BACKEND_TIMEOUT) at
/// most**, then fails with [`QueueError::Unanswered`], naming the queue — or
/// [`QueueError::Panicked`] for a backend that panicked answering. A failure to
/// answer may hide a write that landed: a push retried after one files the job
/// twice, each push minting its own id, unless it carries a unique key.
///
/// **A push inside a unit of work whose data driver keeps an outbox** is written
/// into the unit's transaction, so the job is filed after the commit and never
/// on a rollback, and its receipt names a job that exists exactly when the
/// caller's writes do. The backend is not asked then: a write the outbox
/// refuses is [`QueueError::Outbox`], and a unique key another job holds is
/// checked once the unit committed — the record is then dropped, said at
/// `info` naming both jobs, and no error reaches the caller.
#[derive(Clone)]
pub struct Jobs {
    producer: Arc<dyn JobProducer>,
}

impl Jobs {
    /// The handle over `producer`, a backend's: what its binding registers, and
    /// what a test seeds over a backend of its own.
    pub fn new(producer: Arc<dyn JobProducer>) -> Self {
        Self { producer }
    }

    /// Which backend files the jobs, and what it honours.
    pub fn backend(&self) -> &'static QueueBackend {
        self.producer.backend()
    }

    /// Push `job` onto `destination`, a queue's marker, at once, and answer the
    /// receipt naming the queue and the [`JobId`](crate::JobId) the port minted.
    ///
    /// Fails with [`QueueError::InvalidQueueName`] for a name no backend could
    /// file, [`QueueError::Serialize`] for a payload that does not serialize,
    /// and with whatever the backend returns otherwise.
    pub async fn push<D: Destination>(
        &self,
        destination: D,
        job: D::Job,
    ) -> Result<PushReceipt, QueueError> {
        self.push_with(destination, job, PushOptions::default())
            .await
    }

    /// [`push`](Self::push), honouring `options`: a delay, a unique key.
    ///
    /// Fails, besides, with [`QueueError::Unsupported`] for an option the
    /// backend does not declare and [`QueueError::InvalidUniqueKey`] for a key
    /// no backend could file, both before the backend sees the call, and with
    /// [`QueueError::UniqueKeyHeld`] for a key another job still holds.
    pub async fn push_with<D: Destination>(
        &self,
        destination: D,
        job: D::Job,
        options: PushOptions,
    ) -> Result<PushReceipt, QueueError> {
        let queue = destination.queue_name()?;
        let payload = serde_json::to_value(&job)?;
        push_value(self.producer.as_ref(), &queue, payload, &options).await
    }

    /// Push every job of `jobs` onto `destination`, at once, in calls to the
    /// backend of [`ENQUEUE_BATCH`](crate::ENQUEUE_BATCH) jobs at most, in
    /// order, answering one receipt per job in input order.
    ///
    /// Not atomic on its own: a failure after the backend accepted some of its
    /// calls is [`QueueError::PartiallyQueued`], carrying the receipts of the
    /// jobs of those calls — queued, and to be left out of a retry — beside the
    /// failure that stopped the push; the jobs of the failing call may be
    /// queued too. Inside a unit of work whose data driver keeps an outbox,
    /// every job is written into its transaction in one write, so the push
    /// commits or rolls back whole with it.
    pub async fn push_many<D, I>(
        &self,
        destination: D,
        jobs: I,
    ) -> Result<Vec<PushReceipt>, QueueError>
    where
        D: Destination,
        I: IntoIterator<Item = D::Job>,
    {
        self.push_many_with(destination, jobs, PushOptions::default())
            .await
    }

    /// [`push_many`](Self::push_many), honouring `options` for every job: a
    /// delay. A unique key names one job, so it is refused here with
    /// [`QueueError::InvalidOptions`] — push each job that needs one with
    /// [`push_with`](Self::push_with).
    pub async fn push_many_with<D, I>(
        &self,
        destination: D,
        jobs: I,
        options: PushOptions,
    ) -> Result<Vec<PushReceipt>, QueueError>
    where
        D: Destination,
        I: IntoIterator<Item = D::Job>,
    {
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
        push_values(self.producer.as_ref(), &queue, payloads, &options).await
    }

    /// Cancel the job `receipt` names, so it never starts.
    ///
    /// `Ok(true)`: the job had not started, and now never will — whether it
    /// was waiting on its queue, or on the schedule a delay or a retry holds it
    /// on; its unique key, if it held one, is free for the next push.
    /// `Ok(false)`: it already started — it then runs to its own outcome —
    /// already finished, or is unknown to the backend. Inside a unit of work
    /// whose data driver keeps an outbox, a record of the job no relay has
    /// filed yet is deleted first, on the unit's transaction; either answers
    /// `true`.
    ///
    /// Fails with [`QueueError::Unsupported`] on a backend without
    /// [`Capability::Cancellation`](crate::Capability::Cancellation), before
    /// the backend sees the call.
    pub async fn cancel(&self, receipt: &PushReceipt) -> Result<bool, QueueError> {
        cancel(self.producer.as_ref(), receipt).await
    }

    /// Cancel the job holding the unique key `key` on `destination` —
    /// [`cancel`](Self::cancel), naming the job by its key rather than its
    /// receipt, with the same answer. A record carrying the key in the unit's
    /// outbox is deleted too.
    ///
    /// Fails with [`QueueError::InvalidUniqueKey`] for a key no push could
    /// file, and with [`QueueError::Unsupported`] on a backend without unique
    /// jobs or without cancellation, before the backend sees the call.
    pub async fn cancel_unique<D: Destination>(
        &self,
        destination: D,
        key: &str,
    ) -> Result<bool, QueueError> {
        cancel_unique(self.producer.as_ref(), destination.queue_name()?, key).await
    }
}

impl fmt::Debug for Jobs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Jobs")
            .field("backend", &self.backend().name())
            .finish()
    }
}

/// Re-exported through the crate's `__private`.
pub(crate) mod __private {
    use std::sync::Arc;

    use super::Jobs;
    use crate::JobProducer;

    /// The backend's producer `jobs` hands its pushes to: what the port's
    /// worker and relay, and the driver kit, call the contract through.
    pub fn producer_of(jobs: &Jobs) -> &Arc<dyn JobProducer> {
        &jobs.producer
    }
}

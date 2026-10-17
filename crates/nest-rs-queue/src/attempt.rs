//! [`Attempt`] — the attempt a `#[process]` method is running, handed to it as
//! a parameter beside its job.

use nest_rs_database::JobRef;

use crate::{JobId, QueueName};

/// The attempt a `#[process]` method is running: what it may key its own
/// idempotency on.
///
/// Delivery is at least once, so a method calling out — a mail send, a
/// payment — keys the call on [`job_id`](Self::job_id), the one name its job
/// keeps across every attempt and redelivery:
///
/// ```
/// # use anyhow::Result;
/// # use nest_rs_core::injectable;
/// # use nest_rs_queue::{Attempt, input, processor, queue};
/// # #[input]
/// # #[derive(Clone)]
/// # pub struct NotifyCommand { pub to: String }
/// # #[queue(name = "notify", job = NotifyCommand)]
/// # pub struct NotifyQueue;
/// # #[injectable]
/// # #[derive(Default)]
/// # pub struct NotifyProcessor;
/// # async fn send(to: &str, idempotency_key: &str) -> Result<()> { Ok(()) }
/// # #[processor]
/// # impl NotifyProcessor {
/// #[process(queue = NotifyQueue)]
/// async fn notify(&self, job: NotifyCommand, attempt: Attempt) -> Result<()> {
///     send(&job.to, &attempt.job_id().to_string()).await
/// }
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct Attempt {
    job_id: JobId,
    number: u32,
    queue: QueueName,
}

impl Attempt {
    pub(crate) const fn new(job_id: JobId, number: u32, queue: QueueName) -> Self {
        Self {
            job_id,
            number,
            queue,
        }
    }

    /// The job's id: the one its push's receipt returned, the same on every
    /// attempt and every redelivery.
    pub fn job_id(&self) -> &JobId {
        &self.job_id
    }

    /// Which attempt this is, from 1. A redelivery after a lost answer may
    /// repeat a number.
    pub fn number(&self) -> u32 {
        self.number
    }

    /// The queue the job was pushed on.
    pub fn queue(&self) -> &QueueName {
        &self.queue
    }

    /// The same identity, as the unit-of-work port names it.
    pub(crate) fn job_ref(&self) -> JobRef {
        JobRef::new(self.queue.as_str(), self.job_id.uuid(), self.number)
    }
}

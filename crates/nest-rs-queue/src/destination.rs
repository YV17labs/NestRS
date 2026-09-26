//! [`Destination`] — what a push names: a static queue, or one instance of a
//! dynamic queue.

use crate::{DynamicQueue, Job, QueueError, QueueInstance, QueueName};

/// What a push names, and the payload it accepts.
///
/// Implemented by the marker `#[queue(name = ..)]` declares and by
/// [`QueueInstance`], so `queue.push(AudioQueue, job, None)` and
/// `queue.push(TenantQueue::instance(&slug)?, job, None)` are one call, and the
/// payload is still checked against the queue's `Job` at compile time.
/// "Destination" is the OpenTelemetry messaging word for where a message is
/// sent — what a job span reports as `messaging.destination.name`.
pub trait Destination {
    /// The payload this destination accepts.
    type Job: Job;

    /// The wire name the push enqueues the job under.
    fn queue_name(&self) -> Result<QueueName, QueueError>;
}

impl<Q: DynamicQueue> Destination for QueueInstance<Q> {
    type Job = Q::Job;

    fn queue_name(&self) -> Result<QueueName, QueueError> {
        Ok(self.name().clone())
    }
}

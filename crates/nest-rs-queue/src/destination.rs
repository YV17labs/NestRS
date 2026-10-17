//! [`Destination`] — what a push names: a queue's marker.

use crate::{Job, QueueError, QueueName};

/// What a push names, and the payload it accepts.
///
/// Implemented by the marker `#[queue(name = ..)]` declares, so
/// `queue.push(AudioQueue, job, None)` checks the payload against the queue's
/// `Job` at compile time. "Destination" is the OpenTelemetry messaging word for
/// where a message is sent — what a job span reports as
/// `messaging.destination.name`.
pub trait Destination {
    /// The payload this destination accepts.
    type Job: Job;

    /// The wire name the push enqueues the job under.
    fn queue_name(&self) -> Result<QueueName, QueueError>;
}

//! [`PushReceipt`] — what a push returns for each job it filed.

use serde::{Deserialize, Serialize};

use crate::{JobId, QueueName};

/// What a push returns for each job it filed: the queue it went to, and the
/// [`JobId`] the port minted for it.
///
/// The id is the one every attempt at the job reports as `messaging.message.id`
/// on its span and as `job_id` on its operation line, and the receipt is what
/// [`cancel`](crate::JobProducerExt::cancel) takes to name the job — the queue
/// travels with the id because a job is only ever looked for on its own queue.
///
/// A receipt is data: it serializes as `{"queue": …, "id": …}`, so a caller that
/// cancels later — from another request, another process — keeps it wherever it
/// keeps state, and both halves are checked again when it is read back.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PushReceipt {
    queue: QueueName,
    id: JobId,
}

impl PushReceipt {
    /// The receipt for the job `id` on `queue` — what a push returns, or what a
    /// caller rebuilds from the two halves it kept.
    pub fn new(queue: QueueName, id: JobId) -> Self {
        Self { queue, id }
    }

    /// The queue the job was pushed onto.
    pub fn queue(&self) -> &QueueName {
        &self.queue
    }

    /// The id the port minted for the job.
    pub fn id(&self) -> &JobId {
        &self.id
    }
}

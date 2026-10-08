//! [`QueueBackend`] — who a queue backend is, and what it supports — and
//! [`BACKEND_TIMEOUT`], how long the port waits on it.

use std::future::{Future, poll_fn};
use std::pin::pin;
use std::task::Poll;
use std::time::Duration;

use crate::{Capabilities, QueueError, QueueName};

/// The remedy the boot names when two queue backends bind the queue port.
pub const BACKEND_REMEDY: &str = "Import exactly one queue backend's binding — \
     `nest_rs::redis::RedisQueueModule` binds the queue port over Redis.";

/// How long the port waits for a backend to answer any call it makes — a push's
/// [`enqueue`](crate::JobProducer::enqueue), a cancel's
/// [`remove`](crate::JobProducer::remove) or
/// [`remove_unique`](crate::JobProducer::remove_unique), a checkpoint's
/// [`load`](crate::CheckpointStore::load) or
/// [`save`](crate::CheckpointStore::save), and every
/// [`JobConsumer`](crate::JobConsumer) call. Past it the port stops waiting,
/// drops the call where it stands, and answers [`QueueError::Unanswered`],
/// naming the queue and the call: a push or a cancel fails to its caller, and a
/// checkpoint's read or save fails the attempt, retryably.
///
/// A net, never a backend's budget: it sits above the Redis adapter's
/// per-command budget (`RedisConfig::connect_timeout`, 10 s by default) and
/// below the HTTP edge's request timeout (`HttpConfig::request_timeout`, 30 s).
pub const BACKEND_TIMEOUT: Duration = Duration::from_secs(20);

/// A queue backend's name and optional capabilities, declared once as a
/// constant and read by every site that refuses a declaration the backend
/// cannot honour.
///
/// The name is the backend's `messaging.system` on every job span, spelled as
/// OpenTelemetry's messaging conventions spell a system: lowercase (`"redis"`).
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct QueueBackend {
    name: &'static str,
    capabilities: Capabilities,
}

impl QueueBackend {
    /// A backend named `name`, honouring `capabilities`.
    pub const fn new(name: &'static str, capabilities: Capabilities) -> Self {
        Self { name, capabilities }
    }

    /// The backend's name, as its job spans report it.
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// What the backend honours beyond the contract every backend owes.
    pub const fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    /// Refuse the first capability of `required` this backend does not
    /// declare, with [`QueueError::Unsupported`].
    pub(crate) fn check(&self, required: Capabilities) -> Result<(), QueueError> {
        match required
            .iter()
            .find(|capability| !self.capabilities.contains(*capability))
        {
            Some(capability) => Err(QueueError::Unsupported {
                capability,
                backend: self.name,
            }),
            None => Ok(()),
        }
    }
}

/// A backend's `answer` to `call` on `queue`, awaited for at most
/// [`BACKEND_TIMEOUT`] — past it the call is dropped where it stands, and the
/// answer is [`QueueError::Unanswered`].
///
/// Polled once bare before the net is armed: a backend that answers at once
/// pays no timer, and needs no runtime clock.
pub(crate) async fn bounded<T>(
    queue: &QueueName,
    call: &'static str,
    answer: impl Future<Output = Result<T, QueueError>>,
) -> Result<T, QueueError> {
    let mut answer = pin!(answer);
    if let Poll::Ready(answered) = poll_fn(|cx| Poll::Ready(answer.as_mut().poll(cx))).await {
        return answered;
    }
    match tokio::time::timeout(BACKEND_TIMEOUT, answer).await {
        Ok(answered) => answered,
        Err(_) => Err(QueueError::Unanswered {
            queue: queue.clone(),
            call,
        }),
    }
}

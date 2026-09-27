//! [`QueueBackend`] — who a queue backend is, and what it supports — and
//! [`BACKEND_TIMEOUT`], how long the port waits on it.

use std::future::{Future, poll_fn};
use std::pin::pin;
use std::task::Poll;
use std::time::Duration;

use crate::{Capabilities, QueueError, QueueName};

/// The remedy the boot names when two queue backends bind the queue port —
/// shared with every backend's binding, so the two halves of the rule cannot
/// drift.
pub const BACKEND_REMEDY: &str = "Import exactly one queue backend's bindings — \
     `nest_rs::redis::RedisQueueModule` binds the producer over Redis.";

/// How long the port waits for a backend to answer any call it makes — a push's
/// [`enqueue`](crate::JobProducer::enqueue), a cancel's
/// [`remove`](crate::JobProducer::remove) or
/// [`remove_unique`](crate::JobProducer::remove_unique), a checkpoint's
/// [`load`](crate::CheckpointStore::load), [`save`](crate::CheckpointStore::save)
/// or [`clear`](crate::CheckpointStore::clear). Past it the port stops waiting,
/// drops the call where it stands, and answers [`QueueError::Unanswered`],
/// naming the queue and the call: a push or a cancel fails to its caller, a
/// checkpoint's read or save fails the attempt, retryably, and a checkpoint's
/// clear at the job's end is said at `warn`, the outcome standing.
///
/// **A net, never a backend's budget.** A backend bounds each round trip it
/// makes, so an outage reaches the caller as the backend's own failure — its
/// cause, and what to change — and promptly; the net is what still answers when
/// a backend does not.
///
/// The value sits between the two bounds either side of it, with room on both:
///
/// - **Above the backend the framework ships.** The Redis adapter bounds every
///   command at its connection's budget (`RedisConfig::connect_timeout`, 10 s by
///   default) and fails each call within one budget of Redis going silent: a
///   healthy Redis answers in milliseconds, one reopening a dropped connection
///   within the budget, and one that cannot fails at it with its own sentence.
///   The net must not pre-empt any of the three, or it would cut short an answer
///   still coming and replace a named cause with a bare timeout; twice the
///   default budget leaves room for a deployment that raised it. A push hands
///   the backend at most [`ENQUEUE_BATCH`](crate::ENQUEUE_BATCH) jobs per call,
///   so the net holds a push of any size a healthy backend files.
/// - **Below the HTTP edge's request timeout** (`HttpConfig::request_timeout`,
///   30 s by default), which answers `503` naming nothing. A push or a cancel is
///   usually made inside a request, and its caller has to hear the port's error,
///   naming the queue, while the request can still answer.
///
/// A constant rather than a setting: no backend that answers at all needs
/// longer, and one that needs a different net has a budget of its own to set
/// instead.
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
    ///
    /// `pub(crate)`: the port refuses a declaration before any backend sees it,
    /// so a driver never has a question to ask here.
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
/// The call is polled once bare before the net is armed, as the throttler's
/// guard arms its own: a backend that answers at once — one in process, a test
/// double — pays no timer, and needs no runtime clock to be called.
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

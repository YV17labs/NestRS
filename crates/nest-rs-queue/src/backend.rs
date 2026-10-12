//! [`QueueBackend`] — who a queue backend is, and what it supports — and
//! [`BACKEND_TIMEOUT`], how long the port waits on it.

use std::future::Future;
use std::time::Duration;

use nest_rs_core::{Unanswered, within};

use crate::{Capabilities, QueueError, QueueName, TARGET};

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
/// checkpoint's read or save fails the attempt, retryably. A backend that
/// panics answering is treated the same way, as [`QueueError::Panicked`] — the
/// panic filed once at `error` — and the worker retries its own calls under its
/// backoff.
///
/// A net, never a backend's budget: it sits above the Redis adapter's
/// per-command budget (`RedisConfig::connect_timeout`, 10 s by default) and
/// below the HTTP edge's request timeout (`HttpConfig::request_timeout`, 20 s).
pub const BACKEND_TIMEOUT: Duration = Duration::from_secs(18);

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
/// [`BACKEND_TIMEOUT`] through [`within`] — past it the call is dropped where
/// it stands, and the answer is [`QueueError::Unanswered`]; a backend that
/// panics answering is [`QueueError::Panicked`], its payload filed here once.
///
/// A caller passes an `async` block, so a backend panicking before it hands
/// back its future is contained too.
pub(crate) async fn bounded<T>(
    queue: &QueueName,
    call: &'static str,
    answer: impl Future<Output = Result<T, QueueError>>,
) -> Result<T, QueueError> {
    match within(BACKEND_TIMEOUT, answer).await {
        Ok(answered) => answered,
        Err(Unanswered::Panicked(payload)) => {
            nest_rs_core::contained_panic!(
                target: TARGET,
                payload.as_ref(),
                "queue backend panicked answering a call; the call is abandoned",
                queue = %queue,
                call,
            );
            Err(QueueError::Panicked {
                queue: queue.clone(),
                call,
            })
        }
        Err(_) => Err(QueueError::Unanswered {
            queue: queue.clone(),
            call,
        }),
    }
}

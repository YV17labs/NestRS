//! [`Queue`] — the compile-time identity of a queue — and the two shapes it
//! takes.
//!
//! A queue's name would otherwise be a bare string repeated on both sides, one
//! literal on the consumer and another at the push, with nothing linking the
//! two nor the payload either side agrees on. [`Queue`] makes that identity a
//! **type** carrying both, declared once at the feature port with the
//! [`queue`](macro@crate::queue) attribute, so a typo or a mismatched payload is a
//! compile error rather than a job that never drains.
//!
//! - **Static** — `#[queue(name = "audio", job = TranscodeCommand)]`: one queue,
//!   and the marker itself is what a push names.
//! - **Dynamic** — `#[queue(prefix = "tenant", job = SyncCommand)]`: one queue
//!   per runtime key, which a push creates — `TenantQueue::instance(&slug)?` —
//!   and one `#[process]` method drains, instance by instance.

use std::fmt;
use std::marker::PhantomData;

use crate::{Job, QueueError, QueueName};

/// The type-level identity of a queue: its wire name, its kind and its payload,
/// implemented by the [`queue`](macro@crate::queue) attribute on a unit struct at the
/// feature port.
///
/// ```
/// use nest_rs_queue::{Queue, QueueKind, queue};
///
/// // Any `T: Serialize + DeserializeOwned + Clone + Send + Sync + Unpin` is a
/// // `Job`; a real feature uses its own `TranscodeCommand` payload struct.
/// #[queue(name = "transcode", job = String)]
/// struct TranscodeQueue;
///
/// assert_eq!(<TranscodeQueue as Queue>::NAME, "transcode");
/// assert_eq!(<TranscodeQueue as Queue>::KIND, QueueKind::Static);
/// ```
///
/// Both sides then name the type — `queue.push(TranscodeQueue, job, None)` on the
/// producer, `#[process(queue = TranscodeQueue)]` on the consumer — and the
/// decorator checks the consumer's job argument is `Self::Job`, so a mismatch is
/// a compile error naming both types.
pub trait Queue: 'static {
    /// A static queue's wire name, or a dynamic queue's prefix.
    const NAME: &'static str;
    /// Whether this is one queue, or one per runtime key.
    const KIND: QueueKind;
    /// The payload pushed onto and drained off this queue.
    type Job: Job;
}

/// Whether a queue is one queue, or one per runtime key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QueueKind {
    /// One queue under one name — `#[queue(name = ..)]`.
    Static,
    /// One queue per runtime key under a prefix — `#[queue(prefix = ..)]`.
    Dynamic,
}

impl QueueKind {
    /// The kind in lowercase, for the sentence that names two methods claiming
    /// one queue.
    ///
    /// **Not what the boot line carries** — that is a `dynamic` boolean field,
    /// because a log line is queried on a field rather than read for a word.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Dynamic => "dynamic",
        }
    }
}

/// A queue addressed by a prefix and a runtime key, implemented by
/// `#[queue(prefix = ..)]` — the bound [`QueueInstance`] takes, so an instance
/// of a static queue does not compile.
pub trait DynamicQueue: Queue {}

/// One instance of the dynamic queue `Q`: its prefix and a runtime key — what a
/// push to that instance names.
pub struct QueueInstance<Q> {
    name: QueueName,
    queue: PhantomData<fn() -> Q>,
}

impl<Q: DynamicQueue> QueueInstance<Q> {
    /// The instance of `Q` under `key`, refused with
    /// [`QueueError::InvalidQueueName`] unless `key` is 1 to
    /// [`QueueName::MAX_LEN`] of `[A-Za-z0-9_.-]`.
    ///
    /// `#[doc(hidden)]`: the way to an instance is the decorator's own
    /// `Q::instance(key)`, whose body is this call. Shown beside it, rustdoc
    /// offers two constructors for one thing — the crate's other macro-only
    /// seams (`ProcessMethod::new`, `Checkpoint::open`) are hidden for the same
    /// reason.
    #[doc(hidden)]
    pub fn new(key: &str) -> Result<Self, QueueError> {
        Ok(Self {
            name: QueueName::instance(Q::NAME, key)?,
            queue: PhantomData,
        })
    }
}

impl<Q> QueueInstance<Q> {
    /// The instance's wire name: `Q`'s prefix and the key, joined by `#`.
    pub fn name(&self) -> &QueueName {
        &self.name
    }
}

impl<Q> Clone for QueueInstance<Q> {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            queue: PhantomData,
        }
    }
}

impl<Q> fmt::Debug for QueueInstance<Q> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("QueueInstance").field(&self.name).finish()
    }
}

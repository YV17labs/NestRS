//! Compile-time queue identity.
//!
//! A queue's name is otherwise a bare string repeated on both sides —
//! `#[process(queue = "audio")]` on the consumer, `push_json("audio", …)` on
//! the producer — with nothing linking the two literals or the payload type
//! either side agrees on. [`QueueName`] turns that identity into a **type**
//! that carries both facts: the wire name ([`QueueName::NAME`]) and the job
//! type ([`QueueName::Job`]). Declared once at the feature port next to the
//! [`Job`] payload with the [`queue`](crate::queue) attribute macro, it is the
//! single artifact producer and consumer both import — so a typo or a
//! mismatched payload becomes a compile error instead of a job that silently
//! never drains.

use crate::processor::Job;

/// The type-level identity of a queue: its wire name plus the [`Job`] payload
/// it carries. Implemented by the [`queue`](crate::queue) macro on a unit
/// struct living beside the payload at the feature port:
///
/// ```
/// use nest_rs_queue::{queue, QueueName};
///
/// // Any `T: Serialize + DeserializeOwned + Clone + Send + Sync + Unpin` is a
/// // `Job`; a real feature uses its own `TranscodeCommand` payload struct.
/// #[queue(name = "transcode", job = String)]
/// struct TranscodeQueue;
///
/// assert_eq!(<TranscodeQueue as QueueName>::NAME, "transcode");
/// ```
///
/// Both sides then name the *type*, not the string:
/// [`push_to`](crate::JobProducerExt::push_to) on the producer and
/// `#[process(queue = TranscodeQueue)]` on the consumer. The macro asserts the
/// process method's job argument is `Self::Job`, so a mismatch is a compile
/// error naming both types.
pub trait QueueName: 'static {
    /// The wire name apalis (or any backend) namespaces storage under — the
    /// exact string a legacy `#[process(queue = "…")]` literal would carry.
    const NAME: &'static str;

    /// The payload type pushed onto and drained off this queue. The producer's
    /// `push_to::<Self>` accepts exactly this type; the consumer's
    /// `#[process(queue = Self)]` method must receive exactly this type.
    type Job: Job;
}

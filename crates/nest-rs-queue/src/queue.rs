//! [`Queue`] — the compile-time identity of a queue.
//!
//! A queue's name would otherwise be a bare string repeated on both sides, one
//! literal on the consumer and another at the push, with nothing linking the
//! two nor the payload either side agrees on. [`Queue`] makes that identity a
//! **type** carrying both, declared once at the feature port with the
//! [`queue`](macro@crate::queue) attribute, so a typo or a mismatched payload is a
//! compile error rather than a job that never drains.
//!
//! **One queue per declaration.** A queue per runtime key — a prefix every key
//! extends into a queue of its own — is not offered: the attribute refuses
//! `prefix`, naming why, and a key that varies at runtime rides in the job.

use crate::Job;

/// The type-level identity of a queue: its wire name and its payload,
/// implemented by the [`queue`](macro@crate::queue) attribute on a unit struct at
/// the feature port.
///
/// ```
/// use nest_rs_queue::{Queue, queue};
///
/// // Any `T: Serialize + DeserializeOwned + Clone + Send + Sync + Unpin` is a
/// // `Job`; a real feature uses its own `TranscodeCommand` payload struct.
/// #[queue(name = "transcode", job = String)]
/// struct TranscodeQueue;
///
/// assert_eq!(<TranscodeQueue as Queue>::NAME, "transcode");
/// ```
///
/// Both sides then name the type — `queue.push(TranscodeQueue, job, None)` on the
/// producer, `#[process(queue = TranscodeQueue)]` on the consumer — and the
/// decorator checks the consumer's job argument is `Self::Job`, so a mismatch is
/// a compile error naming both types.
pub trait Queue: 'static {
    /// The queue's wire name.
    const NAME: &'static str;
    /// The payload pushed onto and drained off this queue.
    type Job: Job;
}

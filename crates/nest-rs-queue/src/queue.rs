//! [`Queue`] — the compile-time identity of a queue.
//!
//! One queue per declaration: a key that varies at runtime rides in the job.

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

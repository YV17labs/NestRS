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
/// Both sides then name the type — `jobs.push(TranscodeQueue, job)` on the
/// producer, `#[process(queue = TranscodeQueue)]` on the consumer — and the
/// decorator checks the consumer's job argument is `Self::Job`, so a mismatch is
/// a compile error naming both types.
pub trait Queue: 'static {
    /// The queue's wire name.
    const NAME: &'static str;
    /// The payload pushed onto and drained off this queue.
    type Job: Job;
}

/// What `#[process]` bounds a queue's `Job` by: `T` itself, and nothing else.
///
/// An identity, never `Queue<Job = T>`: a projection's mismatch is `E0271`,
/// which blames the queue's declaration as the type "expected"; a missing impl
/// is `E0277`, which says this sentence naming both types.
#[diagnostic::on_unimplemented(
    message = "`#[process]` takes `{T}`, and its queue's job is `{Self}`",
    label = "this queue's job is `{Self}`",
    note = "a `#[process]` method's argument is the job of the queue it names: take `{Self}`, or \
            name the queue whose job is `{T}`"
)]
pub trait IsJob<T> {}

impl<T> IsJob<T> for T {}

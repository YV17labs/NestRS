//! The `#[processor]` decorator, re-exported by `nest-rs-queue` (the
//! backend-agnostic abstractions crate) so the call site keeps writing
//! `use nest_rs_queue::processor;` regardless of which backend integration
//! (nest-rs-redis, …) is wired in.
#![warn(missing_docs)]

use proc_macro::TokenStream;

mod processor;
mod queue;

/// Orchestrator on an `#[injectable]` provider's `impl` block. Each method
/// tagged with `#[process(queue = "...", concurrency, retries)]` becomes a
/// queue consumer the `QueueWorker` spawns at boot.
///
/// A single provider may carry several `#[process]` methods (different
/// queues, different concurrencies) sharing the same `#[inject]`
/// dependencies — pooling related queue handlers on one service keeps
/// shared state (clients, repositories) in one place.
///
/// The `queue` is named either by a raw string literal (legacy form) or by a
/// `QueueName` **type** — the preferred form,
/// declared with [`queue`](macro@crate::queue) at the feature port:
/// `#[process(queue = AudioQueue)]`. The type form reads
/// `<AudioQueue as QueueName>::NAME` into the inventory entry **and** asserts,
/// at compile time, that this method's job argument is
/// `<AudioQueue as QueueName>::Job` — a mismatch is a build error naming both
/// types, not a job that silently never drains.
///
/// Per-method attributes (exactly one `#[process]` per method):
///
/// - `#[process(queue = "audio")]` — minimal, defaults `concurrency = 1`,
///   `retries = 0`.
/// - `#[process(queue = "audio", concurrency = 5)]` — bound the in-flight jobs
///   per worker.
/// - `#[process(queue = "audio", concurrency = 5, retries = 3)]` — apalis
///   retries before the job lands on the queue's failed list.
///
/// The method signature is `async fn(&self, job: T) -> anyhow::Result<()>`,
/// where `T: Job`. The macro extracts the job type from the second
/// parameter, generates a typed handler, and submits a per-method
/// inventory entry the worker drains.
///
/// # Expands to
///
/// The impl unchanged, plus per `#[process]` method: a hidden type-erased
/// handler `fn` (unwraps the wire envelope, deserializes the job, resolves the
/// provider, dispatches inside the `JobContext`) and a `ProcessMethod`
/// submitted to the link-time inventory. No `Discoverable` — the host's own
/// `#[injectable]` owns it.
///
/// ```text
/// impl AudioProcessor { /* unchanged */ }
/// fn __nestrs_process_handler_audio_processor_transcode(payload, container) -> Pin<Box<dyn Future<…>>> { /* … */ }
/// ::nest_rs_core::inventory::submit! {
///     ::nest_rs_queue::ProcessMethod {
///         name: "AudioProcessor::transcode", queue: "audio",
///         concurrency: 5, retries: 3,
///         provider_type_id: || TypeId::of::<AudioProcessor>(),
///         handler: __nestrs_process_handler_audio_processor_transcode,
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn processor(args: TokenStream, input: TokenStream) -> TokenStream {
    processor::processor(args, input)
}

/// Stamp a unit struct with a compile-time queue identity — its wire name and
/// the `Job` payload it carries — by implementing
/// `QueueName`. Lives beside the payload at the feature port; both the producer
/// (`push_to::<Q>`) and the consumer (`#[process(queue = Q)]`) name the type,
/// so a typo'd name or a mismatched payload is a compile error, not a job that
/// silently never drains.
///
/// ```ignore
/// #[queue(name = "audio", job = TranscodeCommand)]
/// pub struct AudioQueue;
/// ```
///
/// # Expands to
///
/// ```ignore
/// pub struct AudioQueue;
/// impl ::nest_rs_queue::QueueName for AudioQueue {
///     const NAME: &'static str = "audio";
///     type Job = TranscodeCommand;
/// }
/// ```
#[proc_macro_attribute]
pub fn queue(args: TokenStream, input: TokenStream) -> TokenStream {
    queue::queue(args, input)
}

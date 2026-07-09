//! The `#[processor]` decorator, re-exported by `nest-rs-queue` (the
//! backend-agnostic abstractions crate) so the call site keeps writing
//! `use nest_rs_queue::processor;` regardless of which backend integration
//! (nest-rs-redis, …) is wired in.
use proc_macro::TokenStream;

mod processor;

/// Orchestrator on an `#[injectable]` provider's `impl` block. Each method
/// tagged with `#[process(queue = "...", concurrency, retries)]` becomes a
/// queue consumer the `QueueWorker` spawns at boot.
///
/// A single provider may carry several `#[process]` methods (different
/// queues, different concurrencies) sharing the same `#[inject]`
/// dependencies — pooling related queue handlers on one service keeps
/// shared state (clients, repositories) in one place.
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
/// ```ignore
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

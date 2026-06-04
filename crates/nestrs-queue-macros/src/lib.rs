//! The `#[processor]` decorator, re-exported by `nestrs-queue`.

use proc_macro::TokenStream;

mod processor;

/// Orchestrator on an `#[injectable]` provider's `impl` block. Each method
/// tagged with `#[process(queue = "...", concurrency, retries)]` becomes a
/// queue consumer the `QueueWorker` spawns at boot.
///
/// A single provider may carry several `#[process]` methods (different
/// queues, different concurrencies) sharing the same `#[inject]`
/// dependencies — the NestJS-style pattern of pooling related queue handlers
/// on one service.
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
#[proc_macro_attribute]
pub fn processor(args: TokenStream, input: TokenStream) -> TokenStream {
    processor::processor(args, input)
}

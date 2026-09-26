//! The `#[processor]` and `#[queue]` decorators, re-exported by `nest-rs-queue`
//! and reached through the umbrella as `nest_rs::queue::{processor, queue}`.
#![warn(missing_docs)]

use proc_macro::TokenStream;

mod processor;
mod queue;

/// Orchestrator on an `#[injectable]` provider's `impl` block. Each method
/// tagged with `#[process(queue = <Queue>, …)]` becomes a queue consumer that a
/// backend's worker transport drains at boot.
///
/// A single provider may carry several `#[process]` methods (different queues)
/// sharing the same `#[inject]` dependencies — pooling related queue handlers
/// on one service keeps shared state (clients, repositories) in one place.
///
/// The `queue` is named by its `Queue` **type**, declared with
/// [`queue`](macro@crate::queue) at the feature port. The macro reads the
/// queue's name and kind into the inventory entry **and** asserts, at compile
/// time, that this method's job argument is the queue's `Job` — a mismatch is a
/// build error naming both types, not a job that silently never drains.
///
/// Keys, on exactly one `#[process]` per method:
///
/// - `queue = AudioQueue` — required.
/// - `retries = 3` — how many times a failed attempt is run again before the job
///   dead-letters; default `0`. Each retry waits first: one second after the
///   first failure, doubling after each, at most five minutes, jittered by up to
///   a fifth either way — the port's backoff, the same on every backend.
/// - `concurrency = 4` — how many attempts of this method one worker replica
///   runs at once; default `1`, one at a time. **Per method, per replica**:
///   another method's jobs never wait on this one's permits, and a method
///   draining a dynamic queue has one pool of permits across all its instances,
///   not one per key. It is the vertical bound; the horizontal one is the number
///   of replicas the platform runs. Not a capability — every backend honours it.
/// - `throttle(limit = 10, window = "1m")` — at most `limit` attempts of this
///   method **start** per window, **across the deployment**: every replica
///   draining the queue counts against the one limit, and a retry is an attempt
///   like any other. Needs a backend declaring the throttle capability.
/// - `transactional = false` — run on the pool rather than one transaction per
///   attempt; required beside a `Checkpoint<_>` parameter.
///
/// The method is `async fn(&self, job: T) -> Result<(), E>` with `T: Job` and
/// any `E` converting into `Box<dyn Error + Send + Sync>` — `anyhow::Result<()>`
/// is the usual spelling — and may take a `Checkpoint<S>` parameter beside the
/// job: the job's progress, kept across its retries and redeliveries and
/// cleared when it ends. A key the app's queue backend cannot honour fails the
/// worker's boot, naming the method.
///
/// # Expands to
///
/// The impl unchanged, plus per `#[process]` method: a hidden type-erased
/// handler `fn` (deserializes the payload, resolves the provider, dispatches
/// inside the `JobContext`) and a `ProcessMethod` submitted to the link-time
/// inventory. No `Discoverable` — the host's own `#[injectable]` owns it.
///
/// ```text
/// impl AudioProcessor { /* unchanged */ }
/// fn __nestrs_process_handler_audio_processor_transcode(payload, context) -> Pin<Box<dyn Future<…>>> { /* … */ }
/// ::nest_rs_core::inventory::submit! {
///     ::nest_rs_queue::ProcessMethod::new(
///         module_path!(), "AudioProcessor::transcode",
///         <AudioQueue as Queue>::NAME, <AudioQueue as Queue>::KIND,
///         ProcessOptions::DEFAULT.with_retries(3),
///         || TypeId::of::<AudioProcessor>(),
///         __nestrs_process_handler_audio_processor_transcode,
///     )
/// }
/// ```
#[proc_macro_attribute]
pub fn processor(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(processor::processor(args, input).into()).into()
}

/// Stamp a unit struct with a queue's compile-time identity — its wire name or
/// prefix and the `Job` payload it carries — by implementing `Queue`. Lives
/// beside the payload at the feature port; the producer (`queue.push(Q, job,
/// None)`) and the consumer (`#[process(queue = Q)]`) both name the type, so a
/// typo'd name or a mismatched payload is a compile error.
///
/// Two shapes:
///
/// ```ignore
/// // One queue. The marker is the destination a push names.
/// #[queue(name = "audio", job = TranscodeCommand)]
/// pub struct AudioQueue;
///
/// // One queue per runtime key: `TenantQueue::instance(&slug)?` is the destination.
/// #[queue(prefix = "tenant", job = SyncCommand)]
/// pub struct TenantQueue;
/// ```
///
/// A name and a prefix follow the rule `QueueName` states, checked here at
/// compile time.
///
/// # Expands to
///
/// ```ignore
/// pub struct AudioQueue;
/// impl ::nest_rs_queue::Queue for AudioQueue {
///     const NAME: &'static str = "audio";
///     const KIND: QueueKind = QueueKind::Static;
///     type Job = TranscodeCommand;
/// }
/// impl ::nest_rs_queue::Destination for AudioQueue { /* the queue's name */ }
/// ```
///
/// A `prefix` queue implements `Queue` and `DynamicQueue` instead of
/// `Destination`, and gains an inherent `instance(key)` constructor.
#[proc_macro_attribute]
pub fn queue(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(queue::queue(args, input).into()).into()
}

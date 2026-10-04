//! The `#[processor]` and `#[queue]` decorators, re-exported by `nest-rs-queue`
//! and reached through the umbrella as `nest_rs::queue::{processor, queue}`.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod processor;
mod queue;

/// A single provider may carry several `#[process]` methods (different queues)
/// sharing the same `#[inject]` dependencies — pooling related queue handlers
/// on one service keeps shared state (clients, repositories) in one place.
///
/// The `queue` is named by its `Queue` **type**, declared with
/// [`queue`](macro@crate::queue) at the feature port. The macro reads the
/// queue's name into the inventory entry **and** asserts, at compile
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
///   another method's jobs never wait on this one's permits. It is the vertical
///   bound; the horizontal one is the number of replicas the platform runs. Not
///   a capability — every backend honours it.
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
/// The impl is re-emitted unchanged, with no `Discoverable` — the host's own
/// `#[injectable]` owns it.
#[proc_macro_attribute]
pub fn processor(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(processor::processor(args, input).into()).into()
}

/// The name follows the rule `QueueName` states, checked here at compile time.
/// A queue per runtime key (`prefix = ..`) is refused, naming why: the Redis
/// backend drains every queue from a list of its own that each replica polls, so
/// a key that varies at runtime rides in the job instead.
///
/// The struct is re-emitted unchanged and gains `impl Queue` and
/// `impl Destination`.
#[proc_macro_attribute]
pub fn queue(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(queue::queue(args, input).into()).into()
}

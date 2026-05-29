//! The `#[processor]` decorator, re-exported by `nestrs-queue`. The generated
//! code uses absolute paths through the framework (`::nestrs_queue::*`,
//! `::nestrs_core::*`, `::std::*`) and never names `apalis` — so an app that
//! decorates a processor needs only `nestrs-queue`, not a direct apalis
//! dependency. Token-building helpers are shared with the other decorators via
//! `nestrs-codegen`. The implementation lives in `processor`; this is the
//! language-required proc-macro entry.

use proc_macro::TokenStream;

mod processor;

/// Mark a struct as the consumer for a named job queue.
///
/// `#[processor(queue = "welcome-email", concurrency = 5, retries = 3)]` on a
/// struct that implements [`Processor`](../nestrs_queue/trait.Processor.html).
/// Construction mirrors `#[injectable]` — fields tagged `#[inject]` are resolved
/// from the container, others default — and the macro additionally emits
/// `impl Discoverable` attaching a `ProcessorMeta`: the queue name, the worker
/// concurrency, the retry budget, and a monomorphic `register_worker::<Self>`
/// thunk. The `QueueWorker` transport discovers those metas at boot and runs an
/// apalis worker per processor against the shared Redis connection.
///
/// `queue` is required; `concurrency` defaults to `1`, `retries` to `0`.
#[proc_macro_attribute]
pub fn processor(args: TokenStream, input: TokenStream) -> TokenStream {
    processor::processor(args, input)
}

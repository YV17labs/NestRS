//! The `#[processor]` decorator, re-exported by `nestrs-queue`.

use proc_macro::TokenStream;

mod processor;

/// Mark a struct as a queue consumer.
///
/// `#[processor(queue = "welcome-email", concurrency = 5, retries = 3)]` on a
/// struct implementing [`Processor`](../nestrs_queue/trait.Processor.html).
/// `#[inject]` fields resolve from the container; others default. Emits
/// `impl Discoverable` attaching a `ProcessorMeta` the `QueueWorker` transport
/// reads at boot.
///
/// `queue` is required; `concurrency` defaults to `1`, `retries` to `0`.
#[proc_macro_attribute]
pub fn processor(args: TokenStream, input: TokenStream) -> TokenStream {
    processor::processor(args, input)
}

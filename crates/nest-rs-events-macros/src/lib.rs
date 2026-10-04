//! The `#[listeners]` decorator macro, re-exported by `nest-rs-events`.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod listeners;

/// Walks the methods; for each one
/// tagged with `#[on_event]`, subscribes a closure to the
/// [`EventBus`](../nest_rs_events/struct.EventBus.html) at bootstrap and
/// submits a `ListenerMethod` to the link-time inventory the
/// [`EventsModule`](../nest_rs_events/struct.EventsModule.html) drains.
///
/// The struct itself must be a regular `#[injectable]`. Multiple `#[on_event]`
/// methods on the same impl block share the provider's `#[inject]`
/// dependencies — the pattern the framework is built for.
///
/// Provided for **symmetry with the other orchestrators** (`#[processor]`,
/// `#[scheduled]`, `#[hooks]`): one `Discoverable` per host, methods submitted
/// to inventory. The events family carries its weight even before a product
/// feature adopts it, so the orchestrator pattern stays uniform across every
/// transport and concern.
///
/// Per-method requirements (one `#[on_event]` per method):
///
/// - `async fn(&self, event: T)` — the event type `T` is read from the second
///   parameter; the bus enforces `T: Clone + Send + 'static`.
/// - Returns `()` — events are fire-and-forget, handle errors inside.
///
/// `#[on_event]` is a pure marker consumed by `#[listeners]` — using it
/// outside a `#[listeners]` impl block fails the same way `#[get]` outside
/// `#[routes]` does.
///
/// The impl is re-emitted unchanged, with no `Discoverable` — the host's own
/// `#[injectable]` owns it.
#[proc_macro_attribute]
pub fn listeners(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(listeners::listeners(args, input).into()).into()
}

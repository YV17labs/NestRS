//! The `#[listeners]` decorator macro, re-exported by `nest-rs-events`.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod listeners;

/// Walks the methods; for each one
/// tagged with `#[on_event]`, submits a `ListenerMethod` to the link-time
/// inventory the [`EventsModule`](../nest_rs_events/struct.EventsModule.html)
/// drains at the boot's wiring step, before the first lifecycle hook, where it
/// subscribes a closure to the
/// [`EventBus`](../nest_rs_events/struct.EventBus.html).
///
/// The struct itself must be a regular `#[injectable]`; its `#[on_event]`
/// methods share the provider's `#[inject]` dependencies.
///
/// Per-method requirements (one `#[on_event]` per method):
///
/// - `async fn(&self, event: T)` — the event type `T` is read from the second
///   parameter; the bus enforces `T: Clone + Send + 'static`.
/// - Returns `()` — events are fire-and-forget, handle errors inside.
///
/// `#[on_event]` is a marker consumed by `#[listeners]`; outside one it does
/// not resolve.
#[proc_macro_attribute]
pub fn listeners(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(listeners::listeners(args, input).into()).into()
}

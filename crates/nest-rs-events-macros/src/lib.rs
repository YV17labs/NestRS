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
/// methods share the provider's `#[inject]` dependencies. A `#[meta(EXPR)]` on
/// the impl block is declared for every listener, one on a method for it
/// alone, read over the block's of the same type.
///
/// Per-method requirements (one `#[on_event]` per method):
///
/// - `async fn(&self, event: T)` — the event type `T` is read from the second
///   parameter; the bus enforces `T: Clone + Send + 'static`.
/// - Returns `()` or `Result<(), E>` with any `E` converting into
///   `Box<dyn Error + Send + Sync>` — `anyhow::Result<()>` is the usual
///   spelling, read by its type whatever it is called. Each call is a unit of
///   work of its own: an `Err` rolls its transaction back and is reported at
///   `error` on `nest_rs::events`, and the next listener runs either way.
///
/// Keys, both optional:
///
/// - `transactional = false` — run on the pool rather than one transaction
///   per call.
/// - `timeout = "30s"` — how long a call runs before it is cut and its unit
///   rolled back; default ten minutes (`nest_rs::core::JOB_TIMEOUT`), at most
///   `"24h"`.
///
/// `#[on_event]` is a marker consumed by `#[listeners]`; outside one it does
/// not resolve.
#[proc_macro_attribute]
pub fn listeners(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(listeners::listeners(args, input).into()).into()
}

//! The `#[listeners]` decorator macro, re-exported by `nestrs-events`.

use proc_macro::TokenStream;

mod listeners;

/// Orchestrator on a provider's `impl` block. Walks the methods; for each one
/// tagged with `#[on_event]`, subscribes a closure to the
/// [`EventBus`](../nest_rs_events/struct.EventBus.html) at bootstrap and
/// submits a `ListenerMethod` to the link-time inventory the
/// [`EventModule`](../nest_rs_events/struct.EventModule.html) drains.
///
/// The struct itself must be a regular `#[injectable]`. Multiple `#[on_event]`
/// methods on the same impl block share the provider's `#[inject]`
/// dependencies — the pattern the framework is built for.
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
/// ```ignore
/// #[injectable]
/// pub struct PointsHandlers {
///     #[inject] svc: std::sync::Arc<Ledger>,
/// }
///
/// #[listeners]
/// impl PointsHandlers {
///     #[on_event]
///     async fn on_awarded(&self, e: PointsAwarded) {
///         self.svc.credit(e.user_id, e.amount).await;
///     }
///
///     #[on_event]
///     async fn on_redeemed(&self, e: PointsRedeemed) {
///         self.svc.debit(e.user_id, e.amount).await;
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn listeners(args: TokenStream, input: TokenStream) -> TokenStream {
    listeners::listeners(args, input)
}

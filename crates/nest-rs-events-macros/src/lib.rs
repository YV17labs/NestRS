//! The `#[listeners]` decorator macro, re-exported by `nestrs-events`.

use proc_macro::TokenStream;

mod listeners;

/// Orchestrator on a provider's `impl` block. Walks the methods; for each one
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
///
/// # Expands to
///
/// The impl unchanged, plus per `#[on_event]` method: a hidden `wire` fn that
/// resolves the provider and subscribes a closure to the `EventBus`, and a
/// `ListenerMethod` submitted to the link-time inventory the `EventsModule`
/// drains at bootstrap. No `Discoverable` — the host's own `#[injectable]`
/// owns it.
///
/// ```ignore
/// impl PointsHandlers { /* unchanged */ }
/// fn __nestrs_listener_wire_points_handlers_on_awarded(container, bus) { /* subscribe::<PointsAwarded> */ }
/// ::nest_rs_core::inventory::submit! {
///     ::nest_rs_events::ListenerMethod {
///         name: "PointsHandlers::on_awarded",
///         provider_type_id: || TypeId::of::<PointsHandlers>(),
///         wire: __nestrs_listener_wire_points_handlers_on_awarded,
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn listeners(args: TokenStream, input: TokenStream) -> TokenStream {
    listeners::listeners(args, input)
}

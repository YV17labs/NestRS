//! The `#[event_handler]` decorator, re-exported by `nestrs-events`. The generated
//! code uses absolute paths (`::nestrs_events::*`, `::nestrs_core::*`, `::std::*`),
//! so this crate does not depend on them — they resolve at the call site.
//! Token-building helpers are shared with the other decorators via `nestrs-codegen`.

use proc_macro::TokenStream;

mod event_handler;

/// Mark a struct as an event handler, discovered like a controller or cron job.
///
/// Construction mirrors `#[injectable]` — fields tagged `#[inject]` are resolved
/// from the container, others default, and the macro emits `from_container`. It
/// additionally emits `impl Discoverable` attaching an `EventHandlerMeta` whose
/// thunk builds the handler from the (fully-assembled) container and subscribes it
/// to the [`EventBus`](../nestrs_events/struct.EventBus.html). The struct must
/// implement [`EventHandler`](../nestrs_events/trait.EventHandler.html), which
/// declares the `Event` type it handles.
///
/// ```ignore
/// #[event_handler]
/// pub struct SendWelcomeEmail {
///     #[inject] mailer: std::sync::Arc<Mailer>,
/// }
///
/// #[nestrs_events::async_trait]
/// impl nestrs_events::EventHandler for SendWelcomeEmail {
///     type Event = UserRegistered;
///     async fn handle(&self, event: UserRegistered) {
///         self.mailer.welcome(event.email).await;
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn event_handler(args: TokenStream, input: TokenStream) -> TokenStream {
    event_handler::event_handler(args, input)
}

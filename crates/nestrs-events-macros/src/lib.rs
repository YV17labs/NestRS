//! The `#[on_event]` decorator, re-exported by `nestrs-events`.

use proc_macro::TokenStream;

mod on_event;

/// Mark a struct as an event handler, discovered like a controller or cron job.
///
/// `#[inject]` fields resolve from the container; the struct must implement
/// [`EventHandler`](../nestrs_events/trait.EventHandler.html).
///
/// ```ignore
/// #[on_event]
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
pub fn on_event(args: TokenStream, input: TokenStream) -> TokenStream {
    on_event::on_event(args, input)
}

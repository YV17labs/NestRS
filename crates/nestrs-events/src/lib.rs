//! Typed in-process event bus for nestrs — the `@nestjs/event-emitter` analog.
//!
//! An event is any `Clone + Send + 'static` type. A handler is a struct:
//! `#[event_handler]` builds it from the container (its `#[inject]` fields) and
//! emits the single `impl Discoverable` attaching an [`EventHandlerMeta`] — so a
//! handler is wired by listing it in `#[module(providers = [...])]`, exactly like
//! a controller or cron job. Import [`EventModule`] to register the [`EventBus`]
//! and wire every discovered handler at application bootstrap (from the
//! fully-assembled container, so a handler may inject any provider regardless of
//! import order). A producer injects `Arc<EventBus>` and calls
//! [`emit`](EventBus::emit):
//!
//! ```ignore
//! #[derive(Clone)]
//! pub struct UserRegistered { pub email: String }
//!
//! #[event_handler]
//! pub struct SendWelcomeEmail {
//!     #[inject] mailer: std::sync::Arc<Mailer>,
//! }
//!
//! #[nestrs_events::async_trait]
//! impl nestrs_events::EventHandler for SendWelcomeEmail {
//!     type Event = UserRegistered;
//!     async fn handle(&self, event: UserRegistered) {
//!         self.mailer.welcome(&event.email).await;
//!     }
//! }
//!
//! // a producer:
//! self.events.emit(UserRegistered { email }).await;
//!
//! // wiring: #[module(imports = [EventModule], providers = [SendWelcomeEmail, ...])]
//! ```
//!
//! Dispatch is in-process and awaited: [`emit`](EventBus::emit) runs every handler
//! registered for the event type, in registration order, each with its own clone.

mod bus;
mod handler;
mod meta;
mod module;

pub use bus::EventBus;
pub use handler::EventHandler;
pub use meta::EventHandlerMeta;
pub use module::EventModule;

pub use nestrs_events_macros::event_handler;

// Re-exported so an `#[event_handler]` struct can write
// `#[nestrs_events::async_trait]` on its `EventHandler` impl without a direct
// `async_trait` dependency — and so the handler future is boxed `Send`, which the
// bus requires.
pub use async_trait::async_trait;

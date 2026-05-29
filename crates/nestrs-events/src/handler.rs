//! The event-handler trait.

use async_trait::async_trait;

/// Handles events of type [`Event`](EventHandler::Event). Implemented on an
/// `#[event_handler]` struct; the [`EventBus`](crate::EventBus) builds the struct
/// from the container at bootstrap and calls [`handle`](EventHandler::handle) for
/// every matching [`emit`](crate::EventBus::emit).
#[async_trait]
pub trait EventHandler: Send + Sync + 'static {
    /// The event type this handler reacts to.
    type Event: Clone + Send + 'static;

    async fn handle(&self, event: Self::Event);
}

//! [`EventBus`] — the typed publish/subscribe registry.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use parking_lot::RwLock;

/// An event handed to the bus, type-erased so handlers for different event types
/// share one registry. Downcast back to the concrete event by its subscription.
type BoxedEvent = Box<dyn Any + Send>;

/// A subscribed handler, erased over its event type: it downcasts the boxed event
/// and returns the (boxed, `Send`) future of the handler call.
type HandlerFn = Arc<dyn Fn(BoxedEvent) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// A typed, in-process publish/subscribe bus. Register it by importing
/// [`EventModule`](crate::EventModule); inject `Arc<EventBus>` into a provider to
/// [`emit`](EventBus::emit). Every `#[on_event]` for the emitted event type
/// runs. Handlers are filled in once at application bootstrap and the registry is
/// read-only thereafter, so the `RwLock` is uncontended on the hot (`emit`) path.
#[derive(Default)]
pub struct EventBus {
    handlers: RwLock<HashMap<TypeId, Vec<HandlerFn>>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a handler for events of type `E`. Called by [`EventModule`]'s
    /// bootstrap wiring for each discovered `#[on_event]`; apps do not call
    /// it directly.
    pub fn subscribe<E, H, Fut>(&self, handler: H)
    where
        E: Any + Send + 'static,
        H: Fn(E) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let erased: HandlerFn = Arc::new(move |boxed: BoxedEvent| {
            let event = *boxed
                .downcast::<E>()
                .expect("event downcasts to the type its handler subscribed for");
            Box::pin(handler(event)) as Pin<Box<dyn Future<Output = ()> + Send>>
        });
        self.handlers
            .write()
            .entry(TypeId::of::<E>())
            .or_default()
            .push(erased);
    }

    /// Emit an event: every handler registered for `E` runs, in registration
    /// order, each with its own clone, awaited in turn. A no-op when no handler is
    /// registered for `E`.
    pub async fn emit<E: Clone + Send + 'static>(&self, event: E) {
        // Clone out the handler list so the lock is released before awaiting.
        let handlers = self.handlers.read().get(&TypeId::of::<E>()).cloned();
        let Some(handlers) = handlers else { return };
        for handler in handlers {
            handler(Box::new(event.clone())).await;
        }
    }
}

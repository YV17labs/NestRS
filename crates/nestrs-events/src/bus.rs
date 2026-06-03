use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use parking_lot::RwLock;

type BoxedEvent = Box<dyn Any + Send>;
type HandlerFn = Arc<dyn Fn(BoxedEvent) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Handlers are filled in once at application bootstrap and the registry is
/// read-only thereafter, so the `RwLock` is uncontended on the emit path.
#[derive(Default)]
pub struct EventBus {
    handlers: RwLock<HashMap<TypeId, Vec<HandlerFn>>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// Called by `EventModule` at bootstrap; apps don't call it directly.
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

    /// Runs each handler in registration order, awaited in turn. No-op when
    /// nothing is registered for `E`.
    pub async fn emit<E: Clone + Send + 'static>(&self, event: E) {
        // Clone out the list so the lock is released before awaiting.
        let handlers = self.handlers.read().get(&TypeId::of::<E>()).cloned();
        let Some(handlers) = handlers else { return };
        for handler in handlers {
            handler(Box::new(event.clone())).await;
        }
    }
}

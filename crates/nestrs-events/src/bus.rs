use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use parking_lot::RwLock;

type BoxedEvent = Box<dyn Any + Send>;
type ListenerFn = Arc<dyn Fn(BoxedEvent) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Listeners are filled in once at application bootstrap and the registry is
/// read-only thereafter, so the `RwLock` is uncontended on the emit path.
#[derive(Default)]
pub struct EventBus {
    listeners: RwLock<HashMap<TypeId, Vec<ListenerFn>>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// Called by `EventModule` at bootstrap; apps don't call it directly.
    pub fn subscribe<E, H, Fut>(&self, listener: H)
    where
        E: Any + Send + 'static,
        H: Fn(E) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let erased: ListenerFn = Arc::new(move |boxed: BoxedEvent| {
            let event = *boxed
                .downcast::<E>()
                .expect("event downcasts to the type its listener subscribed for");
            Box::pin(listener(event)) as Pin<Box<dyn Future<Output = ()> + Send>>
        });
        self.listeners
            .write()
            .entry(TypeId::of::<E>())
            .or_default()
            .push(erased);
    }

    /// Runs each listener in registration order, awaited in turn. No-op when
    /// nothing is registered for `E`.
    pub async fn emit<E: Clone + Send + 'static>(&self, event: E) {
        // Clone out the list so the lock is released before awaiting.
        let listeners = self.listeners.read().get(&TypeId::of::<E>()).cloned();
        let Some(listeners) = listeners else { return };
        for listener in listeners {
            listener(Box::new(event.clone())).await;
        }
    }
}

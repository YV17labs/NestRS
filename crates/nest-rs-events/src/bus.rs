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

    /// Called by `EventsModule` at bootstrap; apps don't call it directly.
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[derive(Clone)]
    struct OrderPlaced {
        id: u32,
    }

    #[derive(Clone)]
    struct OrderShipped;

    // The bus must be a no-op for events that have no listener — apps emit
    // optimistically, and an unsubscribed event must not panic or alloc.
    #[tokio::test]
    async fn emit_is_a_noop_for_an_unsubscribed_event() {
        let bus = EventBus::new();
        bus.emit(OrderPlaced { id: 1 }).await;
    }

    #[tokio::test]
    async fn a_subscribed_listener_runs_with_the_emitted_event() {
        let bus = EventBus::new();
        let seen = Arc::new(AtomicUsize::new(0));
        let seen2 = seen.clone();
        bus.subscribe(move |evt: OrderPlaced| {
            let seen = seen2.clone();
            async move {
                seen.fetch_add(evt.id as usize, Ordering::SeqCst);
            }
        });

        bus.emit(OrderPlaced { id: 7 }).await;
        assert_eq!(seen.load(Ordering::SeqCst), 7);
    }

    // Listeners run in registration order — apps depend on this for setup-
    // teardown patterns (open a span before, close after).
    #[tokio::test]
    async fn listeners_run_in_registration_order_for_the_same_event() {
        let bus = EventBus::new();
        let order = Arc::new(parking_lot::Mutex::new(Vec::<u32>::new()));

        let o1 = order.clone();
        bus.subscribe(move |_: OrderPlaced| {
            let o = o1.clone();
            async move {
                o.lock().push(1);
            }
        });
        let o2 = order.clone();
        bus.subscribe(move |_: OrderPlaced| {
            let o = o2.clone();
            async move {
                o.lock().push(2);
            }
        });
        let o3 = order.clone();
        bus.subscribe(move |_: OrderPlaced| {
            let o = o3.clone();
            async move {
                o.lock().push(3);
            }
        });

        bus.emit(OrderPlaced { id: 0 }).await;
        assert_eq!(*order.lock(), vec![1, 2, 3]);
    }

    // Two events keyed on distinct types must not cross-fire. The TypeId-keyed
    // map is the routing primitive — a bug that collapsed types would let an
    // OrderShipped listener fire on OrderPlaced.
    #[tokio::test]
    async fn listeners_for_distinct_event_types_do_not_cross_fire() {
        let bus = EventBus::new();
        let placed = Arc::new(AtomicUsize::new(0));
        let shipped = Arc::new(AtomicUsize::new(0));

        let p = placed.clone();
        bus.subscribe(move |_: OrderPlaced| {
            let p = p.clone();
            async move {
                p.fetch_add(1, Ordering::SeqCst);
            }
        });
        let s = shipped.clone();
        bus.subscribe(move |_: OrderShipped| {
            let s = s.clone();
            async move {
                s.fetch_add(1, Ordering::SeqCst);
            }
        });

        bus.emit(OrderPlaced { id: 1 }).await;
        assert_eq!(placed.load(Ordering::SeqCst), 1);
        assert_eq!(shipped.load(Ordering::SeqCst), 0);

        bus.emit(OrderShipped).await;
        assert_eq!(placed.load(Ordering::SeqCst), 1);
        assert_eq!(shipped.load(Ordering::SeqCst), 1);
    }

    // The event is cloned for each listener — verifies the documented
    // "registration order, awaited in turn" runs with a fresh copy per
    // listener. A future "move" optimization that fed only the last listener
    // would fail this test.
    #[tokio::test]
    async fn the_event_is_handed_to_each_listener_independently() {
        let bus = EventBus::new();
        let counter = Arc::new(AtomicUsize::new(0));

        for _ in 0..3 {
            let c = counter.clone();
            bus.subscribe(move |evt: OrderPlaced| {
                let c = c.clone();
                async move {
                    c.fetch_add(evt.id as usize, Ordering::SeqCst);
                }
            });
        }

        bus.emit(OrderPlaced { id: 4 }).await;
        // 3 listeners × event id 4 = 12.
        assert_eq!(counter.load(Ordering::SeqCst), 12);
    }
}

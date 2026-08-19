use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;

use futures_util::FutureExt;
use nest_rs_core::panic_message;
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
    /// An empty bus with no listeners registered yet.
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
    ///
    /// **Contract — in-process, sequential, failure is local.** The emitter
    /// awaits every listener, so a *slow* listener delays the ones after it and
    /// the emitter itself: the bus is for lightweight same-process reactions,
    /// and work that must not block its emitter belongs on the queue (which
    /// buys isolation and retries).
    ///
    /// A **panicking** listener is contained to that listener. It is caught,
    /// logged at `error` on `nest_rs::events`, and the chain continues — the
    /// containment the events page promises. Without it a single `unwrap()` in
    /// a fire-and-forget reaction (`email_the_author`, `index_for_search`)
    /// unwound through `emit` into the emitter: every listener after it was
    /// skipped, the emitter's own post-emit work never ran, and on HTTP the
    /// response was destroyed outright — the client saw a dropped connection
    /// rather than a 500, with the emitter's side effects already committed, so
    /// a retry re-ran them. The process survived, which made it containment at
    /// the wrong granularity: the *process*, not the listener.
    pub async fn emit<E: Clone + Send + 'static>(&self, event: E) {
        // Clone out the list so the lock is released before awaiting.
        let listeners = self.listeners.read().get(&TypeId::of::<E>()).cloned();
        let Some(listeners) = listeners else { return };
        for listener in listeners {
            let outcome = AssertUnwindSafe(listener(Box::new(event.clone())))
                .catch_unwind()
                .await;
            if let Err(payload) = outcome {
                tracing::error!(
                    target: crate::TARGET,
                    event = std::any::type_name::<E>(),
                    panic = panic_message(payload.as_ref()),
                    "event listener panicked — dispatch continues with the next listener",
                );
            }
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

#[cfg(test)]
mod panic_containment {
    use std::sync::Arc;

    use nest_rs_testing::LogCapture;
    use parking_lot::Mutex;

    use super::*;

    #[derive(Clone)]
    struct NotifyRequested {
        id: &'static str,
    }

    /// The events page makes a containment promise — "**Failure is local** — a
    /// listener returns `()`; there is no `Result` to propagate, no retry, no
    /// dead-letter queue, no global rollback." A panic was not local: it
    /// abandoned the chain mid-way, unwound through `emit` into the emitter, and
    /// on HTTP destroyed the response — the client got a dropped connection, not
    /// a 500, with the emitter's side effects already committed.
    #[tokio::test]
    async fn a_panicking_listener_does_not_stop_the_ones_after_it() {
        let bus = EventBus::new();
        let ran = Arc::new(Mutex::new(Vec::<u32>::new()));

        let r1 = ran.clone();
        bus.subscribe(move |_: NotifyRequested| {
            let r = r1.clone();
            async move { r.lock().push(1) }
        });
        bus.subscribe(move |e: NotifyRequested| async move {
            if e.id == "boom" {
                panic!("listener panic for boom");
            }
        });
        let r3 = ran.clone();
        bus.subscribe(move |_: NotifyRequested| {
            let r = r3.clone();
            async move { r.lock().push(3) }
        });

        let logs = LogCapture::install();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        bus.emit(NotifyRequested { id: "boom" }).await;
        std::panic::set_hook(previous);

        assert_eq!(
            *ran.lock(),
            vec![1, 3],
            "the listener after the panicking one still runs",
        );

        // Contained, never swallowed: the panic is an `error` event naming the
        // event type and the panic message. On the field `panic`, which is the
        // name every transport that contains a panic uses — one query reaches a
        // contained panic whichever seam caught it.
        let event = logs.expect_one(
            "nest_rs::events",
            "event listener panicked — dispatch continues with the next listener",
        );
        assert_eq!(event.level, "error");
        assert_eq!(
            event.field("panic").as_deref(),
            Some("listener panic for boom"),
        );
    }

    /// …and `emit` returns normally, so the emitter's own post-emit work runs.
    /// This is what a request handler depends on: the response is built after
    /// `emit`, and the panic used to take it with it.
    #[tokio::test]
    async fn emit_returns_to_its_caller_after_a_listener_panics() {
        let bus = EventBus::new();
        bus.subscribe(move |_: NotifyRequested| async move {
            panic!("listener panic");
        });

        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        bus.emit(NotifyRequested { id: "boom" }).await;
        std::panic::set_hook(previous);

        // Reaching this line *is* the assertion — before the fix the unwind
        // carried straight past it into the caller.
        let emit_returned = true;
        assert!(emit_returned);
    }

    /// The happy path stays quiet — an `error` on every emit would be noise.
    #[tokio::test]
    async fn a_healthy_dispatch_logs_no_containment_event() {
        let bus = EventBus::new();
        bus.subscribe(move |_: NotifyRequested| async move {});
        let logs = LogCapture::install();
        bus.emit(NotifyRequested { id: "ok" }).await;
        assert!(logs.events().is_empty(), "{:#?}", logs.events());
    }
}

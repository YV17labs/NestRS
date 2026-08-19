use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;

use futures_util::FutureExt;
use nest_rs_core::panic_message;
use nest_rs_core::tracing::Instrument;
use parking_lot::RwLock;

type BoxedEvent = Box<dyn Any + Send>;
type ListenerFn = Arc<dyn Fn(BoxedEvent) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// What a listener registered without a declared name is filed as.
///
/// A value rather than the event's type name, which the line already carries as
/// `event` — one value in two fields says nothing the first did not.
const ANONYMOUS_LISTENER: &str = "<anonymous>";

/// One registered listener, carrying the name its unit of work is filed under.
///
/// The name is the qualified `Provider::method` the `#[listeners]` expansion
/// knows and the erased closure does not — without it the operation line could
/// only ever say *some listener for this event ran*, which is the anonymity the
/// line exists to remove.
#[derive(Clone)]
struct Listener {
    name: &'static str,
    run: ListenerFn,
}

/// Listeners are filled in once at application bootstrap and the registry is
/// read-only thereafter, so the `RwLock` is uncontended on the emit path.
#[derive(Default)]
pub struct EventBus {
    listeners: RwLock<HashMap<TypeId, Vec<Listener>>>,
}

impl EventBus {
    /// An empty bus with no listeners registered yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Subscribe a listener that files its unit of work under `name`.
    ///
    /// The seam `#[listeners]` emits, which is the only caller that knows the
    /// qualified `Provider::method`. Apps reach the bus through the decorator.
    #[doc(hidden)]
    pub fn subscribe_named<E, H, Fut>(&self, name: &'static str, listener: H)
    where
        E: Any + Send + 'static,
        H: Fn(E) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let run: ListenerFn = Arc::new(move |boxed: BoxedEvent| {
            let event = *boxed
                .downcast::<E>()
                .expect("event downcasts to the type its listener subscribed for");
            Box::pin(listener(event)) as Pin<Box<dyn Future<Output = ()> + Send>>
        });
        self.listeners
            .write()
            .entry(TypeId::of::<E>())
            .or_default()
            .push(Listener { name, run });
    }

    /// [`subscribe_named`](Self::subscribe_named) for a listener with no
    /// declared name — a hand-built bus in a test.
    ///
    /// The unit is filed under `<anonymous>` rather than under the event type:
    /// the line already carries `event`, so naming the listener after it put one
    /// value in two fields and called something a listener that is not one.
    #[doc(hidden)]
    pub fn subscribe<E, H, Fut>(&self, listener: H)
    where
        E: Any + Send + 'static,
        H: Fn(E) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.subscribe_named(ANONYMOUS_LISTENER, listener);
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
        let event_name = std::any::type_name::<E>();
        // One emit is one cause, so its listeners share one trace — decided
        // here rather than per listener, because minting inside the loop gave
        // each listener a trace of its own whenever nothing ambient carried
        // one, and two reactions to one fact are not two traces.
        let cause = nest_rs_core::Correlation::inherited();
        for Listener { name, run } in listeners {
            dispatch_one(&cause, event_name, name, run(Box::new(event.clone()))).await;
        }
    }
}

/// One listener invocation — the edge's unit of work.
///
/// A listener is a **child** of whatever emitted the event: the emitter is
/// mid-unit when it calls `emit`, so this continues that trace rather than
/// minting one, and an event fired outside any unit simply starts one.
///
/// The panic is contained here rather than in `emit` so the containment and the
/// line that reports it are the same statement — a listener that panicked still
/// files its unit, with `outcome = panic`.
async fn dispatch_one(
    cause: &nest_rs_core::Correlation,
    event: &'static str,
    listener: &'static str,
    fut: Pin<Box<dyn Future<Output = ()> + Send>>,
) {
    // A **child** of the emit, not a copy of it. `Correlation::inherited()`
    // returns the ambient correlation *unchanged*, so filing every listener
    // under it gave two listeners on one event one `span_id` between them, with
    // `parent_span_id` naming the emitter's parent rather than the emitter. A
    // span id names one unit of work; that is the whole property the ids buy.
    let correlation = cause.child();
    let span = nest_rs_core::operation_span!(
        target: crate::TARGET,
        // In-process and same-task: nothing crossed a wire to get here.
        kind: nest_rs_core::operation_log::kind::INTERNAL,
        crate::unit::DISPATCH,
        &correlation,
        event = event,
        listener = listener,
    );
    // `None` scope, correlation only: a listener is not a request and holds no
    // per-request cache, but it is work the framework carries, so
    // `current_trace_id()` must answer inside it. One value, used twice —
    // `scope` around the listener and `enter` around the reporting below —
    // which is the shape `RequestContinuation` documents, rather than
    // assembling the context a second time to re-enter it.
    let continuation = nest_rs_core::RequestContinuation::new(None, correlation);
    let started = std::time::Instant::now();
    let outcome = continuation
        .scope(AssertUnwindSafe(fut).catch_unwind())
        .instrument(span)
        .await;
    let settled = match &outcome {
        Ok(()) => nest_rs_core::operation_log::OK,
        Err(_) => nest_rs_core::operation_log::PANIC,
    };
    // Both lines are filed **inside** the correlation, because they sit after
    // the `.await` that unwound it: a line emitted out here carries no ids at
    // all, which `nest_rs_mcp::propagate` documents having shipped once and
    // fixed the same way. Reporting is the last thing this unit does, so it is
    // re-entered rather than kept open.
    continuation.enter(|| {
        tracing::info!(
            name: crate::unit::DISPATCH,
            target: nest_rs_core::operation_log::TARGET,
            message = crate::unit::DISPATCH,
            event = event,
            listener = listener,
            outcome = settled,
            duration_ms = nest_rs_core::operation_log::duration_ms(started),
        );
        if let Err(payload) = &outcome {
            tracing::error!(
                target: crate::TARGET,
                event = event,
                listener = listener,
                panic = panic_message(payload.as_ref()),
                "event listener panicked — dispatch continues with the next listener",
            );
        }
    });
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

    /// The happy path files its unit of work and nothing else — an `error` on
    /// every emit would be noise, and no line at all would leave the listener
    /// anonymous, which is the state the operation line exists to remove.
    #[tokio::test]
    async fn a_healthy_dispatch_files_its_unit_and_no_containment_event() {
        let bus = EventBus::new();
        bus.subscribe_named(
            "Notifier::on_notify_requested",
            move |_: NotifyRequested| async move {},
        );
        let logs = LogCapture::install();
        bus.emit(NotifyRequested { id: "ok" }).await;

        assert!(
            logs.find(crate::TARGET, "event listener panicked")
                .is_empty(),
            "a healthy dispatch reports no containment: {:#?}",
            logs.events(),
        );

        let line = logs.expect_one(nest_rs_core::operation_log::TARGET, crate::unit::DISPATCH);
        assert_eq!(line.level, "info");
        assert_eq!(
            line.field("listener").as_deref(),
            Some("Notifier::on_notify_requested"),
            "the line names which listener ran, not merely that one did: {:#?}",
            line.fields,
        );
        assert_eq!(
            line.field("outcome").as_deref(),
            Some(nest_rs_core::operation_log::OK),
        );
        assert!(line.field("duration_ms").is_some());
    }

    /// Two listeners on one event are two units of work, so they file two
    /// lines under two span ids inside one trace.
    ///
    /// Neither of the first two tests asserted an id, and that is exactly what
    /// let `Correlation::inherited()` ship here: it returns the ambient
    /// correlation *unchanged*, so both lines carried the emitter's `span_id`
    /// and `parent_span_id` named the emitter's parent. A span id names one unit
    /// of work — a line that reuses one is a line an operator cannot relate.
    #[tokio::test]
    async fn two_listeners_file_two_units_inside_one_trace() {
        let bus = EventBus::new();
        bus.subscribe_named("Notifier::first", move |_: NotifyRequested| async move {});
        bus.subscribe_named("Notifier::second", move |_: NotifyRequested| async move {});
        let logs = LogCapture::install();
        bus.emit(NotifyRequested { id: "two" }).await;

        let lines = logs.find(nest_rs_core::operation_log::TARGET, crate::unit::DISPATCH);
        assert_eq!(
            lines.len(),
            2,
            "one line per listener: {:#?}",
            logs.events()
        );

        // The ids are read off the **span**, never off the line: a log line
        // renders the ambient correlation and carries no span state, so writing
        // them as event fields would be the duplicate CLAUDE.md forbids.
        let units: Vec<_> = logs
            .spans()
            .into_iter()
            .filter(|span| span.name == crate::unit::DISPATCH)
            .collect();
        assert_eq!(units.len(), 2, "one unit per listener: {units:#?}");

        let traces: Vec<_> = units
            .iter()
            .filter_map(|s| s.fields.get("trace_id"))
            .collect();
        let spans: Vec<_> = units
            .iter()
            .filter_map(|s| s.fields.get("span_id"))
            .collect();
        assert_eq!(traces.len(), 2, "every unit carries its ids: {units:#?}");
        assert_eq!(traces[0], traces[1], "one emit is one trace");
        assert_eq!(spans.len(), 2);
        assert_ne!(
            spans[0], spans[1],
            "two units of work are two span ids, not one reused: {units:#?}",
        );
    }

    /// A panicking listener still files its unit — with `outcome = panic`, so a
    /// containment is visible on the operation target an operator already
    /// queries rather than only on this crate's own.
    #[tokio::test]
    async fn a_panicking_listener_files_its_unit_as_a_panic() {
        let bus = EventBus::new();
        bus.subscribe_named("Notifier::boom", move |_: NotifyRequested| async move {
            panic!("listener exploded");
        });
        let logs = LogCapture::install();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        bus.emit(NotifyRequested { id: "boom" }).await;
        std::panic::set_hook(previous);

        let line = logs.expect_one(nest_rs_core::operation_log::TARGET, crate::unit::DISPATCH);
        assert_eq!(
            line.field("outcome").as_deref(),
            Some(nest_rs_core::operation_log::PANIC),
        );
        assert_eq!(line.field("listener").as_deref(), Some("Notifier::boom"));
    }
}

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use nest_rs_core::tracing::Instrument;
use parking_lot::RwLock;

type BoxedEvent = Box<dyn Any + Send>;
type ListenerFn = Arc<dyn Fn(BoxedEvent) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// One registered listener, carrying the `Provider::method` its unit of work is
/// filed under.
#[derive(Clone)]
struct Listener {
    name: &'static str,
    run: ListenerFn,
}

/// The typed in-process event bus; listeners are subscribed once, by
/// [`EventsModule`](crate::EventsModule)'s wiring step, so the `RwLock` is
/// uncontended on the emit path.
#[derive(Default)]
pub struct EventBus {
    listeners: RwLock<HashMap<TypeId, Vec<Listener>>>,
    /// Set once the wiring step subscribed the app's listeners; an emit before
    /// it would reach none, and says so rather than dropping in silence.
    wired: AtomicBool,
}

impl EventBus {
    /// An empty bus, which drops every event until the wiring step has
    /// subscribed the app's listeners.
    pub fn new() -> Self {
        Self::default()
    }

    /// Open the bus: every listener the app will ever have is subscribed.
    pub(crate) fn mark_wired(&self) {
        self.wired.store(true, Ordering::Relaxed);
    }

    /// Runs each listener in registration order, awaited in turn — once the
    /// emitter's transaction has committed, when it emits inside one. No-op when
    /// nothing is registered for `E`.
    ///
    /// Before the wiring step has subscribed the app's listeners — a task a
    /// factory or a constructor spawned can get there first — the event is
    /// dropped with a `warn` on `nest_rs::events` naming its type.
    ///
    /// Inside a unit of work holding a transaction, the dispatch waits for it
    /// through [`nest_rs_database::after_commit`]: it runs on commit, outside
    /// the transaction, and is dropped unrun on rollback or failure. With no
    /// transaction it runs before `emit` returns.
    ///
    /// A slow listener delays the ones after it and the emitter: work that must
    /// not block belongs on the queue. A panicking listener is caught, logged at
    /// `error` on `nest_rs::events`, and the chain continues.
    pub async fn emit<E: Clone + Send + 'static>(&self, event: E) {
        if !self.wired.load(Ordering::Relaxed) {
            tracing::warn!(
                target: crate::TARGET,
                event = std::any::type_name::<E>(),
                "event emitted before the listeners were wired: dropped",
            );
            return;
        }
        // Released before awaiting.
        let listeners = self.listeners.read().get(&TypeId::of::<E>()).cloned();
        let Some(listeners) = listeners else { return };
        let event_name = std::any::type_name::<E>();
        // Outside the loop: one emit is one trace, even with nothing ambient.
        let cause = nest_rs_core::Correlation::inherited();
        nest_rs_database::after_commit(async move {
            for Listener { name, run } in listeners {
                dispatch_one(&cause, event_name, name, run(Box::new(event.clone()))).await;
            }
        })
        .await;
    }
}

/// One listener invocation — the edge's unit of work, a child of the emitter's
/// trace; dropped with its emitter, it is filed `cancelled` by [`DispatchLine`].
async fn dispatch_one(
    cause: &nest_rs_core::Correlation,
    event: &'static str,
    listener: &'static str,
    fut: Pin<Box<dyn Future<Output = ()> + Send>>,
) {
    // `Correlation::inherited()` is the ambient one unchanged: without `child()`
    // two listeners would share one `span_id`.
    let correlation = cause.child();
    let span = nest_rs_core::operation_span!(
        crate::unit::DISPATCH,
        &correlation,
        event = event,
        listener = listener,
    );
    // No request scope, but `current_trace_id()` must answer inside a listener.
    let continuation = nest_rs_core::RequestContinuation::new(None, correlation);
    let line = DispatchLine {
        event,
        listener,
        continuation: &continuation,
        span: span.clone(),
        started: std::time::Instant::now(),
        filed: false,
    };
    let outcome = continuation
        .scope(nest_rs_core::panic::contain(fut))
        .instrument(span)
        .await;
    match outcome {
        Ok(()) => line.file(nest_rs_core::operation_log::OK),
        Err(payload) => {
            line.file(nest_rs_core::operation_log::PANIC);
            continuation.enter(|| {
                nest_rs_core::contained_panic!(
                    target: crate::TARGET,
                    payload.as_ref(),
                    "event listener panicked — dispatch continues with the next listener",
                    event = event,
                    listener = listener,
                );
            });
        }
    }
}

/// One listener's `events.dispatch` line, filed exactly once — by the end the
/// dispatch saw, or by `Drop` when the listener is dropped first.
///
/// Filed inside the re-entered correlation: a line emitted after the `.await`
/// otherwise carries no ids.
struct DispatchLine<'a> {
    event: &'static str,
    listener: &'static str,
    continuation: &'a nest_rs_core::RequestContinuation,
    span: tracing::Span,
    started: std::time::Instant,
    filed: bool,
}

impl DispatchLine<'_> {
    fn file(mut self, outcome: &'static str) {
        self.emit(outcome);
    }

    fn emit(&mut self, outcome: &'static str) {
        self.filed = true;
        self.continuation.enter(|| {
            nest_rs_core::operation_line!(
                crate::unit::DISPATCH,
                span: &self.span,
                outcome: outcome,
                started: self.started,
                event = self.event,
                listener = self.listener,
            );
        });
    }
}

impl Drop for DispatchLine<'_> {
    fn drop(&mut self) {
        if !self.filed {
            self.emit(nest_rs_core::operation_log::CANCELLED);
        }
    }
}

/// Subscribe a listener on `bus` that files its unit of work under `name`; the
/// seam `#[listeners]` emits.
pub fn subscribe_named<E, H, Fut>(bus: &EventBus, name: &'static str, listener: H)
where
    E: Any + Send + 'static,
    H: Fn(E) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let run: ListenerFn = Arc::new(move |boxed: BoxedEvent| {
        #[expect(
            clippy::expect_used,
            reason = "listeners are keyed by the TypeId of E, so only an E reaches this one"
        )]
        let event = *boxed
            .downcast::<E>()
            .expect("event downcasts to the type its listener subscribed for");
        Box::pin(listener(event)) as Pin<Box<dyn Future<Output = ()> + Send>>
    });
    bus.listeners
        .write()
        .entry(TypeId::of::<E>())
        .or_default()
        .push(Listener { name, run });
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A listener with no declared name on a bus opened by hand, as no
    /// wiring step runs here.
    pub(super) fn subscribe<E, H, Fut>(bus: &EventBus, listener: H)
    where
        E: Any + Send + 'static,
        H: Fn(E) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        subscribe_named(bus, "<anonymous>", listener);
        bus.mark_wired();
    }

    #[derive(Clone)]
    struct OrderPlaced {
        id: u32,
    }

    #[derive(Clone)]
    struct OrderShipped;

    #[tokio::test]
    async fn emit_is_a_noop_for_an_unsubscribed_event() {
        let bus = EventBus::new();
        bus.mark_wired();
        bus.emit(OrderPlaced { id: 1 }).await;
    }

    #[tokio::test]
    async fn an_emit_before_the_wiring_reaches_no_listener_and_says_so_once() {
        let bus = EventBus::new();
        let seen = Arc::new(AtomicUsize::new(0));
        let seen2 = seen.clone();
        subscribe_named(&bus, "Orders::on_placed", move |evt: OrderPlaced| {
            let seen = seen2.clone();
            async move {
                seen.fetch_add(evt.id as usize, Ordering::SeqCst);
            }
        });
        let logs = nest_rs_testing::LogCapture::install();

        bus.emit(OrderPlaced { id: 7 }).await;
        assert_eq!(seen.load(Ordering::SeqCst), 0, "the bus is not open yet");
        let event = logs.expect_one(
            crate::TARGET,
            "event emitted before the listeners were wired: dropped",
        );
        assert_eq!(event.level, "warn");
        assert!(
            event
                .field("event")
                .is_some_and(|name| name.ends_with("OrderPlaced")),
            "the line names the event type, got {:?}",
            event.fields,
        );

        bus.mark_wired();
        bus.emit(OrderPlaced { id: 7 }).await;
        assert_eq!(seen.load(Ordering::SeqCst), 7, "the wired bus dispatches");
        assert_eq!(
            logs.find(
                crate::TARGET,
                "event emitted before the listeners were wired: dropped"
            )
            .len(),
            1,
            "the wired bus warns no more",
        );
    }

    #[tokio::test]
    async fn a_subscribed_listener_runs_with_the_emitted_event() {
        let bus = EventBus::new();
        let seen = Arc::new(AtomicUsize::new(0));
        let seen2 = seen.clone();
        subscribe(&bus, move |evt: OrderPlaced| {
            let seen = seen2.clone();
            async move {
                seen.fetch_add(evt.id as usize, Ordering::SeqCst);
            }
        });

        bus.emit(OrderPlaced { id: 7 }).await;
        assert_eq!(seen.load(Ordering::SeqCst), 7);
    }

    #[tokio::test]
    async fn listeners_run_in_registration_order_for_the_same_event() {
        let bus = EventBus::new();
        let order = Arc::new(parking_lot::Mutex::new(Vec::<u32>::new()));

        let o1 = order.clone();
        subscribe(&bus, move |_: OrderPlaced| {
            let o = o1.clone();
            async move {
                o.lock().push(1);
            }
        });
        let o2 = order.clone();
        subscribe(&bus, move |_: OrderPlaced| {
            let o = o2.clone();
            async move {
                o.lock().push(2);
            }
        });
        let o3 = order.clone();
        subscribe(&bus, move |_: OrderPlaced| {
            let o = o3.clone();
            async move {
                o.lock().push(3);
            }
        });

        bus.emit(OrderPlaced { id: 0 }).await;
        assert_eq!(*order.lock(), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn listeners_for_distinct_event_types_do_not_cross_fire() {
        let bus = EventBus::new();
        let placed = Arc::new(AtomicUsize::new(0));
        let shipped = Arc::new(AtomicUsize::new(0));

        let p = placed.clone();
        subscribe(&bus, move |_: OrderPlaced| {
            let p = p.clone();
            async move {
                p.fetch_add(1, Ordering::SeqCst);
            }
        });
        let s = shipped.clone();
        subscribe(&bus, move |_: OrderShipped| {
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

    #[tokio::test]
    async fn the_event_is_handed_to_each_listener_independently() {
        let bus = EventBus::new();
        let counter = Arc::new(AtomicUsize::new(0));

        for _ in 0..3 {
            let c = counter.clone();
            subscribe(&bus, move |evt: OrderPlaced| {
                let c = c.clone();
                async move {
                    c.fetch_add(evt.id as usize, Ordering::SeqCst);
                }
            });
        }

        bus.emit(OrderPlaced { id: 4 }).await;
        assert_eq!(counter.load(Ordering::SeqCst), 12);
    }
}

#[cfg(test)]
mod panic_containment {
    use std::sync::Arc;

    use nest_rs_testing::LogCapture;
    use parking_lot::Mutex;

    use super::tests::subscribe;
    use super::*;

    #[derive(Clone)]
    struct NotifyRequested {
        id: &'static str,
    }

    #[tokio::test]
    async fn a_panicking_listener_does_not_stop_the_ones_after_it() {
        let bus = EventBus::new();
        let ran = Arc::new(Mutex::new(Vec::<u32>::new()));

        let r1 = ran.clone();
        subscribe(&bus, move |_: NotifyRequested| {
            let r = r1.clone();
            async move { r.lock().push(1) }
        });
        subscribe(&bus, move |e: NotifyRequested| async move {
            if e.id == "boom" {
                panic!("listener panic for boom");
            }
        });
        let r3 = ran.clone();
        subscribe(&bus, move |_: NotifyRequested| {
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

        let event = logs.expect_one(
            "nest_rs::events",
            "event listener panicked — dispatch continues with the next listener",
        );
        assert_eq!(event.level, "error");
        assert_eq!(
            event.field(nest_rs_core::panic::FIELD).as_deref(),
            Some("listener panic for boom"),
        );
    }

    #[tokio::test]
    async fn emit_returns_to_its_caller_after_a_listener_panics() {
        let bus = EventBus::new();
        subscribe(&bus, move |_: NotifyRequested| async move {
            panic!("listener panic");
        });

        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        bus.emit(NotifyRequested { id: "boom" }).await;
        std::panic::set_hook(previous);

        // Reaching this line is the assertion.
        let emit_returned = true;
        assert!(emit_returned);
    }

    #[tokio::test]
    async fn a_healthy_dispatch_files_its_unit_and_no_containment_event() {
        let bus = EventBus::new();
        bus.mark_wired();
        subscribe_named(
            &bus,
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

        let line = logs.expect_one(
            nest_rs_core::operation_log::TARGET,
            crate::unit::DISPATCH.name(),
        );
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

    #[tokio::test]
    async fn two_listeners_file_two_units_inside_one_trace() {
        let bus = EventBus::new();
        bus.mark_wired();
        subscribe_named(
            &bus,
            "Notifier::first",
            move |_: NotifyRequested| async move {},
        );
        subscribe_named(
            &bus,
            "Notifier::second",
            move |_: NotifyRequested| async move {},
        );
        let logs = LogCapture::install();
        bus.emit(NotifyRequested { id: "two" }).await;

        let lines = logs.find(
            nest_rs_core::operation_log::TARGET,
            crate::unit::DISPATCH.name(),
        );
        assert_eq!(
            lines.len(),
            2,
            "one line per listener: {:#?}",
            logs.events()
        );

        // Ids live on the span, never as line fields.
        let units: Vec<_> = logs
            .spans()
            .into_iter()
            .filter(|span| span.name == crate::unit::DISPATCH.name())
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

    #[tokio::test]
    async fn a_panicking_listener_files_its_unit_as_a_panic() {
        let bus = EventBus::new();
        bus.mark_wired();
        subscribe_named(
            &bus,
            "Notifier::boom",
            move |_: NotifyRequested| async move {
                panic!("listener exploded");
            },
        );
        let logs = LogCapture::install();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        bus.emit(NotifyRequested { id: "boom" }).await;
        std::panic::set_hook(previous);

        let line = logs.expect_one(
            nest_rs_core::operation_log::TARGET,
            crate::unit::DISPATCH.name(),
        );
        assert_eq!(
            line.field("outcome").as_deref(),
            Some(nest_rs_core::operation_log::PANIC),
        );
        assert_eq!(line.field("listener").as_deref(), Some("Notifier::boom"));
    }

    #[tokio::test]
    async fn a_listener_dropped_with_its_emitter_files_its_unit_cancelled() {
        let bus = EventBus::new();
        bus.mark_wired();
        subscribe_named(
            &bus,
            "Notifier::waits",
            move |_: NotifyRequested| async move {
                std::future::pending::<()>().await;
            },
        );
        let logs = LogCapture::install();

        let emitted = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            bus.emit(NotifyRequested { id: "dropped" }),
        )
        .await;
        assert!(emitted.is_err(), "the emitter was dropped mid-listener");

        let line = logs.expect_one(
            nest_rs_core::operation_log::TARGET,
            crate::unit::DISPATCH.name(),
        );
        assert_eq!(
            line.field("outcome").as_deref(),
            Some(nest_rs_core::operation_log::CANCELLED),
        );
        assert_eq!(line.field("listener").as_deref(), Some("Notifier::waits"));
        assert!(
            line.trace_id.is_some(),
            "in the listener's trace: {line:#?}"
        );
        let span = logs.expect_span(crate::TARGET, crate::unit::DISPATCH.name());
        assert_eq!(
            span.field("error.type").as_deref(),
            Some(nest_rs_core::operation_log::CANCELLED),
            "{:?}",
            span.fields,
        );
        assert_eq!(span.field("otel.status_code").as_deref(), Some("error"));
    }
}

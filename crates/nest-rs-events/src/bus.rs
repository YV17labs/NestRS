use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::error::Error;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::Stream;
use nest_rs_core::__private::{
    Site, Tail, UnitChains, caught_problem, dispatch, new_unit_context, settled, short_type_name,
};
use nest_rs_core::tracing::Instrument;
use nest_rs_core::{Container, Failure, Handler, Outcome, UnitContext};
use nest_rs_database::__private::run_unit_bounded;
use nest_rs_database::{ExecutorScope, TransactionPolicy, UnitOfWork, UnitSpec};
use parking_lot::RwLock;
use tokio::sync::broadcast;

use crate::EventsView;
use crate::answer::ListenerAnswer;
use crate::error::ListenerTimedOut;

type BoxedEvent = Box<dyn Any + Send>;
/// What one listener call came to: `Ok`, or why its unit failed.
type Answered = Result<(), Box<dyn Error + Send + Sync>>;
type ListenerFuture = Pin<Box<dyn Future<Output = Answered> + Send>>;
type ListenerFn = Arc<dyn Fn(BoxedEvent) -> ListenerFuture + Send + Sync>;

/// One registered listener, carrying the `Provider::method` its unit of work is
/// filed under, what its decorator declared of that unit, and the pipeline its
/// site composed when it was subscribed.
#[derive(Clone)]
struct Listener {
    name: &'static str,
    /// The event's type as its lines name it.
    event: Arc<str>,
    /// How the unit settles the listener's data-layer work.
    policy: TransactionPolicy,
    /// How long one call runs before it is cut.
    timeout: Duration,
    /// The site its decorator declared.
    handler: &'static Handler,
    /// The layers that site runs, composed once from the app's container.
    chains: Arc<UnitChains>,
    /// The binding the app provides, resolved with those layers: what opens
    /// the unit of a call whose emitter ran under none.
    unit_of_work: Option<Arc<dyn UnitOfWork>>,
    run: ListenerFn,
}

/// How many events of one type a [`stream`](EventBus::stream)'s reader may fall
/// behind before it loses the oldest.
const STREAM_CAPACITY: usize = 64;

/// The typed in-process event bus; listeners are subscribed once, by
/// [`EventsModule`](crate::EventsModule)'s wiring step, so the `RwLock` is
/// uncontended on the emit path.
#[derive(Default)]
pub struct EventBus {
    /// Each event type's listeners, behind one `Arc` an emit takes rather than
    /// a copy of every listener.
    listeners: RwLock<HashMap<TypeId, Arc<Vec<Listener>>>>,
    /// Each event type's channel to its open streams, a `broadcast::Sender<E>`
    /// made by the first [`stream`](Self::stream) of `E`: an event nothing ever
    /// streamed pays for none.
    streams: RwLock<HashMap<TypeId, Box<dyn Any + Send + Sync>>>,
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

    /// Let go of every listener once the app has ended: a listener that emits
    /// holds the bus that holds it, and neither would ever drop. Every open
    /// stream ends, once it read what it was sent.
    pub(crate) fn unsubscribe_all(&self) {
        self.listeners.write().clear();
        self.streams.write().clear();
    }

    /// The events of type `E` emitted from now on, each once the emitter's
    /// transaction has committed, as [`emit`](Self::emit) dispatches it to a
    /// listener, and never one whose transaction rolled back: the read side of
    /// the bus, for what stays open, a GraphQL subscription among them. An event
    /// committed before the stream opened is not read; one emitted before and
    /// committing after is read only when another stream of `E` was open at that
    /// emit.
    ///
    /// It runs no layer and no transaction, and reaches this process's bus only:
    /// a reader on another replica reads nothing, so what must reach them all is
    /// a backend's. A reader more than 64 events behind the newest of its type
    /// loses the oldest, and one `warn` on `nest_rs::events` says how many. The
    /// stream ends with the app.
    ///
    /// ```
    /// # use anyhow::Context as _;
    /// # use futures_util::StreamExt;
    /// # use nest_rs_core::{App, module};
    /// # use nest_rs_events::{EventBus, EventsModule};
    /// # #[derive(Clone)]
    /// # struct PointsAwarded { amount: u32 }
    /// # #[module(imports = [EventsModule])]
    /// # struct AppModule;
    /// # #[nest_rs_core::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// # let app = App::builder().module::<AppModule>().build().await?;
    /// # let bus = app.container().get::<EventBus>().context("EventsModule provides the bus")?;
    /// let mut awards = std::pin::pin!(bus.stream::<PointsAwarded>());
    ///
    /// bus.emit(PointsAwarded { amount: 10 }).await;
    ///
    /// assert_eq!(awards.next().await.map(|award| award.amount), Some(10));
    /// # Ok(())
    /// # }
    /// ```
    pub fn stream<E: Clone + Send + 'static>(
        &self,
    ) -> impl Stream<Item = E> + Send + 'static + use<E> {
        #[expect(
            clippy::expect_used,
            reason = "streams are keyed by the TypeId of E, so only a Sender<E> sits here"
        )]
        let reader = self
            .streams
            .write()
            .entry(TypeId::of::<E>())
            .or_insert_with(|| Box::new(broadcast::channel::<E>(STREAM_CAPACITY).0))
            .downcast_ref::<broadcast::Sender<E>>()
            .expect("the channel downcasts to the event type it is keyed by")
            .subscribe();
        futures_util::stream::unfold(reader, |mut reader| async move {
            loop {
                match reader.recv().await {
                    Ok(event) => return Some((event, reader)),
                    Err(broadcast::error::RecvError::Lagged(missed)) => tracing::warn!(
                        target: crate::TARGET,
                        event = short_type_name::<E>(),
                        missed,
                        "an event stream's reader fell behind",
                    ),
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        })
    }

    /// The channel to `E`'s streams, when one is open to hear it.
    fn open_streams<E: Clone + Send + 'static>(&self) -> Option<broadcast::Sender<E>> {
        self.streams
            .read()
            .get(&TypeId::of::<E>())?
            .downcast_ref::<broadcast::Sender<E>>()
            .filter(|sender| sender.receiver_count() > 0)
            .cloned()
    }

    /// Runs each listener in registration order, awaited in turn — once the
    /// emitter's transaction has committed, when it emits inside one. No-op when
    /// nothing is registered for `E`. An open [`stream`](Self::stream) of `E`
    /// reads the event first, so no listener holds it back.
    ///
    /// Before the wiring step has subscribed the app's listeners — a task a
    /// factory or a constructor spawned can get there first — the event is
    /// dropped with a `warn` on `nest_rs::events` naming its type.
    ///
    /// Inside a unit of work holding a transaction, the dispatch waits for it
    /// through [`nest_rs_database::after_commit`]: it runs on commit, outside
    /// the transaction, under what the emitter carried into it — its scope,
    /// its ability, a package's values — and is dropped unrun on rollback or
    /// failure. With no transaction to wait for it runs before `emit` returns,
    /// inside the emitter.
    ///
    /// Each listener call is a unit of work of its own, a child of the
    /// emitter's trace, opened by the unit-of-work binding the emitter ran
    /// under, or by the app's for an emitter outside every unit, a task it
    /// spawned, which holds what it carried and a request's scope: one
    /// transaction, committed when the listener answers `Ok` and rolled back
    /// when it fails — on the pool for `transactional = false`. An app binding
    /// none runs its listeners bare, with no executor. A listener's body is cut
    /// at its timeout, ten minutes unless its `#[on_event]` declares one; a
    /// commit never is. An event the listener emits waits for its commit in
    /// turn.
    ///
    /// The app's interceptors and exception filters, then those its
    /// `#[listeners]` block and its `#[on_event]` method bind, run around each
    /// call, reading it through [`EventsView`], inside its transaction.
    ///
    /// A failing listener — an `Err`, its timeout, a commit its driver could
    /// not honour — is logged at `error` on `nest_rs::events` with its whole
    /// chain, or at `warn` when a layer answered it with [`EventsView::skip`]
    /// or a client's problem (a `4xx`); a panicking one is caught and logged
    /// there too. The chain continues with the next listener either way, and
    /// `emit` returns to an emitter whose own work stands. A slow listener
    /// delays the ones after it and the emitter: work that must not block
    /// belongs on the queue.
    pub async fn emit<E: Clone + Send + 'static>(&self, event: E) {
        if !self.wired.load(Ordering::Relaxed) {
            tracing::warn!(
                target: crate::TARGET,
                event = short_type_name::<E>(),
                "event emitted before the listeners were wired: dropped",
            );
            return;
        }
        // Released before awaiting.
        let listeners = self.listeners.read().get(&TypeId::of::<E>()).cloned();
        let streams = self.open_streams::<E>();
        if listeners.is_none() && streams.is_none() {
            return;
        }
        let event_name = std::any::type_name::<E>();
        // Outside the loop: one emit is one trace, even with nothing ambient.
        let cause = nest_rs_core::Correlation::inherited();
        // No capture here: the executor that holds the dispatch carries it into
        // its commit, and one that hands it back runs it inside the emitter.
        nest_rs_database::after_commit(async move {
            if let Some(streams) = streams {
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "every reader left since the emit, so there is nobody to tell"
                )]
                let _ = streams.send(event.clone());
            }
            for listener in listeners.iter().flat_map(|listeners| listeners.iter()) {
                let body = (listener.run)(Box::new(event.clone()));
                dispatch_one(&cause, event_name, listener, body).await;
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
    listener: &Listener,
    body: ListenerFuture,
) {
    // `Correlation::inherited()` is the ambient one unchanged: without `child()`
    // two listeners would share one `span_id`.
    let correlation = cause.child();
    let handler = listener.handler;
    let span = nest_rs_core::operation_span!(
        crate::unit::DISPATCH,
        &correlation,
        event = &*listener.event,
        host = handler.host_name(),
        method = handler.method(),
    );
    let continuation = nest_rs_core::RequestContinuation::new(None, correlation.clone());
    let line = DispatchLine {
        event: &listener.event,
        handler,
        continuation: &continuation,
        span: span.clone(),
        started: std::time::Instant::now(),
        filed: false,
    };
    let mut tail = ListenerTail { body: Some(body) };
    // Nested, never a fresh context, which would drop what the emitter carries.
    let outcome = nest_rs_core::with_request_scope(
        None,
        correlation,
        nest_rs_core::panic::contain(run(listener, event, &mut tail)),
    )
    .instrument(span)
    .await;
    match outcome {
        Ok(Ok(_)) => {
            line.file(nest_rs_core::operation_log::OK);
            if tail.body.is_some() {
                continuation.enter(|| {
                    tracing::debug!(
                        target: crate::TARGET,
                        event = &*listener.event,
                        host = handler.host_name(),
                        method = handler.method(),
                        "event listener skipped by a layer",
                    );
                });
            }
        }
        // A failure a layer skipped, or said was the client's (a `4xx`), is one the
        // app expected; a `5xx` it said is still the server's, at `error`.
        Ok(Err(failure))
            if failure.is_answered()
                || caught_problem(&failure).is_some_and(|problem| !problem.is_server_error()) =>
        {
            line.file(nest_rs_core::operation_log::ERROR);
            continuation.enter(|| {
                tracing::warn!(
                    target: crate::TARGET,
                    event = &*listener.event,
                    host = handler.host_name(),
                    method = handler.method(),
                    error = %nest_rs_core::error_message(failure.error()),
                    "event listener failed; a layer answered it — dispatch continues with the \
                     next listener",
                );
            });
        }
        Ok(Err(failure)) => {
            line.file(nest_rs_core::operation_log::ERROR);
            continuation.enter(|| {
                tracing::error!(
                    target: crate::TARGET,
                    event = &*listener.event,
                    host = handler.host_name(),
                    method = handler.method(),
                    error = %nest_rs_core::error_message(failure.error()),
                    "event listener failed — dispatch continues with the next listener",
                );
            });
        }
        Err(payload) => {
            line.file(nest_rs_core::operation_log::PANIC);
            continuation.enter(|| {
                nest_rs_core::contained_panic!(
                    target: crate::TARGET,
                    payload.as_ref(),
                    "event listener panicked — dispatch continues with the next listener",
                    event = &*listener.event,
                    host = handler.host_name(),
                    method = handler.method(),
                );
            });
        }
    }
}

/// One listener call as a unit of work of its own, opened by the binding its
/// emitter ran under or else the app's — bare with neither — around its
/// pipeline, its body cut `timeout` after it started, never after the listeners
/// queued before it.
async fn run(listener: &Listener, event: &'static str, tail: &mut ListenerTail) -> Outcome {
    let timeout = listener.timeout;
    let deadline = tokio::time::Instant::now() + timeout;
    let binding =
        nest_rs_database::current_unit_of_work().or_else(|| listener.unit_of_work.clone());
    let scope = nest_rs_database::current_executor_scope().unwrap_or(ExecutorScope::Request);
    let spec = UnitSpec::new(crate::unit::DISPATCH, scope, listener.policy);
    let mut view = EventsView::new(event, listener.name);
    run_unit_bounded(
        binding.as_ref(),
        spec,
        deadline,
        async {
            let mut cx = new_unit_context(listener.handler, &mut view, None, Some(deadline));
            dispatch(&mut cx, &listener.chains, tail).await
        },
        Result::is_ok,
        |why| Err(Failure::from(why)),
        move || Err(Failure::from(ListenerTimedOut::new(timeout))),
    )
    .await
}

/// The last step of a listener call's pipeline: the listener's own body,
/// polled once.
struct ListenerTail {
    body: Option<ListenerFuture>,
}

impl Tail for ListenerTail {
    async fn call(&mut self, _: &mut UnitContext<'_>) -> Outcome {
        let Some(body) = self.body.take() else {
            unreachable!("a unit runs its tail once");
        };
        body.await.map(|()| settled()).map_err(Failure::new)
    }
}

/// One listener's `events.dispatch` line, filed exactly once — by the end the
/// dispatch saw, or by `Drop` when the listener is dropped first.
///
/// Filed inside the re-entered correlation: a line emitted after the `.await`
/// otherwise carries no ids.
struct DispatchLine<'a> {
    event: &'a str,
    handler: &'static Handler,
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
                host = self.handler.host_name(),
                method = self.handler.method(),
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

/// Subscribe a listener on `bus` that files its unit of work under `name`, runs
/// it under `policy` around the pipeline `site` composes on `container`, and
/// cuts it `timeout` after it starts; the seam `#[listeners]` emits. A layer
/// the site binds and nothing provides refuses it.
pub fn subscribe_named<E, H, Fut>(
    bus: &EventBus,
    container: &Container,
    site: &'static Site,
    name: &'static str,
    policy: TransactionPolicy,
    timeout: Duration,
    listener: H,
) -> anyhow::Result<()>
where
    E: Any + Send + 'static,
    H: Fn(E) -> Fut + Send + Sync + 'static,
    Fut: Future + Send + 'static,
    Fut::Output: ListenerAnswer,
{
    let chains = site.chains(container)?;
    let unit_of_work = container.get_dyn::<dyn UnitOfWork>();
    let handler = site.handler();
    let event: Arc<str> = Arc::from(short_type_name::<E>());
    tracing::info!(
        target: crate::TARGET,
        host = handler.host_name(),
        method = handler.method(),
        event = event.as_ref(),
        "mounted",
    );
    let run: ListenerFn = Arc::new(move |boxed: BoxedEvent| {
        #[expect(
            clippy::expect_used,
            reason = "listeners are keyed by the TypeId of E, so only an E reaches this one"
        )]
        let event = *boxed
            .downcast::<E>()
            .expect("event downcasts to the type its listener subscribed for");
        let answer = listener(event);
        Box::pin(async move { answer.await.into_outcome() }) as ListenerFuture
    });
    Arc::make_mut(bus.listeners.write().entry(TypeId::of::<E>()).or_default()).push(Listener {
        name,
        event,
        policy,
        timeout,
        handler,
        chains,
        unit_of_work,
        run,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures_util::{FutureExt, StreamExt};

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

    /// A listener taking the defaults `#[on_event]` emits when it declares
    /// no key.
    pub(super) fn subscribe_named<E, H, Fut>(bus: &EventBus, name: &'static str, listener: H)
    where
        E: Any + Send + 'static,
        H: Fn(E) -> Fut + Send + Sync + 'static,
        Fut: Future + Send + 'static,
        Fut::Output: ListenerAnswer,
    {
        super::subscribe_named(
            bus,
            &Container::default(),
            &crate::testing::SITE,
            name,
            TransactionPolicy::PerUnit,
            nest_rs_core::JOB_TIMEOUT,
            listener,
        )
        .expect("a site binding no layer composes");
    }

    #[derive(Clone)]
    struct OrderPlaced {
        id: u32,
    }

    #[tokio::test]
    async fn every_open_stream_of_an_event_reads_it_and_no_other_event() {
        let bus = EventBus::new();
        bus.mark_wired();
        let mut first = std::pin::pin!(bus.stream::<OrderPlaced>());
        let mut second = std::pin::pin!(bus.stream::<OrderPlaced>());
        let mut shipped = std::pin::pin!(bus.stream::<OrderShipped>());

        bus.emit(OrderPlaced { id: 7 }).await;

        assert_eq!(first.next().await.map(|order| order.id), Some(7));
        assert_eq!(second.next().await.map(|order| order.id), Some(7));
        assert!(shipped.next().now_or_never().is_none());
    }

    #[tokio::test]
    async fn a_stream_reads_an_event_it_opened_before_and_never_one_before_it() {
        let bus = EventBus::new();
        bus.mark_wired();
        bus.emit(OrderPlaced { id: 1 }).await;
        let mut stream = std::pin::pin!(bus.stream::<OrderPlaced>());

        assert!(stream.next().now_or_never().is_none());
        bus.emit(OrderPlaced { id: 2 }).await;
        assert_eq!(stream.next().await.map(|order| order.id), Some(2));
    }

    #[tokio::test]
    async fn a_slow_listener_never_holds_back_what_a_stream_reads() {
        let bus = Arc::new(EventBus::new());
        let released = Arc::new(tokio::sync::Notify::new());
        let waiting = released.clone();
        subscribe(&bus, move |_: OrderPlaced| {
            let waiting = waiting.clone();
            async move { waiting.notified().await }
        });
        let mut stream = std::pin::pin!(bus.stream::<OrderPlaced>());

        let emitting = tokio::spawn({
            let bus = bus.clone();
            async move { bus.emit(OrderPlaced { id: 7 }).await }
        });

        assert_eq!(
            stream.next().await.map(|order| order.id),
            Some(7),
            "the listener is still waiting",
        );
        released.notify_one();
        emitting.await.expect("the emit returns once it answered");
    }

    #[tokio::test]
    async fn a_reader_past_the_streams_capacity_loses_the_oldest_and_says_how_many() {
        let bus = EventBus::new();
        bus.mark_wired();
        let mut stream = std::pin::pin!(bus.stream::<OrderPlaced>());
        let logs = nest_rs_testing::LogCapture::install();
        let capacity = u32::try_from(STREAM_CAPACITY).expect("a small capacity");

        for id in 0..capacity + 3 {
            bus.emit(OrderPlaced { id }).await;
        }

        assert_eq!(
            stream.next().await.map(|order| order.id),
            Some(3),
            "the oldest three are gone",
        );
        let said = logs.expect_one(crate::TARGET, "an event stream's reader fell behind");
        assert_eq!(said.level, "warn");
        assert_eq!(
            said.field("missed").as_deref(),
            Some("3"),
            "{:?}",
            said.fields
        );
        assert_eq!(
            said.field("event").as_deref(),
            Some("OrderPlaced"),
            "{:?}",
            said.fields,
        );
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
        assert_eq!(
            event.field("event").as_deref(),
            Some("OrderPlaced"),
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

    use super::tests::{subscribe, subscribe_named};
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
            (
                line.field("host").as_deref(),
                line.field("method").as_deref()
            ),
            (Some("Notifier"), Some("on_event")),
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

    /// A listener body that unwinds, answering `()` as far as its type says.
    fn explode() {
        panic!("listener exploded");
    }

    #[tokio::test]
    async fn a_panicking_listener_files_its_unit_as_a_panic() {
        let bus = EventBus::new();
        bus.mark_wired();
        subscribe_named(
            &bus,
            "Notifier::boom",
            move |_: NotifyRequested| async move { explode() },
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
        assert_eq!(line.field("host").as_deref(), Some("Notifier"));
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
        assert_eq!(line.field("host").as_deref(), Some("Notifier"));
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

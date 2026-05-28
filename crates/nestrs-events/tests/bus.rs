//! End-to-end: a producer injects the bus and emits; a discovered
//! `#[event_handler]` (itself injecting a service) runs — wired at bootstrap from
//! the fully-assembled container.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use nestrs_core::{injectable, module, App};
use nestrs_events::{async_trait, event_handler, EventBus, EventHandler, EventModule};

/// The event — any `Clone + Send + 'static` type.
#[derive(Clone)]
struct PointsAwarded {
    amount: usize,
}

/// A shared singleton the handler injects, so the test can observe dispatch.
#[injectable]
#[derive(Default)]
struct Ledger {
    total: AtomicUsize,
}

#[event_handler]
struct OnPointsAwarded {
    #[inject]
    ledger: Arc<Ledger>,
}

#[async_trait]
impl EventHandler for OnPointsAwarded {
    type Event = PointsAwarded;
    async fn handle(&self, event: PointsAwarded) {
        self.ledger.total.fetch_add(event.amount, Ordering::SeqCst);
    }
}

/// A producer: injects the bus and emits.
#[injectable]
struct Awarder {
    #[inject]
    events: Arc<EventBus>,
}

impl Awarder {
    async fn award(&self, amount: usize) {
        self.events.emit(PointsAwarded { amount }).await;
    }
}

#[module(imports = [EventModule], providers = [Ledger, OnPointsAwarded, Awarder])]
struct EventsTestModule;

#[tokio::test]
async fn a_producer_emits_and_the_discovered_handler_runs() {
    let app = App::new::<EventsTestModule>();
    // Bootstrap wires the discovered handler into the bus from the final container.
    app.init().await.expect("bootstrap wiring succeeds");

    let awarder = app
        .container()
        .get::<Awarder>()
        .expect("Awarder is provided");
    awarder.award(7).await;
    awarder.award(5).await;

    let ledger = app.container().get::<Ledger>().expect("Ledger is provided");
    assert_eq!(ledger.total.load(Ordering::SeqCst), 12);
}

#[tokio::test]
async fn emitting_an_event_with_no_handler_is_a_noop() {
    #[derive(Clone)]
    struct Unobserved;

    let app = App::new::<EventsTestModule>();
    app.init().await.expect("bootstrap wiring succeeds");

    let bus = app
        .container()
        .get::<EventBus>()
        .expect("EventBus is provided");
    bus.emit(Unobserved).await; // no handler registered — must not panic
}

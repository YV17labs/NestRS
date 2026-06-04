//! End-to-end: producer emits via the bus; the discovered `#[on_event]` runs.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nestrs_core::{App, injectable, module};
use nestrs_events::{EventBus, EventHandler, EventModule, async_trait, on_event};

#[derive(Clone)]
struct PointsAwarded {
    amount: usize,
}

#[injectable]
#[derive(Default)]
struct Ledger {
    total: AtomicUsize,
}

#[on_event]
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
    let app = App::new::<EventsTestModule>().expect("boots");
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

    let app = App::new::<EventsTestModule>().expect("boots");
    app.init().await.expect("bootstrap wiring succeeds");

    let bus = app
        .container()
        .get::<EventBus>()
        .expect("EventBus is provided");
    bus.emit(Unobserved).await;
}

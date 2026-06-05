//! End-to-end: producer emits via the bus; the discovered `#[on_event]`
//! method runs. A second `#[on_event]` on the same provider proves the
//! multi-method orchestrator pattern (shared `#[inject]` deps, one struct).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nestrs_core::{App, injectable, module};
use nestrs_events::{EventBus, EventModule, listeners};

#[derive(Clone)]
struct PointsAwarded {
    amount: usize,
}

#[derive(Clone)]
struct PointsRedeemed {
    amount: usize,
}

#[injectable]
#[derive(Default)]
struct Ledger {
    credited: AtomicUsize,
    debited: AtomicUsize,
}

#[injectable]
struct PointsListeners {
    #[inject]
    ledger: Arc<Ledger>,
}

#[listeners]
impl PointsListeners {
    #[on_event]
    async fn on_awarded(&self, event: PointsAwarded) {
        self.ledger.credited.fetch_add(event.amount, Ordering::SeqCst);
    }

    #[on_event]
    async fn on_redeemed(&self, event: PointsRedeemed) {
        self.ledger.debited.fetch_add(event.amount, Ordering::SeqCst);
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

    async fn redeem(&self, amount: usize) {
        self.events.emit(PointsRedeemed { amount }).await;
    }
}

#[module(imports = [EventModule], providers = [Ledger, PointsListeners, Awarder])]
struct EventsTestModule;

#[tokio::test]
async fn a_producer_emits_and_the_discovered_listener_runs() {
    let app = App::new::<EventsTestModule>().expect("boots");
    app.init().await.expect("bootstrap wiring succeeds");

    let awarder = app
        .container()
        .get::<Awarder>()
        .expect("Awarder is provided");
    awarder.award(7).await;
    awarder.award(5).await;

    let ledger = app.container().get::<Ledger>().expect("Ledger is provided");
    assert_eq!(ledger.credited.load(Ordering::SeqCst), 12);
    assert_eq!(ledger.debited.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn several_on_event_methods_share_the_providers_deps() {
    let app = App::new::<EventsTestModule>().expect("boots");
    app.init().await.expect("bootstrap wiring succeeds");

    let awarder = app
        .container()
        .get::<Awarder>()
        .expect("Awarder is provided");
    awarder.award(10).await;
    awarder.redeem(3).await;
    awarder.redeem(4).await;

    let ledger = app.container().get::<Ledger>().expect("Ledger is provided");
    assert_eq!(ledger.credited.load(Ordering::SeqCst), 10);
    assert_eq!(ledger.debited.load(Ordering::SeqCst), 7);
}

#[tokio::test]
async fn emitting_an_event_with_no_listener_is_a_noop() {
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

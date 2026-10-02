//! End-to-end: producer emits via the bus; the discovered `#[on_event]`
//! method runs. A second `#[on_event]` on the same provider proves the
//! multi-method orchestrator pattern (shared `#[inject]` deps, one struct).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{App, injectable, module};
use nest_rs_events::{EventBus, EventsModule, listeners};

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
        self.ledger
            .credited
            .fetch_add(event.amount, Ordering::SeqCst);
    }

    #[on_event]
    async fn on_redeemed(&self, event: PointsRedeemed) {
        self.ledger
            .debited
            .fetch_add(event.amount, Ordering::SeqCst);
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

#[module(imports = [EventsModule], providers = [Ledger, PointsListeners, Awarder])]
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

/// A boundary's transaction as the bus meets it — through the data layer's
/// port, with no ORM behind it: it keeps the work [`after_commit`] hands it
/// until the test settles the boundary one way or the other.
///
/// [`after_commit`]: nest_rs_database::after_commit
#[derive(Default)]
struct OpenTransaction(std::sync::Mutex<Vec<nest_rs_database::Deferred>>);

impl nest_rs_database::Executor for OpenTransaction {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn after_commit(&self, work: nest_rs_database::Deferred) -> Option<nest_rs_database::Deferred> {
        self.held().push(work);
        None
    }
}

impl OpenTransaction {
    fn held(&self) -> std::sync::MutexGuard<'_, Vec<nest_rs_database::Deferred>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// What a boundary does once its transaction has committed.
    async fn commit(&self) {
        let held = std::mem::take(&mut *self.held());
        for work in held {
            work.await;
        }
    }

    /// What a boundary does once its transaction has rolled back.
    fn roll_back(&self) {
        self.held().clear();
    }
}

async fn booted() -> (App, Arc<Awarder>, Arc<Ledger>) {
    let app = App::new::<EventsTestModule>().expect("boots");
    app.init().await.expect("bootstrap wiring succeeds");
    let awarder = app
        .container()
        .get::<Awarder>()
        .expect("Awarder is provided");
    let ledger = app.container().get::<Ledger>().expect("Ledger is provided");
    (app, awarder, ledger)
}

/// The demo's publish shape, reduced: a service emits a fact inside the
/// transaction that writes it. The listener must not see the fact before the
/// write lands — before this, `emit` dispatched inline, and a listener pushing a
/// job or notifying a subscriber did so about a row nothing had committed yet.
#[tokio::test]
async fn an_event_emitted_inside_a_transaction_is_dispatched_once_it_commits() {
    let (_app, awarder, ledger) = booted().await;
    let transaction = Arc::new(OpenTransaction::default());

    nest_rs_database::with_request_executor(transaction.clone(), awarder.award(7)).await;
    assert_eq!(
        ledger.credited.load(Ordering::SeqCst),
        0,
        "no listener runs while the emitter's transaction is open",
    );

    transaction.commit().await;
    assert_eq!(ledger.credited.load(Ordering::SeqCst), 7);
}

/// A fact the transaction rolled back never happened, so nothing reacts to it —
/// and nothing files a unit of work for a listener that never started.
#[tokio::test]
async fn an_event_whose_transaction_rolls_back_is_never_dispatched() {
    let (_app, awarder, ledger) = booted().await;
    let transaction = Arc::new(OpenTransaction::default());
    let logs = nest_rs_testing::LogCapture::install();

    nest_rs_database::with_request_executor(transaction.clone(), awarder.award(7)).await;
    transaction.roll_back();

    assert_eq!(ledger.credited.load(Ordering::SeqCst), 0);
    assert!(
        logs.find(
            nest_rs_core::operation_log::TARGET,
            nest_rs_events::unit::DISPATCH
        )
        .is_empty(),
        "a dispatch that never ran files no unit: {:#?}",
        logs.events(),
    );
}

/// An executor with no transaction to wait for — a pool, a transaction its
/// caller commits — hands the work back, and the listeners run before `emit`
/// returns, exactly as they do with no data layer at all.
#[tokio::test]
async fn an_event_with_no_transaction_to_wait_for_is_dispatched_before_emit_returns() {
    struct Pool;
    impl nest_rs_database::Executor for Pool {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    let (_app, awarder, ledger) = booted().await;
    nest_rs_database::with_request_executor(Arc::new(Pool), awarder.award(7)).await;
    assert_eq!(ledger.credited.load(Ordering::SeqCst), 7);
}

/// A provider whose listeners are declared and reachable, in an app that never
/// imported `EventsModule`. Nothing in the `#[listeners]` expansion makes the
/// host depend on `EventBus`, so this composition boots clean and reacts to
/// nothing — the shape the bootstrap hook has to report rather than skip.
#[module(providers = [Ledger, PointsListeners])]
struct BuslessModule;

#[tokio::test]
async fn listeners_with_no_event_bus_are_reported_at_boot() {
    let logs = nest_rs_testing::LogCapture::install();

    let app = App::new::<BuslessModule>().expect("boots without EventsModule");
    app.init()
        .await
        .expect("a missing bus is never a boot failure");

    let reported = logs.find(nest_rs_events::TARGET, nest_rs_events::NO_BUS_REPORT);
    assert_eq!(
        reported.len(),
        2,
        "one line per declared listener, naming it: {:#?}",
        logs.events(),
    );
    assert!(
        reported.iter().all(|event| event.level == "warn"),
        "a whole feature's reaction surface being dead is a warn, not a debug",
    );
    let named: Vec<_> = reported
        .iter()
        .filter_map(|event| event.field("listener"))
        .collect();
    assert!(
        named.iter().any(|name| name.contains("on_awarded"))
            && named.iter().any(|name| name.contains("on_redeemed")),
        "each line names the listener a developer can find, got {named:?}",
    );
}

#[derive(Clone)]
struct Shaped;

#[cfg(any())]
#[derive(Clone)]
struct CompiledOut;

static SHAPED: AtomicUsize = AtomicUsize::new(0);

#[injectable]
#[derive(Default)]
struct ShapedListeners;

#[listeners]
impl ShapedListeners {
    #[expect(
        clippy::needless_arbitrary_self_type,
        reason = "the spelled-out receiver is the shape under test"
    )]
    #[on_event]
    async fn on_shaped(self: &Self, _event: Shaped) -> () {
        SHAPED.fetch_add(1, Ordering::SeqCst);
    }

    #[cfg(any())]
    #[on_event]
    async fn on_compiled_out(&self, _event: CompiledOut) {}

    #[on_event]
    async fn r#match(&self, _event: Shaped) {
        SHAPED.fetch_add(1, Ordering::SeqCst);
    }

    #[on_event]
    async fn through_its_arc(self: &std::sync::Arc<Self>, _event: Shaped) {
        SHAPED.fetch_add(1, Ordering::SeqCst);
    }
}

#[injectable]
#[derive(Default)]
#[expect(
    non_camel_case_types,
    reason = "a raw-identifier type is the shape under test"
)]
struct r#loop;

#[listeners]
impl r#loop {
    #[on_event]
    async fn on_shaped(&self, _event: Shaped) {}
}

#[module(imports = [EventsModule], providers = [ShapedListeners])]
struct ShapedModule;

/// Compiling is the first half: a listener compiled out takes its wiring and its
/// entry with it — without them the expansion names a method and an event that
/// do not exist. `-> ()` written out and a typed `self: &Self` are the plain
/// shapes spelled out, `self: &Arc<Self>` borrows what the container holds, and a
/// raw identifier is a name — the expansion panicked on `r#` — so every listener
/// is subscribed, and a raw host is named by its name.
#[tokio::test]
async fn a_compiled_out_listener_is_skipped_and_the_spelled_out_shapes_are_served() {
    let app = App::new::<ShapedModule>().expect("boots");
    app.init().await.expect("bootstrap wiring succeeds");
    let bus = app
        .container()
        .get::<EventBus>()
        .expect("EventBus is provided");
    bus.emit(Shaped).await;
    assert_eq!(SHAPED.load(Ordering::SeqCst), 3);

    let mut names: Vec<&str> = nest_rs_core::inventory::iter::<nest_rs_events::ListenerMethod>()
        .map(|listener| listener.name)
        .filter(|name| name.starts_with("ShapedListeners::") || name.starts_with("loop::"))
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "ShapedListeners::match",
            "ShapedListeners::on_shaped",
            "ShapedListeners::through_its_arc",
            "loop::on_shaped",
        ]
    );
}

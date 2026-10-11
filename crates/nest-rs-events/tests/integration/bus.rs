//! A producer emits via the bus; the discovered `#[on_event]` methods run.

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

    let bus = app
        .container()
        .get::<EventBus>()
        .expect("EventBus is provided");
    bus.emit(Unobserved).await;
}

/// Another driver's pool: nothing to hold work for, the trait's defaults.
struct Pool;

impl nest_rs_database::Executor for Pool {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// A boundary's transaction through the data layer's port, no ORM behind it,
/// holding work as a driver does: under what the unit carries into its commit,
/// on its pool, tagged with the scope it was registered under.
#[derive(Default)]
struct OpenTransaction(std::sync::Mutex<Vec<nest_rs_database::Deferred>>);

impl nest_rs_database::Executor for OpenTransaction {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn after_commit(&self, work: nest_rs_database::Deferred) -> Option<nest_rs_database::Deferred> {
        let pool: Arc<dyn nest_rs_database::Executor> = Arc::new(Pool);
        let work: nest_rs_database::Deferred = match nest_rs_database::current_executor_scope() {
            Some(nest_rs_database::ExecutorScope::Request) => {
                Box::pin(nest_rs_database::with_request_executor(pool, work))
            }
            Some(nest_rs_database::ExecutorScope::Job) => {
                Box::pin(nest_rs_database::with_job_executor(pool, work))
            }
            None => Box::pin(nest_rs_database::with_executor(pool, work)),
        };
        let work: nest_rs_database::Deferred =
            match nest_rs_core::RequestContinuation::capture(nest_rs_core::Continuation::Commit) {
                Some(carried) => Box::pin(carried.scope(work)),
                None => work,
            };
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

    async fn commit(&self) {
        let held = std::mem::take(&mut *self.held());
        for work in held {
            work.await;
        }
    }

    fn roll_back(&self) {
        self.held().clear();
    }
}

async fn booted() -> (App, Arc<Awarder>, Arc<Ledger>) {
    let app = App::new::<EventsTestModule>().expect("boots");
    let awarder = app
        .container()
        .get::<Awarder>()
        .expect("Awarder is provided");
    let ledger = app.container().get::<Ledger>().expect("Ledger is provided");
    (app, awarder, ledger)
}

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
            nest_rs_events::unit::DISPATCH.name()
        )
        .is_empty(),
        "a dispatch that never ran files no unit: {:#?}",
        logs.events(),
    );
}

#[tokio::test]
async fn an_event_with_no_transaction_to_wait_for_is_dispatched_before_emit_returns() {
    let (_app, awarder, ledger) = booted().await;
    nest_rs_database::with_request_executor(Arc::new(Pool), awarder.award(7)).await;
    assert_eq!(ledger.credited.load(Ordering::SeqCst), 7);
}

/// Reachable listeners in an app that never imported `EventsModule`.
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

/// Compiling is the first half: a compiled-out listener names a method and an
/// event that do not exist.
#[tokio::test]
async fn a_compiled_out_listener_is_skipped_and_the_spelled_out_shapes_are_served() {
    let app = App::new::<ShapedModule>().expect("boots");
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

/// A package's value, carried by the emitter.
#[derive(Debug)]
struct Tenant(&'static str);

impl nest_rs_core::AmbientValue for Tenant {}

#[injectable]
#[derive(Default)]
struct TenantProbe {
    seen: std::sync::Mutex<Vec<Option<&'static str>>>,
}

#[injectable]
struct TenantListeners {
    #[inject]
    probe: Arc<TenantProbe>,
}

#[derive(Clone)]
struct TenantTouched;

#[listeners]
impl TenantListeners {
    #[on_event]
    async fn on_touched(&self, _event: TenantTouched) {
        let tenant = nest_rs_core::ambient::<Tenant>().map(|tenant| tenant.0);
        self.probe
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(tenant);
    }
}

#[module(imports = [EventsModule], providers = [TenantProbe, TenantListeners])]
struct TenantEventsModule;

async fn tenant_booted() -> (App, Arc<EventBus>, Arc<TenantProbe>) {
    let app = App::new::<TenantEventsModule>().expect("boots");
    let bus = app
        .container()
        .get::<EventBus>()
        .expect("the bus is provided");
    let probe = app
        .container()
        .get::<TenantProbe>()
        .expect("the probe is provided");
    (app, bus, probe)
}

fn seen(probe: &TenantProbe) -> Vec<Option<&'static str>> {
    probe
        .seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

#[tokio::test]
async fn a_listener_reads_a_value_its_emitter_carried() {
    let (_app, bus, probe) = tenant_booted().await;

    nest_rs_core::with_ambient(Arc::new(Tenant("acme")), bus.emit(TenantTouched)).await;

    assert_eq!(seen(&probe), [Some("acme")]);
}

#[tokio::test]
async fn a_listener_held_for_the_commit_still_reads_what_its_emitter_carried() {
    let (_app, bus, probe) = tenant_booted().await;
    let transaction = Arc::new(OpenTransaction::default());

    nest_rs_core::with_ambient(
        Arc::new(Tenant("acme")),
        nest_rs_database::with_request_executor(transaction.clone(), bus.emit(TenantTouched)),
    )
    .await;
    assert!(seen(&probe).is_empty(), "held until the commit");

    transaction.commit().await;
    assert_eq!(
        seen(&probe),
        [Some("acme")],
        "the value crossed into the commit with the event",
    );
}

/// What a listener ran on: the executor, named, and the scope it was tagged with.
type RanOn = (
    Option<&'static str>,
    Option<nest_rs_database::ExecutorScope>,
);

#[injectable]
#[derive(Default)]
struct ExecutorProbe {
    seen: std::sync::Mutex<Vec<RanOn>>,
}

#[injectable]
struct ExecutorListeners {
    #[inject]
    probe: Arc<ExecutorProbe>,
}

#[derive(Clone)]
struct Recorded;

#[listeners]
impl ExecutorListeners {
    #[on_event]
    async fn on_recorded(&self, _event: Recorded) {
        let executor = nest_rs_database::current_executor().map(|executor| {
            if executor.as_any().is::<Pool>() {
                "pool"
            } else if executor.as_any().is::<OpenTransaction>() {
                "transaction"
            } else {
                "another"
            }
        });
        self.probe
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((executor, nest_rs_database::current_executor_scope()));
    }
}

#[module(imports = [EventsModule], providers = [ExecutorProbe, ExecutorListeners])]
struct ExecutorEventsModule;

async fn executor_booted() -> (App, Arc<EventBus>, Arc<ExecutorProbe>) {
    let app = App::new::<ExecutorEventsModule>().expect("boots");
    let bus = app
        .container()
        .get::<EventBus>()
        .expect("the bus is provided");
    let probe = app
        .container()
        .get::<ExecutorProbe>()
        .expect("the probe is provided");
    (app, bus, probe)
}

fn ran_on(probe: &ExecutorProbe) -> Vec<RanOn> {
    probe
        .seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

#[tokio::test]
async fn a_listener_run_at_once_runs_on_its_emitters_executor() {
    let (_app, bus, probe) = executor_booted().await;

    nest_rs_database::with_request_executor(Arc::new(Pool), bus.emit(Recorded)).await;

    assert_eq!(
        ran_on(&probe),
        [(Some("pool"), Some(nest_rs_database::ExecutorScope::Request))],
        "nothing to wait for: the listener runs inside its emitter, on its executor",
    );
}

#[tokio::test]
async fn a_listener_held_for_the_commit_runs_on_what_its_driver_installed() {
    let (_app, bus, probe) = executor_booted().await;
    let transaction = Arc::new(OpenTransaction::default());

    nest_rs_database::with_job_executor(transaction.clone(), bus.emit(Recorded)).await;
    assert!(ran_on(&probe).is_empty(), "held until the commit");

    transaction.commit().await;
    assert_eq!(
        ran_on(&probe),
        [(Some("pool"), Some(nest_rs_database::ExecutorScope::Job))],
        "the driver's pool, under the scope the work was registered with",
    );
}

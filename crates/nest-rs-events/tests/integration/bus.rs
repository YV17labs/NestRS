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
            _ => Box::pin(nest_rs_database::with_executor(pool, work)),
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

/// Who ran, in order.
#[injectable]
#[derive(Default)]
struct Ran {
    seen: std::sync::Mutex<Vec<&'static str>>,
}

impl Ran {
    fn record(&self, who: &'static str) {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(who);
    }

    fn seen(&self) -> Vec<&'static str> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// The sentence a failed listener is reported with.
const LISTENER_FAILED: &str = "event listener failed — dispatch continues with the next listener";

/// The outcome each listener's `events.dispatch` line filed, in order.
fn dispatch_outcomes(logs: &nest_rs_testing::LogCapture) -> Vec<(String, String)> {
    logs.find(
        nest_rs_core::operation_log::TARGET,
        nest_rs_events::unit::DISPATCH.name(),
    )
    .into_iter()
    .map(|line| {
        (
            line.field("listener").unwrap_or_default(),
            line.field("outcome").unwrap_or_default(),
        )
    })
    .collect()
}

#[derive(Clone)]
struct Refused;

/// An answer spelled through an alias: read by its type, never its name.
type Outcome = Result<(), std::io::Error>;

#[injectable]
struct FallibleListeners {
    #[inject]
    ran: Arc<Ran>,
}

#[listeners]
impl FallibleListeners {
    #[on_event]
    async fn refuses(&self, _event: Refused) -> anyhow::Result<()> {
        use anyhow::Context as _;

        self.ran.record("refuses");
        Err(anyhow::anyhow!("the mailer refused the address"))
            .context("could not notify the author")
    }

    #[on_event]
    async fn carries_on(&self, _event: Refused) -> Outcome {
        self.ran.record("carries_on");
        Ok(())
    }
}

#[module(imports = [EventsModule], providers = [Ran, FallibleListeners])]
struct FallibleEventsModule;

#[tokio::test]
async fn a_listener_answering_an_error_is_filed_with_its_chain_and_the_next_one_runs() {
    let app = App::new::<FallibleEventsModule>().expect("boots");
    let bus = app.container().get::<EventBus>().expect("the bus");
    let ran = app.container().get::<Ran>().expect("the probe");
    let logs = nest_rs_testing::LogCapture::install();

    bus.emit(Refused).await;

    assert_eq!(ran.seen(), ["refuses", "carries_on"], "the chain went on");
    assert_eq!(
        dispatch_outcomes(&logs),
        [
            ("FallibleListeners::refuses".to_owned(), "error".to_owned()),
            ("FallibleListeners::carries_on".to_owned(), "ok".to_owned()),
        ],
    );
    let failed = logs.expect_one(nest_rs_events::TARGET, LISTENER_FAILED);
    assert_eq!(failed.level, "error");
    assert_eq!(
        failed.field("listener").as_deref(),
        Some("FallibleListeners::refuses")
    );
    assert!(
        failed
            .field("event")
            .is_some_and(|name| name.ends_with("Refused")),
        "{:?}",
        failed.fields,
    );
    assert_eq!(
        failed.field("error").as_deref(),
        Some("could not notify the author: the mailer refused the address"),
        "the whole chain, never its head alone",
    );
}

#[derive(Clone)]
struct Stalled;

#[injectable]
struct StallingListeners {
    #[inject]
    ran: Arc<Ran>,
}

#[listeners]
impl StallingListeners {
    #[on_event(timeout = "1s")]
    async fn stalls(&self, _event: Stalled) {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        self.ran.record("stalls ended");
    }

    #[on_event]
    async fn after_it(&self, _event: Stalled) {
        self.ran.record("after_it");
    }
}

#[module(imports = [EventsModule], providers = [Ran, StallingListeners])]
struct StallingEventsModule;

#[tokio::test(start_paused = true)]
async fn a_listener_running_past_its_timeout_is_cut_there_and_the_next_one_runs() {
    let app = App::new::<StallingEventsModule>().expect("boots");
    let bus = app.container().get::<EventBus>().expect("the bus");
    let ran = app.container().get::<Ran>().expect("the probe");
    let logs = nest_rs_testing::LogCapture::install();

    let emitted = tokio::time::Instant::now();
    bus.emit(Stalled).await;
    let took = emitted.elapsed();

    assert!(
        took >= std::time::Duration::from_secs(1) && took < std::time::Duration::from_secs(2),
        "cut at its 1 s timeout, not after its 5 s sleep: {took:?}",
    );
    assert_eq!(ran.seen(), ["after_it"], "the cut body never ended");
    assert_eq!(
        dispatch_outcomes(&logs),
        [
            ("StallingListeners::stalls".to_owned(), "error".to_owned()),
            ("StallingListeners::after_it".to_owned(), "ok".to_owned()),
        ],
    );
    let cut = logs.expect_one(nest_rs_events::TARGET, LISTENER_FAILED);
    assert_eq!(
        cut.field("listener").as_deref(),
        Some("StallingListeners::stalls")
    );
    assert_eq!(
        cut.field("error").as_deref(),
        Some(
            "the listener did not end within its 1s timeout, and was cut — \
             `#[on_event(timeout = …)]` sets it"
        ),
    );
}

/// How a unit the recording binding opened ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ended {
    /// Settled, told whether the unit succeeded.
    Settled { ok: bool },
    /// Dropped before it settled.
    Abandoned,
}

/// One unit the recording binding opened, as its spec declared it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Opened {
    unit: &'static str,
    policy: nest_rs_database::TransactionPolicy,
    scope: nest_rs_database::ExecutorScope,
    deadline: bool,
    ended: Option<Ended>,
}

type OpenedUnits = Arc<std::sync::Mutex<Vec<Opened>>>;

/// A unit-of-work binding recording every unit it opens and how each ended;
/// each unit's executor is an [`OpenTransaction`], holding the work an emit
/// hands it until the unit commits.
struct RecordingUnitOfWork {
    opened: OpenedUnits,
    /// Whether a listener's commit never answers.
    commit_hangs: bool,
}

impl RecordingUnitOfWork {
    /// The binding, and the units it will record.
    fn bound(commit_hangs: bool) -> (Arc<dyn nest_rs_database::UnitOfWork>, OpenedUnits) {
        let opened = OpenedUnits::default();
        let binding = Self {
            opened: Arc::clone(&opened),
            commit_hangs,
        };
        (Arc::new(binding), opened)
    }
}

/// The binding's settle net, past which a commit is in doubt.
const SETTLE_NET: std::time::Duration = std::time::Duration::from_secs(16);

impl nest_rs_database::UnitOfWork for RecordingUnitOfWork {
    fn open(
        &self,
        spec: &nest_rs_database::UnitSpec,
    ) -> Result<Box<dyn nest_rs_database::OpenUnit>, nest_rs_database::Unhonoured> {
        let mut opened = self
            .opened
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        opened.push(Opened {
            unit: spec.unit.name(),
            policy: spec.policy,
            scope: spec.scope,
            deadline: spec.deadline.is_some(),
            ended: None,
        });
        Ok(Box::new(RecordingUnit {
            opened: Arc::clone(&self.opened),
            at: opened.len() - 1,
            executor: Arc::new(OpenTransaction::default()),
            commit_hangs: self.commit_hangs && spec.unit == nest_rs_events::unit::DISPATCH,
            settled: false,
        }))
    }

    fn settle_net(&self) -> std::time::Duration {
        SETTLE_NET
    }
}

struct RecordingUnit {
    opened: OpenedUnits,
    at: usize,
    executor: Arc<OpenTransaction>,
    commit_hangs: bool,
    settled: bool,
}

impl RecordingUnit {
    fn end(&self, ended: Ended) {
        self.opened
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[self.at]
            .ended = Some(ended);
    }
}

impl nest_rs_database::OpenUnit for RecordingUnit {
    fn executor(&self) -> Arc<dyn nest_rs_database::Executor> {
        Arc::clone(&self.executor) as Arc<dyn nest_rs_database::Executor>
    }

    fn settle(
        mut self: Box<Self>,
        ok: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = nest_rs_database::Settlement> + Send>>
    {
        self.settled = true;
        self.end(Ended::Settled { ok });
        let executor = Arc::clone(&self.executor);
        let hangs = ok && self.commit_hangs;
        Box::pin(async move {
            if hangs {
                std::future::pending::<()>().await;
            }
            if ok {
                executor.commit().await;
            } else {
                executor.roll_back();
            }
            nest_rs_database::Settlement::Settled
        })
    }
}

impl Drop for RecordingUnit {
    fn drop(&mut self) {
        if !self.settled {
            self.end(Ended::Abandoned);
        }
    }
}

/// A queue attempt's unit, the emitter's here.
const EMITTER: nest_rs_core::operation_log::Unit = nest_rs_core::__private::declare_unit(
    "nest-rs-queue",
    "nest_rs::queue",
    nest_rs_core::operation_log::Kind::Consumer,
    "queue.job",
);

/// Emit `event` inside a unit of `binding`, as a queue attempt would: held
/// until the attempt commits.
async fn emitted_in_a_unit<E: Clone + Send + 'static>(
    binding: &Arc<dyn nest_rs_database::UnitOfWork>,
    bus: &EventBus,
    event: E,
) {
    let spec = nest_rs_database::UnitSpec::new(
        EMITTER,
        nest_rs_database::ExecutorScope::Job,
        nest_rs_database::TransactionPolicy::PerUnit,
    );
    nest_rs_database::run_unit(
        Some(binding),
        spec,
        async {
            bus.emit(event).await;
            Ok::<(), ()>(())
        },
        Result::is_ok,
        |_| Err(()),
    )
    .await
    .expect("the emitter's unit settles");
}

/// The listener units `opened` holds, in the order they opened.
fn listener_units(opened: &OpenedUnits) -> Vec<Opened> {
    opened
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter(|unit| unit.unit == nest_rs_events::unit::DISPATCH.name())
        .copied()
        .collect()
}

#[derive(Clone)]
struct Settling;

#[injectable]
struct SettlingListeners {
    #[inject]
    ran: Arc<Ran>,
}

#[listeners]
impl SettlingListeners {
    #[on_event]
    async fn commits(&self, _event: Settling) -> anyhow::Result<()> {
        self.ran.record("commits");
        Ok(())
    }

    #[on_event]
    async fn fails(&self, _event: Settling) -> anyhow::Result<()> {
        self.ran.record("fails");
        Err(anyhow::anyhow!("the reaction refused"))
    }

    #[on_event(transactional = false)]
    async fn on_the_pool(&self, _event: Settling) -> anyhow::Result<()> {
        self.ran.record("on_the_pool");
        Err(anyhow::anyhow!("the reaction refused on the pool"))
    }
}

#[module(imports = [EventsModule], providers = [Ran, SettlingListeners])]
struct SettlingEventsModule;

#[tokio::test]
async fn each_listener_held_for_its_emitters_commit_settles_a_unit_of_its_own_on_its_answer() {
    let app = App::new::<SettlingEventsModule>().expect("boots");
    let bus = app.container().get::<EventBus>().expect("the bus");
    let ran = app.container().get::<Ran>().expect("the probe");
    let (binding, opened) = RecordingUnitOfWork::bound(false);

    emitted_in_a_unit(&binding, &bus, Settling).await;

    assert_eq!(ran.seen(), ["commits", "fails", "on_the_pool"]);
    let unit = |policy, ok| Opened {
        unit: nest_rs_events::unit::DISPATCH.name(),
        policy,
        scope: nest_rs_database::ExecutorScope::Job,
        deadline: true,
        ended: Some(Ended::Settled { ok }),
    };
    assert_eq!(
        listener_units(&opened),
        [
            unit(nest_rs_database::TransactionPolicy::PerUnit, true),
            unit(nest_rs_database::TransactionPolicy::PerUnit, false),
            unit(nest_rs_database::TransactionPolicy::Pool, false),
        ],
        "one unit per listener, opened by the emitter's binding under its scope, \
         after its commit: {opened:#?}",
    );
}

#[injectable]
struct CutInAUnitListeners {
    #[inject]
    ran: Arc<Ran>,
}

#[derive(Clone)]
struct StalledInAUnit;

#[listeners]
impl CutInAUnitListeners {
    #[on_event(timeout = "1s")]
    async fn stalls(&self, _event: StalledInAUnit) -> anyhow::Result<()> {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        self.ran.record("stalls ended");
        Ok(())
    }

    #[on_event]
    async fn after_it(&self, _event: StalledInAUnit) {
        self.ran.record("after_it");
    }
}

#[module(imports = [EventsModule], providers = [Ran, CutInAUnitListeners])]
struct CutInAUnitEventsModule;

/// The deadline cuts the listener's body inside its unit, never the unit
/// around it, which would drop it unsettled.
#[tokio::test(start_paused = true)]
async fn a_listener_cut_at_its_timeout_settles_its_unit_as_a_failure() {
    let app = App::new::<CutInAUnitEventsModule>().expect("boots");
    let bus = app.container().get::<EventBus>().expect("the bus");
    let ran = app.container().get::<Ran>().expect("the probe");
    let (binding, opened) = RecordingUnitOfWork::bound(false);
    let logs = nest_rs_testing::LogCapture::install();

    let emitted = tokio::time::Instant::now();
    emitted_in_a_unit(&binding, &bus, StalledInAUnit).await;
    let took = emitted.elapsed();

    assert!(
        took >= std::time::Duration::from_secs(1) && took < std::time::Duration::from_secs(2),
        "{took:?}"
    );
    assert_eq!(ran.seen(), ["after_it"]);
    let ended: Vec<Option<Ended>> = listener_units(&opened)
        .iter()
        .map(|unit| unit.ended)
        .collect();
    assert_eq!(
        ended,
        [
            Some(Ended::Settled { ok: false }),
            Some(Ended::Settled { ok: true })
        ],
        "the cut listener's unit rolled back rather than dropping unsettled",
    );
    let cut = logs.expect_one(nest_rs_events::TARGET, LISTENER_FAILED);
    assert!(
        cut.field("error").is_some_and(
            |error| error.starts_with("the listener did not end within its 1s timeout")
        ),
        "a cut whose rollback answered stays a timeout: {:?}",
        cut.fields,
    );
}

#[derive(Clone)]
struct Queued;

#[injectable]
struct QueuedListeners {
    #[inject]
    ran: Arc<Ran>,
}

/// The second listener's 2 s would be spent before it starts, were it counted
/// from the dispatch rather than from its own call.
#[listeners]
impl QueuedListeners {
    #[on_event(timeout = "10s")]
    async fn slow(&self, _event: Queued) -> anyhow::Result<()> {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        self.ran.record("slow");
        Ok(())
    }

    #[on_event(timeout = "2s")]
    async fn behind_it(&self, _event: Queued) -> anyhow::Result<()> {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        self.ran.record("behind_it");
        Ok(())
    }
}

#[module(imports = [EventsModule], providers = [Ran, QueuedListeners])]
struct QueuedEventsModule;

/// Both listeners ran to their end and filed `ok`.
fn assert_both_ended(ran: &Ran, logs: &nest_rs_testing::LogCapture) {
    assert_eq!(ran.seen(), ["slow", "behind_it"], "neither was cut");
    assert_eq!(
        dispatch_outcomes(logs),
        [
            ("QueuedListeners::slow".to_owned(), "ok".to_owned()),
            ("QueuedListeners::behind_it".to_owned(), "ok".to_owned()),
        ],
    );
}

#[tokio::test(start_paused = true)]
async fn a_listener_queued_behind_a_slow_sibling_is_not_charged_for_it() {
    let app = App::new::<QueuedEventsModule>().expect("boots");
    let bus = app.container().get::<EventBus>().expect("the bus");
    let ran = app.container().get::<Ran>().expect("the probe");
    let logs = nest_rs_testing::LogCapture::install();

    bus.emit(Queued).await;

    assert_both_ended(&ran, &logs);
}

#[tokio::test(start_paused = true)]
async fn a_listener_unit_queued_behind_a_slow_sibling_is_not_charged_for_it() {
    let app = App::new::<QueuedEventsModule>().expect("boots");
    let bus = app.container().get::<EventBus>().expect("the bus");
    let ran = app.container().get::<Ran>().expect("the probe");
    let (binding, opened) = RecordingUnitOfWork::bound(false);
    let logs = nest_rs_testing::LogCapture::install();

    emitted_in_a_unit(&binding, &bus, Queued).await;

    assert_both_ended(&ran, &logs);
    assert_eq!(
        listener_units(&opened)
            .iter()
            .map(|unit| unit.ended)
            .collect::<Vec<_>>(),
        [
            Some(Ended::Settled { ok: true }),
            Some(Ended::Settled { ok: true })
        ],
        "each unit's deadline is its own call's: {opened:#?}",
    );
}

#[derive(Clone)]
struct CommitHangs;

#[injectable]
struct HangingCommitListeners {
    #[inject]
    ran: Arc<Ran>,
}

#[listeners]
impl HangingCommitListeners {
    #[on_event(timeout = "1s")]
    async fn answers_at_once(&self, _event: CommitHangs) -> anyhow::Result<()> {
        self.ran.record("answers_at_once");
        Ok(())
    }
}

#[module(imports = [EventsModule], providers = [Ran, HangingCommitListeners])]
struct HangingCommitEventsModule;

#[tokio::test(start_paused = true)]
async fn a_listener_whose_commit_never_answers_is_in_doubt_past_its_settle_net() {
    let app = App::new::<HangingCommitEventsModule>().expect("boots");
    let bus = app.container().get::<EventBus>().expect("the bus");
    let (binding, opened) = RecordingUnitOfWork::bound(true);
    let logs = nest_rs_testing::LogCapture::install();

    let emitted = tokio::time::Instant::now();
    emitted_in_a_unit(&binding, &bus, CommitHangs).await;
    let took = emitted.elapsed();

    let bound = std::time::Duration::from_secs(1) + SETTLE_NET;
    assert!(
        took >= bound && took < bound + std::time::Duration::from_secs(1),
        "answered at its deadline plus the binding's settle net: {took:?}",
    );
    assert_eq!(
        listener_units(&opened)
            .iter()
            .map(|unit| unit.ended)
            .collect::<Vec<_>>(),
        [Some(Ended::Settled { ok: true })],
        "its body answered, so it was committed",
    );
    let doubt = logs.expect_one(nest_rs_events::TARGET, LISTENER_FAILED);
    assert_eq!(
        doubt.field("error").as_deref(),
        Some(nest_rs_database::Unhonoured::IN_DOUBT.reason),
        "a commit that never answered may have landed",
    );
}

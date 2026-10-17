//! Covers `src/app.rs` — how the serve loop reports the transports it runs:
//! the way down they bound, and a transport that stops — and the test
//! overrides: one replaces its provider before anything is built from it, and
//! one nothing in the graph would register fails the boot.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::anyhow;
use nest_rs_core::panic::{FIELD, LOCATION_FIELD};
use nest_rs_core::target;
use nest_rs_core::{
    AccessGraphError, App, Container, ContainerBuilder, Discoverable, DuplicateProviderError,
    InertHost, Module, OverriddenDependencyError, Registering, RequestScope,
    SHUTDOWN_HOOKS_TIMEOUT, ScopeViolationError, Transport, UnclaimedOverrideError, hooks,
    injectable, module,
};
use nest_rs_testing::LogCapture;
use tokio_util::sync::CancellationToken;

use crate::Failing;

struct FailingModule;

impl Module for FailingModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_transport(|_| Ok(Failing))
    }
}

struct Panicking;

#[async_trait::async_trait]
impl Transport for Panicking {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn serve(self: Box<Self>, _cancel: CancellationToken) -> anyhow::Result<()> {
        let _: u64 = serde_json::from_str(r#""sk_live_51HsecretTOKEN""#).unwrap();
        Ok(())
    }

    fn stop_bound(&self) -> Duration {
        Duration::ZERO
    }
}

struct PanickingModule;

impl Module for PanickingModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_transport(|_| Ok(Panicking))
    }
}

#[tokio::test]
async fn a_transport_that_returns_an_error_is_named_before_the_shutdown() {
    let logs = LogCapture::install();
    let err = App::new::<FailingModule>()
        .expect("the module boots")
        .run()
        .await
        .expect_err("a failed transport is the app's error");
    assert!(err.to_string().contains("listener stopped"), "{err}");

    let event = logs.expect_one(target::APP, "transport failed; shutting down");
    assert_eq!(event.level, "error");
    assert!(
        event
            .field("error")
            .is_some_and(|e| e.contains("listener stopped")),
        "the event carries the transport's own error, got {:?}",
        event.fields,
    );
}

/// A transport that serves until it is asked to stop, then fails its stop.
struct FailsItsStop;

#[async_trait::async_trait]
impl Transport for FailsItsStop {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn serve(self: Box<Self>, cancel: CancellationToken) -> anyhow::Result<()> {
        cancel.cancelled().await;
        Err(anyhow!("the open connections did not close"))
    }

    fn stop_bound(&self) -> Duration {
        Duration::ZERO
    }
}

struct FailsTwiceModule;

impl Module for FailsTwiceModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
            .provide_transport(|_| Ok(FailsItsStop))
            .provide_transport(|_| Ok(Failing))
    }
}

#[tokio::test]
async fn each_transport_that_fails_is_named_and_the_first_failure_is_the_app_s_error() {
    let logs = LogCapture::install();
    let err = App::new::<FailsTwiceModule>()
        .expect("the module boots")
        .run()
        .await
        .expect_err("a failed transport is the app's error");
    assert!(
        err.to_string().contains("listener stopped"),
        "the first failure: {err}"
    );

    let mut failed: Vec<(Option<String>, Option<String>)> = logs
        .find(target::APP, "transport failed; shutting down")
        .into_iter()
        .map(|event| {
            assert_eq!(event.level, "error");
            (event.field("transport"), event.field("error"))
        })
        .collect();
    failed.sort();
    assert_eq!(
        failed,
        [
            (
                Some("Failing".to_owned()),
                Some("the listener stopped accepting".to_owned())
            ),
            (
                Some("FailsItsStop".to_owned()),
                Some("the open connections did not close".to_owned())
            ),
        ],
        "each failure is said, naming its transport"
    );
}

#[nest_rs_core::main]
async fn serve_a_transport_that_panics() -> anyhow::Result<()> {
    App::new::<PanickingModule>()?.run().await
}

/// Under the process hook, so a second record of the panic would show.
#[test]
fn a_transport_that_panics_is_one_line_without_its_value_and_the_app_s_error_names_it() {
    let logs = LogCapture::install_global();

    let served = serve_a_transport_that_panics();
    crate::rust_panic_hook();

    let err = served.expect_err("a panicked transport fails the app");

    assert_eq!(err.to_string(), "transport `Panicking` panicked");
    assert!(!format!("{err:?}").contains("sk_live"), "{err:?}");
    let events = logs.events();
    for event in &events {
        assert!(
            event
                .fields
                .values()
                .all(|value| !value.contains("sk_live")),
            "the payload's value reached a line: {event:#?}"
        );
    }
    let errors = logs.events_at(tracing::Level::ERROR);
    assert_eq!(errors.len(), 1, "the panic is said once: {errors:#?}");
    let line = &errors[0];
    assert_eq!(
        (line.target.as_str(), line.message.as_str()),
        (target::APP, "transport task panicked; shutting down")
    );
    assert_eq!(line.field("transport").as_deref(), Some("Panicking"));
    assert_eq!(
        line.field(FIELD).as_deref(),
        Some(
            "called `Result::unwrap()` on an `Err` value: Error(\"invalid type: a string, \
             expected u64\", line: 1, column: 24)"
        )
    );
    let location = line.field(LOCATION_FIELD).unwrap_or_default();
    assert!(
        location.contains("tests/integration/app.rs:"),
        "where it panicked: {line:#?}"
    );
}

/// States a stop bound of `MS` milliseconds: one type per bound.
struct Stated<const MS: u64>;

#[async_trait::async_trait]
impl<const MS: u64> Transport for Stated<MS> {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn serve(self: Box<Self>, _cancel: CancellationToken) -> anyhow::Result<()> {
        Ok(())
    }

    fn stop_bound(&self) -> Duration {
        Duration::from_millis(MS)
    }
}

struct TwoBoundsModule;

impl Module for TwoBoundsModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
            .provide_transport(|_| Ok(Stated::<2_500>))
            .provide_transport(|_| Ok(Stated::<41_000>))
    }
}

#[tokio::test]
async fn the_boot_line_files_the_longest_stop_bound_beside_the_hooks_budget() {
    let logs = LogCapture::install();
    App::new::<TwoBoundsModule>()
        .expect("the module boots")
        .run()
        .await
        .expect_err("both transports end on their own");

    let event = logs.expect_one(target::APP, "way down bounded");
    assert_eq!(event.level, "info");
    assert_eq!(event.field("stop_bound_ms").as_deref(), Some("41000"));
    assert_eq!(
        event.field("hooks_budget_ms"),
        Some(SHUTDOWN_HOOKS_TIMEOUT.as_millis().to_string()),
    );
}

struct Pool;

#[tokio::test]
async fn a_factory_that_fails_is_named_and_its_cause_kept_in_the_chain() {
    let Err(refused) = App::builder()
        .provide_factory::<Pool, _, _>(|_| async {
            Err(anyhow!("connection refused by the database host"))
        })
        .build()
        .await
    else {
        panic!("a failing factory fails the boot");
    };

    let failed = refused
        .downcast_ref::<nest_rs_core::FactoryFailedError>()
        .unwrap_or_else(|| panic!("not the factory refusal: {refused:#}"));
    assert_eq!(failed.type_name, std::any::type_name::<Pool>());
    assert_eq!(
        refused.to_string(),
        format!("the factory for `{}` failed", std::any::type_name::<Pool>()),
    );
    assert!(
        refused
            .chain()
            .any(|cause| cause.to_string() == "connection refused by the database host"),
        "{refused:#}"
    );
    assert_eq!(
        nest_rs_core::error_message(refused.as_ref()),
        format!(
            "the factory for `{}` failed: connection refused by the database host",
            std::any::type_name::<Pool>()
        ),
    );
}

trait Mailer: Send + Sync {
    fn sender(&self) -> &'static str;
}

#[injectable]
#[derive(Default)]
struct SmtpMailer;

impl Mailer for SmtpMailer {
    fn sender(&self) -> &'static str {
        "smtp"
    }
}

struct RecordingMailer;

impl Mailer for RecordingMailer {
    fn sender(&self) -> &'static str {
        "recording"
    }
}

#[injectable]
struct UsersService {
    #[inject]
    mailer: Arc<dyn Mailer>,
}

#[module(providers = [SmtpMailer as dyn Mailer, UsersService])]
struct UsersModule;

fn refused(built: anyhow::Result<App>) -> anyhow::Error {
    match built {
        Ok(_) => panic!("the boot passed"),
        Err(error) => error,
    }
}

#[tokio::test]
async fn an_eager_singleton_injects_the_double_of_the_trait_object_it_holds() {
    let app = App::builder()
        .module::<UsersModule>()
        .override_dyn::<dyn Mailer>(Arc::new(RecordingMailer))
        .build()
        .await
        .expect("the override boots");
    let users = app.container().get::<UsersService>().expect("built");
    assert_eq!(users.mailer.sender(), "recording");
}

struct Unprovided;

#[injectable]
struct Reports {
    #[inject]
    _source: Arc<Unprovided>,
    label: &'static str,
}

#[module(providers = [Reports])]
struct ReportsModule;

#[tokio::test]
async fn an_overridden_provider_is_never_built_so_its_dependencies_need_not_exist() {
    let app = App::builder()
        .module::<ReportsModule>()
        .override_value(Reports {
            _source: Arc::new(Unprovided),
            label: "double",
        })
        .build()
        .await
        .expect("the double stands in for a provider whose dependency nothing provides");
    assert_eq!(
        app.container().get::<Reports>().map(|r| r.label),
        Some("double")
    );
}

static REAL_LEDGERS_BUILT: AtomicUsize = AtomicUsize::new(0);

#[injectable]
struct Ledger {
    real: bool,
}

impl Default for Ledger {
    fn default() -> Self {
        REAL_LEDGERS_BUILT.fetch_add(1, Ordering::SeqCst);
        Self { real: true }
    }
}

#[module(providers = [Ledger])]
struct LedgerModule;

#[tokio::test]
async fn the_real_constructor_of_an_overridden_provider_never_runs() {
    let app = App::builder()
        .module::<LedgerModule>()
        .override_arc(Arc::new(Ledger { real: false }))
        .build()
        .await
        .expect("boots");
    assert_eq!(app.container().get::<Ledger>().map(|l| l.real), Some(false));
    assert_eq!(REAL_LEDGERS_BUILT.load(Ordering::SeqCst), 0);
}

struct NotProvided;

#[tokio::test]
async fn an_override_of_a_type_nothing_provides_fails_the_boot_naming_it() {
    let refused = refused(
        App::builder()
            .module::<UsersModule>()
            .override_value(NotProvided)
            .build()
            .await,
    );
    let unclaimed = refused
        .downcast_ref::<UnclaimedOverrideError>()
        .unwrap_or_else(|| panic!("not the unclaimed override: {refused:#}"));
    assert_eq!(unclaimed.type_name, std::any::type_name::<NotProvided>());
    assert!(
        refused
            .to_string()
            .contains("so the double would never be read"),
        "{refused}"
    );
}

/// What an edge host is to the container: metadata attached under its type,
/// never a value.
struct Mounted;

impl Discoverable for Mounted {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<Self, &'static str>("mounted at /")
    }
}

#[module(providers = [Mounted])]
struct MountedModule;

#[tokio::test]
async fn an_override_of_a_provider_that_registers_metadata_alone_is_unclaimed() {
    let refused = refused(
        App::builder()
            .module::<MountedModule>()
            .override_value(Mounted)
            .build()
            .await,
    );
    assert!(
        refused.downcast_ref::<UnclaimedOverrideError>().is_some(),
        "{refused:#}"
    );
}

#[injectable]
#[derive(Default)]
struct Archive;

#[module(providers = [Archive])]
struct ArchiveModule;

#[injectable]
struct Archivist {
    #[inject]
    _archive: Arc<Archive>,
}

#[module(providers = [Archivist])]
struct ArchivistModule;

#[module(imports = [ArchiveModule, ArchivistModule])]
struct LeakRoot;

#[tokio::test]
async fn an_override_keeps_its_owner_so_a_consumer_outside_the_closure_is_refused() {
    let refused = refused(
        App::builder()
            .module::<LeakRoot>()
            .override_value(Archive)
            .build()
            .await,
    );
    let leak = refused
        .downcast_ref::<AccessGraphError>()
        .unwrap_or_else(|| panic!("not the access violation: {refused:#}"));
    assert_eq!(leak.consumer, "Archivist");
    assert_eq!(leak.owner, "ArchiveModule");
}

#[injectable(scope = request)]
struct Visit {
    real: bool,
}

impl Default for Visit {
    fn default() -> Self {
        Self { real: true }
    }
}

#[module(providers = [Visit])]
struct VisitModule;

#[tokio::test]
async fn an_override_of_a_request_scoped_provider_serves_every_request() {
    let app = App::builder()
        .module::<VisitModule>()
        .override_value(Visit { real: false })
        .build()
        .await
        .expect("boots");
    let scope = RequestScope::new(app.container().clone());
    assert_eq!(scope.get::<Visit>().map(|v| v.real), Some(false));
}

#[injectable]
struct Receptionist {
    #[inject]
    _visit: Arc<Visit>,
}

#[module(providers = [Visit, Receptionist])]
struct FrontDeskModule;

#[tokio::test]
async fn a_singleton_injecting_an_overridden_request_scoped_provider_is_refused_as_in_production() {
    let refused = refused(
        App::builder()
            .module::<FrontDeskModule>()
            .override_value(Visit { real: false })
            .build()
            .await,
    );
    let violation = refused
        .downcast_ref::<ScopeViolationError>()
        .unwrap_or_else(|| panic!("not the scope violation: {refused:#}"));
    assert_eq!(violation.consumer, "Receptionist");
}

static HOST_INITS: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

trait Clock: Send + Sync {}

#[injectable]
struct Warmup {
    name: &'static str,
}

impl Default for Warmup {
    fn default() -> Self {
        Self { name: "real" }
    }
}

impl Clock for Warmup {}

#[hooks]
impl Warmup {
    #[on_module_init]
    async fn warm(&self) {
        HOST_INITS
            .lock()
            .expect("log mutex is not poisoned")
            .push(self.name);
    }
}

#[module(providers = [Warmup])]
struct WarmupModule;

#[injectable]
#[derive(Default)]
struct Sundial;

impl Clock for Sundial {}

static SUNDIAL_INITS: AtomicUsize = AtomicUsize::new(0);

#[hooks]
impl Sundial {
    #[on_module_init]
    async fn warm(&self) {
        SUNDIAL_INITS.fetch_add(1, Ordering::SeqCst);
    }
}

#[module(providers = [Sundial as dyn Clock])]
struct SundialModule;

struct Hourglass;

impl Clock for Hourglass {}

#[tokio::test]
async fn a_host_overridden_by_value_runs_its_hooks_on_the_substitute_once() {
    let app = App::builder()
        .module::<WarmupModule>()
        .override_value(Warmup { name: "double" })
        .build()
        .await
        .expect("boots");
    app.init().await.expect("the init phases drain");
    assert_eq!(
        *HOST_INITS.lock().expect("log mutex is not poisoned"),
        ["double"]
    );
}

#[tokio::test]
async fn a_host_overridden_through_its_trait_object_runs_no_hook_and_says_so_at_debug() {
    let logs = LogCapture::install();
    let app = App::builder()
        .module::<SundialModule>()
        .override_dyn::<dyn Clock>(Arc::new(Hourglass))
        .build()
        .await
        .expect("boots");
    app.init().await.expect("the init phases drain");
    assert_eq!(SUNDIAL_INITS.load(Ordering::SeqCst), 0);
    let inert = logs
        .events()
        .into_iter()
        .find(|event| {
            event.target == target::LIFECYCLE
                && event.field("cause").as_deref() == Some(InertHost::Overridden.cause())
        })
        .unwrap_or_else(|| {
            panic!(
                "no inert line for the overridden host: {:#?}",
                logs.events()
            )
        });
    assert_eq!(inert.level, "debug");
    assert_eq!(inert.field("provider").as_deref(), Some("Sundial"));
    assert!(
        logs.events()
            .iter()
            .all(|event| event.level != "warn"
                || event.field("provider").as_deref() != Some("Sundial")),
        "{:#?}",
        logs.events()
    );
}

#[injectable]
struct Calendar {
    #[inject]
    _sundial: Arc<Sundial>,
}

#[module(imports = [SundialModule], providers = [Calendar])]
struct CalendarModule;

#[tokio::test]
async fn a_consumer_of_a_key_an_override_removed_is_told_to_override_what_it_injects() {
    let refused = refused(
        App::builder()
            .module::<CalendarModule>()
            .override_dyn::<dyn Clock>(Arc::new(Hourglass))
            .build()
            .await,
    );
    let removed = refused
        .downcast_ref::<OverriddenDependencyError>()
        .unwrap_or_else(|| panic!("not the removed dependency: {refused:#}"));
    assert_eq!(removed.consumer, "Calendar");
    assert!(
        refused.to_string().contains(&format!(
            "`Sundial` is overridden through `{}`: override the key this consumer injects",
            std::any::type_name::<Arc<dyn Clock>>()
        )),
        "{refused}"
    );
}

static BUCKETS_OPENED: AtomicUsize = AtomicUsize::new(0);
static BUCKETS_CLOSED: AtomicUsize = AtomicUsize::new(0);

struct Bucket {
    real: bool,
}

struct BucketModule;

impl Module for BucketModule {
    fn collect(builder: ContainerBuilder, _: nest_rs_core::Collecting<Self>) -> ContainerBuilder {
        builder
            .provide_factory::<Bucket, _, _>(|_| async {
                BUCKETS_OPENED.fetch_add(1, Ordering::SeqCst);
                Ok(Bucket { real: true })
            })
            .provide_close::<Bucket, _, _>(|_| async {
                BUCKETS_CLOSED.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }
}

#[tokio::test]
async fn an_override_of_a_factory_output_skips_the_factory_and_is_never_closed() {
    let app = App::builder()
        .module::<BucketModule>()
        .override_value(Bucket { real: false })
        .build()
        .await
        .expect("boots");
    assert_eq!(app.container().get::<Bucket>().map(|p| p.real), Some(false));
    app.close().await;
    assert_eq!(BUCKETS_OPENED.load(Ordering::SeqCst), 0);
    assert_eq!(
        BUCKETS_CLOSED.load(Ordering::SeqCst),
        0,
        "the double is the test's, never the module's to close"
    );
}

struct Region(&'static str);

#[tokio::test]
async fn an_override_of_a_seed_replaces_it_whatever_the_call_order() {
    let app = App::builder()
        .override_value(Region("override"))
        .provide(Region("seed"))
        .module::<UsersModule>()
        .build()
        .await
        .expect("an override of a seed is no duplicate");
    assert_eq!(
        app.container().get::<Region>().map(|r| r.0),
        Some("override")
    );
}

#[injectable]
struct Gazette {
    #[inject]
    edition: Option<Arc<Edition>>,
}

struct Edition;

#[module(providers = [Gazette])]
struct GazetteModule;

/// Registers `Edition` by hand, after `GazetteModule` built its providers.
struct EditionModule;

impl Module for EditionModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide(Edition)
    }
}

#[tokio::test]
async fn an_optional_dependency_an_override_supplies_is_present_when_its_consumer_is_built() {
    let app = App::builder()
        .module::<GazetteModule>()
        .module::<EditionModule>()
        .override_arc(Arc::new(Edition))
        .build()
        .await
        .expect("the override is present before any provider is built");
    let gazette = app.container().get::<Gazette>().expect("built");
    assert!(gazette.edition.is_some());
}

trait Postal: Send + Sync {}

#[injectable]
struct Courier {
    #[inject]
    _post: Arc<dyn Postal>,
}

#[injectable]
struct PostOffice {
    #[inject]
    _courier: Arc<Courier>,
}

impl Postal for PostOffice {}

struct Pigeon;

impl Postal for Pigeon {}

#[module(imports = [PostOfficeModule], providers = [Courier])]
struct CourierModule;

#[module(imports = [CourierModule], providers = [PostOffice as dyn Postal])]
struct PostOfficeModule;

#[tokio::test]
async fn an_override_standing_in_for_one_member_of_a_cross_module_cycle_breaks_it() {
    let app = App::builder()
        .module::<CourierModule>()
        .override_dyn::<dyn Postal>(Arc::new(Pigeon))
        .build()
        .await
        .unwrap_or_else(|refused| panic!("the cycle stands: {refused:#}"));
    assert!(app.container().get::<Courier>().is_some());
}

static DEPOT_INITS: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

#[injectable]
struct Sorter {
    #[inject]
    _depot: Arc<Depot>,
}

#[hooks]
impl Sorter {
    #[on_module_init]
    async fn open(&self) {
        DEPOT_INITS
            .lock()
            .expect("log mutex is not poisoned")
            .push("sorter");
    }
}

#[injectable]
struct Depot {
    #[inject]
    _sorter: Option<Arc<Sorter>>,
}

#[hooks]
impl Depot {
    #[on_module_init]
    async fn open(&self) {
        DEPOT_INITS
            .lock()
            .expect("log mutex is not poisoned")
            .push("depot");
    }
}

#[module(providers = [Sorter, Depot])]
struct DepotModule;

#[tokio::test]
async fn an_override_breaking_a_cycle_inits_its_double_before_what_injects_it() {
    let app = App::builder()
        .module::<DepotModule>()
        .override_value(Depot { _sorter: None })
        .build()
        .await
        .unwrap_or_else(|refused| panic!("the cycle stands: {refused:#}"));
    app.init().await.expect("the init phases drain");
    assert_eq!(
        *DEPOT_INITS.lock().expect("log mutex is not poisoned"),
        ["depot", "sorter"]
    );
}

fn duplicated(built: anyhow::Result<App>) -> &'static str {
    let refused = refused(built);
    refused
        .downcast_ref::<DuplicateProviderError>()
        .unwrap_or_else(|| panic!("not the duplicate: {refused:#}"))
        .type_name
}

#[injectable]
#[derive(Default)]
struct Twice;

#[module(providers = [Twice])]
struct FirstTwiceModule;

#[module(providers = [Twice])]
struct SecondTwiceModule;

#[module(imports = [FirstTwiceModule, SecondTwiceModule])]
struct TwiceRoot;

#[tokio::test]
async fn a_type_two_modules_list_is_a_duplicate_with_an_override_as_without() {
    let duplicated = duplicated(
        App::builder()
            .module::<TwiceRoot>()
            .override_value(Twice)
            .build()
            .await,
    );
    assert_eq!(duplicated, std::any::type_name::<Twice>());
}

trait Port: Send + Sync {}

#[injectable]
#[derive(Default)]
struct LeftPort;

impl Port for LeftPort {}

#[injectable]
#[derive(Default)]
struct RightPort;

impl Port for RightPort {}

struct FakePort;

impl Port for FakePort {}

#[module(providers = [LeftPort as dyn Port])]
struct LeftPortModule;

#[module(providers = [RightPort as dyn Port])]
struct RightPortModule;

#[module(imports = [LeftPortModule, RightPortModule])]
struct PortRoot;

#[tokio::test]
async fn a_trait_object_two_modules_bind_is_a_duplicate_with_an_override_as_without() {
    let duplicated = duplicated(
        App::builder()
            .module::<PortRoot>()
            .override_dyn::<dyn Port>(Arc::new(FakePort))
            .build()
            .await,
    );
    assert_eq!(duplicated, std::any::type_name::<Arc<dyn Port>>());
}

#[module(providers = [LeftPort])]
struct BareLeftPortModule;

#[module(imports = [LeftPortModule, BareLeftPortModule])]
struct SplitPortRoot;

#[tokio::test]
async fn a_key_an_override_removed_listed_again_elsewhere_is_still_a_duplicate() {
    let duplicated = duplicated(
        App::builder()
            .module::<SplitPortRoot>()
            .override_dyn::<dyn Port>(Arc::new(FakePort))
            .build()
            .await,
    );
    assert_eq!(duplicated, std::any::type_name::<LeftPort>());
}

/// What the boot of a [`ListenFailsModule`] app ran, in order.
static LISTEN_BOOT: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());

fn listen_boot(step: &'static str) {
    LISTEN_BOOT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(step);
}

/// Listens on `127.0.0.1:4000` and records that it served.
struct ListensThenServes;

#[async_trait::async_trait]
impl Transport for ListensThenServes {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn listen(&mut self) -> anyhow::Result<nest_rs_core::Listening> {
        listen_boot("listened");
        Ok(nest_rs_core::Listening::on([std::net::SocketAddr::from((
            [127, 0, 0, 1],
            4000,
        ))]))
    }

    async fn serve(self: Box<Self>, cancel: CancellationToken) -> anyhow::Result<()> {
        listen_boot("served");
        cancel.cancelled().await;
        Ok(())
    }

    fn stop_bound(&self) -> Duration {
        Duration::ZERO
    }
}

/// Fails its `listen`, as a transport whose port is taken does.
struct RefusesToListen;

#[async_trait::async_trait]
impl Transport for RefusesToListen {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn listen(&mut self) -> anyhow::Result<nest_rs_core::Listening> {
        Err(anyhow!("the address is already in use"))
    }

    async fn serve(self: Box<Self>, _cancel: CancellationToken) -> anyhow::Result<()> {
        listen_boot("the refused transport served");
        Ok(())
    }

    fn stop_bound(&self) -> Duration {
        Duration::ZERO
    }
}

#[nest_rs_core::injectable]
#[derive(Default)]
struct WarmCache;

#[nest_rs_core::hooks]
impl WarmCache {
    #[on_module_init]
    async fn warm(&self) {
        listen_boot("init hook");
    }

    #[on_module_destroy]
    async fn evict(&self) {
        listen_boot("shutdown hook");
    }
}

#[nest_rs_core::module(imports = [ListensThenRefusesModule], providers = [WarmCache])]
struct ListenFailsModule;

struct ListensThenRefusesModule;

impl Module for ListensThenRefusesModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
            .provide_transport(|_| Ok(ListensThenServes))
            .provide_transport(|_| Ok(RefusesToListen))
    }
}

/// Every transport listens before any serves: one whose `listen` fails fails
/// the boot naming the step, no transport serves, and the init hooks that ran
/// are torn down.
#[tokio::test]
async fn a_transport_that_fails_to_listen_fails_the_boot_before_any_transport_serves() {
    let logs = LogCapture::install();
    let app = App::new::<ListenFailsModule>().expect("the module boots");
    let lifecycle = std::sync::Arc::clone(app.lifecycle());

    let refused = tokio::time::timeout(Duration::from_secs(5), app.run())
        .await
        .expect("the boot stops at the listen")
        .expect_err("the listen failed the boot");

    match refused.downcast_ref::<nest_rs_core::BootStep>() {
        Some(nest_rs_core::BootStep::Listen { transport }) => {
            assert_eq!(transport, "RefusesToListen");
        }
        other => panic!("the boot error carries the listen step, not {other:?}: {refused:#}"),
    }
    assert_eq!(
        nest_rs_core::error_message(refused.as_ref()),
        "the `listen` of transport `RefusesToListen`: the address is already in use",
    );
    assert_eq!(
        *LISTEN_BOOT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        ["init hook", "listened", "shutdown hook"],
        "the hooks ran before the listens, no transport served, and the teardown ran",
    );
    assert_eq!(lifecycle.cause(), Some(nest_rs_core::StopCause::BootFailed));
    logs.expect_none(target::APP, "app ready");
}

/// Panics in its `listen`, as a transport with a bug there does.
struct PanicsToListen;

#[async_trait::async_trait]
impl Transport for PanicsToListen {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn listen(&mut self) -> anyhow::Result<nest_rs_core::Listening> {
        panic!("the listener was never bound");
    }

    async fn serve(self: Box<Self>, _cancel: CancellationToken) -> anyhow::Result<()> {
        listen_boot("the panicked transport served");
        Ok(())
    }

    fn stop_bound(&self) -> Duration {
        Duration::ZERO
    }
}

struct PanicsToListenModule;

impl Module for PanicsToListenModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_transport(|_| Ok(PanicsToListen))
    }
}

#[nest_rs_core::module(imports = [PanicsToListenModule], providers = [WarmCache])]
struct ListenPanicsModule;

/// A `listen` that panics fails the boot as one that errs does: the panic is
/// one line, the error carries the step, and the init hooks are torn down.
#[tokio::test]
async fn a_transport_that_panics_as_it_listens_fails_the_boot_and_its_hooks_are_torn_down() {
    let logs = LogCapture::install();
    let app = App::new::<ListenPanicsModule>().expect("the module boots");
    let lifecycle = std::sync::Arc::clone(app.lifecycle());

    let refused = tokio::time::timeout(Duration::from_secs(5), app.run())
        .await
        .expect("the boot stops at the listen")
        .expect_err("the panicked listen failed the boot");

    assert!(
        matches!(
            refused.downcast_ref::<nest_rs_core::BootStep>(),
            Some(nest_rs_core::BootStep::Listen { transport }) if transport == "PanicsToListen"
        ),
        "the boot error carries the listen step: {refused:#}"
    );
    assert_eq!(
        nest_rs_core::error_message(refused.as_ref()),
        "the `listen` of transport `PanicsToListen`: transport `PanicsToListen` panicked",
    );
    assert_eq!(
        *LISTEN_BOOT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        ["init hook", "shutdown hook"],
        "no transport served, and the teardown ran",
    );
    assert_eq!(lifecycle.cause(), Some(nest_rs_core::StopCause::BootFailed));
    let line = logs.expect_one(
        target::APP,
        "transport panicked during the boot; the boot is aborted",
    );
    assert_eq!(line.field("transport").as_deref(), Some("PanicsToListen"));
    assert_eq!(
        line.field("step").as_deref(),
        Some("the `listen` of transport `PanicsToListen`")
    );
    assert_eq!(
        line.field(FIELD).as_deref(),
        Some("the listener was never bound")
    );
    logs.expect_none(target::APP, "app ready");
}

/// A resource the boot opened before any transport configures.
struct Opened;

/// Panics in its `configure`, after the module's resource opened.
struct PanicsToConfigure;

#[async_trait::async_trait]
impl Transport for PanicsToConfigure {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        panic!("a route could not be composed");
    }

    async fn serve(self: Box<Self>, _cancel: CancellationToken) -> anyhow::Result<()> {
        listen_boot("the panicked transport served");
        Ok(())
    }

    fn stop_bound(&self) -> Duration {
        Duration::ZERO
    }
}

/// Never built: its `build` panics, as one reading settings that are not
/// there does.
struct PanicsAsBuilt;

#[async_trait::async_trait]
impl Transport for PanicsAsBuilt {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn serve(self: Box<Self>, _cancel: CancellationToken) -> anyhow::Result<()> {
        listen_boot("the panicked transport served");
        Ok(())
    }

    fn stop_bound(&self) -> Duration {
        Duration::ZERO
    }
}

/// Opens [`Opened`], then attaches the transport `build` builds.
fn opens_then_panics<T: Transport>(
    builder: ContainerBuilder,
    build: fn(&Container) -> anyhow::Result<T>,
) -> ContainerBuilder {
    builder
        .provide(Opened)
        .provide_close::<Opened, _, _>(|_| async {
            listen_boot("resource closed");
            Ok(())
        })
        .provide_transport(build)
}

struct ConfigurePanicsModule;

impl Module for ConfigurePanicsModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        opens_then_panics(builder, |_| Ok(PanicsToConfigure))
    }
}

struct BuildPanicsModule;

impl Module for BuildPanicsModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        opens_then_panics::<PanicsAsBuilt>(builder, |_| {
            panic!("the transport's settings were never read")
        })
    }
}

/// Run `M`'s app to its boot failure, returning the error and the panic's line.
async fn boot_panicking<M: Module>() -> (anyhow::Error, nest_rs_testing::CapturedEvent) {
    let logs = LogCapture::install();
    let app = App::new::<M>().expect("the module boots");
    let lifecycle = std::sync::Arc::clone(app.lifecycle());

    let refused = tokio::time::timeout(Duration::from_secs(5), app.run())
        .await
        .expect("the boot stops at the panic")
        .expect_err("the panic failed the boot");

    assert_eq!(
        *LISTEN_BOOT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        ["resource closed"],
        "no transport served, and the resource the boot opened closed",
    );
    assert_eq!(lifecycle.cause(), Some(nest_rs_core::StopCause::BootFailed));
    logs.expect_none(target::APP, "app ready");
    let line = logs.expect_one(
        target::APP,
        "transport panicked during the boot; the boot is aborted",
    );
    (refused, line)
}

#[tokio::test]
async fn a_transport_that_panics_as_it_configures_fails_the_boot_and_the_resources_close() {
    let (refused, line) = boot_panicking::<ConfigurePanicsModule>().await;

    assert!(
        matches!(
            refused.downcast_ref::<nest_rs_core::BootStep>(),
            Some(nest_rs_core::BootStep::Configure { transport }) if transport == "PanicsToConfigure"
        ),
        "the boot error carries the configure step: {refused:#}"
    );
    assert_eq!(
        nest_rs_core::error_message(refused.as_ref()),
        "the `configure` of transport `PanicsToConfigure`: transport `PanicsToConfigure` panicked",
    );
    assert_eq!(
        line.field("transport").as_deref(),
        Some("PanicsToConfigure")
    );
    assert_eq!(
        line.field("step").as_deref(),
        Some("the `configure` of transport `PanicsToConfigure`")
    );
    assert_eq!(
        line.field(FIELD).as_deref(),
        Some("a route could not be composed")
    );
}

#[tokio::test]
async fn a_transport_that_panics_as_it_is_built_fails_the_boot_and_the_resources_close() {
    let (refused, line) = boot_panicking::<BuildPanicsModule>().await;

    assert_eq!(refused.to_string(), "transport `PanicsAsBuilt` panicked");
    assert_eq!(line.field("transport").as_deref(), Some("PanicsAsBuilt"));
    assert_eq!(line.field("step"), None);
    assert_eq!(
        line.field(FIELD).as_deref(),
        Some("the transport's settings were never read")
    );
}

/// Fails its `configure`, as a transport whose routes do not compose does.
struct RefusesToConfigure;

#[async_trait::async_trait]
impl Transport for RefusesToConfigure {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Err(anyhow!("two routes claim `/posts`"))
    }

    async fn serve(self: Box<Self>, _cancel: CancellationToken) -> anyhow::Result<()> {
        listen_boot("the refused transport served");
        Ok(())
    }

    fn stop_bound(&self) -> Duration {
        Duration::ZERO
    }
}

struct ConfigureFailsModule;

impl Module for ConfigureFailsModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_transport(|_| Ok(RefusesToConfigure))
    }
}

/// A `configure` that fails fails the boot naming the step, as a `listen`
/// that fails does.
#[tokio::test]
async fn a_transport_that_fails_to_configure_fails_the_boot_naming_the_step() {
    let refused = tokio::time::timeout(
        Duration::from_secs(5),
        App::new::<ConfigureFailsModule>()
            .expect("the module boots")
            .run(),
    )
    .await
    .expect("the boot stops at the configure")
    .expect_err("the configure failed the boot");

    assert!(
        matches!(
            refused.downcast_ref::<nest_rs_core::BootStep>(),
            Some(nest_rs_core::BootStep::Configure { transport }) if transport == "RefusesToConfigure"
        ),
        "the boot error carries the configure step: {refused:#}"
    );
    assert_eq!(
        nest_rs_core::error_message(refused.as_ref()),
        "the `configure` of transport `RefusesToConfigure`: two routes claim `/posts`",
    );
}

/// The first of two modules serving through one transport type.
struct FirstServesFailing;

impl Module for FirstServesFailing {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_transport(|_| Ok(Failing))
    }
}

/// The second, attaching the type [`FirstServesFailing`] attached.
struct SecondServesFailing;

impl Module for SecondServesFailing {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_transport(|_| Ok(Failing))
    }
}

#[module(imports = [FirstServesFailing, SecondServesFailing])]
struct OneTransportTwiceRoot;

/// An app runs one transport of each type: a second module attaching the
/// same type fails the boot naming both imports.
#[tokio::test]
async fn two_modules_attaching_one_transport_type_fail_the_boot_naming_both() {
    for refused in crate::refused_on_both_paths::<OneTransportTwiceRoot>().await {
        let contested = refused
            .downcast_ref::<nest_rs_core::ContestedDeclarationError>()
            .unwrap_or_else(|| panic!("not the contest: {refused:#}"));
        assert_eq!(contested.type_name, std::any::type_name::<Failing>());
        assert_eq!(
            contested.first,
            "`FirstServesFailing` at `imports[0]` of `OneTransportTwiceRoot`"
        );
        assert_eq!(
            contested.second,
            "`SecondServesFailing` at `imports[1]` of `OneTransportTwiceRoot`"
        );
        assert!(
            contested.remedy.contains("one transport of each type"),
            "{refused}"
        );
    }
}

/// Serves the queue `Q` names until it is stopped: one type per queue.
struct Consumes<Q>(std::marker::PhantomData<fn() -> Q>);

#[async_trait::async_trait]
impl<Q: 'static> Transport for Consumes<Q> {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn serve(self: Box<Self>, cancel: CancellationToken) -> anyhow::Result<()> {
        cancel.cancelled().await;
        Ok(())
    }

    fn stop_bound(&self) -> Duration {
        Duration::ZERO
    }
}

struct Orders;
struct Invoices;

struct ConsumesOrders;

impl Module for ConsumesOrders {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_transport(|_| Ok(Consumes::<Orders>(std::marker::PhantomData)))
    }
}

struct ConsumesInvoices;

impl Module for ConsumesInvoices {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_transport(|_| Ok(Consumes::<Invoices>(std::marker::PhantomData)))
    }
}

#[module(imports = [ConsumesOrders, ConsumesInvoices])]
struct TwoQueuesRoot;

/// A transport generic over a marker is one type per marker, so two markers
/// are two transports, each named with its marker.
#[test]
fn a_transport_generic_over_a_marker_is_one_type_per_marker() {
    let app = App::new::<TwoQueuesRoot>().expect("two markers boot");
    let attached =
        nest_rs_core::Discovery::new(app.container()).meta::<nest_rs_core::TransportContribution>();
    let names: Vec<&str> = attached
        .iter()
        .map(|contribution| contribution.meta.name())
        .collect();
    assert_eq!(names, ["Consumes<Orders>", "Consumes<Invoices>"]);
}

/// Serves until the app's stop token fires.
struct ServesUntilStopped;

#[async_trait::async_trait]
impl Transport for ServesUntilStopped {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn serve(self: Box<Self>, cancel: CancellationToken) -> anyhow::Result<()> {
        cancel.cancelled().await;
        Ok(())
    }

    fn stop_bound(&self) -> Duration {
        Duration::ZERO
    }
}

/// Returns from `serve` at once, without an error, as a loop that found
/// nothing to do and ended would.
struct EndsAtOnce;

#[async_trait::async_trait]
impl Transport for EndsAtOnce {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn serve(self: Box<Self>, _cancel: CancellationToken) -> anyhow::Result<()> {
        Ok(())
    }

    fn stop_bound(&self) -> Duration {
        Duration::ZERO
    }
}

struct EndsBesideServingModule;

impl Module for EndsBesideServingModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
            .provide_transport(|_| Ok(ServesUntilStopped))
            .provide_transport(|_| Ok(EndsAtOnce))
    }
}

struct AttachesServesUntilStopped;

impl Module for AttachesServesUntilStopped {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_transport(|_| Ok(ServesUntilStopped))
    }
}

/// The public seam is the whole of it: a module attaches its transport, and
/// the record the kernel reads names it by its type.
#[test]
fn a_module_attaches_a_transport_the_kernel_names_by_its_type() {
    let app = App::new::<AttachesServesUntilStopped>().expect("the module boots");
    let attached =
        nest_rs_core::Discovery::new(app.container()).meta::<nest_rs_core::TransportContribution>();
    let names: Vec<&str> = attached
        .iter()
        .map(|contribution| contribution.meta.name())
        .collect();
    assert_eq!(names, ["ServesUntilStopped"]);
}

const ENDED_EARLY: &str = "transport stopped serving before the app was asked to stop";

#[tokio::test]
async fn a_transport_that_stops_serving_on_its_own_stops_the_app_with_an_error() {
    let logs = LogCapture::install();
    let app = App::new::<EndsBesideServingModule>().expect("the module boots");
    let lifecycle = std::sync::Arc::clone(app.lifecycle());

    let ended = tokio::time::timeout(Duration::from_secs(5), app.run())
        .await
        .expect("the early end stops the transport still serving")
        .expect_err("a transport ending on its own is the app's error");

    assert_eq!(
        ended.to_string(),
        "transport `EndsAtOnce` stopped serving before the app was asked to stop"
    );
    assert_eq!(
        lifecycle.cause(),
        Some(nest_rs_core::StopCause::TransportFailed)
    );
    let line = logs.expect_one(target::APP, ENDED_EARLY);
    assert_eq!(line.level, "error");
    assert_eq!(line.field("transport").as_deref(), Some("EndsAtOnce"));
}

/// Never returns from `serve`, its stop token ignored.
struct IgnoresItsStop(Duration);

#[async_trait::async_trait]
impl Transport for IgnoresItsStop {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn serve(self: Box<Self>, _cancel: CancellationToken) -> anyhow::Result<()> {
        std::future::pending().await
    }

    fn stop_bound(&self) -> Duration {
        self.0
    }
}

/// The bound [`IgnoresItsStop`] states.
const IGNORED_BOUND: Duration = Duration::from_secs(3);

struct IgnoresItsStopModule;

impl Module for IgnoresItsStopModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
            .provide_transport(|_| Ok(IgnoresItsStop(IGNORED_BOUND)))
            .provide_transport(|_| Ok(Failing))
    }
}

const ABANDONED: &str = "transport did not stop within its declared bound: abandoned";

/// The kernel holds each transport to the bound it states: past the longest,
/// plus the settle, one still serving is abandoned and named.
#[tokio::test(start_paused = true)]
async fn a_transport_that_ignores_its_stop_is_abandoned_at_its_bound_then_the_settle() {
    let logs = LogCapture::install();
    let app = App::new::<IgnoresItsStopModule>().expect("the module boots");
    let started = tokio::time::Instant::now();

    let failed = tokio::time::timeout(Duration::from_secs(60), app.run())
        .await
        .expect("the kernel does not wait on the transport past its bound")
        .expect_err("a transport failed");

    let took = started.elapsed();
    let held = IGNORED_BOUND + nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT;
    assert!(
        took >= held && took < held + Duration::from_millis(100),
        "run returned at the bound plus the settle: took {took:?}",
    );
    assert!(
        failed.to_string().contains("listener stopped"),
        "the first failure is still the app's error: {failed:#}"
    );
    let line = logs.expect_one(target::APP, ABANDONED);
    assert_eq!(line.level, "error");
    assert_eq!(line.field("transport").as_deref(), Some("IgnoresItsStop"));
    assert_eq!(line.field("stop_bound_ms").as_deref(), Some("3000"));
}

/// Declares the bound it was built with.
struct Declares(Duration);

#[async_trait::async_trait]
impl Transport for Declares {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn serve(self: Box<Self>, _cancel: CancellationToken) -> anyhow::Result<()> {
        Err(anyhow!("served past the bound check"))
    }

    fn stop_bound(&self) -> Duration {
        self.0
    }
}

/// The boot of one [`Declares`] transport stating `bound`.
async fn run_declaring(bound: Duration) -> anyhow::Error {
    struct DeclaresModule;
    static BOUND: std::sync::Mutex<Duration> = std::sync::Mutex::new(Duration::ZERO);
    impl Module for DeclaresModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder.provide_transport(|_| {
                let bound = *BOUND
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                Ok(Declares(bound))
            })
        }
    }
    *BOUND
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = bound;
    App::new::<DeclaresModule>()
        .expect("the module boots")
        .run()
        .await
        .expect_err("every outcome here is an error")
}

/// An unbounded stop is refused before anything runs; a day is the ceiling,
/// and the longest first-party window, an hour and its settle, is under it.
#[tokio::test]
async fn a_stop_bound_past_a_day_is_refused_at_the_boot() {
    for (bound, refused_ms) in [
        (Duration::MAX, Some(u64::MAX)),
        (
            nest_rs_core::MAX_STOP_BOUND + Duration::from_secs(1),
            Some(86_401_000),
        ),
        (nest_rs_core::MAX_STOP_BOUND, None),
        (Duration::from_millis(3_600_500), None),
    ] {
        let error = run_declaring(bound).await;
        match (
            error.downcast_ref::<nest_rs_core::UnboundedTransportError>(),
            refused_ms,
        ) {
            (Some(refused), Some(ms)) => {
                assert_eq!(refused.transport, "Declares");
                assert_eq!(refused.stop_bound_ms, ms);
                assert!(
                    error
                        .to_string()
                        .ends_with("a transport's stop is bounded: declare its drain window"),
                    "{error}"
                );
            }
            (None, None) => assert!(
                error.to_string().contains("served past the bound check"),
                "{bound:?} is accepted: {error:#}"
            ),
            (refused, _) => panic!("{bound:?}: {refused:?}, {error:#}"),
        }
    }
}

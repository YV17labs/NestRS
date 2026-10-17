//! Covers `src/app_lifecycle.rs` — where the app stands and why it stops, as a
//! provider injecting `Arc<AppLifecycle>` reads it, a boot that fails
//! stopping it before its teardown, and the signal that stops the process,
//! read by a [`ChildProcess`] from a shutdown hook.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::anyhow;
use nest_rs_core::target;
use nest_rs_core::{
    App, AppLifecycle, AppState, Collecting, Container, ContainerBuilder, Listening, Module,
    Registering, StopCause, Transport, hooks, injectable, module,
};
use nest_rs_testing::LogCapture;
use tokio_util::sync::CancellationToken;

use crate::{ChildProcess, Failing};

/// What each step of the boot read from the handle, in the order it read it.
static READ: Mutex<Vec<(&'static str, AppState)>> = Mutex::new(Vec::new());

fn read(at: &'static str, lifecycle: &AppLifecycle) {
    READ.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((at, lifecycle.state()));
}

fn reads() -> Vec<(&'static str, AppState)> {
    READ.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

#[injectable]
struct Observer {
    #[inject]
    lifecycle: Arc<AppLifecycle>,
}

#[hooks]
impl Observer {
    #[on_module_init]
    async fn init(&self) {
        read("init hook", &self.lifecycle);
    }
}

/// Builds a provider the way a module's `register` does, reading the handle
/// as it constructs it.
struct ConstructsModule;

struct Constructed;

struct Built;

impl Module for ConstructsModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        let lifecycle = builder.snapshot().get::<AppLifecycle>();
        match lifecycle {
            Some(lifecycle) => read("constructor", &lifecycle),
            None => return builder.refuse(anyhow!("the handle is seeded before the register")),
        }
        builder.provide(Constructed)
    }
}

/// Reads the handle as it listens and once the app is ready, then stops
/// serving.
struct ReadsOnceReady(Option<Arc<AppLifecycle>>);

#[async_trait::async_trait]
impl Transport for ReadsOnceReady {
    async fn configure(&mut self, container: &Container) -> anyhow::Result<()> {
        self.0 = container.get::<AppLifecycle>();
        Ok(())
    }

    async fn listen(&mut self) -> anyhow::Result<Listening> {
        let lifecycle = self.0.as_ref().ok_or_else(|| anyhow!("configured first"))?;
        read("listen", lifecycle);
        Ok(Listening::on([
            SocketAddr::from(([127, 0, 0, 1], 4000)),
            SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], 4000)),
        ]))
    }

    async fn serve(self: Box<Self>, _stop: CancellationToken) -> anyhow::Result<()> {
        let lifecycle = self.0.ok_or_else(|| anyhow!("the handle is injectable"))?;
        if lifecycle.ready().await {
            read("serving", &lifecycle);
        }
        Ok(())
    }

    fn stop_bound(&self) -> Duration {
        Duration::ZERO
    }
}

#[module(imports = [ConstructsModule, ReadsOnceReadyModule], providers = [Observer])]
struct ObservedModule;

struct ReadsOnceReadyModule;

impl Module for ReadsOnceReadyModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_transport(|_| Ok(ReadsOnceReady(None)))
    }
}

#[tokio::test]
async fn a_provider_reads_starting_through_the_boot_and_ready_once_the_app_serves() {
    let logs = LogCapture::install();
    let app = App::builder()
        .provide_factory(|container| async move {
            let lifecycle = container
                .get::<AppLifecycle>()
                .ok_or_else(|| anyhow!("the handle is seeded before the factories"))?;
            read("factory", &lifecycle);
            Ok(Built)
        })
        .module::<ObservedModule>()
        .build()
        .await
        .expect("the module boots");
    assert_eq!(app.lifecycle().state(), AppState::Starting);

    let lifecycle = Arc::clone(app.lifecycle());
    app.run()
        .await
        .expect_err("the app serves, then its transport ends on its own");

    assert_eq!(
        reads(),
        [
            ("factory", AppState::Starting),
            ("constructor", AppState::Starting),
            ("init hook", AppState::Starting),
            ("listen", AppState::Starting),
            ("serving", AppState::Ready),
        ]
    );
    let ready = logs.expect_one(target::APP, "app ready");
    assert_eq!(ready.level, "info");
    assert_eq!(ready.field("transports").as_deref(), Some("ReadsOnceReady"));
    assert_eq!(
        ready.field("addresses").as_deref(),
        Some("127.0.0.1:4000, [::1]:4000")
    );
    assert_eq!(lifecycle.state(), AppState::Draining);
    assert_eq!(
        lifecycle.cause(),
        Some(StopCause::TransportFailed),
        "a transport stopped serving on its own"
    );
}

/// Serves until the app's stop token fires.
struct ServesUntilStopped;

#[async_trait::async_trait]
impl Transport for ServesUntilStopped {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn serve(self: Box<Self>, stop: CancellationToken) -> anyhow::Result<()> {
        stop.cancelled().await;
        Ok(())
    }

    fn stop_bound(&self) -> Duration {
        Duration::ZERO
    }
}

struct FailingBesideServingModule;

impl Module for FailingBesideServingModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
            .provide_transport(|_| Ok(ServesUntilStopped))
            .provide_transport(|_| Ok(Failing))
    }
}

#[tokio::test]
async fn a_transport_that_fails_stops_the_app_for_that_cause_and_says_so_once() {
    let logs = LogCapture::install();
    let app = App::new::<FailingBesideServingModule>().expect("the module boots");
    let stopping = app.lifecycle().stopping();
    let lifecycle = Arc::clone(app.lifecycle());
    assert_eq!(lifecycle.cause(), None);

    let err = tokio::time::timeout(Duration::from_secs(5), app.run())
        .await
        .expect("the failure stops the transport still serving")
        .expect_err("the failed transport is the app's error");

    assert!(format!("{err:#}").contains("listener stopped"), "{err:#}");
    assert_eq!(lifecycle.cause(), Some(StopCause::TransportFailed));
    assert_eq!(lifecycle.state(), AppState::Draining);
    assert!(stopping.is_cancelled());
    assert!(
        !lifecycle.ready().await,
        "an app that drains is not waited on as ready"
    );
    let draining = logs.expect_one(target::APP, "app draining");
    assert_eq!(draining.level, "info");
    assert_eq!(draining.field("cause").as_deref(), Some("TransportFailed"));
    assert_eq!(draining.field("signal"), None);
}

/// What a shutdown hook or a close read of the stop cause once the app's
/// `stopping()` fired, where it read it.
static JOINED: Mutex<Vec<(&'static str, Option<StopCause>)>> = Mutex::new(Vec::new());

fn joined() -> Vec<(&'static str, Option<StopCause>)> {
    JOINED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Wait for the app's stop, as a loop a provider runs beside the app does,
/// then record the cause.
async fn join_the_stop(at: &'static str, lifecycle: &AppLifecycle) {
    lifecycle.stopping().cancelled().await;
    JOINED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((at, lifecycle.cause()));
}

/// Held well under the shutdown hooks' budget: a teardown that never stopped
/// the app would wait that budget out on the join.
const JOINED_PROMPTLY: Duration = Duration::from_secs(1);

#[injectable]
struct JoinsOnDestroy {
    #[inject]
    lifecycle: Arc<AppLifecycle>,
}

#[hooks]
impl JoinsOnDestroy {
    #[on_module_init]
    async fn start(&self) -> anyhow::Result<()> {
        Err(anyhow!("the poller could not start"))
    }

    #[on_module_destroy]
    async fn join(&self) {
        join_the_stop("shutdown hook", &self.lifecycle).await;
    }
}

#[module(providers = [JoinsOnDestroy])]
struct JoinsOnDestroyModule;

#[tokio::test(start_paused = true)]
async fn an_init_hook_that_fails_stops_the_app_for_that_cause_before_the_shutdown_hooks_run() {
    let started = tokio::time::Instant::now();

    let error = App::new::<JoinsOnDestroyModule>()
        .expect("the module boots")
        .run()
        .await
        .expect_err("the init hook failed the boot");

    assert!(
        format!("{error:#}").contains("the poller could not start"),
        "{error:#}"
    );
    assert_eq!(joined(), [("shutdown hook", Some(StopCause::BootFailed))]);
    assert!(
        started.elapsed() < JOINED_PROMPTLY,
        "the hook joining the stop was not held to the budget: took {:?}",
        started.elapsed(),
    );
}

/// A resource holding the app's lifecycle, whose close joins its stop.
struct JoinsOnClose(Arc<AppLifecycle>);

async fn close_joining(resource: Arc<JoinsOnClose>) -> anyhow::Result<()> {
    join_the_stop("close", &resource.0).await;
    Ok(())
}

/// Fails to configure, as a transport whose certificate cannot be read does.
struct RefusesToConfigure;

#[async_trait::async_trait]
impl Transport for RefusesToConfigure {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Err(anyhow!("the certificate could not be read"))
    }

    async fn serve(self: Box<Self>, _stop: CancellationToken) -> anyhow::Result<()> {
        Ok(())
    }

    fn stop_bound(&self) -> Duration {
        Duration::ZERO
    }
}

struct ConfigureFailsModule;

impl Module for ConfigureFailsModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        let Some(lifecycle) = builder.snapshot().get::<AppLifecycle>() else {
            return builder.refuse(anyhow!("the handle is seeded before the register"));
        };
        builder
            .provide(JoinsOnClose(lifecycle))
            .provide_close::<JoinsOnClose, _, _>(close_joining)
            .provide_transport(|_| Ok(RefusesToConfigure))
    }
}

#[tokio::test(start_paused = true)]
async fn a_transport_that_fails_to_configure_stops_the_app_for_that_cause_before_the_closes_run() {
    let started = tokio::time::Instant::now();

    let error = App::new::<ConfigureFailsModule>()
        .expect("the module boots")
        .run()
        .await
        .expect_err("the transport failed the boot");

    assert!(
        format!("{error:#}").contains("the certificate could not be read"),
        "{error:#}"
    );
    assert_eq!(joined(), [("close", Some(StopCause::BootFailed))]);
    assert!(
        started.elapsed() < JOINED_PROMPTLY,
        "the close joining the stop was not held to the budget: took {:?}",
        started.elapsed(),
    );
}

struct Index;

/// Opens a resource through a factory, attaching its close beside it, then
/// fails a later factory.
struct FactoryFailsModule;

impl Module for FactoryFailsModule {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder
            .provide_factory(|container: Container| async move {
                let lifecycle = container
                    .get::<AppLifecycle>()
                    .ok_or_else(|| anyhow!("the handle is seeded before the factories"))?;
                Ok(JoinsOnClose(lifecycle))
            })
            .provide_close::<JoinsOnClose, _, _>(close_joining)
            .provide_factory_after::<Index, (JoinsOnClose,), _, _>(|_| async {
                Err(anyhow!("the index is corrupt"))
            })
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }
}

#[tokio::test(start_paused = true)]
async fn a_build_that_fails_stops_the_app_for_that_cause_before_the_closes_run() {
    let started = tokio::time::Instant::now();

    let error = App::builder()
        .module::<FactoryFailsModule>()
        .build()
        .await
        .err()
        .expect("the later factory failed the build");

    assert!(
        format!("{error:#}").contains("the index is corrupt"),
        "{error:#}"
    );
    assert_eq!(joined(), [("close", Some(StopCause::BootFailed))]);
    assert!(
        started.elapsed() < JOINED_PROMPTLY,
        "the close joining the stop was not held to the budget: took {:?}",
        started.elapsed(),
    );
}

const CHILD_TEST: &str = "app_lifecycle::app_lifecycle_child_process";

const SERVING: &str = "APP-LIFECYCLE-CHILD SERVING";

const CAUSE: &str = "APP-LIFECYCLE-CHILD CAUSE";

/// Prints [`SERVING`] once the app is ready, so a parent reading it may signal.
struct AnnouncesReady(Option<Arc<AppLifecycle>>);

#[async_trait::async_trait]
impl Transport for AnnouncesReady {
    async fn configure(&mut self, container: &Container) -> anyhow::Result<()> {
        self.0 = container.get::<AppLifecycle>();
        Ok(())
    }

    #[expect(
        clippy::print_stdout,
        reason = "the child's stdout is the protocol its parent test reads"
    )]
    async fn serve(self: Box<Self>, stop: CancellationToken) -> anyhow::Result<()> {
        if let Some(lifecycle) = self.0
            && lifecycle.ready().await
        {
            println!("{SERVING}");
        }
        stop.cancelled().await;
        Ok(())
    }

    fn stop_bound(&self) -> Duration {
        Duration::ZERO
    }
}

struct AnnouncesReadyModule;

impl Module for AnnouncesReadyModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_transport(|_| Ok(AnnouncesReady(None)))
    }
}

#[injectable]
struct ReportsCause {
    #[inject]
    lifecycle: Arc<AppLifecycle>,
}

#[hooks]
impl ReportsCause {
    #[expect(
        clippy::print_stdout,
        reason = "the child's stdout is the protocol its parent test reads"
    )]
    #[on_application_shutdown]
    async fn report(&self) {
        println!("{CAUSE} {:?}", self.lifecycle.cause());
    }
}

#[module(imports = [AnnouncesReadyModule], providers = [ReportsCause])]
struct ReportsCauseModule;

#[nest_rs_core::main]
async fn run_child() -> anyhow::Result<()> {
    App::new::<ReportsCauseModule>()?.run().await
}

#[test]
fn app_lifecycle_child_process() {
    if crate::child_role().is_some() {
        let _ = run_child();
    }
}

#[cfg(unix)]
#[test]
fn a_shutdown_hook_reads_the_signal_that_stopped_the_app() {
    for (signal, cause) in [("TERM", "Terminate"), ("INT", "Interrupt")] {
        let mut child = ChildProcess::spawn(CHILD_TEST, "cause");
        child.expect_line(SERVING);
        child.signal(signal);
        child.expect_line(&format!("{CAUSE} Some({cause})"));

        let (code, printed) = child.exit_within(Duration::from_secs(2));
        assert_eq!(code, Some(0), "SIG{signal}: {printed:#?}");
    }
}

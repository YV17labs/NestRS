//! Covers `src/app_lifecycle.rs` — where the app stands and why it stops, as a
//! provider injecting `Arc<AppLifecycle>` reads it, and the signal that stops
//! the process, read by a [`ChildProcess`] from a shutdown hook.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::anyhow;
use nest_rs_core::__private::TransportContribution;
use nest_rs_core::target;
use nest_rs_core::{
    App, AppLifecycle, AppState, Container, ContainerBuilder, Module, Registering, StopCause,
    Transport, hooks, injectable, module,
};
use nest_rs_testing::LogCapture;
use tokio_util::sync::CancellationToken;

use crate::ChildProcess;

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

/// Serves until the app is ready, reads the handle, then stops serving.
struct ReadsOnceReady(Option<Arc<AppLifecycle>>);

#[async_trait::async_trait]
impl Transport for ReadsOnceReady {
    async fn configure(&mut self, container: &Container) -> anyhow::Result<()> {
        self.0 = container.get::<AppLifecycle>();
        Ok(())
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
        builder.provide_meta(TransportContribution {
            name: "ReadsOnceReady",
            build: |_| Ok(Box::new(ReadsOnceReady(None))),
        })
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
    app.run().await.expect("the app serves, then stops");

    assert_eq!(
        reads(),
        [
            ("factory", AppState::Starting),
            ("constructor", AppState::Starting),
            ("init hook", AppState::Starting),
            ("serving", AppState::Ready),
        ]
    );
    let ready = logs.expect_one(target::APP, "app ready");
    assert_eq!(ready.level, "info");
    assert_eq!(ready.field("transports").as_deref(), Some("ReadsOnceReady"));
    assert_eq!(lifecycle.state(), AppState::Draining);
    assert_eq!(
        lifecycle.cause(),
        Some(StopCause::TransportFailed),
        "a transport stopped serving on its own"
    );
}

/// Fails at once, as if its listener broke.
struct FailsAtOnce;

#[async_trait::async_trait]
impl Transport for FailsAtOnce {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn serve(self: Box<Self>, _stop: CancellationToken) -> anyhow::Result<()> {
        Err(anyhow!("the listener stopped accepting"))
    }

    fn stop_bound(&self) -> Duration {
        Duration::ZERO
    }
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
            .provide_meta(TransportContribution {
                name: "ServesUntilStopped",
                build: |_| Ok(Box::new(ServesUntilStopped)),
            })
            .provide_meta(TransportContribution {
                name: "FailsAtOnce",
                build: |_| Ok(Box::new(FailsAtOnce)),
            })
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
        builder.provide_meta(TransportContribution {
            name: "AnnouncesReady",
            build: |_| Ok(Box::new(AnnouncesReady(None))),
        })
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

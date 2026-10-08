//! Covers `src/app.rs` — how the serve loop reports the transports it runs:
//! the way down they bound, and a transport that stops.

use std::time::Duration;

use anyhow::anyhow;
use nest_rs_core::target;
use nest_rs_core::{
    App, Container, ContainerBuilder, Module, Registering, SHUTDOWN_HOOKS_TIMEOUT, Transport,
    TransportContribution,
};
use nest_rs_testing::LogCapture;
use tokio_util::sync::CancellationToken;

struct Failing;

#[async_trait::async_trait]
impl Transport for Failing {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn serve(self: Box<Self>, _cancel: CancellationToken) -> anyhow::Result<()> {
        Err(anyhow!("the listener stopped accepting"))
    }

    fn stop_bound(&self) -> Duration {
        Duration::ZERO
    }
}

struct FailingModule;

impl Module for FailingModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_meta(TransportContribution {
            name: "Failing",
            build: |_| Ok(Box::new(Failing)),
        })
    }
}

struct Panicking;

#[async_trait::async_trait]
impl Transport for Panicking {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn serve(self: Box<Self>, _cancel: CancellationToken) -> anyhow::Result<()> {
        panic!("the accept loop unwound");
    }

    fn stop_bound(&self) -> Duration {
        Duration::ZERO
    }
}

struct PanickingModule;

impl Module for PanickingModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_meta(TransportContribution {
            name: "Panicking",
            build: |_| Ok(Box::new(Panicking)),
        })
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

#[tokio::test]
async fn a_transport_task_that_panics_is_reported_as_a_panic_not_as_an_error() {
    let logs = LogCapture::install();
    let err = App::new::<PanickingModule>()
        .expect("the module boots")
        .run()
        .await
        .expect_err("a panicked transport still fails the app");
    assert!(err.to_string().contains("panic"), "{err}");

    let event = logs.expect_one(target::APP, "transport task panicked; shutting down");
    assert_eq!(event.level, "error");
    assert!(
        event.field("error").is_some(),
        "the event carries the join error, got {:?}",
        event.fields,
    );
}

struct Stated(Duration);

#[async_trait::async_trait]
impl Transport for Stated {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn serve(self: Box<Self>, _cancel: CancellationToken) -> anyhow::Result<()> {
        Ok(())
    }

    fn stop_bound(&self) -> Duration {
        self.0
    }
}

struct TwoBoundsModule;

impl Module for TwoBoundsModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
            .provide_meta(TransportContribution {
                name: "Short",
                build: |_| Ok(Box::new(Stated(Duration::from_millis(2_500)))),
            })
            .provide_meta(TransportContribution {
                name: "Long",
                build: |_| Ok(Box::new(Stated(Duration::from_secs(41)))),
            })
    }
}

#[tokio::test]
async fn the_boot_line_files_the_longest_stop_bound_beside_the_hooks_budget() {
    let logs = LogCapture::install();
    App::new::<TwoBoundsModule>()
        .expect("the module boots")
        .run()
        .await
        .expect("both transports stop cleanly");

    let event = logs.expect_one(target::APP, "way down bounded");
    assert_eq!(event.level, "info");
    assert_eq!(event.field("stop_bound_ms").as_deref(), Some("41000"));
    assert_eq!(
        event.field("hooks_budget_ms"),
        Some(SHUTDOWN_HOOKS_TIMEOUT.as_millis().to_string()),
    );
}

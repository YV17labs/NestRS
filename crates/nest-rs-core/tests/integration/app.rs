//! Covers `src/app.rs` — how the serve loop reports a transport that stops.
//!
//! Both events are `error` on `nest_rs::app`, and both are the last thing the
//! process says before it exits: `run` returns the error to `main`, which prints
//! it, but *which* transport died and whether it returned or panicked is only
//! here. On a binary serving HTTP beside a scheduler and a queue worker, that
//! difference is the whole diagnosis.
//!
//! A panicking transport is the sharper of the two — `JoinSet` reports a join
//! error rather than the panic payload, so without this line the operator sees
//! a bare "task panicked" with nothing naming the surface.

use anyhow::anyhow;
use nest_rs_core::target;
use nest_rs_core::{App, Container, ContainerBuilder, Module, Transport, TransportContribution};
use nest_rs_testing::LogCapture;
use tokio_util::sync::CancellationToken;

/// A transport that configures cleanly and then fails, the way a socket bind
/// that loses its port mid-flight would.
struct Failing;

#[async_trait::async_trait]
impl Transport for Failing {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn serve(self: Box<Self>, _cancel: CancellationToken) -> anyhow::Result<()> {
        Err(anyhow!("the listener stopped accepting"))
    }
}

struct FailingModule;

impl Module for FailingModule {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.provide_meta(TransportContribution {
            name: "Failing",
            build: |_| Ok(Box::new(Failing)),
        })
    }
}

/// The same shape, except the task dies rather than returning — so the serve
/// loop learns about it through `JoinSet` instead of through a `Result`.
struct Panicking;

#[async_trait::async_trait]
impl Transport for Panicking {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn serve(self: Box<Self>, _cancel: CancellationToken) -> anyhow::Result<()> {
        panic!("the accept loop unwound");
    }
}

struct PanickingModule;

impl Module for PanickingModule {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
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

    // The distinction is what makes this worth its own line: a transport that
    // *returned* an error stopped on purpose, one that panicked did not, and
    // the two lead an operator to different places.
    let event = logs.expect_one(target::APP, "transport task panicked; shutting down");
    assert_eq!(event.level, "error");
    assert!(
        event.field("error").is_some(),
        "the event carries the join error, got {:?}",
        event.fields,
    );
}

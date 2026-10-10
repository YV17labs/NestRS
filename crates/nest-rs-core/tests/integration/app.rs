//! Covers `src/app.rs` — how the serve loop reports the transports it runs:
//! the way down they bound, and a transport that stops.

use std::time::Duration;

use anyhow::anyhow;
use nest_rs_core::__private::TransportContribution;
use nest_rs_core::panic::{FIELD, LOCATION_FIELD};
use nest_rs_core::target;
use nest_rs_core::{
    App, Container, ContainerBuilder, Module, Registering, SHUTDOWN_HOOKS_TIMEOUT, Transport,
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
    let errors: Vec<_> = events
        .iter()
        .filter(|event| event.level == "error")
        .collect();
    assert_eq!(errors.len(), 1, "the panic is said once: {errors:#?}");
    let line = errors[0];
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

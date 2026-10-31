//! Covers `src/module.rs` — `OpenTelemetryModule`, the provider of `Metrics`,
//! imported where a provider injects it, and nowhere else.

use std::sync::Arc;

use nest_rs_core::{App, MissingDependencyError, injectable, module};
use nest_rs_opentelemetry::{Metrics, OpenTelemetryModule};

#[injectable]
struct SignupMetrics {
    #[inject]
    metrics: Arc<Metrics>,
}

#[module(imports = [OpenTelemetryModule], providers = [SignupMetrics])]
struct Recording;

#[module(providers = [SignupMetrics])]
struct Unimported;

/// A suite never runs `main`, so it boots with no `OpenTelemetry::init`: the
/// module provides `Metrics` over the global meter, which records nothing
/// until `init` installs an exporter, and asks the suite for nothing.
#[tokio::test]
async fn the_module_boots_without_init_and_provides_metrics() {
    let app = App::builder()
        .module::<Recording>()
        .build()
        .await
        .expect("a suite boots the module with no init");
    let signup = app
        .container()
        .get::<SignupMetrics>()
        .expect("the provider injecting `Metrics` is built");
    signup.metrics.counter("users.created").add(1, &[]);
}

/// A provider injecting `Metrics` with no import of the module is refused,
/// naming the module that provides it.
#[tokio::test]
async fn a_provider_injecting_metrics_without_the_module_is_refused_naming_it() {
    let Err(refused) = App::builder().module::<Unimported>().build().await else {
        panic!("`Metrics` is provided by nothing the module imports");
    };
    assert!(
        refused.downcast_ref::<MissingDependencyError>().is_some(),
        "{refused:#}"
    );
    let said = format!("{refused:#}");
    assert!(
        said.contains("nest_rs::opentelemetry::OpenTelemetryModule"),
        "the remedy names the module: {said}"
    );
}

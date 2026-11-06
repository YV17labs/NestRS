//! Covers `src/module.rs` — `OpenTelemetryModule`, the provider of `Metrics`,
//! imported where a provider injects it, and nowhere else.

use std::sync::Arc;

use nest_rs_core::{App, MissingDependencyError, UnbuiltProviderError, injectable, module};
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

#[injectable]
struct AuditMetrics {
    #[inject]
    metrics: Arc<Metrics>,
}

#[module(providers = [AuditMetrics])]
struct Skipping;

#[module(imports = [Recording, Skipping])]
struct ImporterFirst;

#[module(imports = [Skipping, Recording])]
struct SkipperFirst;

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
    assert!(said.contains("OpenTelemetryModule"), "{said}");
}

/// The module installs `Metrics` in its own `register`, which the access graph
/// does not gate: a provider whose module skips the import reaches them when a
/// module importing it registered first, and fails the boot when it registers
/// before — so the import goes beside each provider injecting them.
#[tokio::test]
async fn a_module_skipping_the_import_reaches_metrics_by_registration_order_alone() {
    let app = App::builder()
        .module::<ImporterFirst>()
        .build()
        .await
        .expect("`Metrics` were registered before the module skipping the import");
    let audit = app
        .container()
        .get::<AuditMetrics>()
        .expect("the provider skipping the import is built");
    audit.metrics.counter("audit.recorded").add(1, &[]);

    let Err(refused) = App::builder().module::<SkipperFirst>().build().await else {
        panic!("`Metrics` are registered only after the module skipping the import");
    };
    assert!(
        refused.downcast_ref::<UnbuiltProviderError>().is_some(),
        "{refused:#}"
    );
    let said = format!("{refused:#}");
    assert!(said.contains("OpenTelemetryModule"), "{said}");
}

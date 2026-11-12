//! The indicator `StorageModule` attaches, booted beside `HealthModule` alone
//! over `memory:///`: the readiness and startup probes read `storage` up,
//! once however many modules import the store, and liveness waits on it
//! never. A client a test overrides opens no store, so nothing is probed; an
//! app without `HealthModule` boots as it did.

use nest_rs_core::module;
use nest_rs_health::{HealthModule, HealthService, IndicatorStatus, ProbeKind};
use nest_rs_storage::{Storage, StorageConfig, StorageModule};
use nest_rs_testing::{HeadlessApp, TestAppBuilder};

/// The in-process store, pinned where the app opens it.
fn in_memory() -> StorageConfig {
    StorageConfig {
        url: "memory:///probed".into(),
        ..StorageConfig::default()
    }
}

/// `M`, booted by `builder`.
async fn booted<M: nest_rs_core::Module>(builder: TestAppBuilder) -> HeadlessApp {
    builder
        .module::<M>()
        .build_headless()
        .await
        .unwrap_or_else(|refused| panic!("boots: {refused:#}"))
}

/// A feature importing the store bare, as each feature reaching it may.
#[module(imports = [StorageModule])]
struct UploadingFeature;

/// An app pinning the store beside a feature that imports it too.
#[module(imports = [HealthModule, StorageModule::for_root(in_memory()), UploadingFeature])]
struct ImportedTwice;

#[tokio::test]
async fn the_store_answers_the_readiness_and_startup_probes_once_as_storage_never_liveness() {
    let app = booted::<ImportedTwice>(HeadlessApp::builder()).await;
    let health = app
        .container()
        .get::<HealthService>()
        .expect("HealthModule provides it");

    for kind in [ProbeKind::Readiness, ProbeKind::Startup] {
        let report = health.probe(kind).await;
        assert_eq!(report.status, IndicatorStatus::Up, "{kind:?}: {report:#?}");
        assert_eq!(
            report
                .details
                .keys()
                .map(|key| &**key)
                .collect::<Vec<&str>>(),
            ["storage"],
            "{kind:?}: {report:#?}"
        );
    }
    assert!(
        health.probe(ProbeKind::Liveness).await.details.is_empty(),
        "a store outage is no reason to restart the process"
    );
}

#[tokio::test]
async fn a_client_a_test_overrides_opens_no_store_and_probes_none() {
    let builder = HeadlessApp::builder();
    let double = Storage::connect(&StorageConfig {
        url: "memory:///double".into(),
        ..StorageConfig::default()
    })
    .await
    .expect("memory opens with no network");
    let app = booted::<ImportedTwice>(builder.override_value(double)).await;
    let health = app
        .container()
        .get::<HealthService>()
        .expect("HealthModule provides it");

    let ready = health.probe(ProbeKind::Readiness).await;
    assert!(ready.details.is_empty(), "{ready:#?}");
}

#[module(imports = [StorageModule::for_root(in_memory())])]
struct ImportedUnprobed;

#[tokio::test]
async fn without_health_module_an_app_importing_the_store_boots_and_serves_no_probe() {
    let app = booted::<ImportedUnprobed>(HeadlessApp::builder()).await;
    assert!(
        app.container().get::<HealthService>().is_none(),
        "the indicator imports no probe of its own"
    );
}

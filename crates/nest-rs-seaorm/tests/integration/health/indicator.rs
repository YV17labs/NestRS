//! The indicator `SeaOrmModule::for_root` attaches, booted beside
//! `HealthModule` alone over a connection holding no pool, which no round-trip
//! reaches: the readiness and startup probes read `seaorm` down, once however
//! many modules open the pool, its binding included, and liveness waits on it
//! never. An app without `HealthModule` boots as it did.

use nest_rs_core::{App, module};
use nest_rs_health::{HealthModule, HealthService, IndicatorStatus, ProbeKind};
use nest_rs_seaorm::{SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule};
use sea_orm::DatabaseConnection;

/// The pool's config, seeded beside a connection that keeps the module from
/// dialling it.
fn unreached() -> SeaOrmConfig {
    SeaOrmConfig {
        url: "postgres://nestrs@127.0.0.1:9/nestrs".into(),
        ..SeaOrmConfig::default()
    }
}

/// `module`, booted over a connection holding no pool.
async fn booted<M: nest_rs_core::Module>() -> App {
    App::builder()
        .provide(unreached())
        .provide(DatabaseConnection::default())
        .module::<M>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("boots: {refused:#}"))
}

/// A feature opening the pool, as each feature reaching the database may.
#[module(imports = [SeaOrmModule])]
struct ReadingFeature;

/// An app opening the pool beside a feature that opens it too.
#[module(imports = [HealthModule, SeaOrmModule, ReadingFeature])]
struct OpenedTwice;

#[tokio::test]
async fn a_pool_no_round_trip_reaches_fails_readiness_and_startup_once_as_seaorm_never_liveness() {
    let app = booted::<OpenedTwice>().await;
    let health = app
        .container()
        .get::<HealthService>()
        .expect("HealthModule provides it");

    for kind in [ProbeKind::Readiness, ProbeKind::Startup] {
        let report = health.probe(kind).await;
        assert_eq!(
            report.status,
            IndicatorStatus::Down,
            "{kind:?}: {report:#?}"
        );
        assert_eq!(
            report.error.keys().map(|key| &**key).collect::<Vec<&str>>(),
            ["seaorm"],
            "{kind:?}: {report:#?}"
        );
    }
    let live = health.probe(ProbeKind::Liveness).await;
    assert_eq!(
        (live.status, live.details.len()),
        (IndicatorStatus::Up, 0),
        "a database outage is no reason to restart the process: {live:#?}"
    );
}

/// An app naming only the binding, which opens the pool it runs on.
#[module(imports = [HealthModule, SeaOrmDatabaseModule])]
struct OpenedByItsBinding;

#[tokio::test]
async fn a_pool_its_binding_opens_answers_the_probes_as_seaorm() {
    let app = booted::<OpenedByItsBinding>().await;
    let health = app
        .container()
        .get::<HealthService>()
        .expect("HealthModule provides it");
    let report = health.probe(ProbeKind::Readiness).await;
    assert_eq!(
        report.error.keys().map(|key| &**key).collect::<Vec<&str>>(),
        ["seaorm"],
        "{report:#?}"
    );
}

#[module(imports = [SeaOrmModule])]
struct OpenedUnprobed;

#[tokio::test]
async fn without_health_module_an_app_opening_the_pool_boots_and_serves_no_probe() {
    let app = booted::<OpenedUnprobed>().await;
    assert!(
        app.container().get::<HealthService>().is_none(),
        "the indicator imports no probe of its own"
    );
}

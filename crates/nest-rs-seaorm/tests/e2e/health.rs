//! The indicator `SeaOrmModule::for_root` attaches, against the suite's
//! Postgres: an app importing `HealthModule` and nothing else of health's
//! answers the readiness and startup probes with `seaorm` up.

use nest_rs_core::module;
use nest_rs_health::HealthModule;
use nest_rs_seaorm::SeaOrmModule;
use nest_rs_testing::TestApp;

#[module(imports = [HealthModule, SeaOrmModule])]
struct Probed;

#[tokio::test]
async fn the_pool_answers_the_readiness_and_startup_probes_as_seaorm() {
    let app = TestApp::builder()
        .provide(crate::module::config(None))
        .module::<Probed>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("boots: {refused:#}"));
    for probe in ["/health/ready", "/health/startup"] {
        let answer = app.http().get(probe).send().await;
        answer.assert_status_is_ok();
        answer
            .json()
            .await
            .value()
            .object()
            .get("info")
            .object()
            .get("seaorm")
            .object()
            .get("status")
            .assert_string("up");
    }
    app.close().await;
}

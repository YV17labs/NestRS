//! The harness boots the transport the **app** configured, not a fresh one,
//! pinned on the two fields that change the address a request must use.

use nest_rs_core::module;
use nest_rs_http::{
    ApiVersioning, DEFAULT_VERSION_HEADER, HttpConfig, HttpModule, controller, routes,
};
use nest_rs_testing::TestApp;
use poem::http::StatusCode;

#[controller(path = "/widgets", version = "1")]
struct WidgetsController;

#[routes]
impl WidgetsController {
    #[get("/")]
    #[public]
    async fn list(&self) -> String {
        "widgets".into()
    }
}

#[module(
    imports = [HttpModule::for_root(HttpConfig {
        global_prefix: Some("/api".into()),
        ..HttpConfig::default()
    })],
    providers = [WidgetsController],
)]
struct PrefixedApp;

#[tokio::test]
async fn the_harness_serves_under_the_global_prefix_the_module_declared() {
    let app = TestApp::for_module::<PrefixedApp>()
        .await
        .expect("a pinned HttpConfig is ordinary composition");

    let resp = app.http().get("/api/v1/widgets").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("widgets").await;

    app.http()
        .get("/v1/widgets")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[module(
    imports = [HttpModule::for_root(HttpConfig {
        versioning: ApiVersioning::Header,
        default_version: Some("1".into()),
        ..HttpConfig::default()
    })],
    providers = [WidgetsController],
)]
struct HeaderVersionedApp;

#[tokio::test]
async fn the_harness_resolves_the_version_strategy_the_module_declared() {
    let app = TestApp::for_module::<HeaderVersionedApp>()
        .await
        .expect("boots");

    let resp = app
        .http()
        .get("/widgets")
        .header(DEFAULT_VERSION_HEADER, "1")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("widgets").await;

    app.http()
        .get("/v1/widgets")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[module(
    imports = [HttpModule::for_root(HttpConfig {
        versioning: ApiVersioning::Header,
        default_version: Some("9".into()),
        ..HttpConfig::default()
    })],
    providers = [WidgetsController],
)]
struct MisversionedApp;

#[tokio::test]
async fn a_default_version_nothing_declares_fails_the_boot_without_openapi() {
    let err = TestApp::for_module::<MisversionedApp>()
        .await
        .err()
        .expect("a default version no controller declares is a boot failure")
        .to_string();
    assert!(
        err.contains(&nest_rs_config::var_name("http", "DEFAULT_VERSION")),
        "the boot failure names the variable to change: {err}",
    );
    assert!(err.contains('1'), "and the versions that do exist: {err}",);
}

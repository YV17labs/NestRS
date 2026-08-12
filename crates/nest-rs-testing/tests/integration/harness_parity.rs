//! The harness boots the transport the **app** configured, not a fresh one.
//!
//! `TestApp` used to build a bare `HttpTransport::new()`, so every field
//! `HttpModule::for_root(cfg)` sets — the global prefix, the versioning
//! strategy, the body cap, the request timeout, CORS, compression, the security
//! headers — was silently absent under test. A suite then asserted against a
//! transport the deployment never runs, which is the exact failure e2e exists
//! to catch: `CLAUDE.md` opens its testing section with "wiring bugs don't
//! surface in unit tests", and the wiring was what the harness dropped.
//!
//! Two fields are pinned here rather than all of them, chosen because they
//! change the *address* a request must use: a harness that gets these wrong
//! passes a suite whose app answers `404` in production.

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

    // And the un-prefixed address is not a second one. This is the assertion
    // that used to be backwards: the bare harness served here and 404'd above.
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

    // Under `header`, the URI form is not a second address — a fact a bare
    // harness could never observe, because it never installed the selector.
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
    // The check used to live in `nest-rs-openapi`, so an app that publishes no
    // document got no answer at all: every caller stating no version resolved
    // to a path that does not exist and fell through in silence. It is HTTP's
    // config, so it is now HTTP's boot failure — this app imports no
    // `OpenApiModule`.
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

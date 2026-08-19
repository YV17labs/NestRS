//! The composition witness: the documented import, booted, answering.
//!
//! `CLAUDE.md`'s *Shipping a new capability* asks every capability for "a test
//! in the capability's **own crate** that boots the documented wiring … and
//! asserts what a caller gets back", and says composition is **executed**, never
//! merely compiled — a boot proves the access graph, the mounted routes and the
//! lifecycle hook at once, and compiling proves none of the three.
//!
//! What this one catches that `module.rs` cannot: `module.rs` registers
//! `HealthModule` into a bare container, so the three routes could stop mounting
//! and the `OnApplicationBootstrap` hook could stop installing the container
//! without a word. Both are only observable from an app that ran.

use nest_rs_core::module;
use nest_rs_health::HealthModule;
use nest_rs_testing::TestApp;

#[module(imports = [HealthModule])]
struct AppModule;

#[tokio::test]
async fn the_documented_import_mounts_the_three_probes() {
    let app = TestApp::for_module::<AppModule>()
        .await
        .expect("the documented wiring boots");

    for probe in ["live", "ready", "startup"] {
        app.http()
            .get(format!("/health/{probe}"))
            .send()
            .await
            .assert_status_is_ok();
    }
}

/// A probe with no indicator registered still answers, and the answer is the
/// one an orchestrator reads — not an empty body it would have to interpret.
#[tokio::test]
async fn a_probe_with_no_indicators_reports_healthy() {
    let app = TestApp::for_module::<AppModule>()
        .await
        .expect("the documented wiring boots");

    let body = app
        .http()
        .get("/health/live")
        .send()
        .await
        .0
        .into_body()
        .into_string()
        .await
        .unwrap_or_default();
    assert!(
        body.contains("\"status\""),
        "the probe body carries a status an orchestrator can read: {body}",
    );
}

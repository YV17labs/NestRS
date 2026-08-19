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

/// What `global_prefix` does to a probe path, and what the framework owes the
/// operator about it.
///
/// A probe path is a contract with an orchestrator: a manifest written from
/// this crate's documented `/health/live` gets a `404` under a prefixed app,
/// the kubelet scores `404` as a failed probe, and a failed liveness probe
/// restarts the container. The transport nests every mount inside the prefix
/// — nothing this crate contributes can opt out — so the obligation it *can*
/// meet is that the difference is never silent.
mod under_a_global_prefix {
    use nest_rs_core::module;
    use nest_rs_health::HealthModule;
    use nest_rs_http::{HttpConfig, HttpModule};
    use nest_rs_testing::{LogCapture, TestApp};

    fn prefixed_http() -> nest_rs_http::HttpSetup {
        HttpModule::for_root(HttpConfig::default().with_global_prefix("/api/v1"))
    }

    #[module(imports = [prefixed_http(), HealthModule])]
    struct PrefixedApp;

    #[tokio::test]
    async fn the_probes_move_with_the_prefix_and_the_boot_says_where() {
        let logs = LogCapture::install();
        let app = TestApp::for_module::<PrefixedApp>()
            .await
            .expect("the prefixed wiring boots");

        let event = logs.expect_one(
            nest_rs_health::TARGET,
            "health probes are served under the HTTP global prefix",
        );
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("prefix").as_deref(), Some("/api/v1"));
        let served = event.field("served").unwrap_or_default();
        for probe in ["live", "ready", "startup"] {
            let path = format!("/api/v1/health/{probe}");
            assert!(
                served.contains(&path),
                "the line names the path the kubelet must call: {served}",
            );
            app.http().get(&path).send().await.assert_status_is_ok();
        }

        // …and the documented path is exactly the 404 the line exists to warn
        // about.
        app.http()
            .get("/health/live")
            .send()
            .await
            .assert_status(poem::http::StatusCode::NOT_FOUND);
    }

    /// An app with no prefix serves the documented paths and says nothing: the
    /// line is a report of a *difference*, not boot noise every app pays for.
    #[tokio::test]
    async fn an_unprefixed_app_says_nothing() {
        let logs = LogCapture::install();
        let _app = TestApp::for_module::<super::AppModule>()
            .await
            .expect("the documented wiring boots");
        assert!(
            logs.find(
                nest_rs_health::TARGET,
                "health probes are served under the HTTP global prefix",
            )
            .is_empty(),
            "no prefix, no line: {:#?}",
            logs.events(),
        );
    }
}

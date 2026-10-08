//! The composition witness: the documented import, booted, answering.

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

#[tokio::test]
async fn each_probe_is_documented_under_one_health_tag() {
    let app = TestApp::for_module::<AppModule>()
        .await
        .expect("the documented wiring boots");

    let routes: Vec<_> = nest_rs_core::Discovery::new(app.container())
        .meta::<nest_rs_http::HttpControllerMeta>()
        .into_iter()
        .flat_map(|d| d.meta.routes.clone())
        .collect();
    assert_eq!(routes.len(), 3, "the three probes");
    for route in &routes {
        assert_eq!(route.tags, ["Health"], "{}", route.handler);
        assert!(route.summary.is_some(), "{} has a summary", route.handler);
        assert!(
            route.response.is_some(),
            "{} types its report",
            route.handler
        );
    }
}

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

        app.http()
            .get("/health/live")
            .send()
            .await
            .assert_status(poem::http::StatusCode::NOT_FOUND);
    }

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

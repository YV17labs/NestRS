//! Covers `src/module.rs` — an `#[mcp]` host the app reaches without the HTTP
//! transport is named at run, with the import that serves it, and one the
//! transport serves is not.

use nest_rs_core::{App, AppLifecycle, module, target};
use nest_rs_http::{HttpConfig, HttpModule};
use nest_rs_mcp::{McpError, mcp, tools};
use nest_rs_testing::LogCapture;

/// What `App::run` files for a host whose edge's transport the app does not attach.
const UNSERVED: &str = "host not served: its edge's transport is not attached";

#[mcp]
#[derive(Clone, Default)]
struct ReportsTool;

#[tools]
impl ReportsTool {
    #[tool(description = "Count the reports.")]
    #[public]
    async fn count_reports(&self) -> Result<String, McpError> {
        Ok("0".into())
    }
}

/// A hand-written adapter that forgets the transport.
#[module(providers = [ReportsTool])]
struct ReportsMcpModule;

#[tokio::test]
async fn an_mcp_host_the_app_reaches_without_the_transport_is_named_at_run() {
    let logs = LogCapture::install();

    App::builder()
        .module::<ReportsMcpModule>()
        .build()
        .await
        .expect("the module boots")
        .run()
        .await
        .expect("an app serving nothing stops cleanly");

    let unserved = logs.expect_one(target::APP, UNSERVED);
    assert_eq!(unserved.level, "warn");
    assert_eq!(unserved.field("edge").as_deref(), Some("mcp"));
    assert_eq!(unserved.field("host").as_deref(), Some("ReportsTool"));
    assert_eq!(
        unserved.field("module").as_deref(),
        Some("ReportsMcpModule")
    );
    assert_eq!(
        unserved.field("import").as_deref(),
        Some("nest_rs::http::HttpModule")
    );
}

/// The tool's adapter beside the transport, on a free loopback port.
#[module(imports = [HttpModule::for_root(loopback()), ReportsMcpModule])]
struct ServedReportsModule;

fn loopback() -> HttpConfig {
    HttpConfig {
        host: "127.0.0.1".into(),
        port: 0,
        ..HttpConfig::default()
    }
}

#[tokio::test]
async fn an_mcp_host_the_transport_serves_is_not_named_at_run() {
    let logs = LogCapture::install();
    let app = App::builder()
        .module::<ServedReportsModule>()
        .build()
        .await
        .expect("the module boots");
    let lifecycle = app
        .container()
        .get::<AppLifecycle>()
        .expect("every app is seeded its lifecycle");

    // `App::run` names an unserved host before it turns ready, so the run is
    // dropped there.
    let running = tokio::spawn(app.run());
    assert!(lifecycle.ready().await, "the app serves");
    running.abort();

    logs.expect_one(target::APP, "app ready");
    logs.expect_none(target::APP, UNSERVED);
}

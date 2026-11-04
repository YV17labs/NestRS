//! Covers `src/module.rs` — an `#[mcp]` host the app reaches without the HTTP
//! transport is named at run, with the import that serves it, and one the
//! transport serves is not; an adapter importing `McpModule` bare reads
//! `<PREFIX>_MCP__*` for a root that names nothing of MCP's.

use nest_rs_core::{App, AppLifecycle, module, target};
use nest_rs_http::{HttpConfig, HttpModule};
use nest_rs_mcp::{McpConfig, McpError, McpIdentity, McpModule, mcp, tools};
use nest_rs_testing::{LogCapture, TestApp};

/// What `App::run` files for a host whose edge's transport the app does not attach.
const UNSERVED: &str = "host not served: its edge's transport is not attached";

#[mcp]
#[derive(Clone, Default)]
struct ReportsTool;

#[tools]
impl ReportsTool {
    #[tool(description = "Count the reports.")]
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
        Some("nest_rs::mcp::McpModule")
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

/// The tool's adapter as `nestrs g mcp` writes it: `McpModule` bare, which
/// brings the HTTP transport the endpoint mounts on.
#[module(imports = [McpModule], providers = [ReportsTool])]
struct ReportsAdapterModule;

/// A root importing the adapter, and nothing of MCP's own.
#[module(imports = [ReportsAdapterModule])]
struct AdapterOnlyRoot;

/// A root pinning only the app's identity beside the adapter, as the demo's
/// assistant does: the pin and the adapter's bare import are one module.
#[module(imports = [
    McpModule::for_root(McpIdentity::default().title("Reports")),
    ReportsAdapterModule,
])]
struct IdentityRoot;

/// The deployment's Host allowlist reaches the mount through the adapter's
/// bare import, whatever the root says of MCP: the variable the refusal's
/// hint names is the one the app reads.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn an_app_reads_the_mcp_variables_through_its_adapter() {
    // Sync on purpose: figment::Jail scopes the variable to its closure, so the
    // runtime opens inside it, on this thread, where the capture listens.
    figment::Jail::expect_with(|jail| {
        jail.set_env(
            nest_rs_config::var_name("mcp", "ALLOWED_HOSTS"),
            "mcp.example.com",
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        runtime.block_on(async {
            let logs = LogCapture::install();
            let adapter_only = TestApp::for_module::<AdapterOnlyRoot>()
                .await
                .expect("the adapter's root boots");
            let identity = TestApp::for_module::<IdentityRoot>()
                .await
                .expect("the identity's root boots");

            for app in [&adapter_only, &identity] {
                let config = app
                    .container()
                    .get::<McpConfig>()
                    .expect("the adapter's bare import resolves the config");
                assert_eq!(config.allowed_hosts, ["mcp.example.com"]);
            }
            let resolved = logs.find(nest_rs_mcp::TARGET, "mcp host allowlist resolved");
            assert_eq!(resolved.len(), 2, "one mount per app: {resolved:?}");
            for event in resolved {
                assert_eq!(
                    event.field("allowed_hosts").as_deref(),
                    Some(r#"["mcp.example.com"]"#),
                    "each mount runs on the deployment's allowlist",
                );
            }
        });
        Ok(())
    });
}

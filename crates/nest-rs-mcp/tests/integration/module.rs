//! Covers `src/module.rs` — a listed `#[mcp]` host brings `McpModule`, which
//! reads `<PREFIX>_MCP__*` and attaches the HTTP transport, for a root that
//! names nothing of MCP's and for an adapter that names `HttpModule` alone.

use nest_rs_core::{App, AppLifecycle, module, target};
use nest_rs_http::{HttpConfig, HttpModule};
use nest_rs_mcp::{McpConfig, McpError, McpIdentity, McpModule, mcp, tools};
use nest_rs_testing::{HeadlessApp, LogCapture, TestApp};

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

/// An adapter as a developer writes it: the tool, and nothing of the edge's.
#[module(providers = [ReportsTool])]
struct ReportsMcpModule;

#[tokio::test]
async fn a_listed_mcp_host_brings_its_module_and_the_transport_it_mounts_on() {
    let app = HeadlessApp::builder()
        .module::<ReportsMcpModule>()
        .build_headless()
        .await
        .expect("the module boots");

    assert!(
        app.container().get::<McpConfig>().is_some(),
        "the bare import reads the edge's variables",
    );
    assert_eq!(app.attached_transports(), ["http"]);
}

/// The tool's adapter beside the pinned transport, on a free loopback port.
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
async fn a_pinned_transport_serves_a_listed_mcp_host() {
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

    let running = tokio::spawn(app.run());
    assert!(lifecycle.ready().await, "the app serves");
    running.abort();

    logs.expect_one(target::APP, "app ready");
}

/// An adapter naming only the transport, the shape a hand-written one kept.
#[module(imports = [HttpModule], providers = [ReportsTool])]
struct HttpOnlyMcpModule;

/// A root importing the adapter, and nothing of MCP's own.
#[module(imports = [ReportsMcpModule])]
struct AdapterOnlyRoot;

/// A root importing the transport-only adapter, and nothing of MCP's own.
#[module(imports = [HttpOnlyMcpModule])]
struct HttpOnlyRoot;

/// A root pinning only the app's identity beside the adapter, as the demo's
/// assistant does: the pin and the adapter's bare import are one module.
#[module(imports = [
    McpModule::for_root(McpIdentity {
        title: Some("Reports".into()),
        ..Default::default()
    }),
    ReportsMcpModule,
])]
struct IdentityRoot;

/// The deployment's Host allowlist reaches the mount through what the host
/// brings, whatever its adapter or the root says of MCP: the variable the
/// refusal's hint names is the one the app reads.
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
            let http_only = TestApp::for_module::<HttpOnlyRoot>()
                .await
                .expect("the transport-only adapter's root boots");
            let identity = TestApp::for_module::<IdentityRoot>()
                .await
                .expect("the identity's root boots");

            for app in [&adapter_only, &http_only, &identity] {
                let config = app
                    .container()
                    .get::<McpConfig>()
                    .expect("the host's bare import resolves the config");
                assert_eq!(config.allowed_hosts, ["mcp.example.com"]);
            }
            let resolved = logs.find(nest_rs_mcp::TARGET, "mcp host allowlist resolved");
            assert_eq!(resolved.len(), 3, "one mount per app: {resolved:?}");
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

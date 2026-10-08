//! `#[mcp]` self-mounts its endpoint at its declared path, deny-all when no
//! `dyn McpOperationGuard` is wired.

use nest_rs_core::module;
use nest_rs_mcp::{ServerHandler, mcp, tool_handler, tool_router};
use nest_rs_testing::TestApp;
use poem::http::StatusCode;

#[mcp]
#[derive(Clone)]
struct EchoTool;

#[tool_router(allow_empty)]
impl EchoTool {}

#[tool_handler]
impl ServerHandler for EchoTool {}

#[module(providers = [EchoTool])]
struct McpMountModule;

#[tokio::test]
async fn mcp_tool_self_mounts_and_fails_closed_without_a_guard() {
    let app = TestApp::for_module::<McpMountModule>()
        .await
        .expect("boots");

    // 401, not 404: mounted, and closed without an operation guard.
    let resp = app.http().post("/mcp").send().await;
    resp.assert_status(StatusCode::UNAUTHORIZED);

    let resp = app.http().post("/elsewhere").send().await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

/// An empty `allowed_hosts` turns off rmcp's DNS-rebinding defence, invisibly to
/// a client: this warn, from `from_container`, is the whole control.
#[tokio::test]
async fn a_mount_with_no_allowlist_reports_that_host_validation_is_off() {
    let logs = nest_rs_testing::LogCapture::install();
    // The default allowlist is loopback; empty means `<PREFIX>_MCP__ALLOWED_HOSTS` was cleared.
    let container = nest_rs_core::Container::builder()
        .provide(nest_rs_mcp::McpConfig {
            allowed_hosts: Vec::new(),
            ..nest_rs_mcp::McpConfig::default()
        })
        .build();
    let _mount = nest_rs_mcp::McpMount::from_container(&container);

    let event = logs.expect_one(
        "nest_rs::mcp",
        "mcp host allowlist is empty — inbound Host headers are not validated",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(
        event.field("reason").as_deref(),
        Some("host_validation_disabled"),
    );
}

const LISTENING: &str = "/mcp/listening";

#[nest_rs_mcp::mcp(path = "/mcp/listening")]
#[derive(Clone, Default)]
struct ListeningTools;

#[nest_rs_mcp::tools]
impl ListeningTools {
    #[tool(description = "Answers at once.")]
    #[public]
    async fn ping(&self) -> Result<String, nest_rs_mcp::McpError> {
        Ok("pong".to_owned())
    }
}

#[module(providers = [
    ListeningTools,
    nest_rs_mcp::AllowAllMcpGuard as dyn nest_rs_mcp::McpOperationGuard,
])]
struct ListeningModule;

/// The standalone `GET` stream has no end of its own: it ends cleanly at the
/// signal, so the client reconnects elsewhere, and its line files `cancelled`.
#[tokio::test]
async fn the_standalone_stream_ends_at_the_shutdown_signal() {
    let logs = nest_rs_testing::LogCapture::install();
    let window = std::time::Duration::from_secs(5);
    let serving = crate::serve_on_loopback::<ListeningModule>(window).await;
    let session = crate::open_raw_session(serving.port, LISTENING).await;
    let (mut stream, head) = crate::get_stream(serving.port, LISTENING, &session).await;
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "the stream opened: {head}"
    );

    let took = serving.stop().await;

    assert!(
        took < std::time::Duration::from_secs(1),
        "an idle client no longer holds the replica for its {window:?} window, took {took:?}",
    );
    let rest = crate::read_to_end(&mut stream).await;
    assert!(
        rest.ends_with("0\r\n\r\n"),
        "the stream ended with its last chunk rather than a cut: {rest:?}",
    );
    let lines: Vec<_> = logs
        .find(
            nest_rs_core::operation_log::TARGET,
            nest_rs_http::unit::REQUEST.name(),
        )
        .into_iter()
        .filter(|line| line.field("method").as_deref() == Some("GET"))
        .collect();
    assert_eq!(lines.len(), 1, "one line for the stream: {lines:#?}");
    assert_eq!(lines[0].field("status").as_deref(), Some("200"));
    assert_eq!(
        lines[0].field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
    );
}

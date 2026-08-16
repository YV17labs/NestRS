//! `#[mcp]` mount expansion through a real boot: the decorated tool host
//! self-mounts its endpoint on the HTTP transport at its declared path
//! (`HttpEndpointMeta`, posture `Exempt`), and with no
//! `dyn McpOperationGuard` wired it serves deny-all — mounted but closed,
//! never an open tool surface and never a silent no-mount.

use nest_rs_core::module;
use nest_rs_mcp::{ServerHandler, mcp, tool_handler, tool_router};
use nest_rs_testing::TestApp;
use poem::http::StatusCode;

#[mcp]
#[derive(Clone)]
struct EchoTool;

#[tool_router]
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

    // 401 — the path is mounted (a no-mount would 404) and the missing
    // operation guard falls back to deny-all rather than serving open.
    let resp = app.http().post("/mcp").send().await;
    resp.assert_status(StatusCode::UNAUTHORIZED);

    // The mount is scoped to its declared path, not a catch-all.
    let resp = app.http().post("/elsewhere").send().await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

/// The mount says, once, that its Host allowlist is empty.
///
/// An empty `allowed_hosts` turns off rmcp's DNS-rebinding defence. Nothing
/// about that is observable from a client — the endpoint answers identically
/// either way — and the deployment that needs it most is the one that never set
/// `NESTRS_MCP__ALLOWED_HOSTS`. So this warn is the whole control, and it is
/// emitted from `from_container`, the path a real mount takes; `deny_all()`
/// skips it because it builds no config at all.
#[tokio::test]
async fn a_mount_with_no_allowlist_reports_that_host_validation_is_off() {
    let logs = nest_rs_testing::LogCapture::install();
    // The default carries a loopback allowlist, which is correct for the local
    // server it protects — so the empty case is a deployment that *cleared*
    // `NESTRS_MCP__ALLOWED_HOSTS`, and that is what this pins.
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

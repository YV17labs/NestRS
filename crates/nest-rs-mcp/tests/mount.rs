//! `#[mcp]` mount expansion through a real boot: the decorated tool host
//! self-mounts its endpoint on the HTTP transport at its declared path
//! (`HttpEndpointMeta`, posture `Exempt`), and with no
//! `dyn McpOperationGuard` wired it serves deny-all — mounted but closed,
//! never an open tool surface and never a silent no-mount.

use nest_rs_core::module;
use nest_rs_mcp::{ServerHandler, mcp, tool_handler, tool_router};
use nest_rs_testing::TestApp;
use poem::http::StatusCode;

#[mcp(path = "/mcp")]
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

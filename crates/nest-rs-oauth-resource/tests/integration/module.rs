//! Covers `src/module.rs`: what importing the module adds beyond its document
//! and challenge — the MCP endpoint answers the `Host` its resource names.

use nest_rs_authn::{AuthnConfig, AuthnModule};
use nest_rs_core::{Posture, module};
use nest_rs_http::StatusCode;
use nest_rs_mcp::{McpConfig, McpModule, McpOptions};
use nest_rs_oauth_resource::{OAuthResourceConfig, OAuthResourceModule};
use nest_rs_testing::TestApp;

use crate::EchoTool;

const RESOURCE: &str = "https://mcp.example.com/mcp";

#[module(
    imports = [
        AuthnModule::for_root(AuthnConfig {
            secret: Some("allowlist-secret-padded-to-32-byt".into()),
            ..AuthnConfig::default()
        }),
        OAuthResourceModule::for_root(OAuthResourceConfig {
            resource: Some(RESOURCE.into()),
            authorization_servers: vec!["https://auth.example.com".into()],
            ..OAuthResourceConfig::default()
        }),
        McpModule::for_root(McpOptions { posture: Posture::Public, ..Default::default() }),
    ],
    providers = [EchoTool],
)]
struct ResourceServer;

async fn boot(mcp: McpConfig) -> TestApp {
    TestApp::builder()
        .provide(mcp)
        .module::<ResourceServer>()
        .build()
        .await
        .expect("boots")
}

/// `tools/list` sent to `/mcp` with `Host: host`, the status rmcp's host check
/// answers.
async fn status_for(app: &TestApp, host: &str) -> StatusCode {
    app.http()
        .post("/mcp")
        .header("host", host)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body_json(&serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
        .send()
        .await
        .0
        .status()
}

/// The resource is the host a client reaches the server by: the allowlist
/// admits it beside its own entries, so a deployment names its host once.
#[tokio::test]
async fn the_mcp_allowlist_admits_the_host_the_resource_names() {
    let app = boot(McpConfig::default()).await;

    for host in ["mcp.example.com", "mcp.example.com:443", "localhost"] {
        assert_ne!(
            status_for(&app, host).await,
            StatusCode::FORBIDDEN,
            "{host} is admitted"
        );
    }
    assert_eq!(
        status_for(&app, "rebound.example").await,
        StatusCode::FORBIDDEN,
        "a host neither lists stays refused"
    );
}

/// An empty allowlist is a deliberate opening: the resource's host never
/// turns the check back on behind it.
#[tokio::test]
async fn an_emptied_allowlist_stays_open() {
    let app = boot(McpConfig {
        allowed_hosts: Vec::new(),
        ..McpConfig::default()
    })
    .await;

    assert_ne!(
        status_for(&app, "rebound.example").await,
        StatusCode::FORBIDDEN
    );
}

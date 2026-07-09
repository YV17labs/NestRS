use assistant::AssistantModule;
use nest_rs_core::DiscoveryService;
use nest_rs_http::HttpEndpointMeta;
use nest_rs_testing::TestApp;
use serde_json::json;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<AssistantModule>()
        .with_test_telemetry()
        .build()
        .await
        .expect("AssistantModule boots")
}

#[tokio::test]
async fn health_live_probe_is_ok() {
    let app = boot().await;
    app.http()
        .get("/health/live")
        .send()
        .await
        .assert_status_is_ok();
}

#[tokio::test]
async fn weather_tool_self_mounts_the_mcp_endpoint() {
    let app = boot().await;
    let endpoints = DiscoveryService::new(app.container()).meta::<HttpEndpointMeta>();
    assert!(
        endpoints
            .iter()
            .any(|d| d.meta.label() == "mcp" && d.meta.path() == "/mcp"),
        "the #[mcp] WeatherTool self-mounts an MCP endpoint at /mcp",
    );
}

#[tokio::test]
async fn mcp_endpoint_accepts_an_initialize_request() {
    let app = boot().await;

    let resp = app
        .http()
        .post("/mcp")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body_json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "nestrs-e2e", "version": "0" }
            }
        }))
        .send()
        .await;

    resp.assert_status_is_ok();
}

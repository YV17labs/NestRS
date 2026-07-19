use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use assistant::AssistantModule;
use features::{Claims, Role};
use nest_rs_authn::{JwtConfig, JwtOptions, JwtService};
use nest_rs_core::DiscoveryService;
use nest_rs_http::HttpEndpointMeta;
use nest_rs_storage::{Storage, StorageConfig};
use nest_rs_testing::TestApp;
use poem::http::{StatusCode, header};
use serde_json::json;
use uuid::Uuid;

const ORG_ID: &str = "018f0000-0000-7000-8000-000000000000";

const DEV_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIEYTRN4vmCuIfaUslO5G9pKyxkDJn3q3t9WDHo2FCfw3\n-----END PRIVATE KEY-----\n";
const DEV_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAHfPOjd2Y3m1BLM5nBJBMZFAlfWt69WL1NY8XyYeGfeo=\n-----END PUBLIC KEY-----\n";

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<AssistantModule>()
        .with_test_telemetry()
        .provide(JwtConfig {
            public_key: Some(DEV_PUBLIC_KEY.into()),
            ..Default::default()
        })
        .build()
        .await
        .expect("AssistantModule boots")
}

fn bearer() -> String {
    let jwt = JwtService::new(JwtOptions::eddsa(DEV_PRIVATE_KEY, DEV_PUBLIC_KEY))
        .expect("the dev keypair parses");
    let token = jwt
        .sign(&Claims {
            sub: None,
            org_id: Uuid::parse_str(ORG_ID).expect("valid org uuid"),
            roles: vec![Role::Admin],
            exp: jwt.expiry(),
        })
        .expect("sign the test token");
    format!("Bearer {token}")
}

fn storage_client() -> Storage {
    let mut config = StorageConfig::default();
    if let Ok(v) = std::env::var("NESTRS_STORAGE__ENDPOINT") {
        config.endpoint = v;
    }
    if let Ok(v) = std::env::var("NESTRS_STORAGE__ACCESS_KEY") {
        config.access_key = v;
    }
    if let Ok(v) = std::env::var("NESTRS_STORAGE__SECRET_KEY") {
        config.secret_key = v;
    }
    if let Ok(v) = std::env::var("NESTRS_STORAGE__BUCKET") {
        config.bucket = v;
    }
    Storage::new(Arc::new(config))
}

async fn ensure_bucket() {
    if let Ok(url) = storage_client()
        .presign_put("", Duration::from_secs(60))
        .await
    {
        let _ = reqwest::Client::new().put(&url).send().await;
    }
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
async fn audio_tool_self_mounts_the_mcp_endpoint() {
    let app = boot().await;
    let endpoints = DiscoveryService::new(app.container()).meta::<HttpEndpointMeta>();
    assert!(
        endpoints
            .iter()
            .any(|d| d.meta.label() == "mcp" && d.meta.path() == "/mcp"),
        "the #[mcp] AudioTool self-mounts an MCP endpoint at /mcp",
    );
}

#[tokio::test]
async fn mcp_endpoint_refuses_an_unauthenticated_request() {
    let app = boot().await;
    let resp = app
        .http()
        .post("/mcp")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body_json(&initialize_request())
        .send()
        .await;
    assert_eq!(
        resp.0.status(),
        StatusCode::UNAUTHORIZED,
        "no token — the MCP endpoint must refuse before reaching the tool",
    );
}

fn initialize_request() -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "nestrs-e2e", "version": "0" }
        }
    })
}

#[tokio::test]
async fn audio_tool_reports_transcode_status_through_a_guarded_session() {
    let app = boot().await;
    ensure_bucket().await;
    let auth = bearer();

    let init = app
        .http()
        .post("/mcp")
        .header("host", "localhost")
        .header(header::AUTHORIZATION, &auth)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body_json(&initialize_request())
        .send()
        .await;
    init.assert_status_is_ok();
    let session = init
        .0
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .expect("initialize returns an Mcp-Session-Id header")
        .to_owned();

    let ack = app
        .http()
        .post("/mcp")
        .header("host", "localhost")
        .header(header::AUTHORIZATION, &auth)
        .header("mcp-session-id", &session)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body_json(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .send()
        .await;
    assert!(
        ack.0.status().is_success(),
        "initialized notification is accepted: {}",
        ack.0.status(),
    );

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let file = format!("e2e-mcp-{}-{}.mp3", std::process::id(), nonce);
    let call = |id: u32, file: String| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": "transcode_status", "arguments": { "file": file } }
        })
    };

    let pending = app
        .http()
        .post("/mcp")
        .header("host", "localhost")
        .header(header::AUTHORIZATION, &auth)
        .header("mcp-session-id", &session)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body_json(&call(2, file.clone()))
        .send()
        .await;
    pending.assert_status_is_ok();
    let body = pending.0.into_body().into_string().await.expect("sse body");
    assert!(
        body.contains("pending"),
        "a fresh key has no derived object yet: {body}",
    );

    storage_client()
        .put_bytes(
            &format!("transcoded/{file}"),
            b"derived bytes".to_vec(),
            "audio/mpeg",
        )
        .await
        .expect("seed the derived object into live RustFS");

    let ready = app
        .http()
        .post("/mcp")
        .header("host", "localhost")
        .header(header::AUTHORIZATION, &auth)
        .header("mcp-session-id", &session)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body_json(&call(3, file.clone()))
        .send()
        .await;
    ready.assert_status_is_ok();
    let body = ready.0.into_body().into_string().await.expect("sse body");
    assert!(
        body.contains("ready") && body.contains("download"),
        "the seeded derived object flips the tool to ready + link: {body}",
    );
}

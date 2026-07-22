use std::time::{SystemTime, UNIX_EPOCH};

use nest_rs_core::DiscoveryService;
use nest_rs_http::HttpEndpointMeta;
use nest_rs_testing::mcp::{initialize_request, open_session, post_message};
use poem::http::StatusCode;
use serde_json::json;

use super::harness::*;

#[tokio::test]
async fn health_live_probe_is_ok() {
    let (_db, app) = boot().await;
    app.http()
        .get("/health/live")
        .send()
        .await
        .assert_status_is_ok();
}

#[tokio::test]
async fn audio_tool_self_mounts_the_mcp_endpoint() {
    let (_db, app) = boot().await;
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
    let (_db, app) = boot().await;
    let resp = post_message(app.http(), "/mcp", None, None, &initialize_request()).await;
    assert_eq!(
        resp.0.status(),
        StatusCode::UNAUTHORIZED,
        "no token — the MCP endpoint must refuse before reaching the tool",
    );
}

#[tokio::test]
async fn audio_tool_reports_transcode_status_through_a_guarded_session() {
    let (_db, app) = boot().await;
    ensure_bucket().await;
    let auth = bearer();

    let session = open_session(app.http(), "/mcp", Some(&auth)).await;

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

    let pending = post_message(
        app.http(),
        "/mcp",
        Some(&session),
        Some(&auth),
        &call(2, file.clone()),
    )
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

    let ready = post_message(
        app.http(),
        "/mcp",
        Some(&session),
        Some(&auth),
        &call(3, file.clone()),
    )
    .await;
    ready.assert_status_is_ok();
    let body = ready.0.into_body().into_string().await.expect("sse body");
    assert!(
        body.contains("ready") && body.contains("download"),
        "the seeded derived object flips the tool to ready + link: {body}",
    );
}

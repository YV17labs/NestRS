use nest_rs::core::Discovery;
use nest_rs::http::HttpEndpointMeta;
use nest_rs::http::StatusCode;
use nest_rs::storage::Storage;
use nest_rs::testing::mcp::{initialize_request, open_session, post_message};
use nest_rs::testing::{CapturedEvent, LogCapture};
use serde_json::json;

use crate::*;

fn is_rmcp_own_event_below_info(event: &CapturedEvent) -> bool {
    (event.target == "rmcp" || event.target.starts_with("rmcp::"))
        && matches!(event.level.as_str(), "debug" | "trace")
}

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
    let endpoints = Discovery::new(app.container()).meta::<HttpEndpointMeta>();
    assert!(
        endpoints
            .iter()
            .any(|d| d.meta.label() == "mcp" && d.meta.path() == SHARED_ENDPOINT),
        "the #[mcp] AudioTool self-mounts an MCP endpoint at /mcp",
    );
}

#[tokio::test]
async fn mcp_endpoint_refuses_an_unauthenticated_request() {
    let (_db, app) = boot().await;
    let resp = post_message(
        app.http(),
        SHARED_ENDPOINT,
        None,
        None,
        &initialize_request(),
    )
    .await;
    assert_eq!(
        resp.0.status(),
        StatusCode::UNAUTHORIZED,
        "no token — the MCP endpoint must refuse before reaching the tool",
    );
}

#[tokio::test]
async fn audio_tool_reports_transcode_status_through_a_guarded_session() {
    let (_db, app) = boot().await;
    let auth = bearer();

    let session = open_session(app.http(), SHARED_ENDPOINT, Some(&auth)).await;

    let source = Uuid::now_v7();
    let file = source.to_string();
    let call = |id: u32, file: String| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": "transcode_status", "arguments": { "file": file } }
        })
    };

    let logs = LogCapture::install();
    let pending = post_message(
        app.http(),
        SHARED_ENDPOINT,
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
    assert!(
        !body.contains(&file)
            && logs
                .events()
                .iter()
                .filter(|event| !is_rmcp_own_event_below_info(event))
                .all(|event| !format!("{:?}", event.fields).contains(&file)),
        "a poll for an unknown key replies and logs none of it: {body} {:#?}",
        logs.events(),
    );
    drop(logs);

    app.container()
        .get::<Storage>()
        .expect("AssistantModule opens the audio feature's store")
        .put_bytes(
            &format!("transcoded/{file}"),
            b"derived bytes".to_vec(),
            "audio/mpeg",
        )
        .await
        .expect("seed the derived object into live RustFS");

    let ready = post_message(
        app.http(),
        SHARED_ENDPOINT,
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

    delete_transcode(app.container(), source).await;
}

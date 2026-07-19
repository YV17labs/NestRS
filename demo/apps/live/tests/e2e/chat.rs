//! Chat/notify gateways over a real socket: echo, broadcast, presence, namespaces.

use futures_util::SinkExt;
use nest_rs_http::HttpTransport;
use nest_rs_http::poem::http::StatusCode;
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

use super::harness::*;

#[tokio::test]
async fn gateway_endpoint_is_mounted() {
    let app = boot_builder().build().await.expect("LiveModule boots");

    let resp = app.http().get("/ws").send().await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn gateway_echoes_messages_over_a_real_socket() {
    let bind = "127.0.0.1:13344";

    let app = boot_builder()
        .build_headless()
        .await
        .expect("LiveModule boots headless");
    let handle = app
        .spawn_transport(HttpTransport::new().bind(bind))
        .await
        .expect("HTTP transport serves");
    let token = test_token().await;

    let mut socket = connect_with_retry(&format!("ws://{bind}/ws"), &token).await;

    socket
        .send(Message::Text(
            json!({ "event": "message", "data": { "author": "ada", "text": "hello" } })
                .to_string()
                .into(),
        ))
        .await
        .expect("send message");
    let echoed = next_json(&mut socket).await;
    assert_eq!(echoed["event"], "message");
    assert_eq!(echoed["data"]["author"], "ada");
    assert_eq!(echoed["data"]["text"], "hello");

    socket
        .send(Message::Text(
            json!({ "event": "history" }).to_string().into(),
        ))
        .await
        .expect("send history");
    let history = next_json(&mut socket).await;
    assert_eq!(history["event"], "history");
    assert_eq!(history["data"].as_array().expect("array").len(), 1);
    assert_eq!(history["data"][0]["text"], "hello");

    socket
        .send(Message::Text(json!({ "event": "nope" }).to_string().into()))
        .await
        .expect("send unknown");
    let unknown = next_json(&mut socket).await;
    assert!(
        unknown["data"]["error"]
            .as_str()
            .expect("error string")
            .contains("unknown event")
    );

    socket.close(None).await.ok();
    handle.shutdown().await.expect("transport shuts down");
}

#[tokio::test]
async fn a_message_is_broadcast_to_every_connected_client() {
    let bind = "127.0.0.1:13345";

    let app = boot_builder()
        .build_headless()
        .await
        .expect("LiveModule boots headless");
    let handle = app
        .spawn_transport(HttpTransport::new().bind(bind))
        .await
        .expect("HTTP transport serves");
    let token = test_token().await;

    let mut alice = connect_with_retry(&format!("ws://{bind}/ws"), &token).await;
    let mut bob = connect_with_retry(&format!("ws://{bind}/ws"), &token).await;

    alice
        .send(Message::Text(
            json!({ "event": "message", "data": { "author": "alice", "text": "hi all" } })
                .to_string()
                .into(),
        ))
        .await
        .expect("alice sends");

    let to_alice = next_json(&mut alice).await;
    let to_bob = next_json(&mut bob).await;
    for frame in [&to_alice, &to_bob] {
        assert_eq!(frame["event"], "message");
        assert_eq!(frame["data"]["author"], "alice");
        assert_eq!(frame["data"]["text"], "hi all");
    }

    alice.close(None).await.ok();
    bob.close(None).await.ok();
    handle.shutdown().await.expect("transport shuts down");
}

#[tokio::test]
async fn lifecycle_hooks_track_presence_and_a_per_message_guard_rejects_a_banned_author() {
    let bind = "127.0.0.1:13346";

    let app = boot_builder()
        .build_headless()
        .await
        .expect("LiveModule boots headless");
    let handle = app
        .spawn_transport(HttpTransport::new().bind(bind))
        .await
        .expect("HTTP transport serves");
    let token = test_token().await;

    let mut alice = connect_with_retry(&format!("ws://{bind}/ws"), &token).await;
    wait_for_presence(&mut alice, 1).await;
    let mut bob = connect_with_retry(&format!("ws://{bind}/ws"), &token).await;
    wait_for_presence(&mut alice, 2).await;

    bob.send(Message::Text(
        json!({ "event": "message", "data": { "author": "banned", "text": "hi" } })
            .to_string()
            .into(),
    ))
    .await
    .expect("bob sends");
    let denied = next_json(&mut bob).await;
    assert_eq!(denied["event"], "message");
    assert!(
        denied["data"]["error"]
            .as_str()
            .expect("error string")
            .contains("not allowed")
    );

    bob.close(None).await.ok();
    wait_for_presence(&mut alice, 1).await;

    alice.close(None).await.ok();
    handle.shutdown().await.expect("transport shuts down");
}

#[tokio::test]
async fn namespaced_gateways_isolate_their_broadcasts() {
    let bind = "127.0.0.1:13347";

    let app = boot_builder()
        .build_headless()
        .await
        .expect("LiveModule boots headless");
    let handle = app
        .spawn_transport(HttpTransport::new().bind(bind))
        .await
        .expect("HTTP transport serves");
    let token = test_token().await;

    let mut chat = connect_with_retry(&format!("ws://{bind}/ws"), &token).await;
    let mut notify = connect_with_retry(&format!("ws://{bind}/notify"), &token).await;

    chat.send(Message::Text(
        json!({ "event": "message", "data": { "author": "ada", "text": "hi" } })
            .to_string()
            .into(),
    ))
    .await
    .expect("chat sends");
    assert_eq!(next_json(&mut chat).await["event"], "message");
    assert_no_frame(&mut notify).await;

    notify
        .send(Message::Text(json!({ "event": "ping" }).to_string().into()))
        .await
        .expect("notify sends");
    assert_eq!(next_json(&mut notify).await["event"], "pong");
    assert_no_frame(&mut chat).await;

    chat.close(None).await.ok();
    notify.close(None).await.ok();
    handle.shutdown().await.expect("transport shuts down");
}

use nest_rs::http::poem::http::StatusCode;
use nest_rs::ws::CloseCode;
use serde_json::{Value, json};

use super::harness::*;

const QUIET: std::time::Duration = std::time::Duration::from_millis(150);

#[tokio::test]
async fn gateway_endpoint_is_mounted() {
    let app = boot_builder().build().await.expect("LiveModule boots");

    let resp = app.http().get("/ws").send().await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn gateway_echoes_messages_over_a_real_socket() {
    let app = serve().await;
    let mut socket = open(&app, "/ws").await;

    socket
        .send("message", json!({ "author": "ada", "text": "hello" }))
        .await;
    let echoed = socket.next_envelope().await;
    assert_eq!(echoed["event"], "message");
    assert_eq!(echoed["data"]["author"], "ada");
    assert_eq!(echoed["data"]["text"], "hello");

    socket.send("history", Value::Null).await;
    let history = socket.next_envelope().await;
    assert_eq!(history["event"], "history");
    assert_eq!(history["data"].as_array().expect("array").len(), 1);
    assert_eq!(history["data"][0]["text"], "hello");

    socket.send("nope", Value::Null).await;
    let unknown = socket.next_envelope().await;
    assert!(
        unknown["data"]["error"]
            .as_str()
            .expect("error string")
            .contains("unknown event")
    );

    socket.close(CloseCode::Normal, "done").await;
    app.shutdown().await.expect("transport shuts down");
}

#[tokio::test]
async fn a_request_scoped_provider_is_reachable_per_message_over_ws() {
    let app = serve().await;
    let mut socket = open(&app, "/ws").await;

    socket.send("seq", Value::Null).await;
    let first = socket.next_envelope().await;
    assert_eq!(first["event"], "seq");
    let first_seq = first["data"].as_u64().expect("seq is a number");

    socket.send("seq", Value::Null).await;
    let second = socket.next_envelope().await;
    let second_seq = second["data"].as_u64().expect("seq is a number");

    assert_ne!(
        first_seq, second_seq,
        "each WS message must build its own request-scoped RequestSeq (per-message scope)",
    );

    socket.close(CloseCode::Normal, "done").await;
    app.shutdown().await.expect("transport shuts down");
}

#[tokio::test]
async fn a_message_is_broadcast_to_every_connected_client() {
    let app = serve().await;
    let mut alice = open(&app, "/ws").await;
    let mut bob = open(&app, "/ws").await;

    alice
        .send("message", json!({ "author": "alice", "text": "hi all" }))
        .await;

    let to_alice = alice.next_envelope().await;
    let to_bob = bob.next_envelope().await;
    for frame in [&to_alice, &to_bob] {
        assert_eq!(frame["event"], "message");
        assert_eq!(frame["data"]["author"], "alice");
        assert_eq!(frame["data"]["text"], "hi all");
    }

    alice.close(CloseCode::Normal, "done").await;
    bob.close(CloseCode::Normal, "done").await;
    app.shutdown().await.expect("transport shuts down");
}

#[tokio::test]
async fn lifecycle_hooks_track_presence_and_a_per_message_guard_rejects_a_banned_author() {
    let app = serve().await;
    let mut alice = open(&app, "/ws").await;
    wait_for_presence(&mut alice, 1).await;
    let mut bob = open(&app, "/ws").await;
    wait_for_presence(&mut alice, 2).await;

    bob.send("message", json!({ "author": "banned", "text": "hi" }))
        .await;
    let denied = bob.next_envelope().await;
    assert_eq!(denied["event"], "message");
    assert!(
        denied["data"]["error"]
            .as_str()
            .expect("error string")
            .contains("not allowed")
    );

    bob.close(CloseCode::Normal, "done").await;
    wait_for_presence(&mut alice, 1).await;

    alice.close(CloseCode::Normal, "done").await;
    app.shutdown().await.expect("transport shuts down");
}

#[tokio::test]
async fn namespaced_gateways_isolate_their_broadcasts() {
    let app = serve().await;
    let mut chat = open(&app, "/ws").await;
    let mut notify = open(&app, "/notify").await;

    chat.send("message", json!({ "author": "ada", "text": "hi" }))
        .await;
    assert_eq!(chat.next_envelope().await["event"], "message");
    notify.expect_silence(QUIET).await;

    notify.send("ping", Value::Null).await;
    assert_eq!(notify.next_envelope().await["event"], "pong");
    chat.expect_silence(QUIET).await;

    chat.close(CloseCode::Normal, "done").await;
    notify.close(CloseCode::Normal, "done").await;
    app.shutdown().await.expect("transport shuts down");
}

#[tokio::test]
async fn an_oversized_message_is_rejected_and_closes_the_socket() {
    let app = serve().await;
    let mut socket = open(&app, "/ws").await;

    let oversized = "x".repeat(128 * 1024);
    socket
        .send("message", json!({ "author": "ada", "text": oversized }))
        .await;

    let (code, _) = socket.expect_close().await;
    assert_eq!(
        code,
        CloseCode::Error,
        "an over-cap message ends the socket with a status, never a bare drop",
    );

    app.shutdown().await.expect("transport shuts down");
}

#[tokio::test]
async fn the_socket_lifetime_ceiling_closes_the_socket() {
    let app = boot_builder()
        .provide(
            nest_rs::ws::WsConfig::default().with_max_connection(std::time::Duration::from_secs(1)),
        )
        .build_ws()
        .await
        .expect("LiveModule serves on a real port");
    let mut socket = open(&app, "/ws").await;

    let (code, reason) = socket.expect_close().await;
    assert_eq!(
        code,
        CloseCode::Away,
        "the ceiling asks the client to re-upgrade, so it says so",
    );
    assert!(reason.contains("re-upgrade"), "{reason}");

    app.shutdown().await.expect("transport shuts down");
}

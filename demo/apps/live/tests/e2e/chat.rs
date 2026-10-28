use nest_rs::http::StatusCode;
use nest_rs::ws::CloseCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::harness::*;

const QUIET: std::time::Duration = std::time::Duration::from_millis(150);

#[tokio::test]
async fn gateway_endpoint_is_mounted() {
    let (_db, app) = boot().await;

    let resp = app.http().get("/ws").send().await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn gateway_echoes_messages_over_a_real_socket() {
    let (_db, app) = serve().await;
    let ada = Uuid::now_v7();
    let mut socket = open_as(&app, "/ws", &user_token(ada)).await;

    socket.send("message", json!({ "text": "hello" })).await;
    let echoed = socket.next_envelope().await;
    assert_eq!(echoed["event"], "message");
    assert_eq!(echoed["data"]["author"], ada.to_string());
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
    let (_db, app) = serve().await;
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
    let (_db, app) = serve().await;
    let alice_id = Uuid::now_v7();
    let mut alice = open_as(&app, "/ws", &user_token(alice_id)).await;
    let mut bob = open(&app, "/ws").await;

    alice.send("message", json!({ "text": "hi all" })).await;

    let to_alice = alice.next_envelope().await;
    let to_bob = bob.next_envelope().await;
    for frame in [&to_alice, &to_bob] {
        assert_eq!(frame["event"], "message");
        assert_eq!(frame["data"]["author"], alice_id.to_string());
        assert_eq!(frame["data"]["text"], "hi all");
    }

    alice.close(CloseCode::Normal, "done").await;
    bob.close(CloseCode::Normal, "done").await;
    app.shutdown().await.expect("transport shuts down");
}

#[tokio::test]
async fn a_message_naming_an_author_is_refused_and_reaches_no_one() {
    let (_db, app) = serve().await;
    let alice_id = Uuid::now_v7();
    let mut alice = open_as(&app, "/ws", &user_token(alice_id)).await;
    wait_for_presence(&mut alice, 1).await;
    let mut mallory = open(&app, "/ws").await;
    wait_for_presence(&mut alice, 2).await;

    for event in ["message", "typing"] {
        mallory
            .send(
                event,
                json!({ "author": alice_id.to_string(), "text": "hi" }),
            )
            .await;
        let refused = mallory.next_envelope().await;
        assert_eq!(refused["event"], event);
        assert!(refused["data"]["error"].is_string(), "{refused}");
        assert!(
            !refused.to_string().contains(&alice_id.to_string()),
            "the refusal never quotes the name it was sent: {refused}",
        );
    }
    alice.expect_silence(QUIET).await;

    mallory.send("history", Value::Null).await;
    let history = mallory.next_envelope().await;
    assert_eq!(history["event"], "history");
    assert_eq!(history["data"], json!([]));

    mallory.close(CloseCode::Normal, "done").await;
    alice.close(CloseCode::Normal, "done").await;
    app.shutdown().await.expect("transport shuts down");
}

#[tokio::test]
async fn typing_is_broadcast_under_the_callers_name() {
    let (_db, app) = serve().await;
    let alice_id = Uuid::now_v7();
    let mut alice = open_as(&app, "/ws", &user_token(alice_id)).await;
    let mut bob = open(&app, "/ws").await;

    alice.send("typing", json!({ "text": "..." })).await;

    for frame in [alice.next_envelope().await, bob.next_envelope().await] {
        assert_eq!(frame["event"], "typing");
        assert_eq!(frame["data"]["author"], alice_id.to_string());
        assert_eq!(frame["data"]["text"], "...");
    }

    alice.close(CloseCode::Normal, "done").await;
    bob.close(CloseCode::Normal, "done").await;
    app.shutdown().await.expect("transport shuts down");
}

#[tokio::test]
async fn lifecycle_hooks_track_presence_and_a_per_message_guard_refuses_a_token_naming_no_user() {
    let (_db, app) = serve().await;
    let mut alice = open(&app, "/ws").await;
    wait_for_presence(&mut alice, 1).await;
    let mut bot = open_as(&app, "/ws", &machine_token()).await;
    wait_for_presence(&mut alice, 2).await;

    for event in ["message", "typing"] {
        bot.send(event, json!({ "text": "hi" })).await;
        let denied = bot.next_envelope().await;
        assert_eq!(denied["event"], event);
        assert!(
            denied["data"]["error"]
                .as_str()
                .expect("error string")
                .contains("naming a user"),
            "{denied}",
        );
    }
    alice.expect_silence(QUIET).await;

    bot.close(CloseCode::Normal, "done").await;
    wait_for_presence(&mut alice, 1).await;

    alice.close(CloseCode::Normal, "done").await;
    app.shutdown().await.expect("transport shuts down");
}

#[tokio::test]
async fn namespaced_gateways_isolate_their_broadcasts() {
    let (_db, app) = serve().await;
    let mut chat = open(&app, "/ws").await;
    let mut notify = open(&app, "/notify").await;

    chat.send("message", json!({ "text": "hi" })).await;
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
    let (_db, app) = serve().await;
    let mut socket = open(&app, "/ws").await;

    let oversized = "x".repeat(128 * 1024);
    socket.send("message", json!({ "text": oversized })).await;

    let (code, _) = socket.expect_close().await;
    assert_eq!(
        code,
        CloseCode::Size,
        "an over-cap message ends the socket with 1009 Message Too Big, never a bare drop",
    );

    app.shutdown().await.expect("transport shuts down");
}

#[tokio::test]
async fn the_socket_lifetime_ceiling_closes_the_socket() {
    let (_db, builder) = boot_builder().await;
    let app = builder
        .provide(nest_rs::ws::WsConfig {
            max_connection: Some(std::time::Duration::from_secs(1)),
            ..Default::default()
        })
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

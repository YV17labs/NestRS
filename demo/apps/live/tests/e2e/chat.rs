//! Chat/notify gateways over a real socket: echo, broadcast, presence, namespaces.

use futures_util::{SinkExt, StreamExt};
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
async fn a_request_scoped_provider_is_reachable_per_message_over_ws() {
    let bind = "127.0.0.1:13357";

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

    // `seq` resolves an #[injectable(scope = request)] RequestSeq via
    // nest_rs_ws::Scoped inside the handler. A fresh scope per message means the
    // stamped sequence differs across messages — a singleton would repeat.
    socket
        .send(Message::Text(json!({ "event": "seq" }).to_string().into()))
        .await
        .expect("send first seq");
    let first = next_json(&mut socket).await;
    assert_eq!(first["event"], "seq");
    let first_seq = first["data"].as_u64().expect("seq is a number");

    socket
        .send(Message::Text(json!({ "event": "seq" }).to_string().into()))
        .await
        .expect("send second seq");
    let second = next_json(&mut socket).await;
    let second_seq = second["data"].as_u64().expect("seq is a number");

    assert_ne!(
        first_seq, second_seq,
        "each WS message must build its own request-scoped RequestSeq (per-message scope)",
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

#[tokio::test]
async fn an_oversized_message_is_rejected_and_closes_the_socket() {
    let bind = "127.0.0.1:13355";

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

    // The default WsConfig cap is 64 KiB (WS-I1), enforced at the protocol layer
    // on both frame and message size — a giant frame is refused *before* it is
    // fully buffered. A payload well over the cap must terminate the socket, not
    // be echoed like a normal message.
    let oversized = "x".repeat(128 * 1024);
    let huge =
        json!({ "event": "message", "data": { "author": "ada", "text": oversized } }).to_string();
    assert!(huge.len() > 64 * 1024, "payload must exceed the 64 KiB cap");

    // The send may error if the server closes mid-write, or succeed — either
    // way the socket must then close rather than deliver an echo.
    let _ = socket.send(Message::Text(huge.into())).await;

    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(2), socket.next()).await {
            // Stream ended, errored, or a Close frame — the server refused it.
            Ok(None) | Ok(Some(Err(_))) | Ok(Some(Ok(Message::Close(_)))) => break,
            Err(_) => panic!("the socket must close after an over-cap message, not stay open"),
            Ok(Some(Ok(Message::Text(t)))) => {
                panic!("an over-cap message must not be echoed, got {t}")
            }
            Ok(Some(Ok(_))) => continue, // ping/pong — keep reading for the close
        }
    }

    handle.shutdown().await.expect("transport shuts down");
}

#[tokio::test]
async fn the_socket_lifetime_ceiling_closes_the_socket() {
    let bind = "127.0.0.1:13356";

    // The socket-lifetime ceiling (WsConfig::max_connection) is the security
    // bound on the stale-privilege window: a WS connection captures its
    // ability once at the upgrade, so when the ceiling elapses the server must
    // close the socket, forcing a fresh upgrade (and a fresh authn/authz +
    // `exp` check) — DATA-S7. Pin a 1s ceiling so that bound is observable.
    let app = boot_builder()
        .provide(
            nest_rs_ws::WsConfig::default().with_max_connection(std::time::Duration::from_secs(1)),
        )
        .build_headless()
        .await
        .expect("LiveModule boots headless");
    let handle = app
        .spawn_transport(HttpTransport::new().bind(bind))
        .await
        .expect("HTTP transport serves");
    let token = test_token().await;

    let mut socket = connect_with_retry(&format!("ws://{bind}/ws"), &token).await;

    // The socket is open now; once the 1s ceiling elapses the server closes it.
    // Generous headroom keeps the timing assertion off the flaky edge.
    let closed = tokio::time::timeout(std::time::Duration::from_secs(6), async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => continue, // echo/ping — keep reading for the close
            }
        }
    })
    .await;
    assert!(
        closed.is_ok(),
        "the socket-lifetime ceiling must close the socket after it elapses",
    );

    handle.shutdown().await.expect("transport shuts down");
}

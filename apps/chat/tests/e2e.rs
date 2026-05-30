use chat::AppModule;
use futures_util::{SinkExt, StreamExt};
use nestrs_http::poem::http::StatusCode;
use nestrs_http::HttpTransport;
use nestrs_testing::TestApp;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn gateway_endpoint_is_mounted() {
    let app = TestApp::builder()
        .module::<AppModule>()
        .with_test_telemetry()
        .build()
        .await
        .expect("AppModule boots and self-mounts the gateway");

    let resp = app.http().get("/ws").send().await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn gateway_echoes_messages_over_a_real_socket() {
    let bind = "127.0.0.1:13344";

    let app = TestApp::builder()
        .module::<AppModule>()
        .with_test_telemetry()
        .build_headless()
        .await
        .expect("AppModule boots headless");
    let handle = app
        .spawn_transport(HttpTransport::new().bind(bind))
        .await
        .expect("HTTP transport serves");

    let mut socket = connect_with_retry(&format!("ws://{bind}/ws")).await;

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
    assert!(unknown["data"]["error"]
        .as_str()
        .expect("error string")
        .contains("unknown event"));

    socket.close(None).await.ok();
    handle.shutdown().await.expect("transport shuts down");
}

#[tokio::test]
async fn a_message_is_broadcast_to_every_connected_client() {
    let bind = "127.0.0.1:13345";

    let app = TestApp::builder()
        .module::<AppModule>()
        .with_test_telemetry()
        .build_headless()
        .await
        .expect("AppModule boots headless");
    let handle = app
        .spawn_transport(HttpTransport::new().bind(bind))
        .await
        .expect("HTTP transport serves");

    // Two independent clients on the same gateway.
    let mut alice = connect_with_retry(&format!("ws://{bind}/ws")).await;
    let mut bob = connect_with_retry(&format!("ws://{bind}/ws")).await;

    // Alice speaks; the RoomService broadcasts the recorded message to the whole
    // registry, so *both* sockets receive it — not just the sender.
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

    let app = TestApp::builder()
        .module::<AppModule>()
        .with_test_telemetry()
        .build_headless()
        .await
        .expect("AppModule boots headless");
    let handle = app
        .spawn_transport(HttpTransport::new().bind(bind))
        .await
        .expect("HTTP transport serves");

    // `on_connect` increments presence for each socket; the count is observable
    // through the `presence` message, so the hook is verified end-to-end.
    let mut alice = connect_with_retry(&format!("ws://{bind}/ws")).await;
    wait_for_presence(&mut alice, 1).await;
    let mut bob = connect_with_retry(&format!("ws://{bind}/ws")).await;
    wait_for_presence(&mut alice, 2).await;

    // The `ModeratedGuard` bound to `message` rejects a banned author before the
    // handler runs — the client gets an error frame, nothing is recorded.
    bob.send(Message::Text(
        json!({ "event": "message", "data": { "author": "banned", "text": "hi" } })
            .to_string()
            .into(),
    ))
    .await
    .expect("bob sends");
    let denied = next_json(&mut bob).await;
    assert_eq!(denied["event"], "message");
    assert!(denied["data"]["error"]
        .as_str()
        .expect("error string")
        .contains("not allowed"));

    // `on_disconnect` decrements presence — visible to the surviving socket.
    bob.close(None).await.ok();
    wait_for_presence(&mut alice, 1).await;

    alice.close(None).await.ok();
    handle.shutdown().await.expect("transport shuts down");
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn wait_for_presence(socket: &mut Socket, want: u64) {
    for _ in 0..50 {
        socket
            .send(Message::Text(
                json!({ "event": "presence" }).to_string().into(),
            ))
            .await
            .expect("send presence");
        let frame = next_json(socket).await;
        assert_eq!(frame["event"], "presence");
        if frame["data"].as_u64().expect("presence count") == want {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("presence never reached {want}");
}

async fn connect_with_retry(url: &str) -> Socket {
    for _ in 0..50 {
        match tokio_tungstenite::connect_async(url).await {
            Ok((socket, _)) => return socket,
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    panic!("could not connect to {url}");
}

async fn next_json(socket: &mut Socket) -> Value {
    loop {
        match socket.next().await.expect("a frame").expect("a message") {
            Message::Text(text) => return serde_json::from_str(&text).expect("json envelope"),
            Message::Close(_) => panic!("socket closed before a reply"),
            _ => continue,
        }
    }
}

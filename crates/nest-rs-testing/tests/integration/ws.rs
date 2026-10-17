//! Covers `src/ws.rs` — the WebSocket driver serves the app's transport on the
//! port it listened on, so the first handshake lands.

use std::time::Duration;

use nest_rs_core::module;
use nest_rs_testing::TestApp;
use nest_rs_ws::{WsModule, gateway, messages};

#[gateway(path = "/echo")]
struct EchoGateway;

#[messages]
impl EchoGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

#[module(imports = [WsModule], providers = [EchoGateway])]
struct EchoModule;

/// The transport listens before `build_ws` returns: a connection made before
/// the serving task ever ran is accepted, on the port the system gave it.
#[tokio::test]
async fn the_driver_listens_before_it_returns_on_the_port_the_transport_bound() {
    let app = TestApp::builder()
        .module::<EchoModule>()
        .build_ws()
        .await
        .expect("the app serves");

    assert_ne!(app.addr().port(), 0, "the real port");
    std::net::TcpStream::connect(app.addr())
        .expect("the transport listens before its task first runs");

    app.close().await;
}

/// The first handshake lands, and one against a port nothing listens on is
/// refused at once rather than retried.
#[tokio::test]
async fn a_handshake_is_tried_once() {
    let app = TestApp::builder()
        .module::<EchoModule>()
        .build_ws()
        .await
        .expect("the app serves");
    let url = app.url("/echo");
    let mut socket = app.socket("/echo").connect().await;
    socket.send("ping", serde_json::json!({})).await;
    assert_eq!(socket.next_envelope().await["data"], "pong");
    drop(socket);
    let gone = app.socket("/echo");
    app.close().await;

    let refused = tokio::time::timeout(Duration::from_secs(1), gone.try_connect())
        .await
        .unwrap_or_else(|_| panic!("a refused handshake to {url} is not retried"));
    assert!(refused.is_err(), "nothing listens on {url} any more");
}

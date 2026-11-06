//! What the two socket pages say of a peer gone without a close: neither edge
//! sends a Ping, so a quiet socket lives to its connection ceiling, four hours by
//! default. A proxy's idle timeout is the client's to beat, and each way the pages
//! give it is answered: its own Ping, and for a browser, which has none, a gateway's
//! event or the subscription protocol's `ping`. Read raw, since the kit's socket
//! hides Ping and Pong.

use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use nest_rs_core::module;
use nest_rs_graphql::async_graphql::futures_util::stream::{self, Stream};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_http::DEFAULT_MAX_CONNECTION;
use nest_rs_testing::TestApp;
use nest_rs_ws::{WsModule, gateway, messages};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{Bytes, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type RawSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// A frame the server already sent reads at once, so this is what telling one from
/// silence costs each run.
const SILENCE: Duration = Duration::from_millis(100);

/// Only guards against a hang.
const PATIENCE: Duration = Duration::from_secs(60);

/// What the reads on the real clock have left of the ceiling. More than `PATIENCE`,
/// so a stalled runner never reaches it during them with the behaviour right.
const LEFT_OF_THE_CEILING: Duration = Duration::from_secs(120);

#[gateway(path = "/quiet")]
struct QuietGateway;

#[messages]
impl QuietGateway {
    #[subscribe_message("echo")]
    #[public]
    async fn echo(&self, text: String) -> String {
        text
    }
}

#[module(imports = [WsModule], providers = [QuietGateway])]
struct QuietGatewayModule;

#[resolver]
struct QuietResolver;

#[operations]
impl QuietResolver {
    #[query]
    #[public]
    async fn ping(&self) -> i32 {
        0
    }

    #[subscription]
    #[public]
    fn never(&self) -> impl Stream<Item = i32> {
        stream::pending()
    }
}

#[module(imports = [GraphqlModule], providers = [QuietResolver])]
struct QuietResolverModule;

/// Let `quiet` pass on paused time while nothing reads: what the server sends
/// meanwhile waits buffered for the read after it, back on the real clock.
async fn stays_quiet_for(quiet: Duration) {
    tokio::time::pause();
    tokio::time::sleep(quiet).await;
    tokio::time::resume();
}

/// The next frame of any kind.
async fn next_frame(socket: &mut RawSocket) -> Message {
    tokio::time::timeout(PATIENCE, socket.next())
        .await
        .expect("the server answers")
        .expect("the socket is open")
        .expect("a frame")
}

/// Nothing arrives, the frames the kit's socket hides included.
async fn assert_hears_nothing(socket: &mut RawSocket) {
    if let Ok(heard) = tokio::time::timeout(SILENCE, socket.next()).await {
        panic!("the server spoke to a quiet peer: {heard:?}");
    }
}

/// A client's Ping comes back as its Pong.
async fn assert_its_ping_is_answered(socket: &mut RawSocket) {
    let payload = Bytes::from_static(b"still here");
    socket
        .send(Message::Ping(payload.clone()))
        .await
        .expect("the socket is open");
    match next_frame(socket).await {
        Message::Pong(answered) => assert_eq!(answered, payload),
        other => panic!("expected its Pong, got {other:?}"),
    }
}

/// Past the ceiling, the next frame is the Close saying the server ended it.
async fn assert_closed_at_the_ceiling(socket: &mut RawSocket) {
    match next_frame(socket).await {
        Message::Close(Some(close)) => assert_eq!(close.code, CloseCode::Away),
        other => panic!("expected a Close at the ceiling, got {other:?}"),
    }
}

async fn assert_silent_until_the_ceiling(socket: &mut RawSocket) {
    stays_quiet_for(DEFAULT_MAX_CONNECTION - LEFT_OF_THE_CEILING).await;
    assert_hears_nothing(socket).await;
    assert_its_ping_is_answered(socket).await;
    stays_quiet_for(LEFT_OF_THE_CEILING + Duration::from_secs(1)).await;
    assert_closed_at_the_ceiling(socket).await;
}

/// The next frame is text, and it holds `expected`.
async fn assert_answered_with(socket: &mut RawSocket, expected: &str) {
    match next_frame(socket).await {
        Message::Text(text) => assert!(text.contains(expected), "{text}"),
        other => panic!("expected {expected}, got {other:?}"),
    }
}

/// A subscription socket past its `connection_ack`.
async fn acknowledged_subscription_socket(url: String) -> RawSocket {
    let mut request = url.into_client_request().expect("a request");
    request.headers_mut().insert(
        "sec-websocket-protocol",
        "graphql-transport-ws".parse().expect("a header value"),
    );
    let (mut socket, _) = connect_async(request).await.expect("opens");
    socket
        .send(Message::text(r#"{"type":"connection_init"}"#))
        .await
        .expect("the socket is open");
    assert_answered_with(&mut socket, "connection_ack").await;
    socket
}

#[tokio::test]
async fn a_quiet_gateway_socket_hears_no_ping_and_lives_to_its_ceiling() {
    let app = TestApp::builder()
        .module::<QuietGatewayModule>()
        .build_ws()
        .await
        .expect("boots");
    let (mut socket, _) = connect_async(app.url("/quiet")).await.expect("opens");

    assert_silent_until_the_ceiling(&mut socket).await;

    app.close().await;
}

#[tokio::test]
async fn a_quiet_subscription_socket_hears_no_ping_and_lives_to_its_ceiling() {
    let app = TestApp::builder()
        .module::<QuietResolverModule>()
        .build_ws()
        .await
        .expect("boots");
    let mut socket = acknowledged_subscription_socket(app.url("/graphql")).await;

    assert_silent_until_the_ceiling(&mut socket).await;

    app.close().await;
}

/// What a browser sends for want of a Ping, on a gateway.
#[tokio::test]
async fn a_gateway_answers_the_event_a_browser_sends_for_want_of_a_ping() {
    let app = TestApp::builder()
        .module::<QuietGatewayModule>()
        .build_ws()
        .await
        .expect("boots");
    let (mut socket, _) = connect_async(app.url("/quiet")).await.expect("opens");

    socket
        .send(Message::text(r#"{"event":"echo","data":"still here"}"#))
        .await
        .expect("the socket is open");
    assert_answered_with(&mut socket, "still here").await;

    app.close().await;
}

/// What a browser sends for want of a Ping, on a subscription socket.
#[tokio::test]
async fn a_subscription_socket_answers_the_protocol_s_ping_with_its_pong() {
    let app = TestApp::builder()
        .module::<QuietResolverModule>()
        .build_ws()
        .await
        .expect("boots");
    let mut socket = acknowledged_subscription_socket(app.url("/graphql")).await;

    socket
        .send(Message::text(r#"{"type":"ping"}"#))
        .await
        .expect("the socket is open");
    assert_answered_with(&mut socket, r#""type":"pong""#).await;

    app.close().await;
}

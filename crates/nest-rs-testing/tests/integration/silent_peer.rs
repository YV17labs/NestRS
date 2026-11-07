//! What the two socket pages say of a peer gone without a close: once the peer
//! has been silent for the Ping interval, each edge sends it a Ping (RFC 6455
//! §5.5.2), and a peer that sends nothing back within the Ping timeout is
//! dropped, its socket released; with the Pings off, a quiet socket lives to its
//! connection ceiling. A client's own Ping is answered too, and for a browser,
//! which sends none, a gateway's event or the subscription protocol's `ping`.
//! Read raw, since the kit's socket hides Ping and Pong; a peer that answers
//! nothing is one that reads nothing, since a client answers a Ping as it reads.
//!
//! That an answer keeps the socket is proved beside each loop, over an
//! in-memory pipe: paused time outruns a frame crossing the kernel's loopback,
//! so a Pong in flight here would read as silence.

use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use nest_rs_core::module;
use nest_rs_graphql::async_graphql::futures_util::stream::{self, Stream};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
use nest_rs_http::{DEFAULT_MAX_CONNECTION, DEFAULT_PING_INTERVAL, DEFAULT_PING_TIMEOUT};
use nest_rs_testing::{LogCapture, TestApp};
use nest_rs_ws::{WsConfig, WsModule, gateway, messages};
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

/// What a peer that answers nothing is left silent for: past the Ping and its wait.
const PAST_THE_PING_TIMEOUT: Duration = DEFAULT_PING_INTERVAL
    .saturating_add(DEFAULT_PING_TIMEOUT)
    .saturating_add(Duration::from_secs(1));

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

/// The gateway with its Pings turned off.
#[module(
    imports = [WsModule::for_root(WsConfig { ping_interval: None, ..WsConfig::default() })],
    providers = [QuietGateway],
)]
struct UnpingedGatewayModule;

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

/// The subscription socket with its Pings turned off.
#[module(
    imports = [GraphqlModule::for_root(GraphqlConfig {
        ping_interval: None,
        ..GraphqlConfig::default()
    })],
    providers = [QuietResolver],
)]
struct UnpingedResolverModule;

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

/// With its Pings off, a quiet socket hears nothing until its ceiling.
async fn assert_silent_until_the_ceiling(socket: &mut RawSocket) {
    stays_quiet_for(DEFAULT_MAX_CONNECTION - LEFT_OF_THE_CEILING).await;
    assert_hears_nothing(socket).await;
    assert_its_ping_is_answered(socket).await;
    stays_quiet_for(LEFT_OF_THE_CEILING + Duration::from_secs(1)).await;
    assert_closed_at_the_ceiling(socket).await;
}

/// A peer reading nothing hears its Ping, sent once it has been silent for the
/// interval; past the Ping's timeout the server drops it without a Close (RFC
/// 6455 §7.1.7: a peer that answers nothing is unlikely to read one), so the
/// read after the Ping finds the connection gone.
async fn assert_dropped_past_the_ping_timeout(socket: &mut RawSocket) {
    stays_quiet_for(PAST_THE_PING_TIMEOUT).await;
    match tokio::time::timeout(SILENCE, socket.next()).await {
        Ok(Some(Ok(Message::Ping(_)))) => {}
        heard => panic!("expected the server's Ping, got {heard:?}"),
    }
    match tokio::time::timeout(PATIENCE, socket.next())
        .await
        .expect("the server has dropped the socket")
    {
        None | Some(Err(_)) => {}
        Some(Ok(frame)) => panic!("expected the connection gone, read {frame:?}"),
    }
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
async fn a_gateway_peer_answering_no_ping_is_dropped_past_its_timeout() {
    let logs = LogCapture::install();
    let app = TestApp::builder()
        .module::<QuietGatewayModule>()
        .build_ws()
        .await
        .expect("boots");
    let (mut socket, _) = connect_async(app.url("/quiet")).await.expect("opens");

    assert_dropped_past_the_ping_timeout(&mut socket).await;

    app.close().await;
    let line = logs.expect_one(
        nest_rs_ws::TARGET,
        "closing socket: the peer sent nothing back within the ping timeout",
    );
    assert_eq!(line.level, "warn");
    assert_eq!(line.field("host").as_deref(), Some("QuietGateway"));
    assert_eq!(line.field("path").as_deref(), Some("/quiet"));
    let timeout_ms = DEFAULT_PING_TIMEOUT.as_millis().to_string();
    assert_eq!(
        line.field("ping_timeout_ms").as_deref(),
        Some(timeout_ms.as_str())
    );
    let released = logs.find(
        nest_rs_core::operation_log::TARGET,
        nest_rs_ws::unit::DISCONNECT.name(),
    );
    assert_eq!(released.len(), 1, "its disconnect hook ran: {released:#?}");
}

#[tokio::test]
async fn an_unpinged_gateway_socket_hears_nothing_and_lives_to_its_ceiling() {
    let app = TestApp::builder()
        .module::<UnpingedGatewayModule>()
        .build_ws()
        .await
        .expect("boots");
    let (mut socket, _) = connect_async(app.url("/quiet")).await.expect("opens");

    assert_silent_until_the_ceiling(&mut socket).await;

    app.close().await;
}

#[tokio::test]
async fn a_subscription_peer_answering_no_ping_is_dropped_past_its_timeout() {
    let logs = LogCapture::install();
    let app = TestApp::builder()
        .module::<QuietResolverModule>()
        .build_ws()
        .await
        .expect("boots");
    let mut socket = acknowledged_subscription_socket(app.url("/graphql")).await;

    assert_dropped_past_the_ping_timeout(&mut socket).await;

    app.close().await;
    let line = logs.expect_one(
        nest_rs_graphql::TARGET,
        "closing subscription socket: the peer sent nothing back within the ping timeout",
    );
    assert_eq!(line.level, "warn");
    assert_eq!(line.field("host").as_deref(), Some("GraphqlModule"));
    assert_eq!(line.field("path").as_deref(), Some("/graphql"));
    let ended = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_graphql::unit::SUBSCRIPTION.name(),
    );
    assert_eq!(
        ended.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
        "the server ended it: {ended:#?}",
    );
}

#[tokio::test]
async fn an_unpinged_subscription_socket_hears_nothing_and_lives_to_its_ceiling() {
    let app = TestApp::builder()
        .module::<UnpingedResolverModule>()
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

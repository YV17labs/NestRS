//! The accept loop over real loopback sockets: the connection cap, an upgraded
//! socket holding its place under it, the HTTP/1 head cap, HTTP/2 by prior
//! knowledge, and a connection or an HTTP/2 stream task that panics.

use std::time::Duration;

use nest_rs_core::module;
use nest_rs_http::{HttpConfig, controller, routes};
use nest_rs_testing::LogCapture;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use crate::transport::{PATIENCE, Serving, connect, read_head, read_to_end, request, serve_module};

/// The line the loop files when the cap holds a connection back.
const CAP_REACHED: &str = "connection cap reached; new connections wait in the listen backlog";

/// How long a connection held back by the cap is watched for an answer that
/// must not come.
const HELD_BACK: Duration = Duration::from_millis(300);

#[controller(path = "/server")]
struct ServerController;

#[routes]
impl ServerController {
    #[get("/ping")]
    #[public]
    async fn ping(&self) -> &'static str {
        "pong"
    }

    #[get("/panic")]
    #[public]
    async fn panics(&self) -> &'static str {
        panic!("the handler panicked")
    }
}

#[module(providers = [ServerController])]
struct ServerModule;

/// [`ServerModule`] and the echo socket at `/socket`, served from `config`.
async fn serve(config: HttpConfig) -> (u16, CancellationToken) {
    let Serving { port, cancel, .. } = serve_module::<ServerModule>(config).await;
    (port, cancel)
}

/// A connection the cap holds back is not answered while it waits, then is
/// once a held connection closes; one `warn` names the variable.
#[tokio::test]
async fn past_the_cap_a_connection_waits_in_the_backlog_until_one_closes() {
    let logs = LogCapture::install();
    let (port, cancel) = serve(HttpConfig {
        max_concurrent_connections: 2,
        ..HttpConfig::default()
    })
    .await;
    let mut first = request(port, "/server/ping").await;
    assert!(read_head(&mut first).await.starts_with("HTTP/1.1 200"));
    let mut second = request(port, "/server/ping").await;
    assert!(read_head(&mut second).await.starts_with("HTTP/1.1 200"));

    let mut third = request(port, "/server/ping").await;
    assert!(
        tokio::time::timeout(HELD_BACK, read_head(&mut third))
            .await
            .is_err(),
        "two connections are held, so the third waits unanswered",
    );

    drop(first);
    let head = tokio::time::timeout(PATIENCE, read_head(&mut third))
        .await
        .expect("answered once a held connection closes");
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");

    let reached = logs.expect_one(nest_rs_http::target::HTTP, CAP_REACHED);
    assert_eq!(reached.level, "warn");
    assert_eq!(
        reached.field("variable"),
        Some(nest_rs_config::var_name(
            "http",
            "MAX_CONCURRENT_CONNECTIONS"
        )),
    );
    assert_eq!(
        reached.field("max_concurrent_connections").as_deref(),
        Some("2")
    );
    drop(second);
    cancel.cancel();
}

/// A socket upgraded to a WebSocket leaves the HTTP connection behind, and
/// still counts against the cap until it closes.
#[tokio::test]
async fn an_upgraded_websocket_holds_its_place_under_the_cap_until_it_closes() {
    let (port, cancel) = serve(HttpConfig {
        max_concurrent_connections: 1,
        ..HttpConfig::default()
    })
    .await;
    drop(connect(port).await);
    let (websocket, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/socket"))
        .await
        .expect("the upgrade succeeds");

    let mut next = request(port, "/server/ping").await;
    assert!(
        tokio::time::timeout(HELD_BACK, read_head(&mut next))
            .await
            .is_err(),
        "the upgraded socket holds the one place, so the next connection waits",
    );

    drop(websocket);
    let head = tokio::time::timeout(PATIENCE, read_head(&mut next))
        .await
        .expect("answered once the upgraded socket closes");
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    cancel.cancel();
}

/// An HTTP/1 head larger than the read buffer is refused with the status
/// RFC 6585 §5 names, rather than buffered.
#[tokio::test]
async fn an_http1_head_past_64_kib_answers_431() {
    let (port, cancel) = serve(HttpConfig::default()).await;
    let mut stream = connect(port).await;
    let filler = "a".repeat(70 * 1024);
    // The server answers before it has read it all, so the tail may meet a reset.
    let _ = stream
        .write_all(
            format!("GET /server/ping HTTP/1.1\r\nHost: localhost\r\nX-Filler: {filler}\r\n\r\n")
                .as_bytes(),
        )
        .await;
    let head = read_head(&mut stream).await;
    assert!(head.starts_with("HTTP/1.1 431"), "{head}");
    cancel.cancel();
}

/// A plaintext client speaking HTTP/2 from its first byte is served, and told
/// how many streams it may open at once.
#[tokio::test]
async fn h2_by_prior_knowledge_on_plaintext_reads_the_stream_cap() {
    let (port, cancel) = serve(HttpConfig::default()).await;
    let (client, mut connection) = h2::client::handshake(connect(port).await)
        .await
        .expect("the HTTP/2 preface is accepted");
    let mut client = client.ready().await.expect("a stream can open");
    let request = poem::http::Request::get(format!("http://127.0.0.1:{port}/server/ping"))
        .body(())
        .expect("a request");
    let (response, _) = client.send_request(request, true).expect("sent");
    let response = tokio::select! {
        response = response => response.expect("answered"),
        ended = &mut connection => panic!("the connection ended first: {ended:?}"),
    };
    assert_eq!(response.status(), 200);
    assert_eq!(
        connection.max_concurrent_send_streams(),
        200,
        "SETTINGS_MAX_CONCURRENT_STREAMS bounds what one connection runs at once",
    );
    cancel.cancel();
}

/// A panic in a connection's task drops that connection, is filed once, and
/// leaves the server accepting.
#[tokio::test]
async fn a_panic_in_a_connection_is_contained_and_the_server_keeps_serving() {
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if info.payload().downcast_ref::<&str>() != Some(&"the handler panicked") {
            report(info);
        }
    }));
    let logs = LogCapture::install();
    let (port, cancel) = serve(HttpConfig::default()).await;

    let mut panicked = request(port, "/server/panic").await;
    assert_eq!(read_to_end(&mut panicked).await, "", "nothing was answered");
    let line = logs.expect_one(nest_rs_http::target::HTTP, "a connection task panicked");
    assert_eq!(line.level, "error");
    assert_eq!(
        line.field(nest_rs_core::panic::FIELD).as_deref(),
        Some("the handler panicked"),
    );

    let mut after = request(port, "/server/ping").await;
    let head = tokio::time::timeout(PATIENCE, read_head(&mut after))
        .await
        .expect("the server still answers");
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    cancel.cancel();
}

/// A panic in one HTTP/2 stream's task resets that stream alone, is filed
/// once, and the connection keeps serving its other streams.
#[tokio::test]
async fn a_panic_in_an_http2_stream_resets_it_and_its_connection_keeps_serving() {
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if info.payload().downcast_ref::<&str>() != Some(&"the handler panicked") {
            report(info);
        }
    }));
    let logs = LogCapture::install();
    let (port, cancel) = serve(HttpConfig::default()).await;
    let (client, connection) = h2::client::handshake(connect(port).await)
        .await
        .expect("the HTTP/2 preface is accepted");
    let connection = tokio::spawn(connection);
    let get = |path: &str| {
        poem::http::Request::get(format!("http://127.0.0.1:{port}{path}"))
            .body(())
            .expect("a request")
    };

    let mut client = client.ready().await.expect("a stream can open");
    let (panicked, _) = client
        .send_request(get("/server/panic"), true)
        .expect("sent");
    let reset = tokio::time::timeout(PATIENCE, panicked)
        .await
        .expect("the stream ends")
        .expect_err("a stream whose task panicked is not answered");
    assert_eq!(reset.reason(), Some(h2::Reason::CANCEL), "{reset}");
    let line = logs.expect_one(nest_rs_http::target::HTTP, "an HTTP/2 stream task panicked");
    assert_eq!(line.level, "error");
    assert_eq!(
        line.field(nest_rs_core::panic::FIELD).as_deref(),
        Some("the handler panicked"),
    );

    let mut client = client
        .ready()
        .await
        .expect("the connection opens another stream");
    let (after, _) = client
        .send_request(get("/server/ping"), true)
        .expect("sent");
    let response = tokio::time::timeout(PATIENCE, after)
        .await
        .expect("the stream ends")
        .expect("answered on the same connection");
    assert_eq!(response.status(), 200);
    assert!(!connection.is_finished(), "the connection keeps serving");
    logs.expect_none(nest_rs_http::target::HTTP, "a connection task panicked");
    cancel.cancel();
}

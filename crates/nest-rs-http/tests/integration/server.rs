//! The accept loop over real loopback sockets: the connection cap, an upgraded
//! socket holding its place under it, the HTTP/1 head cap, HTTP/2 by prior
//! knowledge, a connection or an HTTP/2 stream task that panics, and the
//! deadline each phase of a connection carries.

use std::time::Duration;

use bytes::Bytes;
use nest_rs_core::module;
use nest_rs_http::{HttpConfig, controller, routes};
use nest_rs_testing::{LogCapture, wait_until};
use poem::Body;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::transport::{
    PATIENCE, Serving, after, ask, connect, read_head, read_to_end, request, serve_module,
    small_window, still_open,
};

/// The line the loop files when the cap holds a connection back.
const CAP_REACHED: &str = "connection cap reached; new connections wait in the listen backlog";

/// The line the loop files once nothing waits on the cap any more.
const CAP_LEFT: &str = "connection cap no longer reached; accepting again";

/// The body `/server/large` answers: more than any socket's buffers hold.
const LARGE: usize = 50 * 1024 * 1024;

/// How long a closed phase is given past its deadline: hyper's own close, or
/// the second's grace before the connection is dropped.
const PAST: Duration = Duration::from_millis(1500);

/// How long a connection held back by the cap is watched for an answer that
/// must not come.
const HELD_BACK: Duration = Duration::from_millis(300);

/// The line a cut at the send deadline files, on every member of its family.
const SEND_CUT: &str = "the peer took nothing within the send deadline; it is cut off";

/// How long `/server/slow` takes to answer.
const SLOW: Duration = Duration::from_secs(3);

#[controller(path = "/server")]
struct ServerController;

#[routes]
impl ServerController {
    #[get("/ping")]
    #[public]
    async fn ping(&self) -> &'static str {
        "pong"
    }

    #[get("/hello")]
    #[public]
    async fn hello(&self) -> &'static str {
        "hello"
    }

    /// Answers once [`SLOW`] has passed.
    #[get("/slow")]
    #[public]
    async fn slow(&self) -> &'static str {
        tokio::time::sleep(SLOW).await;
        "slow"
    }

    #[get("/panic")]
    #[public]
    async fn panics(&self) -> &'static str {
        panic!("the handler panicked")
    }

    #[get("/large")]
    #[public]
    async fn large(&self) -> Body {
        Body::from(vec![b'x'; LARGE])
    }

    /// 16 KiB chunks, for as long as it is read.
    #[get("/endless")]
    #[public]
    async fn endless(&self) -> Body {
        Body::from_bytes_stream(futures_util::stream::repeat_with(|| {
            Ok::<_, std::io::Error>(vec![b'x'; 16 * 1024])
        }))
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

/// A round trip on a fresh connection: every connection opened before it has
/// been accepted, since the loop takes them from the backlog in order.
async fn accepted(port: u16) {
    let mut witness = request(port, "/server/ping").await;
    assert_eq!(answer(&mut witness).await, "pong");
}

/// Read one answer to `/server/ping` or `/server/hello` off a kept-alive
/// connection, its body included: the body the route answers.
async fn answer(stream: &mut TcpStream) -> String {
    let head = tokio::time::timeout(PATIENCE, read_head(stream))
        .await
        .expect("answered");
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let length: usize = head
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|value| value.trim().parse().expect("a length"))
        })
        .expect("a sized body");
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body).await.expect("the body");
    String::from_utf8(body).expect("text")
}

/// The server's deadlines, every other setting at its default.
fn deadlines(header_read: u64, idle: u64, send: u64) -> HttpConfig {
    HttpConfig {
        header_read_timeout: Duration::from_secs(header_read),
        idle_timeout: Duration::from_secs(idle),
        send_timeout: Duration::from_secs(send),
        ..HttpConfig::default()
    }
}

/// Plaintext, where the version sniff is the first thing read: a client that
/// opens a connection and sends nothing loses it at the head deadline, counted
/// from accept.
#[tokio::test]
async fn a_socket_that_sends_nothing_is_closed_at_the_head_deadline() {
    let (port, cancel) = serve(deadlines(5, 60, 60)).await;
    let mut silent = connect(port).await;
    accepted(port).await;

    after(Duration::from_secs(4)).await;
    assert!(still_open(&mut silent).await, "inside the head deadline");
    after(Duration::from_secs(1) + PAST).await;
    assert_eq!(read_to_end(&mut silent).await, "", "closed, not answered");
    cancel.cancel();
}

/// Each byte of a head is progress hyper would wait on; the deadline is the
/// head's, counted from accept, so trickling it buys nothing.
#[tokio::test]
async fn a_head_sent_a_byte_at_a_time_is_cut_at_the_head_deadline() {
    let (port, cancel) = serve(deadlines(5, 60, 60)).await;
    let mut slow = connect(port).await;
    accepted(port).await;

    for byte in b"GET /server/ping".iter().take(4) {
        slow.write_all(&[*byte]).await.expect("a byte is sent");
        after(Duration::from_secs(1)).await;
    }
    assert!(still_open(&mut slow).await, "inside the head deadline");
    after(Duration::from_secs(1) + PAST).await;
    assert_eq!(
        read_to_end(&mut slow).await,
        "",
        "cut before a head was read"
    );
    cancel.cancel();
}

/// An idle kept-alive connection waits for the idle deadline, not the head
/// one: a request after three idle seconds is answered on it, and it closes
/// five idle seconds after its last answer.
#[tokio::test]
async fn an_idle_keep_alive_connection_lives_until_the_idle_deadline_not_the_head_one() {
    let (port, cancel) = serve(deadlines(1, 5, 60)).await;
    let mut client = request(port, "/server/ping").await;
    assert_eq!(answer(&mut client).await, "pong");

    after(Duration::from_secs(3)).await;
    ask(&mut client, "/server/hello").await;
    assert_eq!(
        answer(&mut client).await,
        "hello",
        "still served after 3 s idle"
    );

    after(Duration::from_secs(4)).await;
    assert!(still_open(&mut client).await, "inside the idle deadline");
    after(Duration::from_secs(1) + PAST).await;
    assert_eq!(
        read_to_end(&mut client).await,
        "",
        "closed at the idle deadline"
    );
    cancel.cancel();
}

/// On HTTP/1 the first byte read while idle starts a head, which then has the
/// head deadline, far inside the idle one.
#[tokio::test]
async fn a_slow_head_after_an_idle_period_is_cut_at_the_head_deadline() {
    let (port, cancel) = serve(deadlines(2, 60, 60)).await;
    let mut client = request(port, "/server/ping").await;
    assert_eq!(answer(&mut client).await, "pong");
    after(Duration::from_secs(5)).await;

    client
        .write_all(b"GET /server/hello HTTP/1.1\r\n")
        .await
        .expect("half a head is sent");
    after(Duration::from_secs(1)).await;
    assert!(still_open(&mut client).await, "inside the head deadline");
    after(Duration::from_secs(1) + PAST).await;
    assert_eq!(
        read_to_end(&mut client).await,
        "",
        "cut before the head ended"
    );
    cancel.cancel();
}

/// A request still in flight when the first head's deadline passes leaves no
/// longer span behind it: a slow head after its answer is cut at the head
/// deadline, far inside the idle one.
#[tokio::test]
async fn a_slow_head_after_a_request_outliving_the_first_head_deadline_is_cut_at_its_own() {
    let (port, cancel) = serve(deadlines(2, 60, 60)).await;
    let mut client = request(port, "/server/slow").await;
    after(SLOW + Duration::from_secs(1)).await;
    assert_eq!(answer(&mut client).await, "slow");

    client
        .write_all(b"GET /server/hello HTTP/1.1\r\n")
        .await
        .expect("half a head is sent");
    after(Duration::from_secs(1)).await;
    assert!(still_open(&mut client).await, "inside the head deadline");
    after(Duration::from_secs(1) + PAST).await;
    assert_eq!(
        read_to_end(&mut client).await,
        "",
        "cut before the head ended"
    );
    cancel.cancel();
}

/// Two requests in one write are answered in the order they were sent, the
/// second one's head read while the first was in flight.
#[tokio::test]
async fn pipelined_requests_are_answered_in_order() {
    let (port, cancel) = serve(deadlines(1, 5, 60)).await;
    let mut client = connect(port).await;
    client
        .write_all(
            b"GET /server/ping HTTP/1.1\r\nHost: localhost\r\n\r\n\
              GET /server/hello HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await
        .expect("both requests are sent");
    assert_eq!(answer(&mut client).await, "pong");
    assert_eq!(answer(&mut client).await, "hello");
    cancel.cancel();
}

/// An HTTP/2 connection opening no stream is idle once its preface is read: it
/// lives past the head deadline and is sent `GOAWAY` at the idle one.
#[tokio::test]
async fn an_h2_connection_with_no_stream_receives_goaway_at_the_idle_deadline() {
    let (port, cancel) = serve(deadlines(2, 5, 60)).await;
    let (client, mut connection) = h2::client::handshake(connect(port).await)
        .await
        .expect("the HTTP/2 preface is accepted");
    let mut ping = connection.ping_pong().expect("the ping handle");
    let connection = tokio::spawn(connection);
    // A PING is no stream: its answer only proves the preface was read.
    ping.ping(h2::Ping::opaque())
        .await
        .expect("the server answers a PING");

    after(Duration::from_secs(4)).await;
    assert!(
        !connection.is_finished(),
        "past the head deadline, inside the idle one"
    );
    after(Duration::from_secs(1) + PAST).await;
    let ended = tokio::time::timeout(PATIENCE, connection)
        .await
        .expect("the connection ends at the idle deadline")
        .expect("its task does not panic");
    assert!(ended.is_ok(), "ended by GOAWAY, not cut: {ended:?}");
    let refused = client
        .ready()
        .await
        .expect_err("no stream opens on a connection that went away");
    assert!(refused.is_go_away(), "{refused}");
    assert_eq!(refused.reason(), Some(h2::Reason::NO_ERROR), "{refused}");
    cancel.cancel();
}

/// `GET /server/large` on a connection whose receive buffer is 4 KiB, its
/// response head read: from here the server writes into full buffers.
async fn large_download(port: u16) -> TcpStream {
    let mut stream = small_window(port).await;
    ask(&mut stream, "/server/large").await;
    let head = tokio::time::timeout(PATIENCE, read_head(&mut stream))
        .await
        .expect("the download starts");
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    stream
}

/// Read `stream` until its end, a reset counted as one; how many bytes came.
async fn drained(stream: &mut TcpStream) -> usize {
    let mut total = 0;
    let mut chunk = vec![0_u8; 64 * 1024];
    loop {
        match tokio::time::timeout(PATIENCE, stream.read(&mut chunk)).await {
            Ok(Ok(0)) => return total,
            Ok(Ok(read)) => total += read,
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::ConnectionReset => return total,
            Ok(Err(error)) => panic!("the socket failed rather than ending: {error}"),
            Err(_) => panic!("the server never ended the download"),
        }
    }
}

/// A peer that stops reading parks the server's write; past the send deadline
/// the connection is dropped and its place under the cap frees, unasked, with
/// one `warn`.
#[tokio::test]
async fn a_peer_that_stops_reading_is_disconnected_at_the_send_deadline() {
    let logs = LogCapture::install();
    let (port, cancel) = serve(HttpConfig {
        max_concurrent_connections: 1,
        ..deadlines(30, 75, 2)
    })
    .await;
    let mut stalled = large_download(port).await;

    after(Duration::from_secs(3 * 2)).await;
    let mut next = request(port, "/server/ping").await;
    let head = tokio::time::timeout(PATIENCE, read_head(&mut next))
        .await
        .expect("answered once the stalled connection is gone");
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let read = drained(&mut stalled).await;
    assert!(read < LARGE, "the download was cut, {read} bytes in");
    let cut = logs.expect_one(nest_rs_http::target::HTTP, SEND_CUT);
    assert_eq!(cut.level, "warn");
    assert_eq!(cut.field("send_timeout_ms").as_deref(), Some("2000"));
    cancel.cancel();
}

/// A reader taking a few megabytes, then nothing for most of the send
/// deadline, again and again, outlasts the deadline many times over: it is
/// counted between two writes.
#[tokio::test]
async fn a_slow_but_steady_reader_is_not() {
    let (port, cancel) = serve(deadlines(30, 75, 2)).await;
    let mut reader = large_download(port).await;
    let mut chunk = vec![0_u8; 64 * 1024];

    for _ in 0..4 {
        // Far more than the kernel buffers hold, so the server wrote meanwhile.
        let mut taken = 0;
        while taken < 8 * 1024 * 1024 {
            let read = tokio::time::timeout(PATIENCE, reader.read(&mut chunk))
                .await
                .expect("the download flows")
                .expect("readable");
            assert!(
                read > 0,
                "the server ended the download {taken} bytes into a pass"
            );
            taken += read;
        }
        after(Duration::from_millis(1500)).await;
    }
    let read = tokio::time::timeout(PATIENCE, reader.read(&mut chunk))
        .await
        .expect("the download still flows")
        .expect("readable");
    assert!(read > 0, "still served after 6 s of a 2 s send deadline");
    cancel.cancel();
}

/// At a cap of one, a connection held alone, then connections coming one after
/// another, each closed before the next, never wait on the cap: neither line is
/// filed.
#[tokio::test]
async fn connections_coming_one_after_another_at_a_cap_of_one_file_neither_line() {
    let logs = LogCapture::install();
    let (port, cancel) = serve(HttpConfig {
        max_concurrent_connections: 1,
        ..HttpConfig::default()
    })
    .await;
    for _ in 0..3 {
        let mut client = request(port, "/server/ping").await;
        assert_eq!(answer(&mut client).await, "pong");
        tokio::time::sleep(HELD_BACK).await;
        logs.expect_none(nest_rs_http::target::HTTP, CAP_REACHED);
        client.shutdown().await.expect("the client closes");
        assert_eq!(
            read_to_end(&mut client).await,
            "",
            "the server closes in turn"
        );
    }
    logs.expect_none(nest_rs_http::target::HTTP, CAP_REACHED);
    logs.expect_none(nest_rs_http::target::HTTP, CAP_LEFT);
    cancel.cancel();
}

/// Connections queued behind a cap of one are served one after the other
/// under one `warn`, and one `info` follows once the queue is empty — not a
/// pair per connection.
#[tokio::test]
async fn connections_queued_at_the_cap_file_one_warn_and_one_info_for_the_episode() {
    let logs = LogCapture::install();
    let (port, cancel) = serve(HttpConfig {
        max_concurrent_connections: 1,
        ..HttpConfig::default()
    })
    .await;
    let mut held = request(port, "/server/ping").await;
    assert_eq!(answer(&mut held).await, "pong");
    let mut queued = Vec::new();
    for _ in 0..3 {
        queued.push(request(port, "/server/hello").await);
    }

    drop(held);
    for mut next in queued {
        assert_eq!(answer(&mut next).await, "hello", "served in its turn");
        logs.expect_none(nest_rs_http::target::HTTP, CAP_LEFT);
    }
    wait_until(PATIENCE, || {
        !logs.find(nest_rs_http::target::HTTP, CAP_LEFT).is_empty()
    })
    .await;
    let left = logs.expect_one(nest_rs_http::target::HTTP, CAP_LEFT);
    assert_eq!(left.level, "info");
    let reached = logs.expect_one(nest_rs_http::target::HTTP, CAP_REACHED);
    assert_eq!(reached.level, "warn");
    cancel.cancel();
}

/// An HTTP/2 client by prior knowledge whose streams open with `stream_window`
/// bytes of window and whose connection has plenty, so one stream's window
/// holds back no other; its connection runs on a task of its own.
async fn h2_client(
    port: u16,
    stream_window: u32,
) -> (
    h2::client::SendRequest<Bytes>,
    JoinHandle<Result<(), h2::Error>>,
) {
    let (client, connection) = h2::client::Builder::new()
        .initial_window_size(stream_window)
        .initial_connection_window_size(64 * 1024 * 1024)
        .handshake::<_, Bytes>(connect(port).await)
        .await
        .expect("the HTTP/2 preface is accepted");
    (client, tokio::spawn(connection))
}

/// `GET path` on a new stream of `client`, its `200` head read: the body to come.
async fn h2_get(client: &h2::client::SendRequest<Bytes>, port: u16, path: &str) -> h2::RecvStream {
    let mut client = client.clone().ready().await.expect("a stream can open");
    let request = poem::http::Request::get(format!("http://127.0.0.1:{port}{path}"))
        .body(())
        .expect("a request");
    let (response, _) = client.send_request(request, true).expect("sent");
    let response = tokio::time::timeout(PATIENCE, response)
        .await
        .expect("the head comes")
        .expect("answered");
    assert_eq!(response.status(), 200);
    response.into_body()
}

/// Read what came of `body`, releasing no window, up to the error that ends it.
async fn reset(body: &mut h2::RecvStream) -> h2::Error {
    loop {
        match tokio::time::timeout(PATIENCE, body.data()).await {
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(error))) => return error,
            Ok(None) => panic!("the stream ended rather than being reset"),
            Err(_) => panic!("the stream was never reset"),
        }
    }
}

/// On HTTP/2 a peer that reads its connection but grants a stream no window
/// parks that stream with no write waiting on the socket: past the send
/// deadline the stream is reset, one `warn` says so, and the connection serves
/// its other streams on.
#[tokio::test]
async fn an_h2_stream_whose_window_is_withheld_is_reset_at_the_send_deadline() {
    let logs = LogCapture::install();
    let (port, cancel) = serve(deadlines(30, 75, 2)).await;
    let (client, connection) = h2_client(port, 16 * 1024).await;
    let mut parked = h2_get(&client, port, "/server/endless").await;

    after(Duration::from_secs(3)).await;
    let error = reset(&mut parked).await;
    assert_eq!(error.reason(), Some(h2::Reason::CANCEL), "{error}");
    let cut = logs.expect_one(nest_rs_http::target::HTTP, SEND_CUT);
    assert_eq!(cut.level, "warn");
    assert_eq!(cut.field("send_timeout_ms").as_deref(), Some("2000"));

    let mut pong = h2_get(&client, port, "/server/ping").await;
    let body = tokio::time::timeout(PATIENCE, pong.data())
        .await
        .expect("the body comes")
        .expect("a frame")
        .expect("data");
    assert_eq!(body, "pong");
    assert!(!connection.is_finished(), "the connection serves on");
    cancel.cancel();
}

/// A body answered in one chunk is handed to HTTP/2 a piece at a time, so a
/// download whose window is withheld is reset at the send deadline too, rather
/// than left to the connection once its handler is done.
#[tokio::test]
async fn an_h2_download_in_one_chunk_whose_window_is_withheld_is_reset_at_the_send_deadline() {
    let (port, cancel) = serve(deadlines(30, 75, 2)).await;
    let (client, _connection) = h2_client(port, 16 * 1024).await;
    let mut parked = h2_get(&client, port, "/server/large").await;

    after(Duration::from_secs(3)).await;
    let error = reset(&mut parked).await;
    assert_eq!(error.reason(), Some(h2::Reason::CANCEL), "{error}");
    cancel.cancel();
}

/// A download in one chunk stays in flight while its reader takes it, slowly
/// but steadily: the idle deadline never sees its connection idle.
#[tokio::test]
async fn an_h2_download_in_one_chunk_read_steadily_outlives_the_idle_deadline() {
    let (port, cancel) = serve(deadlines(30, 3, 2)).await;
    let (client, _connection) = h2_client(port, 1024 * 1024).await;
    let mut download = h2_get(&client, port, "/server/large").await;

    for _ in 0..4 {
        let mut taken = 0;
        while taken < 8 * 1024 * 1024 {
            let chunk = tokio::time::timeout(PATIENCE, download.data())
                .await
                .expect("the download flows")
                .expect("not ended")
                .expect("not reset");
            taken += chunk.len();
            download
                .flow_control()
                .release_capacity(chunk.len())
                .expect("window released");
        }
        after(Duration::from_millis(1500)).await;
    }
    let chunk = tokio::time::timeout(PATIENCE, download.data())
        .await
        .expect("the download still flows")
        .expect("not ended")
        .expect("not reset");
    assert!(
        !chunk.is_empty(),
        "still served after 6 s of a 3 s idle deadline"
    );
    cancel.cancel();
}

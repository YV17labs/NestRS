//! The transport-wide request body cap: `max_body_bytes` bounds every
//! extractor at the transport edge, not only `RawBody`. Nothing is read at the
//! edge: a body is read by its extractor, after the guards, under the request
//! budget counted from arrival and a bound on each gap between its frames —
//! the cases a client drives over a real socket.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use nest_rs_core::{Layer, UnitContext, injectable, module};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::{HttpConfig, HttpView, RawBody, async_trait, controller, routes};
use poem::http::{StatusCode, header};
use poem::test::TestClient;
use poem::web::Json;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::Notify;

use crate::transport::{PATIENCE, Serving, after, connect, read_head, read_to_end, serve_module};

const CAP: usize = 64;

#[derive(Deserialize, schemars::JsonSchema)]
struct Payload {
    value: String,
}

#[controller(path = "/body")]
struct BodyController;

#[routes]
impl BodyController {
    #[post("/json")]
    async fn take_json(&self, body: Json<Payload>) -> String {
        body.0.value
    }

    #[post("/string")]
    async fn take_string(&self, body: String) -> String {
        format!("{} bytes", body.len())
    }

    #[post("/raw")]
    async fn take_raw(&self, body: RawBody) -> String {
        format!("{} bytes", body.len())
    }

    // A streamed body has no extractor limit: its only bound is the edge's.
    #[post("/stream")]
    async fn take_stream(&self, body: poem::Body) -> poem::Result<String> {
        let mut stream = body.into_bytes_stream();
        let mut seen = 0usize;
        while let Some(chunk) = stream.next().await {
            seen += chunk.map_err(poem::error::InternalServerError)?.len();
        }
        Ok(format!("{seen} bytes"))
    }
}

#[module(providers = [BodyController])]
struct BodyModule;

async fn boot() -> TestClient<poem::endpoint::BoxEndpoint<'static, poem::Response>> {
    boot_with(false).await
}

async fn boot_with(
    compression: bool,
) -> TestClient<poem::endpoint::BoxEndpoint<'static, poem::Response>> {
    crate::boot_on::<BodyModule>(HttpConfig {
        max_body_bytes: Some(CAP),
        compression,
        ..HttpConfig::default()
    })
    .await
}

fn oversized_json() -> Vec<u8> {
    format!(r#"{{"value":"{}"}}"#, "x".repeat(CAP * 4)).into_bytes()
}

#[tokio::test]
async fn bare_json_handler_rejects_an_oversized_body_with_413() {
    let client = boot().await;
    let resp = client
        .post("/body/json")
        .content_type("application/json")
        .body(oversized_json())
        .send()
        .await;
    resp.assert_status(StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn bare_string_handler_rejects_an_oversized_body_with_413() {
    let client = boot().await;
    let resp = client
        .post("/body/string")
        .body(vec![b'x'; CAP + 1])
        .send()
        .await;
    resp.assert_status(StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn raw_body_handler_rejects_an_oversized_body_with_413() {
    let client = boot().await;
    let resp = client
        .post("/body/raw")
        .body(vec![b'x'; CAP + 1])
        .send()
        .await;
    resp.assert_status(StatusCode::PAYLOAD_TOO_LARGE);
}

// poem's `Compression` decompresses before the edge and leaves the compressed
// `Content-Length` in the headers; the gzip is real so that middleware runs.
#[tokio::test]
async fn a_compressed_body_cannot_outrun_the_cap_it_declares_it_is_under() {
    let client = boot_with(true).await;

    let payload = vec![b'x'; CAP * 100];
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(&payload).expect("gzip the payload");
    let compressed = encoder.finish().expect("finish the gzip stream");
    assert!(
        compressed.len() <= CAP,
        "the compressed body must fit under the cap for this to be the real case: {} bytes",
        compressed.len(),
    );

    let resp = client
        .post("/body/stream")
        .header(header::CONTENT_LENGTH, compressed.len().to_string())
        .header(header::CONTENT_ENCODING, "gzip")
        .body(compressed)
        .send()
        .await;

    resp.assert_status(StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn a_streamed_body_within_the_cap_is_passed_through() {
    let client = boot().await;
    let resp = client
        .post("/body/stream")
        .header(header::CONTENT_LENGTH, CAP.to_string())
        .body(vec![b'x'; CAP])
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text(format!("{CAP} bytes")).await;
}

#[tokio::test]
async fn a_body_within_the_cap_is_accepted() {
    let client = boot().await;
    let resp = client
        .post("/body/json")
        .content_type("application/json")
        .body(br#"{"value":"ok"}"#.to_vec())
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("ok").await;
}

/// Refuses every request, before anything reads its body.
#[injectable]
#[derive(Default)]
struct Refuse;

impl Layer for Refuse {}

#[async_trait]
impl Guard for Refuse {
    async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::unauthorized("no credential"))
    }
}

impl GuardFor<HttpView> for Refuse {}

/// Told once `/read/counted` has its body's first frame in hand.
static FIRST_FRAME: Notify = Notify::const_new();

/// Set when `/read/counted` has read its body to the end.
static COMPLETED: AtomicBool = AtomicBool::new(false);

#[controller(path = "/read")]
struct ReadController;

#[routes]
impl ReadController {
    #[post("/denied")]
    #[use_guards(Refuse)]
    async fn denied(&self, body: RawBody) -> String {
        format!("{} bytes", body.len())
    }

    /// Reads its body frame by frame, failing on the first error a read meets.
    #[post("/counted")]
    #[public]
    async fn counted(&self, body: poem::Body) -> poem::Result<String> {
        let mut stream = body.into_bytes_stream();
        let mut seen = 0usize;
        while let Some(chunk) = stream.next().await {
            if seen == 0 {
                FIRST_FRAME.notify_one();
            }
            seen += chunk.map_err(poem::error::BadRequest)?.len();
        }
        COMPLETED.store(true, Ordering::SeqCst);
        Ok(format!("{seen} bytes"))
    }
}

#[module(providers = [Refuse, ReadController])]
struct ReadModule;

/// [`ReadModule`] on a socket, under the cap, with no request budget unless
/// `request_timeout` names one.
async fn serve_read(request_timeout: Option<Duration>) -> Serving {
    serve_module::<ReadModule>(HttpConfig {
        max_body_bytes: Some(CAP),
        request_timeout,
        ..HttpConfig::default()
    })
    .await
}

/// A `POST path` head on a fresh connection, with `headers` (each ending
/// `\r\n`), then `body` as sent so far.
async fn post(port: u16, path: &str, headers: &str, body: &[u8]) -> TcpStream {
    let mut stream = connect(port).await;
    stream
        .write_all(format!("POST {path} HTTP/1.1\r\nHost: localhost\r\n{headers}\r\n").as_bytes())
        .await
        .expect("the head is sent");
    stream
        .write_all(body)
        .await
        .expect("the body so far is sent");
    stream
}

/// `data` as one chunk of a chunked body.
fn chunk(data: &[u8]) -> Vec<u8> {
    let mut framed = format!("{:x}\r\n", data.len()).into_bytes();
    framed.extend_from_slice(data);
    framed.extend_from_slice(b"\r\n");
    framed
}

const CHUNKED: &str = "Transfer-Encoding: chunked\r\n";

/// The default gap bound, and a second past it.
fn past_the_gap() -> Duration {
    HttpConfig::default().body_read_timeout + Duration::from_secs(1)
}

/// The guard answers while the body is still open: the client sent one chunk
/// and nothing more, so nothing past it was read before the refusal.
#[tokio::test]
async fn a_chunked_body_to_a_denied_route_is_answered_before_it_is_read() {
    let serving = serve_read(None).await;
    let mut stream = post(serving.port, "/read/denied", CHUNKED, &chunk(b"part")).await;

    let head = tokio::time::timeout(PATIENCE, read_head(&mut stream))
        .await
        .expect("answered with the body still open, its first chunk the only one sent");
    assert!(head.starts_with("HTTP/1.1 401"), "{head}");
    serving.cancel.cancel();
}

/// hyper writes `100 Continue` when the body is first read, so a request the
/// guards refuse never hears it.
#[tokio::test]
async fn expect_100_continue_is_not_answered_for_a_denied_request() {
    let serving = serve_read(None).await;
    let mut stream = post(
        serving.port,
        "/read/denied",
        "Transfer-Encoding: chunked\r\nExpect: 100-continue\r\n",
        b"",
    )
    .await;

    let head = tokio::time::timeout(PATIENCE, read_head(&mut stream))
        .await
        .expect("answered without the body");
    assert!(head.starts_with("HTTP/1.1 401"), "{head}");
    serving.cancel.cancel();
}

/// A body that keeps arriving, a byte at a time, never stalls; the request
/// budget runs from the request's arrival and cuts it all the same.
#[tokio::test]
async fn a_trickling_body_is_cut_by_the_request_deadline_from_arrival() {
    let serving = serve_read(Some(Duration::from_secs(1))).await;
    let mut stream = post(serving.port, "/read/counted", CHUNKED, &chunk(b"x")).await;
    for _ in 0..5 {
        after(Duration::from_millis(300)).await;
        // The answer may already be out and the connection closing.
        let _ = stream.write_all(&chunk(b"x")).await;
    }

    let head = tokio::time::timeout(PATIENCE, read_head(&mut stream))
        .await
        .expect("answered before the body ended");
    assert!(head.starts_with("HTTP/1.1 503"), "{head}");
    assert!(
        head.to_ascii_lowercase().contains("retry-after: 1\r\n"),
        "{head}"
    );
    assert!(!COMPLETED.load(Ordering::SeqCst), "the handler was cut");
    serving.cancel.cancel();
}

/// The gap bound, once its first frame is in: what is left is never read, so
/// the HTTP/1 connection cannot carry another request and is closed.
#[tokio::test]
async fn a_stalled_body_is_answered_408_and_the_http1_connection_closed() {
    let serving = serve_read(None).await;
    let mut stream = post(serving.port, "/read/counted", CHUNKED, &chunk(b"part")).await;
    tokio::time::timeout(PATIENCE, FIRST_FRAME.notified())
        .await
        .expect("the handler reads the body as it arrives");

    after(past_the_gap()).await;
    let answer = read_to_end(&mut stream).await;
    assert!(answer.starts_with("HTTP/1.1 408"), "{answer}");
    assert!(
        answer
            .to_ascii_lowercase()
            .contains("connection: close\r\n"),
        "{answer}"
    );
    serving.cancel.cancel();
}

/// A declared length is no promise the bytes come: the wrapper bounds its gaps
/// too.
#[tokio::test]
async fn a_declared_length_body_that_stalls_is_answered_408() {
    let serving = serve_read(None).await;
    let mut stream = post(
        serving.port,
        "/read/counted",
        "Content-Length: 32\r\n",
        b"eight by",
    )
    .await;
    tokio::time::timeout(PATIENCE, FIRST_FRAME.notified())
        .await
        .expect("the handler reads the body as it arrives");

    after(past_the_gap()).await;
    let answer = read_to_end(&mut stream).await;
    assert!(answer.starts_with("HTTP/1.1 408"), "{answer}");
    serving.cancel.cancel();
}

/// At the defaults the gap is shorter than the request budget, which started at
/// arrival: a body that stops arriving is answered `408`, not the budget's
/// `503` inviting a retry.
#[tokio::test]
async fn a_stalled_body_is_answered_408_inside_the_default_request_timeout() {
    let serving = serve_read(HttpConfig::default().request_timeout).await;
    let mut stream = post(serving.port, "/read/counted", CHUNKED, &chunk(b"part")).await;
    tokio::time::timeout(PATIENCE, FIRST_FRAME.notified())
        .await
        .expect("the handler reads the body as it arrives");

    after(past_the_gap()).await;
    let head = tokio::time::timeout(PATIENCE, read_head(&mut stream))
        .await
        .expect("answered");
    assert!(
        head.starts_with("HTTP/1.1 408"),
        "the default gap answers before the default budget: {head}"
    );
    serving.cancel.cancel();
}

/// Read as it arrives rather than buffered first, a body past the cap still
/// answers `413`, and the handler reading it never reaches its end.
#[tokio::test]
async fn a_chunked_body_past_the_cap_is_413_and_the_handler_never_completes() {
    let serving = serve_read(None).await;
    let mut body = chunk(&[b'x'; CAP]);
    body.extend(chunk(b"one too many"));
    body.extend_from_slice(b"0\r\n\r\n");
    let mut stream = post(serving.port, "/read/counted", CHUNKED, &body).await;

    let head = tokio::time::timeout(PATIENCE, read_head(&mut stream))
        .await
        .expect("answered");
    assert!(head.starts_with("HTTP/1.1 413"), "{head}");
    assert!(
        !COMPLETED.load(Ordering::SeqCst),
        "the handler never read past the cap"
    );
    serving.cancel.cancel();
}

/// On HTTP/2 a stalled body ends its stream alone: the `408` carries no
/// `Connection` (RFC 9113 §8.2.2), and the connection serves the next stream.
#[tokio::test]
async fn a_stalled_body_on_http2_is_answered_408_and_its_connection_keeps_serving() {
    let serving = serve_read(None).await;
    let (client, connection) = h2::client::handshake(connect(serving.port).await)
        .await
        .expect("the HTTP/2 preface is accepted");
    let connection = tokio::spawn(connection);
    let mut client = client.ready().await.expect("a stream can open");
    let request =
        poem::http::Request::post(format!("http://127.0.0.1:{}/read/counted", serving.port))
            .body(())
            .expect("a request");
    let (response, mut body) = client.send_request(request, false).expect("sent");
    body.send_data(bytes::Bytes::from_static(b"part"), false)
        .expect("a first frame");
    tokio::time::timeout(PATIENCE, FIRST_FRAME.notified())
        .await
        .expect("the handler reads the body as it arrives");

    after(past_the_gap()).await;
    let response = tokio::time::timeout(PATIENCE, response)
        .await
        .expect("the stream is answered")
        .expect("answered, not reset");
    assert_eq!(response.status(), 408);
    assert!(response.headers().get(header::CONNECTION).is_none());

    let mut client = client
        .ready()
        .await
        .expect("the connection opens another stream");
    let next = poem::http::Request::post(format!("http://127.0.0.1:{}/read/counted", serving.port))
        .body(())
        .expect("a request");
    let (next, _) = client.send_request(next, true).expect("sent");
    let next = tokio::time::timeout(PATIENCE, next)
        .await
        .expect("the next stream is answered")
        .expect("answered on the same connection");
    assert_eq!(next.status(), 200);
    assert!(!connection.is_finished(), "the connection keeps serving");
    serving.cancel.cancel();
}

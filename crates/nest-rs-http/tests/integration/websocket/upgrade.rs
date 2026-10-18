//! `WebSocketUpgrade`: the opening handshake's refusals as a client reads them
//! on the wire, and a socket served over the upgrade they admit.

use futures_util::{SinkExt, StreamExt};
use nest_rs_core::{ContainerBuilder, module};
use nest_rs_http::{HttpConfig, HttpCors, HttpEndpointMeta, Method, endpoint_fn};
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::transport::{PATIENCE, Serving, connect, echo, read_head, serve_module};

/// The echo socket, self-mounted at `/socket` for the `GET` its handshake is.
struct EchoMount;

impl nest_rs_core::Discoverable for EchoMount {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<EchoMount, HttpEndpointMeta>(
            HttpEndpointMeta::new(
                &crate::PUBLIC_MOUNT,
                "/socket",
                "echo",
                |_container, routes| {
                    routes.route(Method::GET, "/socket", endpoint_fn(echo));
                },
            )
            .owned_by("EchoMount"),
        )
    }
}

#[module(providers = [EchoMount])]
struct EchoModule;

/// The handshake a browser sends, one header line per entry.
const HANDSHAKE: [&str; 4] = [
    "upgrade: websocket",
    "connection: Upgrade",
    "sec-websocket-version: 13",
    "sec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==",
];

/// The head answering `method /socket` with `HANDSHAKE`'s lines, each
/// `replaced` line given its new value and an empty one dropped.
async fn answer(serving: &Serving, method: &str, replaced: &[(&str, &str)]) -> String {
    let mut lines = String::new();
    for line in HANDSHAKE {
        let name = line.split(':').next().unwrap_or_default();
        match replaced.iter().find(|(replaced, _)| *replaced == name) {
            Some((_, "")) => {}
            Some((_, value)) => lines.push_str(&format!("{name}: {value}\r\n")),
            None => lines.push_str(&format!("{line}\r\n")),
        }
    }
    let mut stream = connect(serving.port).await;
    stream
        .write_all(
            format!("{method} /socket HTTP/1.1\r\nhost: localhost\r\n{lines}\r\n").as_bytes(),
        )
        .await
        .expect("the handshake is sent");
    tokio::time::timeout(PATIENCE, read_head(&mut stream))
        .await
        .expect("answered")
}

#[tokio::test]
async fn each_refused_handshake_answers_its_status_on_the_wire() {
    let serving = serve_module::<EchoModule>(HttpConfig::default()).await;
    for (method, replaced, status) in [
        ("GET", &[("sec-websocket-key", "")][..], "400"),
        ("GET", &[("connection", "keep-alive")][..], "400"),
        ("GET", &[("upgrade", "")][..], "400"),
        ("GET", &[("sec-websocket-version", "8")][..], "426"),
        ("POST", &[][..], "405"),
    ] {
        let head = answer(&serving, method, replaced).await;
        assert!(
            head.starts_with(&format!("HTTP/1.1 {status}")),
            "{method} {replaced:?}: {head}",
        );
        assert!(
            head.contains("application/problem+json"),
            "{method} {replaced:?}: each refusal is a problem document: {head}",
        );
    }
    let version = answer(&serving, "GET", &[("sec-websocket-version", "8")]).await;
    assert!(
        version.contains("sec-websocket-version: 13"),
        "RFC 6455 §4.4: the version spoken is named: {version}",
    );
    let post = answer(&serving, "POST", &[]).await;
    assert!(
        post.contains("allow: GET, HEAD"),
        "another method meets the route table's `405`: {post}",
    );
    serving.cancel.cancel();
}

/// The table serves a `HEAD` through the `GET` entry, so the handshake sees
/// a `GET`: it still refuses it, since a handshake is a `GET` (RFC 6455
/// §4.1), and no socket opens.
#[tokio::test]
async fn a_head_carrying_the_handshake_is_refused_and_switches_nothing() {
    let serving = serve_module::<EchoModule>(HttpConfig::default()).await;
    let head = answer(&serving, "HEAD", &[]).await;
    assert!(head.starts_with("HTTP/1.1 400"), "{head}");
    assert!(
        !head.contains("sec-websocket-accept"),
        "nothing switches: {head}"
    );
    serving.cancel.cancel();
}

/// A text frame goes out and comes back over the upgrade the handshake
/// opened, with `config`'s layers between the client and the socket.
async fn echoes(config: HttpConfig) {
    let serving = serve_module::<EchoModule>(config).await;
    let mut request = format!("ws://127.0.0.1:{}/socket", serving.port)
        .into_client_request()
        .expect("a websocket url");
    request.headers_mut().insert(
        nest_rs_http::header::ACCEPT_ENCODING,
        nest_rs_http::HeaderValue::from_static("gzip, br"),
    );
    request.headers_mut().insert(
        nest_rs_http::header::ORIGIN,
        nest_rs_http::HeaderValue::from_static("https://app.example"),
    );
    let (mut socket, response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("the handshake is accepted");
    assert_eq!(
        response.status(),
        nest_rs_http::StatusCode::SWITCHING_PROTOCOLS
    );

    socket
        .send(Message::text("ping"))
        .await
        .expect("a frame is sent");
    let echoed = tokio::time::timeout(PATIENCE, socket.next())
        .await
        .expect("the echo arrives")
        .expect("the socket is open")
        .expect("a frame");
    assert_eq!(echoed, Message::text("ping"));
    socket.close(None).await.expect("the close handshake runs");
    serving.cancel.cancel();
}

#[tokio::test]
async fn an_echo_round_trips_over_a_real_upgrade() {
    echoes(HttpConfig::default()).await;
}

/// An upgrade request crosses the CORS and compression layers with its socket
/// still reachable, and its `101` leaves them unencoded.
#[tokio::test]
async fn an_upgrade_crosses_cors_and_compression() {
    echoes(HttpConfig {
        cors: Some(HttpCors {
            origins: vec!["https://app.example".into()],
            ..HttpCors::default()
        }),
        compression: true,
        ..HttpConfig::default()
    })
    .await;
}

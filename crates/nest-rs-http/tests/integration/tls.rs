//! The TLS listener: what it negotiates, and the material it watches, a
//! renewal swapped into the running listener — observed through real
//! handshakes on two leaves differing only in their name.

use std::net::TcpListener as StdTcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use nest_rs_core::{App, Transport, module};
use nest_rs_http::{HttpTls, HttpTransport, controller, routes};
use nest_rs_testing::{TestAuthority, TestCertificate};
use poem::Result;
use rustls::pki_types::ServerName;
use rustls::pki_types::pem::PemObject;
use tokio::io::AsyncWriteExt;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_util::sync::CancellationToken;

use crate::transport::{after, connect, free_port, read_head, read_to_end, still_open};

const HOST_A: &str = "a.nestrs.test";
const HOST_B: &str = "b.nestrs.test";

/// Two leaves under one authority, differing only in their name, issued for
/// this run.
struct Pki {
    authority: TestAuthority,
    a: TestCertificate,
    b: TestCertificate,
}

static PKI: LazyLock<Pki> = LazyLock::new(|| {
    let authority = TestAuthority::new();
    Pki {
        a: authority.server(&[HOST_A]),
        b: authority.server(&[HOST_B]),
        authority,
    }
});

fn cert_a() -> &'static [u8] {
    PKI.a.cert.as_bytes()
}

fn key_a() -> &'static [u8] {
    PKI.a.key.as_bytes()
}

fn cert_b() -> &'static [u8] {
    PKI.b.cert.as_bytes()
}

fn key_b() -> &'static [u8] {
    PKI.b.key.as_bytes()
}

/// The shortest watch interval the seconds-grained knob allows.
const RELOAD_SECS: u64 = 1;

#[controller(path = "/")]
struct PingController;

#[routes]
impl PingController {
    #[get("/ping")]
    async fn ping(&self) -> Result<&'static str> {
        Ok("pong")
    }
}

#[module(providers = [PingController])]
struct PingModule;

/// A directory the test owns, so the swap rewrites files nothing else reads.
struct Material {
    dir: PathBuf,
}

impl Material {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("nestrs-tls-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let material = Self { dir };
        material.write(cert_a(), key_a());
        material
    }

    fn write(&self, cert: &[u8], key: &[u8]) {
        std::fs::write(self.cert(), cert).expect("write cert");
        std::fs::write(self.key(), key).expect("write key");
    }

    fn cert(&self) -> PathBuf {
        self.dir.join("cert.pem")
    }

    fn key(&self) -> PathBuf {
        self.dir.join("key.pem")
    }
}

impl Drop for Material {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A client that trusts the test authority and resolves both test names to the
/// bound port.
///
/// The authority is the client's only root: merged into the platform's store,
/// macOS's 825-day ceiling on server certificates would fail the test leaves.
fn client(host: &str, port: u16) -> reqwest::Client {
    reqwest::Client::builder()
        .tls_certs_only([
            reqwest::Certificate::from_pem(PKI.authority.pem().as_bytes())
                .expect("the test authority parses"),
        ])
        .resolve(host, ([127, 0, 0, 1], port).into())
        .build()
        .expect("client builds")
}

/// A request on `client`'s own connection pool, reusing its connection.
async fn ping_with(
    client: &reqwest::Client,
    host: &str,
    port: u16,
) -> reqwest::Result<reqwest::Response> {
    client
        .get(format!("https://{host}:{port}/ping"))
        .send()
        .await
}

/// A request on a fresh pool, so every call handshakes.
async fn ping(host: &str, port: u16) -> reqwest::Result<reqwest::Response> {
    ping_with(&client(host, port), host, port).await
}

/// Poll until `host` answers.
async fn ping_until_ok(host: &str, port: u16, within: Duration) -> Option<String> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        if let Ok(resp) = ping(host, port).await
            && resp.status().is_success()
        {
            return resp.text().await.ok();
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Let the watcher take `ticks` intervals on paused time. No request is in
/// flight while the clock is paused, so no other timer is passed over.
async fn watch(ticks: u32) {
    tokio::time::pause();
    tokio::time::sleep(Duration::from_secs(RELOAD_SECS) * ticks).await;
    tokio::time::resume();
}

/// A configured transport over the material on disk, not yet served.
async fn transport_for(port: u16, material: &Material, reload_secs: u64) -> HttpTransport {
    let tls = HttpTls::from_files(material.cert(), material.key())
        .expect("the test material reads")
        .with_reload_secs(reload_secs);
    let app = App::builder()
        .module::<PingModule>()
        .build()
        .await
        .expect("module boots");
    let mut transport = HttpTransport::new()
        .bind(format!("127.0.0.1:{port}"))
        .tls(tls);
    transport
        .configure(app.container())
        .await
        .expect("transport configures");
    transport
}

async fn serve(port: u16, material: &Material, reload_secs: u64) -> CancellationToken {
    let transport = transport_for(port, material, reload_secs).await;
    let cancel = CancellationToken::new();
    let handle = cancel.clone();
    tokio::spawn(async move {
        let _ = Box::new(transport).serve(handle).await;
    });
    cancel
}

#[tokio::test]
async fn a_renewed_certificate_is_served_without_dropping_the_listener() {
    let material = Material::new("swap");
    let port = free_port();
    let cancel = serve(port, &material, RELOAD_SECS).await;

    let body = ping_until_ok(HOST_A, port, Duration::from_secs(10)).await;
    assert_eq!(body.as_deref(), Some("pong"), "leaf A serves {HOST_A}");
    assert!(
        ping(HOST_B, port).await.is_err(),
        "leaf A cannot answer for {HOST_B}",
    );

    material.write(cert_b(), key_b());
    watch(3).await;

    let body = ping_until_ok(HOST_B, port, Duration::from_secs(10)).await;
    assert_eq!(
        body.as_deref(),
        Some("pong"),
        "the renewed leaf B is picked up on the same listener",
    );
    assert!(
        ping(HOST_A, port).await.is_err(),
        "and the superseded leaf A is no longer presented",
    );

    cancel.cancel();
}

/// A connection established before the swap keeps its session: a held client
/// still answers for the superseded name, which a fresh one is refused.
#[tokio::test]
async fn a_connection_open_across_the_swap_is_answered_not_reset() {
    let material = Material::new("in-flight");
    let port = free_port();
    let cancel = serve(port, &material, RELOAD_SECS).await;

    assert_eq!(
        ping_until_ok(HOST_A, port, Duration::from_secs(10))
            .await
            .as_deref(),
        Some("pong"),
        "the server comes up on leaf A",
    );

    // Read the body to completion, so the connection returns to the pool.
    let held = client(HOST_A, port);
    let opened = ping_with(&held, HOST_A, port)
        .await
        .expect("the held client connects under leaf A");
    assert!(opened.status().is_success());
    assert_eq!(opened.text().await.ok().as_deref(), Some("pong"));

    material.write(cert_b(), key_b());
    watch(3).await;

    assert_eq!(
        ping_until_ok(HOST_B, port, Duration::from_secs(10))
            .await
            .as_deref(),
        Some("pong"),
        "the renewal is picked up",
    );
    assert!(
        ping(HOST_A, port).await.is_err(),
        "a new connection can no longer be opened under the superseded leaf",
    );

    let survived = ping_with(&held, HOST_A, port)
        .await
        .expect("the connection opened before the swap was never dropped");
    assert!(survived.status().is_success());
    assert_eq!(
        survived.text().await.ok().as_deref(),
        Some("pong"),
        "and it is still served, on the session it handshook under leaf A",
    );

    cancel.cancel();
}

#[tokio::test]
async fn watching_off_keeps_serving_the_certificate_it_booted_with() {
    let material = Material::new("pinned");
    let port = free_port();
    let cancel = serve(port, &material, 0).await;

    assert_eq!(
        ping_until_ok(HOST_A, port, Duration::from_secs(10))
            .await
            .as_deref(),
        Some("pong"),
    );
    material.write(cert_b(), key_b());
    watch(3).await;
    assert!(
        ping(HOST_B, port).await.is_err(),
        "watching is off, so the renewal is not picked up",
    );
    assert!(
        ping(HOST_A, port).await.is_ok(),
        "and the booted certificate keeps serving",
    );

    cancel.cancel();
}

/// The listener's certificate resolver hands out any `CertifiedKey` unchecked,
/// so the transport must refuse a pair that cannot serve before it binds.
#[tokio::test]
async fn material_that_cannot_serve_fails_the_boot_rather_than_binding() {
    let material = Material::new("boot-refused");
    // Leaf B's certificate beside leaf A's key: both parse, neither corresponds.
    material.write(cert_b(), key_a());
    let port = free_port();
    let transport = transport_for(port, &material, 0).await;

    let err = Box::new(transport)
        .serve(CancellationToken::new())
        .await
        .expect_err("a pair that cannot serve does not reach the listener");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("do not correspond"),
        "the boot failure names what is wrong: {msg}",
    );

    assert!(
        StdTcpListener::bind(("127.0.0.1", port)).is_ok(),
        "and the port was never taken",
    );
}

/// A renewal to an empty certificate or a mismatched pair is refused: either,
/// installed, fails every handshake.
#[tokio::test]
async fn a_renewal_that_cannot_serve_is_refused_and_the_certificate_in_use_keeps_serving() {
    let material = Material::new("renewal-refused");
    let port = free_port();
    let cancel = serve(port, &material, RELOAD_SECS).await;

    assert_eq!(
        ping_until_ok(HOST_A, port, Duration::from_secs(10))
            .await
            .as_deref(),
        Some("pong"),
        "leaf A is serving to begin with",
    );

    // An empty certificate parses as a chain holding nothing.
    material.write(b"", key_a());
    watch(3).await;
    assert_eq!(
        ping_until_ok(HOST_A, port, Duration::from_secs(5))
            .await
            .as_deref(),
        Some("pong"),
        "an empty certificate is refused and leaf A keeps serving",
    );

    material.write(cert_b(), key_a());
    watch(3).await;
    assert_eq!(
        ping_until_ok(HOST_A, port, Duration::from_secs(5))
            .await
            .as_deref(),
        Some("pong"),
        "a mismatched pair is refused and leaf A still keeps serving",
    );
    assert!(
        ping(HOST_B, port).await.is_err(),
        "and leaf B's certificate was never presented",
    );

    // A corresponding pair still lands: the watcher is not stuck on the rejection.
    material.write(cert_b(), key_b());
    watch(3).await;
    assert_eq!(
        ping_until_ok(HOST_B, port, Duration::from_secs(10))
            .await
            .as_deref(),
        Some("pong"),
        "a usable renewal after a refused one is still picked up",
    );

    cancel.cancel();
}

#[test]
fn from_files_reports_the_path_it_could_not_read() {
    let err = HttpTls::from_files(Path::new("/nonexistent/cert.pem"), Path::new("/dev/null"))
        .expect_err("a missing certificate is not silently skipped");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("/nonexistent/cert.pem"),
        "the diagnostic names the path: {msg}",
    );
}

/// A client trusting only the test authority, offering `alpn` in that order,
/// and built on a provider of its own so it installs no process default.
fn connector(alpn: &[&[u8]]) -> TlsConnector {
    let mut roots = rustls::RootCertStore::empty();
    roots.add_parsable_certificates(
        rustls::pki_types::CertificateDer::pem_slice_iter(PKI.authority.pem().as_bytes())
            .filter_map(std::result::Result::ok),
    );
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("the provider speaks the default versions")
    .with_root_certificates(roots)
    .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
    TlsConnector::from(Arc::new(config))
}

/// A TLS session with the transport on `port`, under leaf A's name, offering `alpn`.
async fn handshake(port: u16, alpn: &[&[u8]]) -> TlsStream<tokio::net::TcpStream> {
    connector(alpn)
        .connect(
            ServerName::try_from(HOST_A).expect("a DNS name"),
            connect(port).await,
        )
        .await
        .expect("the handshake completes")
}

/// The listener prefers HTTP/2 and still speaks HTTP/1.1.
#[tokio::test]
async fn the_listener_offers_h2_then_http1() {
    let material = Material::new("alpn");
    let port = free_port();
    let cancel = serve(port, &material, 0).await;

    let either = handshake(port, &[b"http/1.1", b"h2"]).await;
    assert_eq!(
        either.get_ref().1.alpn_protocol(),
        Some(&b"h2"[..]),
        "offered both, the server's own order chooses",
    );
    let only_http1 = handshake(port, &[b"http/1.1"]).await;
    assert_eq!(
        only_http1.get_ref().1.alpn_protocol(),
        Some(&b"http/1.1"[..])
    );

    cancel.cancel();
}

/// An HTTP/2 client over TLS is told how many streams it may open at once.
#[tokio::test]
async fn an_h2_client_over_tls_reads_the_stream_cap() {
    let material = Material::new("h2");
    let port = free_port();
    let cancel = serve(port, &material, 0).await;

    let (client, mut connection) = h2::client::handshake(handshake(port, &[b"h2"]).await)
        .await
        .expect("the HTTP/2 preface is accepted");
    let mut client = client.ready().await.expect("a stream can open");
    let request = poem::http::Request::get(format!("https://{HOST_A}:{port}/ping"))
        .body(())
        .expect("a request");
    let (response, _) = client.send_request(request, true).expect("sent");
    let response = tokio::select! {
        response = response => response.expect("answered"),
        ended = &mut connection => panic!("the connection ended first: {ended:?}"),
    };
    assert_eq!(response.status(), 200);
    assert_eq!(connection.max_concurrent_send_streams(), 200);

    cancel.cancel();
}

/// The head cap holds behind TLS as it does in plaintext.
#[tokio::test]
async fn an_http1_head_past_64_kib_answers_431_over_tls() {
    let material = Material::new("head-cap");
    let port = free_port();
    let cancel = serve(port, &material, 0).await;

    let mut stream = handshake(port, &[b"http/1.1"]).await;
    let filler = "a".repeat(70 * 1024);
    // The server answers before it has read it all, so the tail may meet a reset.
    let _ = stream
        .write_all(
            format!("GET /ping HTTP/1.1\r\nHost: {HOST_A}\r\nX-Filler: {filler}\r\n\r\n")
                .as_bytes(),
        )
        .await;
    let head = read_head(&mut stream).await;
    assert!(head.starts_with("HTTP/1.1 431"), "{head}");

    cancel.cancel();
}

/// Serving TLS installs aws-lc-rs as the process provider when the app chose
/// none, so a client built later from the default finds one. nextest runs each
/// test in a process of its own.
#[tokio::test]
async fn the_tls_listener_installs_the_default_crypto_provider_when_none_is() {
    assert!(
        rustls::crypto::CryptoProvider::get_default().is_none(),
        "nothing installed a provider before the transport served",
    );
    let material = Material::new("provider");
    let port = free_port();
    let cancel = serve(port, &material, 0).await;
    drop(connect(port).await);

    let installed =
        rustls::crypto::CryptoProvider::get_default().expect("the listener installed one");
    assert_eq!(
        format!("{:?}", installed.key_provider),
        format!(
            "{:?}",
            rustls::crypto::aws_lc_rs::default_provider().key_provider
        ),
    );

    cancel.cancel();
}

/// A client that opens a TCP connection and never starts the handshake is
/// dropped at the head deadline — 30 s by default — counted from accept: the
/// handshake is part of the first head's time.
#[tokio::test]
async fn a_tls_client_that_never_sends_a_client_hello_is_dropped_at_the_head_deadline() {
    let material = Material::new("silent");
    let port = free_port();
    let cancel = serve(port, &material, 0).await;
    let mut silent = connect(port).await;
    // Answered over a handshake of its own: the silent socket was accepted first.
    assert_eq!(
        ping_until_ok(HOST_A, port, Duration::from_secs(5))
            .await
            .as_deref(),
        Some("pong"),
    );

    after(Duration::from_secs(29)).await;
    assert!(still_open(&mut silent).await, "inside the head deadline");
    after(Duration::from_secs(2)).await;
    assert_eq!(read_to_end(&mut silent).await, "", "dropped without a byte");

    cancel.cancel();
}

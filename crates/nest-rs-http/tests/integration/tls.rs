//! TLS material is watched, and a renewal is swapped into the running
//! listener — observed through real handshakes on two leaves differing only in
//! their name.

use std::net::TcpListener as StdTcpListener;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use nest_rs_core::{App, Transport, module};
use nest_rs_http::{HttpTls, HttpTransport, controller, routes};
use nest_rs_testing::{TestAuthority, TestCertificate};
use poem::Result;
use tokio_util::sync::CancellationToken;

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

/// An OS-assigned free port, bound and released for the transport to take.
fn free_port() -> u16 {
    let listener = StdTcpListener::bind(("127.0.0.1", 0)).expect("bind an ephemeral port");
    listener.local_addr().expect("local addr").port()
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

/// poem does not validate a *stream* of `RustlsConfig` (`into_stream` is
/// `Ok(self)`), so the transport must refuse material that cannot serve.
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

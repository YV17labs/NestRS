//! TLS material is watched, and a renewal is swapped into the running
//! listener.
//!
//! The swap is only observable through a real handshake, so this module binds
//! an actual port and drives it with a real client. Two leaves under one CA,
//! differing only in their subject name, make "which certificate is serving?"
//! a question the client can answer: it trusts both, so the hostname the
//! handshake accepts is the only thing the swap changes.

use std::net::TcpListener as StdTcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

use nest_rs_core::{App, Transport, module};
use nest_rs_http::{HttpTransport, TlsConfig, controller, routes};
use poem::Result;
use tokio_util::sync::CancellationToken;

const CA: &[u8] = include_bytes!("fixtures/tls_ca.pem");
const CERT_A: &[u8] = include_bytes!("fixtures/tls_a.pem");
const KEY_A: &[u8] = include_bytes!("fixtures/tls_a.key.pem");
const CERT_B: &[u8] = include_bytes!("fixtures/tls_b.pem");
const KEY_B: &[u8] = include_bytes!("fixtures/tls_b.key.pem");

const HOST_A: &str = "a.nestrs.test";
const HOST_B: &str = "b.nestrs.test";

/// The watch interval the serving tests use — the shortest the seconds-grained
/// knob allows, so a renewal lands within a tick or two.
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
        material.write(CERT_A, KEY_A);
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

/// A client that trusts the fixture CA and resolves both test names to the
/// bound port, so the *only* reason a request can fail is the certificate the
/// server presents.
fn client(host: &str, port: u16) -> reqwest::Client {
    reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(CA).expect("fixture CA parses"))
        .resolve(host, ([127, 0, 0, 1], port).into())
        .build()
        .expect("client builds")
}

/// An OS-assigned free port. Bound and released, so the transport can take it —
/// racy in principle, adequate in practice and the same trick the rest of the
/// suite's socket tests use.
fn free_port() -> u16 {
    let listener = StdTcpListener::bind(("127.0.0.1", 0)).expect("bind an ephemeral port");
    listener.local_addr().expect("local addr").port()
}

/// A request on `client`'s own connection pool — so a second call reuses the
/// connection the first one opened, instead of handshaking again.
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

/// A request on a **fresh** pool: every call handshakes, so what it observes is
/// the certificate the listener presents right now.
async fn ping(host: &str, port: u16) -> reqwest::Result<reqwest::Response> {
    ping_with(&client(host, port), host, port).await
}

/// Poll until `host` answers, so the test waits on the server being ready
/// rather than on a fixed sleep.
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

/// A configured transport over the material on disk, not yet served.
async fn transport_for(port: u16, material: &Material, reload_secs: u64) -> HttpTransport {
    let tls = TlsConfig::from_files(material.cert(), material.key())
        .expect("fixture material reads")
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

    // Before the swap: leaf A is serving, so its name verifies and B's does not.
    let body = ping_until_ok(HOST_A, port, Duration::from_secs(10)).await;
    assert_eq!(body.as_deref(), Some("pong"), "leaf A serves {HOST_A}");
    assert!(
        ping(HOST_B, port).await.is_err(),
        "leaf A cannot answer for {HOST_B}",
    );

    // Renew in place. Nothing restarts, nothing rebinds.
    material.write(CERT_B, KEY_B);

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

/// The reason the swap goes through the listener's config *stream* rather than
/// a rebind: a connection already established keeps the session it handshook
/// on, so a request in flight when the renewal lands is answered instead of
/// reset. Rebuilding the listener would drop it, and the test above — which
/// handshakes afresh every time — could not tell the difference.
///
/// The two clients are what make it observable. One is held across the swap and
/// never handshakes again; the other is built per request, so it does. After
/// the renewal the held connection answers **for the superseded name**, which a
/// new connection can no longer be opened for at all.
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

    // Open the connection under leaf A and read the body to completion, so it
    // goes back to this client's pool rather than being torn down.
    let held = client(HOST_A, port);
    let opened = ping_with(&held, HOST_A, port)
        .await
        .expect("the held client connects under leaf A");
    assert!(opened.status().is_success());
    assert_eq!(opened.text().await.ok().as_deref(), Some("pong"));

    material.write(CERT_B, KEY_B);

    // The swap has landed once a *fresh* handshake is answered under leaf B…
    assert_eq!(
        ping_until_ok(HOST_B, port, Duration::from_secs(10))
            .await
            .as_deref(),
        Some("pong"),
        "the renewal is picked up",
    );
    // …and, at that same moment, a fresh connection for leaf A is refused. So
    // anything still answering for {HOST_A} below is not handshaking.
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
    // `reload_secs = 0` is the opt-out, and it must be a real one: a renewal on
    // disk changes nothing until the process restarts.
    let material = Material::new("pinned");
    let port = free_port();
    let cancel = serve(port, &material, 0).await;

    assert_eq!(
        ping_until_ok(HOST_A, port, Duration::from_secs(10))
            .await
            .as_deref(),
        Some("pong"),
    );
    material.write(CERT_B, KEY_B);
    tokio::time::sleep(Duration::from_secs(2)).await;
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

/// poem validates a `RustlsConfig` handed to it directly, but a **stream** of
/// them takes the blanket `IntoTlsConfigStream` impl, whose `into_stream` is
/// `Ok(self)`. So material that cannot serve booted, bound the port, accepted
/// TCP and dropped every connection — healthy by every signal an operator has.
#[tokio::test]
async fn material_that_cannot_serve_fails_the_boot_rather_than_binding() {
    let material = Material::new("boot-refused");
    // Leaf B's certificate beside leaf A's key: both halves parse, and neither
    // corresponds to the other.
    material.write(CERT_B, KEY_A);
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

/// The renewal half of the same question. A settled read cannot see that two
/// atomically-written halves do not correspond, nor that a file settled at zero
/// bytes — and either one, installed, fails **every** handshake. Both used to be
/// published and announced as `tls certificate renewed on disk`.
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

    // An empty certificate — a truncate that stalls, or a writer that creates
    // before it writes. It parses as a chain holding nothing.
    material.write(b"", KEY_A);
    tokio::time::sleep(Duration::from_secs(3 * RELOAD_SECS)).await;
    assert_eq!(
        ping_until_ok(HOST_A, port, Duration::from_secs(5))
            .await
            .as_deref(),
        Some("pong"),
        "an empty certificate is refused and leaf A keeps serving",
    );

    // A mismatched pair: leaf B's certificate, leaf A's key.
    material.write(CERT_B, KEY_A);
    tokio::time::sleep(Duration::from_secs(3 * RELOAD_SECS)).await;
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

    // A pair that *does* correspond still lands, so the refusals above did not
    // leave the watcher stuck on the material it rejected.
    material.write(CERT_B, KEY_B);
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
    let err = TlsConfig::from_files(Path::new("/nonexistent/cert.pem"), Path::new("/dev/null"))
        .expect_err("a missing certificate is not silently skipped");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("/nonexistent/cert.pem"),
        "the diagnostic names the path: {msg}",
    );
}

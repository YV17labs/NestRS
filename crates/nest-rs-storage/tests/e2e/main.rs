//! `nest-rs-storage`'s suite against a live S3-compatible server over TLS: the
//! dev container's RustFS and its `nestrs` bucket, which the environment
//! creates as it does the database, or the store `<PREFIX>_STORAGE__URL` and
//! `<PREFIX>_STORAGE__ENDPOINT` name.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod client;
mod store;
mod transfer;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nest_rs_config::Config;
use nest_rs_storage::{Storage, StorageConfig};
use nest_rs_testing::{TestAuthority, TestCertificate, system_connector};
use rustls::pki_types::ServerName;
use std::sync::LazyLock;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The authority this test process issues the proxy's certificate with.
static AUTHORITY: LazyLock<TestAuthority> = LazyLock::new(TestAuthority::new);

/// What the proxy presents: the loopback it listens on.
static PROXY: LazyLock<TestCertificate> =
    LazyLock::new(|| AUTHORITY.server(&["127.0.0.1", "localhost"]));

/// The suite's store as an app resolves it: `<PREFIX>_STORAGE__*` over the
/// test profile's defaults — the dev container's RustFS and the pair it
/// accepts, unless the environment names others.
fn config() -> StorageConfig {
    nest_rs_testing::load_project_env();
    StorageConfig::load().expect("the suite's storage config resolves")
}

/// The store's endpoint, which `<PREFIX>_STORAGE__ENDPOINT` names, or the
/// test profile names the dev container's RustFS.
fn endpoint() -> String {
    config()
        .endpoint
        .expect("the suite's store names an S3-compatible server")
}

/// A client of the suite's store, opened and proved.
async fn storage() -> Storage {
    Storage::connect(&config())
        .await
        .expect("the suite's store opens")
}

/// A client reaching the store through the proxy at `proxy`, with `config`'s
/// other settings, trusting the test authority the proxy's certificate is of.
/// The proof of its bucket takes the proxy's first connection.
async fn proxied(proxy: SocketAddr, config: StorageConfig) -> Storage {
    Storage::connect(&StorageConfig {
        endpoint: Some(nest_rs_testing::url_at(
            &config.endpoint.clone().expect("an endpoint"),
            proxy,
        )),
        tls: AUTHORITY.client_tls(),
        ..config
    })
    .await
    .expect("the store opens through the proxy")
}

/// A key no other run can collide with — the bucket is shared with every other
/// suite in the devcontainer, and `list` asserts on an exact set.
fn unique(label: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("e2e-{}-{}-{}", std::process::id(), nanos, label)
}

/// What a [`proxy`] does with one connection's bytes: everything, at once,
/// unless a field says otherwise.
#[derive(Clone, Copy, Default)]
struct Carry {
    /// The caller's bytes, this many every 10 ms — an upload moving slowly.
    request_pace: Option<usize>,
    /// The store's bytes, this many every 10 ms — a download moving slowly.
    answer_pace: Option<usize>,
    /// Where the store's answer stops, and what the connection does then.
    answer_ends: Option<(Until, Then)>,
}

/// How far a [`proxy`] carries the store's answer.
#[derive(Clone, Copy)]
enum Until {
    /// Its first `n` bytes, headers included.
    Bytes(usize),
    /// Its headers, and not a byte of its body.
    BodyStarts,
}

/// What a [`proxy`] connection does once its answer stopped.
#[derive(Clone, Copy)]
enum Then {
    /// Holds still, the socket open — a transfer stalled.
    Hold,
    /// Closes on the caller — a connection broken mid-body.
    Close,
}

/// A proxy to the store carrying the `n`th connection of a client's calls,
/// from 0, as `carry(n)` says — ending TLS on both sides, so what it carries is
/// HTTP. The connection before them, the proof of the bucket a client opens
/// with, is carried whole and not counted. Returns its address and the count
/// of the calls' connections it took.
async fn proxy(carry: impl Fn(usize) -> Carry + Send + 'static) -> (SocketAddr, Arc<AtomicUsize>) {
    let parsed = reqwest::Url::parse(&endpoint()).expect("the storage endpoint parses");
    let host = parsed
        .host_str()
        .expect("the storage endpoint names a host")
        .to_owned();
    let upstream = format!(
        "{host}:{}",
        parsed
            .port_or_known_default()
            .expect("the storage endpoint has a port"),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the proxy");
    let addr = listener.local_addr().expect("the proxy's address");
    let taken = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&taken);
    tokio::spawn(async move {
        let (proof, _) = listener.accept().await.expect("accept the proof");
        let at = (host.clone(), upstream.clone());
        tokio::spawn(carried(proof, at, Carry::default()));
        loop {
            let (client, _) = listener.accept().await.expect("accept");
            let carry = carry(counted.fetch_add(1, Ordering::SeqCst));
            let at = (host.clone(), upstream.clone());
            tokio::spawn(carried(client, at, carry));
        }
    });
    (addr, taken)
}

/// One connection of a [`proxy`]'s, ending TLS from `client` and opening it to
/// the store `at` (its host, and its address), carried as `carry` says.
async fn carried(client: tokio::net::TcpStream, (host, upstream): (String, String), carry: Carry) {
    let Ok(client) = PROXY.acceptor(None).accept(client).await else {
        return;
    };
    let server = tokio::net::TcpStream::connect(&upstream)
        .await
        .expect("reach the store");
    let name = ServerName::try_from(host).expect("the store's host is a name");
    let server = system_connector()
        .connect(name, server)
        .await
        .expect("a TLS handshake with the store");
    let (mut client_read, mut client_write) = tokio::io::split(client);
    let (mut server_read, mut server_write) = tokio::io::split(server);
    tokio::spawn(async move {
        let _ = paced(
            &mut client_read,
            &mut server_write,
            carry.request_pace,
            None,
        )
        .await;
    });
    let ended = paced(
        &mut server_read,
        &mut client_write,
        carry.answer_pace,
        carry.answer_ends.map(|(until, _)| until),
    )
    .await;
    if ended && matches!(carry.answer_ends, Some((_, Then::Hold))) {
        std::future::pending::<()>().await;
    }
    // A split stream's half closes nothing when dropped: the connection ends
    // here, as a broken one does.
    let _ = client_write.shutdown().await;
    drop((server_read, client_write));
}

/// Copy `from` into `to`, `pace` bytes every 10 ms when set, up to `until`.
/// Answers whether it stopped there, rather than at the end of `from` or a
/// failed write.
async fn paced(
    from: &mut (impl AsyncRead + Unpin),
    to: &mut (impl AsyncWrite + Unpin),
    pace: Option<usize>,
    until: Option<Until>,
) -> bool {
    let mut carried = Vec::new();
    let mut buffer = vec![0; pace.unwrap_or(8 * 1024)];
    loop {
        let Ok(read @ 1..) = from.read(&mut buffer).await else {
            return false;
        };
        let sent = carried.len();
        carried.extend_from_slice(&buffer[..read]);
        let end = match until {
            None => None,
            Some(Until::Bytes(n)) => Some(n),
            Some(Until::BodyStarts) => carried
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|at| at + 4),
        };
        let upto = end.map_or(carried.len(), |end| end.min(carried.len()));
        if upto > sent
            && (to.write_all(&carried[sent..upto]).await.is_err() || to.flush().await.is_err())
        {
            return false;
        }
        if end.is_some_and(|end| carried.len() >= end) {
            return true;
        }
        if pace.is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

//! `nest-rs-storage`'s suite against a live S3-compatible server over TLS: the
//! dev container's RustFS, or the one `<PREFIX>_STORAGE__ENDPOINT` names.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod client;
mod transfer;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nest_rs_config::Config;
use nest_rs_storage::{Storage, StorageConfig, StorageTls};
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

/// The store's endpoint: the documented `<PREFIX>_STORAGE__ENDPOINT` override, or
/// the dev container's RustFS when it is unset.
fn endpoint() -> String {
    config().endpoint
}

fn storage() -> Storage {
    Storage::new(Arc::new(config()))
}

/// A client reaching the store through the proxy at `proxy`, with `config`'s
/// other settings, trusting the test authority the proxy's certificate is of.
fn proxied(proxy: SocketAddr, config: StorageConfig) -> Storage {
    Storage::new(Arc::new(StorageConfig {
        endpoint: nest_rs_testing::url_at(&endpoint(), proxy),
        tls: StorageTls {
            ca_cert: Some(AUTHORITY.pem().as_bytes().to_vec()),
        },
        ..config
    }))
}

/// Best-effort bucket creation: a presigned PUT on the bucket root is an S3
/// `CreateBucket`, and a 409 means it already exists.
#[expect(
    clippy::print_stderr,
    reason = "the bucket's state is shown for a reader of a failing run; the round-trip is the assertion"
)]
async fn ensure_bucket(s: &Storage, http: &reqwest::Client) {
    let url = s
        .presign_put("", Duration::from_secs(60))
        .await
        .expect("presign bucket-root PUT");
    match http.put(&url).send().await {
        Ok(resp) => eprintln!("ensure_bucket: {} ({})", resp.status(), s.bucket_name()),
        Err(e) => eprintln!("ensure_bucket: request error (ignored): {e}"),
    }
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

/// A proxy to the store carrying its `n`th connection, from 0, as `carry(n)`
/// says — ending TLS on both sides, so what it carries is HTTP. Returns its
/// address and the count of connections it took.
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
    let acceptor = PROXY.acceptor(None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the proxy");
    let addr = listener.local_addr().expect("the proxy's address");
    let taken = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&taken);
    tokio::spawn(async move {
        loop {
            let (client, _) = listener.accept().await.expect("accept");
            let carry = carry(counted.fetch_add(1, Ordering::SeqCst));
            let acceptor = acceptor.clone();
            let (host, upstream) = (host.clone(), upstream.clone());
            tokio::spawn(async move {
                let Ok(client) = acceptor.accept(client).await else {
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
                // A split stream's half closes nothing when dropped: the
                // connection ends here, as a broken one does.
                let _ = client_write.shutdown().await;
                drop((server_read, client_write));
            });
        }
    });
    (addr, taken)
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

//! `nest-rs-storage`'s suite: a live presign round-trip against an
//! S3-compatible server (RustFS in the dev container). Proves that `object_store`'s `Signer` produces URLs a plain HTTP
//! client can PUT to and GET from, in path-style over plain HTTP.
//!
//! Config starts from `StorageConfig::default()`, which targets the dev
//! container's RustFS (`http://rustfs:9000`, `nestrs`/`nestrs`, bucket
//! `nestrs`, path-style). The endpoint honors the documented
//! `NESTRS_STORAGE__ENDPOINT` override so the round-trip can point at a server
//! outside the dev container; unset, it falls back to the default.
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

use nest_rs_storage::{Storage, StorageConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The store's endpoint: the documented `<PREFIX>_STORAGE__ENDPOINT` override, or
/// the dev container's RustFS when it is unset.
fn endpoint() -> String {
    nest_rs_config::ConfigService::for_namespace("storage")
        .get("ENDPOINT")
        .expect("a readable storage endpoint")
        .unwrap_or_else(|| StorageConfig::default().endpoint)
}

fn storage() -> Storage {
    Storage::new(Arc::new(StorageConfig {
        endpoint: endpoint(),
        ..StorageConfig::default()
    }))
}

/// A client reaching the store through the proxy at `proxy`, with `config`'s
/// other settings.
fn proxied(proxy: SocketAddr, config: StorageConfig) -> Storage {
    Storage::new(Arc::new(StorageConfig {
        endpoint: nest_rs_testing::url_at(&endpoint(), proxy),
        ..config
    }))
}

/// Best-effort bucket creation: a presigned PUT on the bucket root is an S3
/// `CreateBucket`. A 2xx means created, a 409 means it already exists — both are
/// fine. Anything else we surface for visibility but don't fail on (the object
/// round-trip below is the real assertion).
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

/// A TCP proxy to the store carrying its `n`th connection, from 0, as
/// `carry(n)` says. Returns its address and the count of connections it took.
async fn proxy(carry: impl Fn(usize) -> Carry + Send + 'static) -> (SocketAddr, Arc<AtomicUsize>) {
    let parsed = reqwest::Url::parse(&endpoint()).expect("the storage endpoint parses");
    let upstream = format!(
        "{}:{}",
        parsed
            .host_str()
            .expect("the storage endpoint names a host"),
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
        loop {
            let (client, _) = listener.accept().await.expect("accept");
            let carry = carry(counted.fetch_add(1, Ordering::SeqCst));
            let server = tokio::net::TcpStream::connect(&upstream)
                .await
                .expect("reach the store");
            let (mut client_read, mut client_write) = client.into_split();
            let (mut server_read, mut server_write) = server.into_split();
            tokio::spawn(async move {
                let _ = paced(
                    &mut client_read,
                    &mut server_write,
                    carry.request_pace,
                    None,
                )
                .await;
            });
            tokio::spawn(async move {
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
    from: &mut tokio::net::tcp::OwnedReadHalf,
    to: &mut tokio::net::tcp::OwnedWriteHalf,
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
        if upto > sent && to.write_all(&carried[sent..upto]).await.is_err() {
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

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

/// The store's endpoint: the documented `NESTRS_STORAGE__ENDPOINT` override, or
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

/// What a [`proxy`] does with one connection's bytes.
#[derive(Clone, Copy)]
enum Carry {
    /// Both ways, whole.
    Whole,
    /// The store's first `n` bytes, then nothing more, the socket open — a
    /// transfer stalled mid-body.
    AnswerUpTo(usize),
    /// The store's answer up to the end of its headers, then nothing more — a
    /// body stalled before its first byte.
    AnswerHeadersOnly,
    /// The caller's bytes, `n` every 10 ms — an upload moving slowly.
    RequestAt(usize),
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
                let Carry::RequestAt(pace) = carry else {
                    let _ = tokio::io::copy(&mut client_read, &mut server_write).await;
                    return;
                };
                let mut buffer = vec![0; pace];
                while let Ok(read @ 1..) = client_read.read(&mut buffer).await {
                    if server_write.write_all(&buffer[..read]).await.is_err() {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            });
            tokio::spawn(async move {
                if matches!(carry, Carry::Whole | Carry::RequestAt(_)) {
                    let _ = tokio::io::copy(&mut server_read, &mut client_write).await;
                    return;
                }
                let mut seen = Vec::new();
                let mut buffer = vec![0; 8 * 1024];
                loop {
                    let Ok(read @ 1..) = server_read.read(&mut buffer).await else {
                        return;
                    };
                    let sent = seen.len();
                    seen.extend_from_slice(&buffer[..read]);
                    let end = match carry {
                        Carry::AnswerUpTo(n) => Some(n),
                        _ => seen
                            .windows(4)
                            .position(|window| window == b"\r\n\r\n")
                            .map(|at| at + 4),
                    };
                    let upto = end.map_or(seen.len(), |end| end.min(seen.len()));
                    if upto > sent && client_write.write_all(&seen[sent..upto]).await.is_err() {
                        return;
                    }
                    if end.is_some_and(|end| seen.len() >= end) {
                        break;
                    }
                }
                std::future::pending::<()>().await;
                drop((server_read, client_write));
            });
        }
    });
    (addr, taken)
}

//! The producer's scripts against a Redis that forgets them: a `SCRIPT FLUSH`,
//! or a failover to a primary that never loaded them, landing between the
//! client's reload and its retry.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use nest_rs_queue::{JobProducer, QueueName};
use nest_rs_redis::{RedisConfig, RedisConnection, RedisQueueProducer};
use redis::{ErrorKind, ServerErrorKind};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::harness::connection::command_length;

/// A script flushed again right after its reload still runs: the call loads it
/// once more rather than failing the push or the cancel.
#[tokio::test]
async fn a_script_flushed_again_after_its_reload_still_runs() {
    let server = ForgetfulRedis::start(1).await;
    let producer = RedisQueueProducer::new(server.connect().await);
    let queue = QueueName::new("forgotten").expect("a valid name");

    let cancelled = producer
        .remove_unique(&queue, "key")
        .await
        .expect("the script runs once loaded again");

    assert!(cancelled, "the script's answer reaches the caller");
    assert_eq!(
        server.loads(),
        2,
        "the load Redis lost, then the one it kept"
    );
}

/// A Redis that never keeps a script fails the call after a bounded number of
/// loads, never holding its caller in a loop.
#[tokio::test]
async fn a_script_redis_never_keeps_fails_after_a_bounded_number_of_loads() {
    let server = ForgetfulRedis::start(usize::MAX).await;
    let producer = RedisQueueProducer::new(server.connect().await);
    let queue = QueueName::new("forgotten").expect("a valid name");

    let error = producer
        .remove_unique(&queue, "key")
        .await
        .expect_err("a script never kept never runs");

    let refused = std::error::Error::source(&error)
        .and_then(|source| source.downcast_ref::<redis::RedisError>())
        .map(redis::RedisError::kind);
    assert_eq!(
        refused,
        Some(ErrorKind::Server(ServerErrorKind::NoScript)),
        "{error}"
    );
    assert_eq!(server.loads(), 3, "the loads are bounded");
}

/// The binding's producer has its scripts loaded at boot, so the first push
/// is one round trip, not a refused call, a load and the call again.
#[tokio::test]
async fn the_bindings_producer_loads_its_scripts_at_boot() {
    let server = ForgetfulRedis::start(0).await;
    let redis = RedisConfig {
        url: format!("rediss://{}/", server.addr),
        connect_timeout: Duration::from_secs(2),
        tls: crate::harness::tls::trusting_the_test_authority(),
        ..RedisConfig::default()
    };
    nest_rs_core::App::builder()
        .module::<ProducerModule>()
        .provide(redis)
        .build()
        .await
        .expect("the binding boots");

    assert_eq!(server.loads(), 2, "the push's script and the cancel's");
}

#[nest_rs_core::module(imports = [nest_rs_redis::RedisModule::for_root(None), nest_rs_redis::RedisQueueModule])]
struct ProducerModule;

/// A server whose first `lost` script loads are flushed as soon as they land:
/// it answers a script call `NOSCRIPT` until a load is kept and `1` after,
/// answers a load with the script's digest, and `PONG`s the rest.
struct ForgetfulRedis {
    addr: SocketAddr,
    loads: Arc<AtomicUsize>,
}

impl ForgetfulRedis {
    async fn start(lost: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the server");
        let addr = listener.local_addr().expect("the server's address");
        let loads = Arc::new(AtomicUsize::new(0));
        let loading = Arc::clone(&loads);
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let loading = Arc::clone(&loading);
                tokio::spawn(async move {
                    if let Some(client) = crate::harness::tls::accept(client).await {
                        forget(client, lost, loading).await;
                    }
                });
            }
        });
        Self { addr, loads }
    }

    async fn connect(&self) -> RedisConnection {
        RedisConnection::connect(&RedisConfig {
            url: format!("rediss://{}/", self.addr),
            connect_timeout: Duration::from_secs(2),
            tls: crate::harness::tls::trusting_the_test_authority(),
            ..RedisConfig::default()
        })
        .await
        .expect("the server answers the boot")
    }

    fn loads(&self) -> usize {
        self.loads.load(Ordering::SeqCst)
    }
}

/// Answer each command `client` sends as [`ForgetfulRedis`] does.
async fn forget(
    mut client: impl AsyncRead + AsyncWrite + Unpin,
    lost: usize,
    loads: Arc<AtomicUsize>,
) {
    let mut received = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        while let Some(length) = command_length(&received) {
            let command = received.drain(..length).collect::<Vec<_>>();
            let reply = match argument(&command, 0).to_ascii_uppercase().as_slice() {
                b"EVALSHA" if loads.load(Ordering::SeqCst) > lost => b":1\r\n".to_vec(),
                b"EVALSHA" => b"-NOSCRIPT No matching script. Please use EVAL.\r\n".to_vec(),
                b"SCRIPT" => {
                    loads.fetch_add(1, Ordering::SeqCst);
                    let code = String::from_utf8_lossy(bulk(&command, 2));
                    format!("$40\r\n{}\r\n", redis::Script::new(&code).get_hash()).into_bytes()
                }
                _ => b"+PONG\r\n".to_vec(),
            };
            if client.write_all(&reply).await.is_err() || client.flush().await.is_err() {
                return;
            }
        }
        match client.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => received.extend_from_slice(&chunk[..read]),
        }
    }
}

/// Argument `at` of the whole command `bytes` holds, as sent — a script spans
/// lines.
fn bulk(bytes: &[u8], at: usize) -> &[u8] {
    let mut rest = bytes;
    let Some(count) = header(&mut rest) else {
        return &[];
    };
    for index in 0..count {
        let Some(length) = header(&mut rest) else {
            return &[];
        };
        let (body, tail) = rest.split_at(length.min(rest.len()));
        if index == at {
            return body;
        }
        rest = tail.get(2..).unwrap_or_default();
    }
    &[]
}

/// The number a `*` or `$` header at the front of `rest` holds, past it.
fn header(rest: &mut &[u8]) -> Option<usize> {
    let end = rest.windows(2).position(|pair| pair == b"\r\n")?;
    let number = std::str::from_utf8(rest.get(1..end)?).ok()?.parse().ok();
    *rest = &rest[end + 2..];
    number
}

/// Argument `at` of the whole command `bytes` holds, read as a line — every
/// argument this server reads by line is one.
fn argument(bytes: &[u8], at: usize) -> &[u8] {
    bytes
        .split(|&byte| byte == b'\n')
        .nth(2 + 2 * at)
        .and_then(|line| line.strip_suffix(b"\r"))
        .unwrap_or_default()
}

//! Live-Redis e2e for `nest-rs-redis`. One module per concern in `src/`:
//! [`connection`] for the shared connection's boot, bound and recovery,
//! [`tls`] for `rediss://`, [`throttler`] for the cross-process rate-limit
//! store, [`concurrency`] and [`replicas`] for the worker's fetch guarantees,
//! [`correlation`] for the trace context that crosses the producer/consumer
//! process boundary, and [`portable_producer`] for the two names
//! `RedisQueueModule` binds.
//!
//! Needs a reachable Redis — gated out of `unit` by the nextest `binary(e2e)`
//! filter, and behind the `throttler` feature (off by default, so producer /
//! consumer apps that never rate-limit pull neither `redis` nor
//! `nest-rs-throttler`). Run it explicitly:
//!
//! ```bash
//! cargo nextest run -p nest-rs-redis --features throttler -E 'binary(e2e)'
//! ```
//!
//! The URL comes from `NESTRS_REDIS__URL` (the dev container wires
//! `redis://redis:6379`); unset, it falls back to that default. This file holds
//! the suite's shared fixtures and nothing else — every test lives in the
//! module named for the concern it covers.

mod concurrency;
mod connection;
mod correlation;
mod portable_producer;
mod replicas;
mod throttler;
mod tls;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nest_rs_redis::{RedisConfig, RedisConnection, RedisQueueProducer, RedisWorker};
use nest_rs_testing::{TestApp, TransportHandle};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

fn redis_url() -> String {
    std::env::var(nest_rs_config::var_name("redis", "URL"))
        .unwrap_or_else(|_| "redis://redis:6379".to_string())
}

/// The dev container Redis's `host:port`, for a proxy standing in front of it.
fn redis_address() -> String {
    redis::IntoConnectionInfo::into_connection_info(redis_url().as_str())
        .expect("the dev container Redis URL parses")
        .addr
        .to_string()
}

/// Pinned rather than read from the env: the framework workspace ships no
/// `.env`, so `for_root(None)` would resolve to the localhost default and a
/// suite would fail on connect instead of measuring anything.
fn redis_config() -> RedisConfig {
    RedisConfig {
        url: redis_url(),
        ..Default::default()
    }
}

/// The logical databases a test selects when its keys or its connections must
/// not meet another test's — one per test, all declared here so that no two
/// collide. Every other test shares database 0.
const DB_CONNECTION_DROP: u8 = 11;
const DB_TLS_FLUSH: u8 = 12;
const DB_TLS_REFUSED_REOPEN: u8 = 13;

/// The dev container Redis's URL on database `db`, for a test whose keys must
/// not meet another test's — a connection drop aimed at its clients, or a
/// `FLUSHDB`.
fn redis_url_on(db: u8) -> String {
    let url = redis_url();
    let url = url.trim_end_matches('/');
    let base = match url.rsplit_once('/') {
        Some((head, index)) if !head.ends_with('/') && index.parse::<u8>().is_ok() => head,
        _ => url,
    };
    format!("{base}/{db}")
}

/// A key unique to this process, call site and wall-clock instant, so a rerun
/// (or a recycled PID) never inherits a prior run's still-live window.
fn unique_key(tag: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("redis-q2:{tag}:{}:{nanos}", std::process::id())
}

async fn connect() -> RedisConnection {
    RedisConnection::connect(&redis_config())
        .await
        .expect("connect to the dev container Redis")
}

/// Close, from the server's side, every client connection selected on `db` —
/// what a Redis restart, a failover or an idle timeout does to a running app.
/// Returns how many it closed.
async fn drop_every_connection_on(db: u8) -> usize {
    let mut admin = connect().await;
    let clients: String = redis::cmd("CLIENT")
        .arg("LIST")
        .query_async(&mut admin)
        .await
        .expect("CLIENT LIST");
    let selected = format!(" db={db} ");
    let ids: Vec<String> = clients
        .lines()
        .filter(|client| client.contains(&selected))
        .filter_map(|client| {
            client
                .split(' ')
                .find_map(|field| field.strip_prefix("id="))
                .map(str::to_owned)
        })
        .collect();
    for id in &ids {
        redis::cmd("CLIENT")
            .arg("KILL")
            .arg("ID")
            .arg(id)
            .query_async::<i64>(&mut admin)
            .await
            .expect("CLIENT KILL");
    }
    ids.len()
}

/// A TCP proxy in front of the dev container Redis that can go dark: it then
/// drops every connection it carries and accepts new ones without ever
/// answering — Redis gone behind a network that still accepts the dial.
struct DarkeningProxy {
    addr: SocketAddr,
    dark: Arc<watch::Sender<bool>>,
    dials_while_dark: Arc<AtomicUsize>,
}

impl DarkeningProxy {
    async fn start() -> Self {
        let upstream = redis_address();
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the proxy");
        let addr = listener.local_addr().expect("the proxy's address");
        let dark = Arc::new(watch::channel(false).0);
        let dials_while_dark = Arc::new(AtomicUsize::new(0));
        let accepting = Arc::clone(&dark);
        let counting = Arc::clone(&dials_while_dark);
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((mut client, _)) = listener.accept().await {
                if *accepting.borrow() {
                    counting.fetch_add(1, Ordering::SeqCst);
                    held.push(client);
                    continue;
                }
                let Ok(mut server) = TcpStream::connect(&upstream).await else {
                    continue;
                };
                let mut went_dark = accepting.subscribe();
                tokio::spawn(async move {
                    tokio::select! {
                        _ = tokio::io::copy_bidirectional(&mut client, &mut server) => {}
                        _ = went_dark.wait_for(|dark| *dark) => {}
                    }
                });
            }
        });
        Self {
            addr,
            dark,
            dials_while_dark,
        }
    }

    fn url(&self) -> String {
        format!("redis://{}/", self.addr)
    }

    fn go_dark(&self) {
        self.dark.send_replace(true);
    }

    fn dials_while_dark(&self) -> usize {
        self.dials_while_dark.load(Ordering::SeqCst)
    }
}

/// One worker replica: its running transport, and the producer its app bound.
struct Replica {
    worker: TransportHandle,
    producer: RedisQueueProducer,
}

/// Boot the worker app `M` and start its worker — one replica. The app is
/// leaked: the transport borrows the container it owns, the way a process would
/// hold it.
async fn replica<M: nest_rs_core::Module + 'static>() -> Replica {
    let app = TestApp::builder()
        .module::<M>()
        .build_headless()
        .await
        .expect("the worker app boots against the dev container Redis");
    app.init().await.expect("init phases");
    let producer = RedisQueueProducer::clone(
        &app.container()
            .get::<RedisQueueProducer>()
            .expect("RedisQueueModule binds the producer"),
    );
    let worker = app
        .spawn_transport(RedisWorker::default())
        .await
        .expect("the queue worker transport starts");
    Box::leak(Box::new(app));
    Replica { worker, producer }
}

/// Poll `ready` until it holds or `within` elapses — the wait a live worker's
/// asynchronous progress needs, bounded so a regression fails rather than hangs.
async fn wait_until(within: Duration, ready: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

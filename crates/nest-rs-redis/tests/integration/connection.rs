//! The shared connection, without a Redis: a scripted server that stays busy,
//! refuses a `SELECT` or is demoted under its clients, and the budget's place
//! below the ports' nets.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use nest_rs_redis::{RedisConfig, RedisConnection, RedisError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::harness::connection::{
    NOT_READY, ScriptedRedis, answer, command_length, database_refused_at_once,
};

/// A Redis that stays not ready spends the budget and fails as one that
/// answered — its last answer as the source, the budget to widen — never as an
/// unreachable one sent to check its URL.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redis_that_stays_busy_fails_at_the_budget_naming_its_answer() {
    let proxy = ScriptedRedis::start(None, None).await;
    proxy.answer_with(Some(NOT_READY[0]));
    let budget = Duration::from_millis(1500);
    let started = Instant::now();
    let Err(error) = RedisConnection::connect(&RedisConfig {
        url: proxy.url(),
        connect_timeout: budget,
        ..RedisConfig::default()
    })
    .await
    else {
        panic!("a Redis that never serves must not connect")
    };
    let took = started.elapsed();

    assert!(
        took >= budget && took < budget * 3,
        "at the budget, took {took:?}"
    );
    assert!(matches!(error, RedisError::Unready { .. }), "{error}");
    let rendered = error.to_string();
    assert!(
        rendered.contains(&nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS"))
            && !rendered.contains(&nest_rs_config::var_name("redis", "URL")),
        "the budget is what to widen, not the URL to check: {rendered}"
    );
    assert!(
        answer(&error).contains("busy running a script"),
        "the source is Redis's last answer: {}",
        answer(&error)
    );
}

/// config-1r2: a server in cluster mode answers a `SELECT` with an `ERR` the
/// client reports without its code, so it was retried for the whole budget. It
/// fails at once, naming the index.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_that_serves_database_zero_alone_fails_the_boot_at_once_naming_the_index() {
    let proxy = ScriptedRedis::start(None, None).await;
    proxy.answer_with(Some("ERR SELECT is not allowed in cluster mode"));
    let error =
        database_refused_at_once(format!("{}2", proxy.url()), 2, "a server in cluster mode").await;
    assert!(
        answer(&error).contains("not allowed in cluster mode"),
        "the source says what Redis answered: {}",
        answer(&error),
    );
}

/// The default budget sits below the net of every port a binding serves — the
/// queue's, the throttler guard's and the scheduler's — with the room each
/// net's documentation argues — twice the budget, for a deployment that raised
/// it — read from the constants that set them rather than retyped.
#[test]
fn the_connection_budget_answers_before_every_ports_net() {
    let budget = RedisConfig::default().connect_timeout;
    for (net, what) in [
        (nest_rs_queue::BACKEND_TIMEOUT, "the queue port's net"),
        (nest_rs_throttler::HIT_TIMEOUT, "the throttler guard's net"),
        (nest_rs_schedule::LOCK_TIMEOUT, "the scheduler's net"),
    ] {
        assert!(
            budget < net,
            "the connection budget ({budget:?}) must answer before {what} ({net:?})"
        );
        assert!(
            budget * 2 <= net,
            "{what} ({net:?}) leaves room for a budget raised to twice its default ({budget:?})"
        );
    }
}

/// A primary demoted under a live connection — a failover behind a name that
/// now reaches the new primary — answers every write `READONLY`. The connection
/// is reopened at the first, so the writes after it reach the new primary
/// rather than failing until the process restarts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_to_a_demoted_primary_is_reopened() {
    let logs = nest_rs_testing::LogCapture::install_global();
    let server = DemotedRedis::start().await;
    let mut conn = RedisConnection::connect(&RedisConfig {
        url: server.url(),
        connect_timeout: Duration::from_secs(2),
        ..RedisConfig::default()
    })
    .await
    .expect("the primary serves the boot");
    server.demote();
    let write = || redis::cmd("SET").arg("nestrs-demoted").arg("1").clone();
    let refused = write()
        .query_async::<()>(&mut conn)
        .await
        .expect_err("the demoted primary refuses the write");
    assert_eq!(refused.code(), Some("READONLY"), "{refused}");

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match write().query_async::<()>(&mut conn).await {
            Ok(()) => break,
            Err(error) => {
                assert!(
                    Instant::now() < deadline,
                    "the connection still reaches the demoted primary: {error}"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    let said = logs.expect_one(
        nest_rs_redis::TARGET,
        "redis connection opened again: the server it reached answered as a read-only replica",
    );
    assert_eq!(said.level, "warn");
}

/// A primary demoted under its clients: until [`demote`](Self::demote), every
/// connection is answered `+PONG`; after it, the connections it had accepted
/// answer every command `READONLY`, as a demoted primary does, and the ones it
/// accepts next answer `+PONG` again — the name now reaching the new primary.
pub(crate) struct DemotedRedis {
    addr: SocketAddr,
    accepted: Arc<AtomicUsize>,
    demoted_below: Arc<AtomicUsize>,
}

impl DemotedRedis {
    pub(crate) async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the server");
        let addr = listener.local_addr().expect("the server's address");
        let accepted = Arc::new(AtomicUsize::new(0));
        let demoted_below = Arc::new(AtomicUsize::new(0));
        let counting = Arc::clone(&accepted);
        let demoting = Arc::clone(&demoted_below);
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let id = counting.fetch_add(1, Ordering::SeqCst);
                let demoting = Arc::clone(&demoting);
                tokio::spawn(async move {
                    answer_as_its_role(client, id, &demoting).await;
                });
            }
        });
        Self {
            addr,
            accepted,
            demoted_below,
        }
    }

    pub(crate) fn url(&self) -> String {
        format!("redis://{}/", self.addr)
    }

    /// Demote every connection accepted so far.
    pub(crate) fn demote(&self) {
        self.demoted_below
            .store(self.accepted.load(Ordering::SeqCst), Ordering::SeqCst);
    }
}

/// Answer each command connection `id` sends: `READONLY` once it is below the
/// demotion, `+PONG` otherwise.
async fn answer_as_its_role(mut client: TcpStream, id: usize, demoted_below: &AtomicUsize) {
    let mut received = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        while let Some(length) = command_length(&received) {
            received.drain(..length);
            let reply: &[u8] = if id < demoted_below.load(Ordering::SeqCst) {
                b"-READONLY You can't write against a read only replica.\r\n"
            } else {
                b"+PONG\r\n"
            };
            if client.write_all(reply).await.is_err() {
                return;
            }
        }
        match client.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => received.extend_from_slice(&chunk[..read]),
        }
    }
}

//! `nest-rs-redis`'s suite against the dev container's Redis; what needs none
//! is in `integration`, and the doubles both use in `harness`. One module per
//! concern in `src/`: [`connection`] for the shared connection's boot, bound and
//! recovery, [`tls`] for `rediss://`, [`throttler`] for the cross-process
//! rate-limit store, [`layout`] for where a queue lives — run by users confined
//! to the framework's keys as the docs prescribe them — [`queue`] for the queue
//! binding, its producer and its consumer under the port's worker, the
//! behaviour kit every queue backend runs included, [`correlation`] for the
//! trace context that crosses the producer/consumer boundary, and [`schedule`]
//! for the occurrence lock a job firing once across replicas claims through.
//!
//! Valkey is reached over TLS alone, at `<PREFIX>_REDIS__URL` or the dev
//! container's `rediss://valkey-standalone:6379`, its certificate verified as the framework
//! verifies it by default — against the system's authorities, where the dev
//! container and CI install the services'. A double the suite puts in front of
//! it presents a certificate of the test authority's, which only a connection
//! to the double trusts (`harness::tls`). This file holds the suite's shared
//! fixtures and nothing else.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod cluster;
mod connection;
mod correlation;
#[path = "../harness/mod.rs"]
mod harness;
mod layout;
mod queue;
mod schedule;
mod sentinel;
mod throttler;
mod tls;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nest_rs_core::Transport;
use nest_rs_queue::{QueueConfig, QueueWorker};
use nest_rs_redis::{
    RedisConfig, RedisConnection, RedisModule, RedisQueueConfig, RedisQueueModule,
    RedisQueueProducer, RedisTopology,
};
use nest_rs_testing::{
    CapturedEvent, TestApp, TransportHandle, url_as, url_at, url_on, wait_for, wait_until,
};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// The Valkey the deployment names, as the app would read it, or the dev
/// container's.
fn redis_url() -> String {
    nest_rs_config::ConfigService::for_namespace("redis")
        .get("URL")
        .expect("a readable Redis URL")
        .unwrap_or_else(|| "rediss://valkey-standalone:6379".to_owned())
}

/// Pinned rather than read from the env: the framework workspace ships no
/// `.env`, so `for_root(None)` would resolve to the localhost default and a
/// suite would fail on connect instead of measuring anything.
fn redis_config() -> RedisConfig {
    RedisConfig {
        url: redis_url(),
        connect_timeout: BUDGET,
        ..Default::default()
    }
}

/// [`redis_config`] at a double's `url`, trusting the test authority its
/// certificate is issued by.
fn through(url: String) -> RedisConfig {
    RedisConfig {
        url,
        tls: harness::tls::trusting_the_test_authority(),
        ..redis_config()
    }
}

/// A bare `redis` client of `url`, trusting the system's authorities as the
/// framework's connection does — for a test that speaks to Valkey without it.
fn bare_client(url: &str) -> redis::Client {
    let certificates = redis::TlsCertificates {
        client_tls: None,
        root_cert: Some(nest_rs_config::system_authorities().to_vec()),
    };
    redis::Client::build_with_tls(url, certificates).expect("a TLS client of the suite's Valkey")
}

/// The boot's refusal of a Redis budget at a net reaching it, its setting
/// naming the variable a deployment lowers.
fn budget_past_net(refused: anyhow::Error) -> nest_rs_core::BudgetPastNetError {
    let refused = refused
        .downcast::<nest_rs_core::BudgetPastNetError>()
        .unwrap_or_else(|other| panic!("not a budget refusal: {other:#}"));
    assert!(
        refused
            .setting
            .contains(&nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS")),
        "{refused}"
    );
    refused
}

/// The budget the suite's connections hold a command to: under two thirds of
/// [`LEASE`], or the queue binding refuses the boot.
const BUDGET: Duration = Duration::from_millis(500);

/// The logical databases a test selects when its keys or its connections must
/// not meet another test's — one per test, all declared here so that no two
/// collide. Every other test shares database 0, on queue names of its own.
/// The framework's are 9 to 15: the demo's suites run on the same Redis and
/// hold 1 to 8, so a `FLUSHDB` here never reaches a job one of theirs filed.
const DB_CONFINED_TO_THE_PREFIX: u8 = 9;
const DB_BLOCKED_READ: u8 = 10;
const DB_CONNECTION_DROP: u8 = 11;
/// Through a TLS proxy in front of the suite's Redis.
const DB_TLS_FLUSH: u8 = 12;
/// Through a TLS proxy in front of the suite's Redis.
const DB_TLS_REFUSED_REOPEN: u8 = 13;
/// Claims only: every scheduler the `schedule` tests boot claims here, so the
/// key layout is asserted over the whole database.
const DB_SCHEDULE: u8 = 14;
const DB_SELECT_RETRIED: u8 = 15;

/// On the TLS Redis — another server, which only the framework's suite uses —
/// so its blocked read is the one `CLIENT LIST` finds there.
const DB_TLS_BLOCKED_READ: u8 = 15;

// Two tests sharing a database meet each other's keys and connections, Redis
// ships sixteen, and the demo holds the lower half: all three facts are checked
// where the lists are written, one per server.
const _: () = {
    const fn hold_apart(dbs: &[u8]) {
        let mut i = 0;
        while i < dbs.len() {
            assert!(
                dbs[i] >= 9 && dbs[i] < 16,
                "a framework e2e database is 9 to 15; 1 to 8 are the demo's",
            );
            let mut j = i + 1;
            while j < dbs.len() {
                assert!(dbs[i] != dbs[j], "two e2e tests share a logical database");
                j += 1;
            }
            i += 1;
        }
    }
    hold_apart(&[
        DB_CONFINED_TO_THE_PREFIX,
        DB_BLOCKED_READ,
        DB_CONNECTION_DROP,
        DB_TLS_FLUSH,
        DB_TLS_REFUSED_REOPEN,
        DB_SCHEDULE,
        DB_SELECT_RETRIED,
    ]);
    hold_apart(&[DB_TLS_BLOCKED_READ]);
};

/// The dev container Redis's URL on database `db`, for a test whose keys must
/// not meet another test's — a connection drop aimed at its clients, or a
/// `FLUSHDB`.
fn redis_url_on(db: u8) -> String {
    url_on(&redis_url(), db)
}

/// [`redis_config`] on database `db`.
fn redis_config_on(db: u8) -> RedisConfig {
    RedisConfig {
        url: redis_url_on(db),
        ..redis_config()
    }
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
    RedisConnection::connect(&at_default_budget(redis_config()))
        .await
        .expect("connect to the dev container Redis")
}

/// [`connect`], on database `db`.
async fn connect_on(db: u8) -> RedisConnection {
    RedisConnection::connect(&at_default_budget(redis_config_on(db)))
        .await
        .expect("connect to the dev container Redis")
}

/// `config` at the framework's default budget, for the test's own commands:
/// [`BUDGET`] is the setting the apps under test run with, and a loaded machine
/// can spend half a second on a dial alone.
fn at_default_budget(config: RedisConfig) -> RedisConfig {
    RedisConfig {
        connect_timeout: RedisConfig::default().connect_timeout,
        ..config
    }
}

/// The topology the suite runs on, as the scheme of its URL declares it.
fn topology() -> RedisTopology {
    let url = redis_url();
    match url
        .split_once("://")
        .map(|(scheme, _)| scheme.to_ascii_lowercase())
        .as_deref()
    {
        Some("redis-sentinel" | "rediss-sentinel") => RedisTopology::Sentinel,
        Some("redis-cluster" | "rediss-cluster") => RedisTopology::Cluster,
        _ => RedisTopology::Standalone,
    }
}

/// The suite's URL read apart: it names one server, or several hosts.
fn parsed_url() -> url::Url {
    url::Url::parse(&redis_url()).expect("the suite's Valkey URL parses")
}

/// Every host the suite's URL names — the server, the sentinels, or the
/// Cluster's seeds — as `host:port`.
fn named_hosts() -> Vec<String> {
    let url = parsed_url();
    let first = format!(
        "{}:{}",
        url.host_str().expect("the URL names a host"),
        url.port().unwrap_or(match topology() {
            RedisTopology::Sentinel => 26379,
            _ => 6379,
        })
    );
    std::iter::once(first)
        .chain(
            url.query_pairs()
                .filter(|(key, _)| key == "node")
                .map(|(_, node)| node.into_owned()),
        )
        .collect()
}

/// The URL of the one server at `addr`, with the suite's encryption and
/// credentials.
fn node_url(addr: &str) -> String {
    let url = parsed_url();
    let scheme = if url.scheme().starts_with("rediss") {
        "rediss"
    } else {
        "redis"
    };
    let userinfo = match (url.username(), url.password()) {
        ("", None) => String::new(),
        (user, None) => format!("{user}@"),
        (user, Some(password)) => format!("{user}:{password}@"),
    };
    format!("{scheme}://{userinfo}{addr}")
}

/// A connection to the one server at `addr`.
async fn node(addr: &str) -> redis::aio::MultiplexedConnection {
    bare_client(&node_url(addr))
        .get_multiplexed_async_connection()
        .await
        .unwrap_or_else(|error| panic!("{addr} answers: {error}"))
}

/// The name the suite's sentinels monitor the primary under.
fn service_name() -> String {
    parsed_url()
        .query_pairs()
        .find(|(key, _)| key == "sentinelServiceName")
        .map(|(_, name)| name.into_owned())
        .expect("a Sentinel URL names its primary")
}

/// The primary the suite's sentinels name now, as `host:port`.
async fn sentinel_primary() -> String {
    let (host, port): (String, u16) = redis::cmd("SENTINEL")
        .arg("GET-MASTER-ADDR-BY-NAME")
        .arg(service_name())
        .query_async(&mut node(&named_hosts()[0]).await)
        .await
        .expect("the sentinels name a primary");
    format!("{host}:{port}")
}

/// One node of the suite's Cluster, as `CLUSTER NODES` lists it.
struct ClusterNode {
    id: String,
    addr: String,
    primary: bool,
    /// The slot ranges a primary serves.
    slots: Vec<(u16, u16)>,
}

impl ClusterNode {
    fn serves(&self, slot: u16) -> bool {
        self.primary
            && self
                .slots
                .iter()
                .any(|(from, to)| (*from..=*to).contains(&slot))
    }
}

/// Every node the suite's Cluster lists and does not hold failed, asked of
/// the first seed that answers: any may be a node a test froze.
async fn cluster_nodes() -> Vec<ClusterNode> {
    for seed in named_hosts() {
        let Ok(mut seed) = bare_client(&node_url(&seed))
            .get_multiplexed_async_connection()
            .await
        else {
            continue;
        };
        let Ok(listed) = redis::cmd("CLUSTER")
            .arg("NODES")
            .query_async::<String>(&mut seed)
            .await
        else {
            continue;
        };
        return listed
            .lines()
            .filter(|line| !line.contains("fail") && !line.contains("noaddr"))
            .map(|line| {
                let fields: Vec<&str> = line.split(' ').collect();
                ClusterNode {
                    id: fields[0].to_owned(),
                    addr: fields[1]
                        .split(['@', ','])
                        .next()
                        .unwrap_or(fields[1])
                        .to_owned(),
                    primary: fields[2].contains("master"),
                    slots: fields[8..]
                        .iter()
                        .filter(|range| !range.starts_with('['))
                        .filter_map(|range| {
                            let (from, to) = range.split_once('-').unwrap_or((range, range));
                            Some((from.parse().ok()?, to.parse().ok()?))
                        })
                        .collect(),
                }
            })
            .collect();
    }
    panic!("no seed of the Cluster answers CLUSTER NODES");
}

/// A bare client of each sentinel the suite's URL names.
fn sentinels() -> Vec<redis::Client> {
    clients(named_hosts())
}

/// The address of every data node the suite's URL reaches — the server; the
/// primary the sentinels name, and its replicas; every node of the Cluster —
/// replicas included or not.
async fn node_addresses(with_replicas: bool) -> Vec<String> {
    match topology() {
        RedisTopology::Standalone => vec![named_hosts().remove(0)],
        RedisTopology::Sentinel => {
            let mut addresses = vec![sentinel_primary().await];
            if with_replicas {
                let replicas: Vec<std::collections::HashMap<String, String>> =
                    redis::cmd("SENTINEL")
                        .arg("REPLICAS")
                        .arg(service_name())
                        .query_async(&mut node(&named_hosts()[0]).await)
                        .await
                        .expect("SENTINEL REPLICAS");
                addresses.extend(
                    replicas
                        .iter()
                        .filter(|replica| {
                            replica
                                .get("flags")
                                .is_some_and(|flags| !flags.contains("s_down"))
                        })
                        .map(|replica| format!("{}:{}", replica["ip"], replica["port"])),
                );
            }
            addresses
        }
        RedisTopology::Cluster => cluster_nodes()
            .await
            .into_iter()
            .filter(|node| with_replicas || node.primary)
            .map(|node| node.addr)
            .collect(),
    }
}

/// A bare client of every data node the suite's URL reaches, for what the
/// suite asks of each: a user, its ACL log, its clients.
async fn data_nodes() -> Vec<redis::Client> {
    clients(node_addresses(true).await)
}

/// A bare client of every primary the suite's URL reaches: the nodes an app
/// sends its commands to, and so the ones a test watches.
async fn primaries() -> Vec<redis::Client> {
    clients(node_addresses(false).await)
}

fn clients(addresses: Vec<String>) -> Vec<redis::Client> {
    addresses
        .iter()
        .map(|addr| bare_client(&node_url(addr)))
        .collect()
}

/// The URL of the first primary the suite's URL reaches, on database `db`: a
/// connection of a test's own to the server its app sends to.
async fn a_primary_url_on(db: u8) -> String {
    url_on(&node_url(&node_addresses(false).await[0]), db)
}

/// `cmd`'s answer from each of `nodes`, asked of all at once.
async fn on_each<T: redis::FromRedisValue>(nodes: &[redis::Client], cmd: &redis::Cmd) -> Vec<T> {
    futures_util::future::join_all(nodes.iter().map(|node| async move {
        let mut connection = node
            .get_multiplexed_async_connection()
            .await
            .expect("every data node answers");
        cmd.query_async(&mut connection)
            .await
            .unwrap_or_else(|error| panic!("a data node runs {cmd:?}: {error}"))
    }))
    .await
}

/// `cmd`'s answer from every data node, as [`data_nodes`] lists them.
async fn on_every_node<T: redis::FromRedisValue>(cmd: &redis::Cmd) -> Vec<T> {
    on_each(&data_nodes().await, cmd).await
}

/// How many clients selected on `db` sit blocked in `XREADGROUP` on `nodes`.
/// `CLIENT LIST` names every client of a Redis the suites share, so only a
/// database one test owns makes a client its worker's, and `cmd` is only the
/// last command a client sent: the `b` flag of a client waiting in a blocking
/// call is what says the read waits there.
async fn blocked_reads_among(nodes: &[redis::Client], db: u8) -> usize {
    let selected = format!("db={db}");
    on_each::<String>(nodes, redis::cmd("CLIENT").arg("LIST"))
        .await
        .iter()
        .flat_map(|clients| clients.lines())
        .filter(|client| {
            let fields: Vec<&str> = client.split(' ').collect();
            fields.contains(&selected.as_str())
                && fields.contains(&"cmd=xreadgroup")
                && fields.iter().any(|field| {
                    field
                        .strip_prefix("flags=")
                        .is_some_and(|flags| flags.contains('b'))
                })
        })
        .count()
}

/// [`blocked_reads_among`] every data node.
async fn blocked_reads_on(db: u8) -> usize {
    blocked_reads_among(&data_nodes().await, db).await
}

/// Whether a read blocks on `db` within a second: a worker sends its read
/// again at the end of each wait, so one listing can fall between two.
async fn a_read_blocks_on(db: u8) -> bool {
    let nodes = data_nodes().await;
    for _ in 0..20 {
        if blocked_reads_among(&nodes, db).await > 0 {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// Close, from the server's side, every client connection selected on `db`,
/// on every data node — what a Valkey restart, a failover or an idle timeout
/// does to a running app. Returns how many it closed.
async fn drop_every_connection_on(db: u8) -> usize {
    let selected = format!(" db={db} ");
    let nodes = data_nodes().await;
    let closed = futures_util::future::join_all(nodes.iter().map(|node| {
        let selected = &selected;
        async move {
            let mut node = node
                .get_multiplexed_async_connection()
                .await
                .expect("every data node answers");
            let clients: String = redis::cmd("CLIENT")
                .arg("LIST")
                .query_async(&mut node)
                .await
                .expect("CLIENT LIST");
            let mut kills = redis::pipe();
            for id in clients
                .lines()
                .filter(|client| client.contains(selected.as_str()))
                .filter_map(|client| {
                    client
                        .split(' ')
                        .find_map(|field| field.strip_prefix("id="))
                })
            {
                kills.cmd("CLIENT").arg("KILL").arg("ID").arg(id);
            }
            if kills.is_empty() {
                return 0;
            }
            kills
                .query_async::<Vec<i64>>(&mut node)
                .await
                .expect("CLIENT KILL")
                .len()
        }
    }))
    .await;
    closed.into_iter().sum()
}

/// Freeze the node at `addr` for six seconds — longer than the suite's
/// topologies take to fail it over — as a crashed primary stops answering,
/// returning once it no longer answers.
async fn freeze(addr: &str) {
    let mut frozen = node(addr).await;
    let probe = node(addr).await;
    tokio::spawn(async move {
        let _: Result<(), _> = redis::cmd("DEBUG")
            .arg("SLEEP")
            .arg(6)
            .query_async(&mut frozen)
            .await;
    });
    wait_for(Duration::from_secs(2), || {
        let mut probe = probe.clone();
        async move {
            let ping = redis::cmd("PING");
            tokio::time::timeout(
                Duration::from_millis(100),
                ping.query_async::<()>(&mut probe),
            )
            .await
            .is_err()
        }
    })
    .await;
}

/// A proxy in front of the dev container's Valkey that can go dark: it then
/// drops every connection it carries and accepts new ones without ever
/// answering — Valkey gone behind a network that still accepts the dial.
struct DarkeningProxy {
    addr: SocketAddr,
    dark: Arc<watch::Sender<bool>>,
    dials_while_dark: Arc<AtomicUsize>,
}

impl DarkeningProxy {
    async fn start() -> Self {
        let upstream = redis_url();
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
            while let Ok((client, _)) = listener.accept().await {
                if *accepting.borrow() {
                    counting.fetch_add(1, Ordering::SeqCst);
                    held.push(client);
                    continue;
                }
                let upstream = upstream.clone();
                let mut went_dark = accepting.subscribe();
                tokio::spawn(async move {
                    let Some((mut client, mut server)) =
                        harness::tls::bridge(client, &upstream).await
                    else {
                        return;
                    };
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

    /// The suite's URL, at the proxy's address.
    fn url(&self) -> String {
        url_at(&redis_url(), self.addr)
    }

    fn go_dark(&self) {
        self.dark.send_replace(true);
    }

    fn dials_while_dark(&self) -> usize {
        self.dials_while_dark.load(Ordering::SeqCst)
    }
}

/// A proxy in front of the dev container's Valkey that, once muted, still
/// carries every command to Valkey and drops every reply — a command that runs
/// and whose answer is lost, the case only a network can make. It ends TLS on
/// both sides, so what it drops is a whole reply, never a record.
struct MutingProxy {
    addr: SocketAddr,
    muted: Arc<AtomicBool>,
}

impl MutingProxy {
    async fn start() -> Self {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let upstream = redis_url();
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the proxy");
        let addr = listener.local_addr().expect("the proxy's address");
        let muted = Arc::new(AtomicBool::new(false));
        let muting = Arc::clone(&muted);
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let upstream = upstream.clone();
                let muting = Arc::clone(&muting);
                tokio::spawn(async move {
                    let Some((client, server)) = harness::tls::bridge(client, &upstream).await
                    else {
                        return;
                    };
                    let (mut from_client, mut to_client) = tokio::io::split(client);
                    let (mut from_server, mut to_server) = tokio::io::split(server);
                    tokio::spawn(async move {
                        let _ = tokio::io::copy(&mut from_client, &mut to_server).await;
                    });
                    let mut chunk = [0_u8; 16 * 1024];
                    while let Ok(read) = from_server.read(&mut chunk).await {
                        if read == 0 {
                            break;
                        }
                        if muting.load(Ordering::SeqCst) {
                            continue;
                        }
                        if to_client.write_all(&chunk[..read]).await.is_err()
                            || to_client.flush().await.is_err()
                        {
                            break;
                        }
                    }
                });
            }
        });
        Self { addr, muted }
    }

    /// The suite's URL, at the proxy's address.
    fn url(&self) -> String {
        url_at(&redis_url(), self.addr)
    }

    fn mute(&self) {
        self.muted.store(true, Ordering::SeqCst);
    }
}

/// One worker replica: its running transport, and the producer its app bound.
struct Replica {
    worker: TransportHandle,
    producer: RedisQueueProducer,
}

/// Boot the worker app `M` and start its worker — one replica, leasing for
/// [`LEASE`] and draining within [`WINDOW`]. The app is leaked: the transport
/// borrows the container it owns, the way a process would hold it.
async fn replica<M: nest_rs_core::Module + 'static>() -> Replica {
    replica_of::<M>(TestApp::builder()).await
}

/// [`replica`], reaching Redis as `redis` says whatever the environment says:
/// the config is seeded, which freezes it against the deployment's variables —
/// the hermetic-test hatch, for a suite that needs a database or a user of its
/// own while `<PREFIX>_REDIS__URL` points every other one at the shared Redis.
async fn replica_on<M: nest_rs_core::Module + 'static>(redis: RedisConfig) -> Replica {
    replica_of::<M>(TestApp::builder().provide(redis)).await
}

async fn replica_of<M: nest_rs_core::Module + 'static>(
    builder: nest_rs_testing::TestAppBuilder,
) -> Replica {
    let (app, producer) = booted::<M>(builder).await;
    let worker = app
        .spawn_transport(QueueWorker::new())
        .await
        .expect("the queue worker transport starts");
    Box::leak(Box::new(app));
    Replica { worker, producer }
}

/// The worker app `M`, booted through its init phases, and the producer it
/// binds.
async fn booted<M: nest_rs_core::Module + 'static>(
    builder: nest_rs_testing::TestAppBuilder,
) -> (nest_rs_testing::HeadlessApp, RedisQueueProducer) {
    let app = brisk(builder)
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
    (app, producer)
}

/// A replica whose transport can die without a shutdown: aborting `serve`
/// drops the worker where it stands — its deliveries and their leases'
/// renewals — the way a killed process does.
struct Mortal {
    serving: tokio::task::JoinHandle<anyhow::Result<()>>,
    producer: RedisQueueProducer,
}

impl Mortal {
    /// Kill the replica, and wait until nothing of it runs any more.
    async fn kill(self) {
        self.serving.abort();
        let _ = self.serving.await;
    }
}

/// Boot the worker app `M` like [`replica`], and run its worker on a task a
/// test can abort.
async fn mortal_replica<M: nest_rs_core::Module + 'static>() -> Mortal {
    let (app, producer) = booted::<M>(TestApp::builder()).await;
    let mut worker = QueueWorker::new();
    worker
        .configure(app.container())
        .await
        .expect("the queue worker configures");
    let serving = tokio::spawn(Box::new(worker).serve(CancellationToken::new()));
    Box::leak(Box::new(app));
    Mortal { serving, producer }
}

/// The lease every replica of the suite holds its deliveries for: short, so a
/// job a dead replica held is free again within a test.
const LEASE: Duration = Duration::from_secs(1);

/// The drain window every replica of the suite stops within.
const WINDOW: Duration = Duration::from_secs(2);

/// `builder` with the suite's lease and drain window seeded — seeded rather
/// than pinned, so the suite's own variables cannot move them.
fn brisk(builder: nest_rs_testing::TestAppBuilder) -> nest_rs_testing::TestAppBuilder {
    builder
        .provide(RedisQueueConfig { lease: LEASE })
        .provide(QueueConfig {
            shutdown_timeout: WINDOW,
        })
}

/// A marker no earlier run of this suite chose. Queue names are compile-time
/// literals, so a run killed mid-job leaves work behind for the next one, and a
/// count that read it would be measuring an earlier run.
fn this_run() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos() as u64)
        .unwrap_or_default()
        ^ u64::from(std::process::id())
}

/// When each attempt at a run's job started — and, for the suites that care,
/// when one finished — one list per test.
struct Runs {
    started: Mutex<Vec<(u64, Instant)>>,
    finished: Mutex<Vec<u64>>,
}

impl Runs {
    const fn new() -> Self {
        Self {
            started: Mutex::new(Vec::new()),
            finished: Mutex::new(Vec::new()),
        }
    }

    /// Record an attempt at `run`'s job starting, answering which one it is
    /// from 1.
    fn start(&self, run: u64) -> usize {
        let mut seen = self.started.lock().expect("runs lock");
        seen.push((run, Instant::now()));
        seen.iter().filter(|(of, _)| *of == run).count()
    }

    /// Record an attempt at `run`'s job running to its end.
    fn finish(&self, run: u64) {
        self.finished.lock().expect("runs lock").push(run);
    }

    /// When each attempt at `run`'s job started.
    fn of(&self, run: u64) -> Vec<Instant> {
        self.started
            .lock()
            .expect("runs lock")
            .iter()
            .filter(|(of, _)| *of == run)
            .map(|(_, at)| *at)
            .collect()
    }

    /// How many attempts at `run`'s job ran to their end.
    fn finished(&self, run: u64) -> usize {
        self.finished
            .lock()
            .expect("runs lock")
            .iter()
            .filter(|of| **of == run)
            .count()
    }
}

/// The key of the structure `structure` of the queue named `queue` —
/// `…:jobs`, `…:unique` — spelled from the layout the documentation states
/// rather than reached, since the crate keeps its layout private: a test that
/// read the constant could not notice it moving.
fn key_of(queue: &str, structure: &str) -> String {
    format!("nestrs:queue:{{{queue}}}:{structure}")
}

/// How many jobs of `queue` sit on its stream, waiting or running — what an
/// autoscaler reads.
async fn filed(queue: &str) -> i64 {
    redis::cmd("XLEN")
        .arg(key_of(queue, "jobs"))
        .query_async(&mut connect().await)
        .await
        .expect("XLEN")
}

/// The field `field` of the hash `structure` of `queue`, if it is there.
async fn field_of(queue: &str, structure: &str, field: &str) -> Option<String> {
    redis::cmd("HGET")
        .arg(key_of(queue, structure))
        .arg(field)
        .query_async(&mut connect().await)
        .await
        .expect("HGET")
}

/// Every key the queue named `queue` holds, sorted.
async fn keys_of(queue: &str) -> Vec<String> {
    let mut keys: Vec<String> = redis::cmd("KEYS")
        .arg(key_of(queue, "*"))
        .query_async(&mut connect().await)
        .await
        .expect("KEYS");
    keys.sort();
    keys
}

/// Remove every key of the queue named `queue` — what a test that files jobs
/// no worker drains leaves behind.
async fn forget(queue: &str) {
    let keys = keys_of(queue).await;
    if !keys.is_empty() {
        let _: i64 = redis::cmd("DEL")
            .arg(&keys)
            .query_async(&mut connect().await)
            .await
            .expect("DEL");
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct FailoverCommand {
    run: u64,
}

/// Every attempt at a failover test's jobs. One topology runs per process, so
/// the Sentinel and the Cluster test share the fixture.
static FAILOVER: Runs = Runs::new();

#[nest_rs_queue::queue(name = "nestrs-e2e-failover", job = FailoverCommand)]
struct FailoverQueue;

#[nest_rs_core::injectable]
#[derive(Default)]
struct FailoverProcessor;

#[nest_rs_queue::processor]
impl FailoverProcessor {
    #[process(queue = FailoverQueue, retries = 5)]
    async fn run(&self, job: FailoverCommand) -> anyhow::Result<()> {
        FAILOVER.start(job.run);
        FAILOVER.finish(job.run);
        Ok(())
    }
}

#[nest_rs_core::module(
    imports = [RedisModule::for_root(None), RedisQueueModule, nest_rs_queue::QueueModule::for_root(None)],
    providers = [FailoverProcessor],
)]
struct FailoverModule;

/// Push `run`'s job once the producer answers again: through a failover, a
/// push fails until the topology names its new primary.
async fn push_through_a_failover(producer: &RedisQueueProducer, run: u64) {
    use nest_rs_queue::JobProducerExt as _;
    wait_for(Duration::from_secs(20), || async {
        producer
            .push(FailoverQueue, FailoverCommand { run }, None)
            .await
            .is_ok()
    })
    .await;
}

#[nest_rs_core::module(imports = [RedisModule::for_root(redis_config()), RedisQueueModule])]
struct ProducerOnlyModule;

/// A producer-only app's producer: pushes, and cancels, with no worker running
/// anywhere to take a job.
async fn producer() -> RedisQueueProducer {
    producer_on(redis_config()).await
}

/// A producer-only app reaching Redis as `redis` says.
async fn producer_on(redis: RedisConfig) -> RedisQueueProducer {
    let app = TestApp::builder()
        .provide(redis)
        .module::<ProducerOnlyModule>()
        .build_headless()
        .await
        .expect("a producer-only app boots against the dev container Redis");
    let producer = RedisQueueProducer::clone(
        &app.container()
            .get::<RedisQueueProducer>()
            .expect("the producer binding"),
    );
    Box::leak(Box::new(app));
    producer
}

/// The password every ACL user a test creates is given — a fixture, never a
/// secret.
const ACL_PASSWORD: &str = "nestrs-e2e-acl";

/// The Redis ACL rule the docs page `page` prescribes for `role`: the line of
/// its `` ```text title="Redis ACL — <role>" `` block, read from the page
/// itself.
fn documented_acl(page: &str, role: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/src/content/docs")
        .join(page);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("the page {} reads: {error}", path.display()));
    let fence = format!("```text title=\"Redis ACL — {role}\"");
    let mut blocks = text
        .lines()
        .zip(text.lines().skip(1))
        .filter(|(line, _)| line.trim() == fence)
        .map(|(_, rule)| rule.trim().to_owned());
    let rule = blocks.next().unwrap_or_else(|| {
        panic!("{page} prescribes a Redis ACL in a block titled `Redis ACL — {role}`")
    });
    assert!(
        blocks.next().is_none(),
        "{page} prescribes one Redis ACL for {role}"
    );
    rule
}

/// The page naming what every role's rule adds on a topology other than
/// standalone.
const CONNECTION_PAGE: &str = "queue/topologies.mdx";

/// What the suite's topology adds to every role's rule, as the connection's
/// page prescribes it: nothing on one server or under Sentinel.
fn topology_addition() -> String {
    match topology() {
        RedisTopology::Standalone | RedisTopology::Sentinel => String::new(),
        RedisTopology::Cluster => documented_acl(CONNECTION_PAGE, "Cluster"),
    }
}

/// `role`'s rule on `page`, with what the suite's topology adds to it.
fn documented_rule(page: &str, role: &str) -> String {
    format!("{} {}", documented_acl(page, role), topology_addition())
}

/// `rule` as the command creating `user`, its placeholders filled.
fn creating(rule: &str, user: &str) -> redis::Cmd {
    let tokens: Vec<String> = rule
        .split_whitespace()
        .map(|token| {
            token
                .replace("<user>", user)
                .replace("<password>", ACL_PASSWORD)
        })
        .collect();
    assert_eq!(
        tokens.get(..3),
        Some(["ACL".to_owned(), "SETUSER".to_owned(), user.to_owned()].as_slice()),
        "the rule creates the app's user: {rule}"
    );
    let mut create = redis::cmd(&tokens[0]);
    create.arg(&tokens[1..]);
    create
}

/// Create `user` exactly as the page `page` prescribes for `role` — its rule,
/// with what the suite's topology adds, sent as it is written on every data
/// node, the `<user>` and `<password>` placeholders filled and nothing added —
/// and answer the config that reaches database `db` as that user, so the user a
/// test runs as is the one the page tells an operator to create.
async fn documented_user(page: &str, role: &str, user: &str, db: u8) -> RedisConfig {
    let create = creating(&documented_rule(page, role), user);
    forget_user(user).await;
    let _: Vec<()> = on_every_node(&create).await;
    RedisConfig {
        url: url_as(&redis_url_on(db), user, ACL_PASSWORD),
        ..redis_config()
    }
}

/// The user `config` connects as may load a script — the `SCRIPT LOAD` a client
/// sends the first time Redis has not cached the script it calls.
///
/// A run through the user cannot show it: a script some earlier run loaded is
/// cached, the first `EVALSHA` answers, and no load is sent. So the permission
/// is asked of a script of its own: the ACL checks the command, and a script's
/// own commands only when it runs, which the run itself proves.
async fn assert_may_load_a_script(config: &RedisConfig) {
    let mut user = RedisConnection::connect(config)
        .await
        .expect("the page's user passes the boot's proof");
    let _: String = redis::cmd("SCRIPT")
        .arg("LOAD")
        .arg("return 1")
        .query_async(&mut user)
        .await
        .expect("the page's user may load a script");
}

/// An ACL user name of this run's own, from `base`: Redis's `ACL LOG` outlives
/// the user and the run, so a name an earlier run used carries that run's
/// denials into this one's.
fn acl_user(base: &str) -> String {
    format!("{base}-{}", this_run())
}

/// Remove `user`, if it is there.
async fn forget_user(user: &str) {
    forget_user_among(&data_nodes().await, user).await;
}

/// Remove `user` from `nodes`, where it is there.
async fn forget_user_among(nodes: &[redis::Client], user: &str) {
    let _: Vec<i64> = on_each(nodes, redis::cmd("ACL").arg("DELUSER").arg(user)).await;
}

/// Whether `event` carries Redis's refusal of a command or a key — the answer
/// an ACL gives.
fn refused_by_acl(event: &CapturedEvent) -> bool {
    event
        .field("error")
        .is_some_and(|error| error.contains("NOPERM") || error.contains("no permissions"))
}

/// Redis denied `user` nothing but the commands `besides` names on any data
/// node: its own ACL log holds every denial, so a refusal the app swallowed
/// shows there even when no line does.
async fn assert_redis_denied_nothing_but(user: &str, besides: &[&str]) {
    assert_denied_nothing_among(&data_nodes().await, user, besides).await;
}

/// [`assert_redis_denied_nothing_but`] on `nodes`.
async fn assert_denied_nothing_among(nodes: &[redis::Client], user: &str, besides: &[&str]) {
    // Every entry Redis keeps, not the ten `ACL LOG` answers by default: a
    // denial behind ten newer ones would pass unseen.
    let entries: Vec<std::collections::HashMap<String, redis::Value>> =
        on_each::<Vec<_>>(nodes, redis::cmd("ACL").arg("LOG").arg(i64::from(u32::MAX)))
            .await
            .into_iter()
            .flatten()
            .collect();
    let text = |entry: &std::collections::HashMap<String, redis::Value>, field: &str| {
        entry
            .get(field)
            .and_then(|value| redis::from_redis_value_ref::<String>(value).ok())
            .unwrap_or_default()
    };
    let denied: Vec<String> = entries
        .iter()
        .filter(|entry| text(entry, "username") == user)
        .map(|entry| text(entry, "object"))
        .filter(|object| !besides.contains(&object.as_str()))
        .collect();
    assert!(denied.is_empty(), "Redis denied {user}: {denied:?}");
}

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
//! The URL comes from `<PREFIX>_REDIS__URL` (the dev container wires
//! `redis://redis:6379`); unset, it falls back to that default. This file holds
//! the suite's shared fixtures and nothing else.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod connection;
mod correlation;
#[path = "../harness/mod.rs"]
mod harness;
mod layout;
mod queue;
mod schedule;
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
    RedisQueueProducer,
};
use nest_rs_testing::{CapturedEvent, TestApp, TransportHandle};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[expect(
    clippy::disallowed_methods,
    reason = "the suite targets the Redis the deployment names, as the app would"
)]
fn redis_url() -> String {
    std::env::var(nest_rs_config::var_name("redis", "URL"))
        .unwrap_or_else(|_| "redis://redis:6379".to_string())
}

/// The dev container Redis's `host:port`, for a proxy standing in front of it.
fn redis_address() -> String {
    redis::IntoConnectionInfo::into_connection_info(redis_url().as_str())
        .expect("the dev container Redis URL parses")
        .addr()
        .to_string()
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
const DB_CONNECTION_DROP: u8 = 11;
const DB_CONFINED_TO_THE_PREFIX: u8 = 9;
const DB_TLS_FLUSH: u8 = 12;
const DB_TLS_REFUSED_REOPEN: u8 = 13;
/// Claims only: every scheduler the `schedule` tests boot claims here, so the
/// key layout is asserted over the whole database.
const DB_SCHEDULE: u8 = 14;

// Two tests sharing a database meet each other's keys and connections, Redis
// ships sixteen, and the demo holds the lower half: all three facts are checked
// where the list is written.
const _: () = {
    let dbs = [
        DB_CONFINED_TO_THE_PREFIX,
        DB_CONNECTION_DROP,
        DB_TLS_FLUSH,
        DB_TLS_REFUSED_REOPEN,
        DB_SCHEDULE,
    ];
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
};

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

/// A proxy in front of the dev container Redis that, once muted, still carries
/// every command to Redis and drops every reply — a command that runs and whose
/// answer is lost, the case only a network can make.
struct MutingProxy {
    addr: SocketAddr,
    muted: Arc<AtomicBool>,
}

impl MutingProxy {
    async fn start() -> Self {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let upstream = redis_address();
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the proxy");
        let addr = listener.local_addr().expect("the proxy's address");
        let muted = Arc::new(AtomicBool::new(false));
        let muting = Arc::clone(&muted);
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let Ok(server) = TcpStream::connect(&upstream).await else {
                    continue;
                };
                let (mut from_client, mut to_client) = client.into_split();
                let (mut from_server, mut to_server) = server.into_split();
                tokio::spawn(async move {
                    let _ = tokio::io::copy(&mut from_client, &mut to_server).await;
                });
                let muting = Arc::clone(&muting);
                tokio::spawn(async move {
                    let mut chunk = [0_u8; 16 * 1024];
                    while let Ok(read) = from_server.read(&mut chunk).await {
                        if read == 0 {
                            break;
                        }
                        if muting.load(Ordering::SeqCst) {
                            continue;
                        }
                        if to_client.write_all(&chunk[..read]).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        Self { addr, muted }
    }

    fn url(&self) -> String {
        format!("redis://{}/", self.addr)
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
    let worker = app
        .spawn_transport(QueueWorker::new())
        .await
        .expect("the queue worker transport starts");
    Box::leak(Box::new(app));
    Replica { worker, producer }
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
    let app = brisk(TestApp::builder())
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

#[nest_rs_core::module(imports = [RedisModule::for_root(redis_config()), RedisQueueModule])]
struct ProducerOnlyModule;

/// A producer-only app's producer: pushes, and cancels, with no worker running
/// anywhere to take a job.
async fn producer() -> RedisQueueProducer {
    let app = TestApp::builder()
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

/// Create `user` exactly as the page `page` prescribes for `role` — its rule sent as it is
/// written, the `<user>` and `<password>` placeholders filled and nothing added
/// — and answer the config that reaches database `db` as that user, so the user
/// a test runs as is the one the page tells an operator to create.
async fn documented_user(page: &str, role: &str, user: &str, db: u8) -> RedisConfig {
    let rule = documented_acl(page, role);
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
        "{page}'s rule creates the app's user: {rule}"
    );
    forget_user(user).await;
    let _: () = redis::cmd(&tokens[0])
        .arg(&tokens[1..])
        .query_async(&mut connect().await)
        .await
        .unwrap_or_else(|error| panic!("Redis takes {page}'s rule `{rule}`: {error}"));
    RedisConfig {
        url: redis_url_on(db).replacen("://", &format!("://{user}:{ACL_PASSWORD}@"), 1),
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
    let _: i64 = redis::cmd("ACL")
        .arg("DELUSER")
        .arg(user)
        .query_async(&mut connect().await)
        .await
        .expect("ACL DELUSER");
}

/// Whether `event` carries Redis's refusal of a command or a key — the answer
/// an ACL gives.
fn refused_by_acl(event: &CapturedEvent) -> bool {
    event
        .field("error")
        .is_some_and(|error| error.contains("NOPERM") || error.contains("no permissions"))
}

/// Redis denied `user` nothing but the commands `besides` names: its own ACL
/// log holds every denial, so a refusal the app swallowed shows there even
/// when no line does.
async fn assert_redis_denied_nothing_but(user: &str, besides: &[&str]) {
    // Every entry Redis keeps, not the ten `ACL LOG` answers by default: a
    // denial behind ten newer ones would pass unseen.
    let entries: Vec<std::collections::HashMap<String, redis::Value>> = redis::cmd("ACL")
        .arg("LOG")
        .arg(i64::from(u32::MAX))
        .query_async(&mut connect().await)
        .await
        .expect("ACL LOG");
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

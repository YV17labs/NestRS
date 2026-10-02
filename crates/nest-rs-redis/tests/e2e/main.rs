//! Live-Redis e2e for `nest-rs-redis`. One module per concern in `src/`:
//! [`connection`] for the shared connection's boot, bound and recovery,
//! [`tls`] for `rediss://`, [`throttler`] for the cross-process rate-limit
//! store, [`layout`] for where a queue lives — its namespace, the 6.x layout a
//! worker refuses to start beside and the documented move out of it, and a user
//! confined to the framework's keys —
//! [`queue`] for the producer binding, a delayed push and its promotion, and
//! [`worker`] for the consumer: its fetch and concurrency, each delivery's
//! retries and hand-backs, and the lease that keeps a second delivery from
//! running beside the first.
//! [`correlation`] covers the trace context that crosses the producer/consumer
//! process boundary, which is both halves' concern.
//! [`schedule`] covers the occurrence lock a job firing once across replicas
//! claims through.
//!
//! Needs a reachable Redis — gated out of `unit` by the nextest `binary(e2e)`
//! filter, and behind the `throttler` and `schedule` features (off by default,
//! so producer / consumer apps that never rate-limit or schedule once pull
//! neither `nest-rs-throttler` nor `nest-rs-schedule`). Run it explicitly:
//!
//! ```bash
//! cargo nextest run -p nest-rs-redis --features throttler,schedule -E 'binary(e2e)'
//! ```
//!
//! The URL comes from `NESTRS_REDIS__URL` (the dev container wires
//! `redis://redis:6379`); unset, it falls back to that default. This file holds
//! the suite's shared fixtures and nothing else — every test lives in the
//! module named for the concern it covers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod connection;
mod correlation;
mod layout;
mod queue;
mod schedule;
mod throttler;
mod tls;
mod worker;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nest_rs_core::Transport;
use nest_rs_redis::{
    RedisConfig, RedisConnection, RedisModule, RedisQueueModule, RedisQueueProducer, RedisWorker,
    RedisWorkerConfig,
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
/// collide. Every other test shares database 0, on queue names of its own.
/// The framework's are 9 to 15: the demo's suites run on the same Redis and
/// hold 1 to 8, so a `FLUSHDB` here never reaches a job one of theirs filed.
const DB_CONNECTION_DROP: u8 = 11;
const DB_CONNECTION_RESET_MID_ATTEMPT: u8 = 10;
const DB_CONFINED_TO_THE_PREFIX: u8 = 9;
const DB_TLS_FLUSH: u8 = 12;
const DB_TLS_REFUSED_REOPEN: u8 = 13;
/// Claims only: every scheduler the `schedule` tests boot claims here, so the
/// key layout is asserted over the whole database.
const DB_SCHEDULE: u8 = 14;
const DB_UNIQUE_REFUSED: u8 = 15;

// Two tests sharing a database meet each other's keys and connections, Redis
// ships sixteen, and the demo holds the lower half: all three facts are checked
// where the list is written.
const _: () = {
    let dbs = [
        DB_CONFINED_TO_THE_PREFIX,
        DB_CONNECTION_RESET_MID_ATTEMPT,
        DB_CONNECTION_DROP,
        DB_TLS_FLUSH,
        DB_TLS_REFUSED_REOPEN,
        DB_SCHEDULE,
        DB_UNIQUE_REFUSED,
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
        ..Default::default()
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

/// Boot the worker app `M` and start its worker — one replica. The app is
/// leaked: the transport borrows the container it owns, the way a process would
/// hold it.
async fn replica<M: nest_rs_core::Module + 'static>() -> Replica {
    replica_of::<M>(TestApp::builder()).await
}

/// [`replica`], reaching Redis as `redis` says whatever the environment says:
/// the config is seeded, which freezes it against the deployment's variables —
/// the hermetic-test hatch, for a suite that needs a database or a user of its
/// own while `NESTRS_REDIS__URL` points every other one at the shared Redis.
async fn replica_on<M: nest_rs_core::Module + 'static>(redis: RedisConfig) -> Replica {
    replica_of::<M>(TestApp::builder().provide(redis)).await
}

async fn replica_of<M: nest_rs_core::Module + 'static>(
    builder: nest_rs_testing::TestAppBuilder,
) -> Replica {
    let app = builder
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

/// A replica whose transport can die without a shutdown: aborting `serve`
/// drops the worker where it stands — its deliveries, their leases' renewals,
/// its heartbeat — the way a killed process does.
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
    let mut worker = RedisWorker::default();
    worker
        .configure(app.container())
        .await
        .expect("the queue worker configures");
    let serving = tokio::spawn(Box::new(worker).serve(CancellationToken::new()));
    Box::leak(Box::new(app));
    Mortal { serving, producer }
}

/// The worker settings the guard's suites run under: a lease of two seconds, so
/// a job a dead replica held is free again within a test, and the shortest
/// orphan threshold accepted — polling as a deployment does by default.
fn brisk() -> RedisWorkerConfig {
    RedisWorkerConfig {
        shutdown_timeout: Duration::from_secs(2),
        orphan_after: Duration::from_secs(5),
        lease: Duration::from_secs(2),
        ..Default::default()
    }
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

/// Leave for `queue` what an earlier run of the suite, killed mid-job, leaves
/// behind: a job of that run in the in-flight set of a consumer that no longer
/// beats, with no settled mark — the state a starting replica's sweep, or a
/// running one's once the threshold passes, hands back to this run's replica.
/// `payload` is the job's, carrying a marker no test of this run chose.
///
/// A test that counts every job its queue holds, rather than its own run's,
/// counts this one too: calling it first is what proves a test does not. Names
/// are apalis's, read off its `Config`, so the ghost sits where apalis looks.
async fn ghost(queue: &str, payload: serde_json::Value) {
    use apalis::prelude::{Request, Storage};

    let apalis = apalis_redis::Config::default().set_namespace(&namespace(queue));
    let mut storage: apalis_redis::RedisStorage<serde_json::Value, RedisConnection> =
        apalis_redis::RedisStorage::new_with_config(connect().await, apalis.clone());
    let filed = storage
        .push_request(Request::new(serde_json::json!({
            "v": nest_rs_queue::WIRE_FORMAT_VERSION,
            "payload": payload,
        })))
        .await
        .expect("an earlier run's job, filed as apalis files one");
    let id = filed.task_id.to_string();
    let consumer = format!("{}:ghost-{}", apalis.inflight_jobs_set(), this_run());
    let _: () = redis::pipe()
        .lrem(apalis.active_jobs_list(), 0, &id)
        .ignore()
        .sadd(&consumer, &id)
        .ignore()
        .zadd(apalis.consumers_set(), &consumer, 0)
        .ignore()
        .query_async(&mut connect().await)
        .await
        .expect("the job in flight under a consumer long silent");
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

/// Whether `event` names the job `id`.
fn names(event: &CapturedEvent, id: &nest_rs_queue::JobId) -> bool {
    event.field("job_id").as_deref() == Some(id.to_string().as_str())
}

/// The namespace the queue named `queue` lives under — spelled here rather than
/// reached, since the crate keeps its layout private: a test that read the
/// constant could not notice it moving.
fn namespace(queue: &str) -> String {
    format!("nestrs:queue:{queue}")
}

/// The length of the list a worker fetches `queue`'s jobs from — the one an
/// autoscaler reads.
async fn waiting(queue: &str) -> i64 {
    let mut admin = connect().await;
    redis::cmd("LLEN")
        .arg(format!("{}:active", namespace(queue)))
        .query_async(&mut admin)
        .await
        .expect("LLEN")
}

/// The key of `member` in the structure `structure` under the queue named
/// `queue` — `…:unique:<key>`, `…:checkpoints:<job>` — spelled from the layout
/// the documentation states, like [`namespace`].
fn key_of(queue: &str, structure: &str, member: &str) -> String {
    format!("{}:{structure}:{member}", namespace(queue))
}

/// How long `key` has left, in milliseconds: `-2` when it is gone, `-1` when it
/// never lapses.
async fn pttl(key: &str) -> i64 {
    redis::cmd("PTTL")
        .arg(key)
        .query_async(&mut connect().await)
        .await
        .expect("PTTL")
}

/// What `key` holds, if it is there.
async fn read(key: &str) -> Option<String> {
    redis::cmd("GET")
        .arg(key)
        .query_async(&mut connect().await)
        .await
        .expect("GET")
}

/// Remove every key under the queue named `queue` — what a test that files jobs
/// no worker drains leaves behind.
async fn forget(queue: &str) {
    let mut admin = connect().await;
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{}:*", namespace(queue)))
        .query_async(&mut admin)
        .await
        .expect("KEYS");
    if !keys.is_empty() {
        let _: i64 = redis::cmd("DEL")
            .arg(&keys)
            .query_async(&mut admin)
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

/// The one Redis ACL rule the docs page `page` prescribes: the line of its
/// `` ```text title="Redis ACL" `` block, read from the page itself.
fn documented_acl(page: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/src/content/docs")
        .join(page);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("the page {} reads: {error}", path.display()));
    let mut blocks = text
        .lines()
        .zip(text.lines().skip(1))
        .filter(|(fence, _)| fence.trim() == "```text title=\"Redis ACL\"")
        .map(|(_, rule)| rule.trim().to_owned());
    let rule = blocks
        .next()
        .unwrap_or_else(|| panic!("{page} prescribes a Redis ACL in a block titled `Redis ACL`"));
    assert!(blocks.next().is_none(), "{page} prescribes one Redis ACL");
    rule
}

/// Create `user` exactly as the page `page` prescribes — its rule sent as it is
/// written, the `<user>` and `<password>` placeholders filled and nothing added
/// — and answer the config that reaches database `db` as that user, so the user
/// a test runs as is the one the page tells an operator to create.
async fn documented_user(page: &str, user: &str, db: u8) -> RedisConfig {
    let rule = documented_acl(page);
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
        ..Default::default()
    }
}

/// The user `config` connects as may load a script — the `SCRIPT LOAD` a client
/// sends the first time Redis has not cached the script it calls.
///
/// A run through the user cannot show it: a script some earlier run loaded is
/// cached, the first `EVALSHA` answers, and no load is sent. Flushing the cache
/// would show it and break every other test mid-call, so the permission is asked
/// of a script of its own: the ACL checks the command, and a script's own
/// commands only when it runs, which the run itself proves.
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

/// The one command a documented rule leaves out on purpose: the client
/// library's `CLIENT SETINFO`, which it sends on every connection and whose
/// refusal it ignores. Redis 7.0 knows no such subcommand and refuses a rule
/// naming it, so allowing it would cost the rule every server before 7.2. Its
/// refusal reaches `ACL LOG` as `client|setinfo` from 7.2 on, and as `client` on
/// Redis 6, which logs a command without its subcommand.
const SETINFO: [&str; 2] = ["client|setinfo", "client"];

/// Whether `event` carries Redis's refusal of a command or a key — the answer
/// an ACL gives.
fn refused_by_acl(event: &CapturedEvent) -> bool {
    event
        .field("error")
        .is_some_and(|error| error.contains("NOPERM") || error.contains("no permissions"))
}

/// Redis denied `user` nothing but [`SETINFO`], which a documented rule leaves
/// out, and the commands `besides` names: its own ACL log holds every denial,
/// so a refusal the app swallowed shows there even when no line does.
async fn assert_redis_denied_nothing_but(user: &str, besides: &[&str]) {
    let entries: Vec<std::collections::HashMap<String, redis::Value>> = redis::cmd("ACL")
        .arg("LOG")
        .query_async(&mut connect().await)
        .await
        .expect("ACL LOG");
    let text = |entry: &std::collections::HashMap<String, redis::Value>, field: &str| {
        entry
            .get(field)
            .and_then(|value| redis::from_redis_value::<String>(value).ok())
            .unwrap_or_default()
    };
    let denied: Vec<String> = entries
        .iter()
        .filter(|entry| text(entry, "username") == user)
        .map(|entry| text(entry, "object"))
        .filter(|object| !SETINFO.contains(&object.as_str()) && !besides.contains(&object.as_str()))
        .collect();
    assert!(denied.is_empty(), "Redis denied {user}: {denied:?}");
}

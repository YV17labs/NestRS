//! A scheduled job declared `replicas = "one"` fires once per occurrence across
//! the replicas of an app, against a live Redis — through the occurrence lock
//! `RedisScheduleModule` binds.
//!
//! Every app here is the documented wiring — `ScheduleModule`,
//! `RedisModule::for_root(None)`, `RedisScheduleModule` and a `#[scheduled]`
//! host — booted through the harness, with its scheduler started the way the
//! app's run starts it. Each boot seeds its `RedisConfig`, the hermetic-test
//! hatch: a seed freezes the namespace, so the `NESTRS_REDIS__URL` the suite
//! runs under cannot move an app off the database or the proxy the test chose.
//! All of them claim on [`crate::DB_SCHEDULE`], which holds nothing but claims,
//! so the key layout is asserted over the whole database.
//!
//! Four questions:
//!
//! 1. **Does the wiring claim in Redis?** Each occurrence fired is one key, laid
//!    out as the page states and held for the port's hold.
//! 2. **Do two replicas fire each occurrence once?** Both reach every instant,
//!    since an `#[every]` declared so ticks on the epoch; each claims it with one
//!    `SET … NX PX`, and only the replica whose `SET` created the key fires.
//! 3. **Does a claim behave as the port's contract says?** First wins, the key
//!    lasts the hold, `claimed` follows the key.
//! 4. **What does an outage do?** Every occurrence whose claim Redis cannot
//!    answer is skipped with a `warn` — firing unclaimed would fire it on every
//!    replica — and the schedule claims and fires again once Redis answers.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nest_rs_core::{injectable, module};
use nest_rs_redis::{
    RedisConfig, RedisConnection, RedisModule, RedisOccurrenceLock, RedisScheduleModule,
};
use nest_rs_schedule::{OccurrenceLock, ScheduleModule, Scheduler, scheduled};
use nest_rs_testing::{LogCapture, TestApp, TransportHandle};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

/// Where every claim lives — the layout the schedule page states and an
/// operator scans, read here as written there so that moving it fails.
const CLAIMS: &str = "nestrs:schedule:claims";

/// How long a live scheduler is given to reach what a test waits for. Periods
/// here are a quarter of a second, so this is a regression's bound, not a pace.
const WITHIN: Duration = Duration::from_secs(20);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

fn schedule_config() -> RedisConfig {
    RedisConfig {
        url: crate::redis_url_on(crate::DB_SCHEDULE),
        ..crate::redis_config()
    }
}

async fn schedule_connection() -> RedisConnection {
    RedisConnection::connect(&schedule_config())
        .await
        .expect("connect to the schedule's database")
}

/// Boot one replica of `M` on `config` and start its scheduler. The app is
/// leaked: the transport borrows the container it owns, the way a process would
/// hold it.
async fn schedule_replica<M: nest_rs_core::Module + 'static>(
    config: RedisConfig,
) -> TransportHandle {
    let app = TestApp::builder()
        .module::<M>()
        .provide(config)
        .build_headless()
        .await
        .expect("a replica boots with RedisScheduleModule against the dev container Redis");
    app.init().await.expect("init phases");
    let scheduler = app
        .spawn_transport(Scheduler::new())
        .await
        .expect("the scheduler configures: RedisScheduleModule binds the occurrence lock");
    Box::leak(Box::new(app));
    scheduler
}

/// Every key on the schedule's database.
async fn every_key(conn: &RedisConnection) -> Vec<String> {
    redis::cmd("KEYS")
        .arg("*")
        .query_async(&mut conn.clone())
        .await
        .expect("read the schedule's database")
}

/// The claims of `provider:method` on occurrences at or after `since_ms` — a run
/// before this one leaves claims for a minute, and they are not this run's.
async fn claims_since(conn: &RedisConnection, job: &str, since_ms: u64) -> Vec<String> {
    let mut claims: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{CLAIMS}:{job}:*"))
        .query_async(&mut conn.clone())
        .await
        .expect("read the claims");
    claims.retain(|key| {
        key.rsplit(':')
            .next()
            .and_then(|ms| ms.parse::<u64>().ok())
            .is_some_and(|ms| ms >= since_ms)
    });
    claims.sort();
    claims
}

/// Poll until `job` holds at least `count` claims since `since_ms`, within
/// [`WITHIN`]; the caller asserts on what it finds.
async fn await_claims(conn: &RedisConnection, job: &str, since_ms: u64, count: usize) {
    let deadline = tokio::time::Instant::now() + WITHIN;
    while tokio::time::Instant::now() < deadline {
        if claims_since(conn, job, since_ms).await.len() >= count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// --- 1. the documented wiring --------------------------------------------------

static WIRED_RUNS: AtomicUsize = AtomicUsize::new(0);

#[injectable]
#[derive(Default)]
struct WiredTasks;

#[scheduled]
impl WiredTasks {
    #[every("250ms", replicas = "one")]
    async fn sweep(&self) -> anyhow::Result<()> {
        WIRED_RUNS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[module(
    imports = [ScheduleModule, RedisModule::for_root(None), RedisScheduleModule],
    providers = [WiredTasks],
)]
struct WiredModule;

/// The documented wiring boots, and what it binds is Redis's lock: every
/// occurrence the job fired is one key under the claims structure, named by the
/// port's token verbatim, holding who claimed it for the port's hold.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_documented_wiring_claims_each_occurrence_it_fires_in_redis() {
    let conn = schedule_connection().await;
    let started_ms = now_ms();
    let scheduler = schedule_replica::<WiredModule>(schedule_config()).await;
    await_claims(&conn, "WiredTasks:sweep", started_ms, 3).await;
    scheduler.shutdown().await.expect("clean shutdown");

    let claims = claims_since(&conn, "WiredTasks:sweep", started_ms).await;
    assert!(
        claims.len() >= 3,
        "a quarter-second job claimed its occurrences within {WITHIN:?}: {claims:?}"
    );
    assert_eq!(
        WIRED_RUNS.load(Ordering::SeqCst),
        claims.len(),
        "each occurrence claimed was fired, and none fired unclaimed: {claims:?}"
    );
    for key in &claims {
        let instant = key
            .strip_prefix(&format!("{CLAIMS}:WiredTasks:sweep:"))
            .and_then(|ms| ms.parse::<u64>().ok())
            .unwrap_or_else(|| panic!("`{key}` is the claims structure, then the port's token"));
        assert_eq!(instant % 250, 0, "an instant on the epoch's period: {key}");
        let holder: String = redis::cmd("GET")
            .arg(key)
            .query_async(&mut conn.clone())
            .await
            .expect("read the claim");
        assert!(!holder.is_empty(), "the claim records who holds it");
        let left: i64 = redis::cmd("PTTL")
            .arg(key)
            .query_async(&mut conn.clone())
            .await
            .expect("read the claim's expiry");
        assert!(
            (1..=60_000).contains(&left),
            "the claim expires within the port's one-minute hold, it has {left} ms left"
        );
    }
}

// --- 2. two replicas ------------------------------------------------------------

static REPLICATED_RUNS: AtomicUsize = AtomicUsize::new(0);

#[injectable]
#[derive(Default)]
struct ReplicatedTasks;

#[scheduled]
impl ReplicatedTasks {
    #[every("250ms", replicas = "one")]
    async fn sweep(&self) -> anyhow::Result<()> {
        REPLICATED_RUNS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[module(
    imports = [ScheduleModule, RedisModule::for_root(None), RedisScheduleModule],
    providers = [ReplicatedTasks],
)]
struct ReplicatedModule;

/// Two replicas sharing one Redis fire each occurrence once between them: the
/// runs across both equal the claims, every occurrence after both are up was
/// contested — the replica that lost says so — and the database holds claims
/// and nothing else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_replicas_sharing_redis_fire_each_occurrence_once() {
    // Global: the loser's line is emitted by a scheduler task, on a worker thread.
    let logs = LogCapture::install_global();
    let conn = schedule_connection().await;
    let started_ms = now_ms();
    let first = schedule_replica::<ReplicatedModule>(schedule_config()).await;
    let second = schedule_replica::<ReplicatedModule>(schedule_config()).await;
    let both_up_ms = now_ms();
    await_claims(&conn, "ReplicatedTasks:sweep", both_up_ms, 4).await;
    first.shutdown().await.expect("clean shutdown");
    second.shutdown().await.expect("clean shutdown");

    let contested = claims_since(&conn, "ReplicatedTasks:sweep", both_up_ms).await;
    assert!(
        contested.len() >= 4,
        "occurrences were claimed with both replicas up, within {WITHIN:?}: {contested:?}"
    );
    let claims = claims_since(&conn, "ReplicatedTasks:sweep", started_ms).await;
    assert_eq!(
        REPLICATED_RUNS.load(Ordering::SeqCst),
        claims.len(),
        "each occurrence ran once across the two replicas, not once on each: {claims:?}"
    );
    let lost = logs
        .find(
            nest_rs_schedule::TARGET,
            "occurrence claimed by another replica",
        )
        .into_iter()
        .filter(|event| event.field("provider").as_deref() == Some("ReplicatedTasks"))
        .count();
    assert!(
        lost >= 1,
        "the replica that lost a contested occurrence says so, and none did"
    );

    let foreign: Vec<String> = every_key(&conn)
        .await
        .into_iter()
        .filter(|key| !key.starts_with(&format!("{CLAIMS}:")))
        .collect();
    assert!(
        foreign.is_empty(),
        "the occurrence lock writes claims and nothing else: {foreign:?}"
    );
}

// --- 3. the claim ---------------------------------------------------------------

/// The claim itself: the first `SET` wins, a second loses, the key carries the
/// port's hold as its expiry, and what `claimed` answers follows the key — so
/// the key is the whole of the coordination, and removing it frees the
/// occurrence.
#[tokio::test]
async fn the_first_claim_wins_and_the_key_is_the_whole_claim() {
    let conn = schedule_connection().await;
    let lock = RedisOccurrenceLock::new(conn.clone());
    let occurrence = format!("ClaimProbe:hold:{}", now_ms());
    let key = format!("{CLAIMS}:{occurrence}");
    let hold = Duration::from_secs(30);

    assert!(
        !lock.claimed(&occurrence).await.expect("Redis answers"),
        "nobody holds an occurrence before its first claim",
    );
    assert!(
        lock.claim(&occurrence, hold).await.expect("Redis answers"),
        "the first claim creates the key",
    );
    assert!(
        lock.claimed(&occurrence).await.expect("Redis answers"),
        "and a replica asking about it sees it held",
    );
    assert!(
        !lock.claim(&occurrence, hold).await.expect("Redis answers"),
        "a second claim on the same occurrence loses",
    );
    let left: i64 = redis::cmd("PTTL")
        .arg(&key)
        .query_async(&mut conn.clone())
        .await
        .expect("read the claim's expiry");
    assert!(
        (1..=30_000).contains(&left),
        "the key expires with the hold it was claimed for, it has {left} ms left"
    );

    redis::cmd("DEL")
        .arg(&key)
        .query_async::<()>(&mut conn.clone())
        .await
        .expect("remove the claim");
    assert!(
        !lock.claimed(&occurrence).await.expect("Redis answers"),
        "with the key gone nobody holds it",
    );
    assert!(
        lock.claim(&occurrence, hold).await.expect("Redis answers"),
        "and the occurrence can be claimed again",
    );
    redis::cmd("DEL")
        .arg(&key)
        .query_async::<()>(&mut conn.clone())
        .await
        .expect("remove the claim");
}

// --- 4. an outage ---------------------------------------------------------------

/// A TCP proxy in front of the dev container Redis that can sever the app from
/// it and then let it back: severed, it drops every connection it carries and
/// closes each new one on accept — Redis gone, as a restart or a failover leaves
/// it — and restored, it carries new connections again.
struct SeveringProxy {
    addr: SocketAddr,
    severed: watch::Sender<bool>,
}

impl SeveringProxy {
    async fn start() -> Self {
        let upstream = crate::redis_address();
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the proxy");
        let addr = listener.local_addr().expect("the proxy's address");
        let severed = watch::channel(false).0;
        let accepting = severed.subscribe();
        tokio::spawn(async move {
            while let Ok((mut client, _)) = listener.accept().await {
                if *accepting.borrow() {
                    drop(client);
                    continue;
                }
                let Ok(mut server) = TcpStream::connect(&upstream).await else {
                    continue;
                };
                let mut cut = accepting.clone();
                tokio::spawn(async move {
                    tokio::select! {
                        _ = tokio::io::copy_bidirectional(&mut client, &mut server) => {}
                        _ = cut.wait_for(|severed| *severed) => {}
                    }
                });
            }
        });
        Self { addr, severed }
    }

    fn url_on(&self, db: u8) -> String {
        format!("redis://{}/{db}", self.addr)
    }

    fn sever(&self) {
        self.severed.send_replace(true);
    }

    fn restore(&self) {
        self.severed.send_replace(false);
    }
}

/// The outage app's Redis: the schedule's database, through `proxy`.
fn proxied_config(proxy: &SeveringProxy) -> RedisConfig {
    RedisConfig {
        url: proxy.url_on(crate::DB_SCHEDULE),
        // A command Redis cannot answer fails at this budget, so an occurrence
        // whose claim is lost to the outage is skipped within a second.
        connect_timeout: Duration::from_secs(1),
        ..crate::redis_config()
    }
}

static OUTAGE_RUNS: AtomicUsize = AtomicUsize::new(0);

#[injectable]
#[derive(Default)]
struct OutageTasks;

#[scheduled]
impl OutageTasks {
    #[every("250ms", replicas = "one")]
    async fn sweep(&self) -> anyhow::Result<()> {
        OUTAGE_RUNS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[module(
    imports = [ScheduleModule, RedisModule::for_root(None), RedisScheduleModule],
    providers = [OutageTasks],
)]
struct OutageModule;

/// The skips the outage caused, as the scheduler reports them.
fn skipped(logs: &LogCapture) -> Vec<nest_rs_testing::CapturedEvent> {
    logs.find(
        nest_rs_schedule::TARGET,
        "occurrence skipped: its lock could not be claimed",
    )
    .into_iter()
    .filter(|event| event.field("provider").as_deref() == Some("OutageTasks"))
    .collect()
}

/// Redis going away while the schedule runs: no occurrence fires while its
/// claim cannot be made, each one skipped is said at `warn` with the job, the
/// occurrence and Redis's error, and once Redis answers again the schedule
/// claims and fires without a restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redis_outage_skips_occurrences_aloud_and_the_schedule_recovers() {
    // Global: the skips are emitted by the scheduler's task, on a worker thread.
    let logs = LogCapture::install_global();
    let proxy = SeveringProxy::start().await;
    let scheduler = schedule_replica::<OutageModule>(proxied_config(&proxy)).await;
    crate::wait_until(WITHIN, || OUTAGE_RUNS.load(Ordering::SeqCst) >= 1).await;
    assert!(
        OUTAGE_RUNS.load(Ordering::SeqCst) >= 1,
        "the job fires through the proxy before the outage"
    );

    proxy.sever();
    crate::wait_until(WITHIN, || !skipped(&logs).is_empty()).await;
    let first_skips = skipped(&logs).len();
    assert!(first_skips >= 1, "an occurrence lost to the outage is said");
    let runs_in_outage = OUTAGE_RUNS.load(Ordering::SeqCst);
    crate::wait_until(WITHIN, || skipped(&logs).len() >= first_skips + 2).await;
    assert!(
        skipped(&logs).len() >= first_skips + 2,
        "every occurrence of the outage is skipped aloud, not only the first"
    );
    assert_eq!(
        OUTAGE_RUNS.load(Ordering::SeqCst),
        runs_in_outage,
        "no occurrence fires while its claim cannot be made"
    );

    proxy.restore();
    crate::wait_until(WITHIN, || {
        OUTAGE_RUNS.load(Ordering::SeqCst) > runs_in_outage
    })
    .await;
    assert!(
        OUTAGE_RUNS.load(Ordering::SeqCst) > runs_in_outage,
        "the schedule fires again once Redis answers, within {WITHIN:?}"
    );
    scheduler.shutdown().await.expect("clean shutdown");

    let skip = skipped(&logs).remove(0);
    assert_eq!(skip.level, "warn");
    assert_eq!(skip.field("method").as_deref(), Some("sweep"));
    assert!(
        skip.field("occurrence")
            .is_some_and(|ms| ms.parse::<u64>().is_ok_and(|ms| ms % 250 == 0)),
        "names the occurrence it skipped: {skip:?}"
    );
    assert!(
        skip.field("error").is_some_and(|error| !error.is_empty()),
        "names why Redis could not answer: {skip:?}"
    );
}

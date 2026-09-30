//! A scheduled job declared `replicas = "one"` fires once per occurrence across
//! the replicas of an app, and never while a run of it is going on elsewhere,
//! against a live Redis — through the occurrence lock `RedisScheduleModule`
//! binds.
//!
//! Every app here is the documented wiring — `ScheduleModule`,
//! `RedisModule::for_root(None)`, `RedisScheduleModule` and a `#[scheduled]`
//! host — booted through the harness, with its scheduler started the way the
//! app's run starts it. Each boot seeds its `RedisConfig`, the hermetic-test
//! hatch: a seed freezes the namespace, so the `NESTRS_REDIS__URL` the suite
//! runs under cannot move an app off the database or the proxy the test chose.
//! All of them claim on [`crate::DB_SCHEDULE`], which holds nothing but claims
//! and leases, so the key layout is asserted over the whole database.
//!
//! Six questions:
//!
//! 1. **Does the wiring claim in Redis?** Each occurrence fired is one key, laid
//!    out as the page states and held for the port's hold, and each run gives
//!    its lease back.
//! 2. **Do two replicas fire each occurrence once?** Both reach every instant,
//!    since an `#[every]` declared so ticks on the epoch; each claims it with one
//!    script, and only the replica whose claim created the key fires.
//! 3. **Does a run hold the job on every replica?** A run three periods long on
//!    three replicas: never two in flight.
//! 4. **Are two apps' same-named jobs two jobs?** Each claims its own
//!    occurrences, under the path that declared it.
//! 5. **Does a claim behave as the port's contract says?** Lease first, then the
//!    claim; first wins; each key lasts its hold; only the run holding a lease
//!    renews or releases it.
//! 6. **What does an outage do?** Every occurrence whose claim Redis cannot
//!    answer is skipped with a `warn` — firing unclaimed would fire it on every
//!    replica — and the schedule claims and fires again once Redis answers.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nest_rs_core::{injectable, module};
use nest_rs_redis::{
    RedisConfig, RedisConnection, RedisModule, RedisOccurrenceLock, RedisScheduleModule,
};
use nest_rs_schedule::{
    Occurrence, OccurrenceClaim, OccurrenceLock, ScheduleModule, Scheduler, scheduled,
};
use nest_rs_testing::{LogCapture, TestApp, TransportHandle};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

/// Where every claim lives — the layout the schedule page states and an
/// operator scans, read here as written there so that moving it fails.
const CLAIMS: &str = "nestrs:schedule:claims";

/// Where every run lease lives, the same way.
const LEASES: &str = "nestrs:schedule:leases";

/// The identity of a job declared in `origin`'s module: its path, a level per
/// `::`, then `provider:method` — as the page states it.
fn identity(origin: &str, provider_method: &str) -> String {
    format!("{}:{provider_method}", origin.replace("::", ":"))
}

/// The identity of a job this module declares.
fn job(provider_method: &str) -> String {
    identity(module_path!(), provider_method)
}

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

/// The claims of the job `job` identifies on occurrences at or after
/// `since_ms` — a run before this one leaves claims for a minute, and they are
/// not this run's.
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
    await_claims(&conn, &job("WiredTasks:sweep"), started_ms, 3).await;
    scheduler.shutdown().await.expect("clean shutdown");

    let claims = claims_since(&conn, &job("WiredTasks:sweep"), started_ms).await;
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
            .strip_prefix(&format!("{CLAIMS}:{}:", job("WiredTasks:sweep")))
            .and_then(|ms| ms.parse::<u64>().ok())
            .unwrap_or_else(|| panic!("`{key}` is the claims structure, then the port's token"));
        assert_eq!(instant % 250, 0, "an instant on the epoch's period: {key}");
        let holder: String = redis::cmd("GET")
            .arg(key)
            .query_async(&mut conn.clone())
            .await
            .expect("read the claim");
        let (process, run) = holder
            .split_once('/')
            .unwrap_or_else(|| panic!("the claim records who holds it: {holder}"));
        assert!(!process.is_empty(), "the process: {holder}");
        assert_eq!(run.len(), 32, "the run's trace id: {holder}");
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
    let lease: bool = redis::cmd("EXISTS")
        .arg(format!("{LEASES}:{}", job("WiredTasks:sweep")))
        .query_async(&mut conn.clone())
        .await
        .expect("read the lease");
    assert!(!lease, "each run gave the job's lease back once it ended");
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
    await_claims(&conn, &job("ReplicatedTasks:sweep"), both_up_ms, 4).await;
    first.shutdown().await.expect("clean shutdown");
    second.shutdown().await.expect("clean shutdown");

    let contested = claims_since(&conn, &job("ReplicatedTasks:sweep"), both_up_ms).await;
    assert!(
        contested.len() >= 4,
        "occurrences were claimed with both replicas up, within {WITHIN:?}: {contested:?}"
    );
    let claims = claims_since(&conn, &job("ReplicatedTasks:sweep"), started_ms).await;
    assert_eq!(
        REPLICATED_RUNS.load(Ordering::SeqCst),
        claims.len(),
        "each occurrence ran once across the two replicas, not once on each: {claims:?}"
    );
    // The loser meets the winner's lease while the run lasts, and its claim
    // once the run has given the lease back: either way it says so.
    let lost = [
        "occurrence left to another replica, which is running the job",
        "occurrence claimed by another replica",
    ]
    .into_iter()
    .flat_map(|message| logs.find(nest_rs_schedule::TARGET, message))
    .filter(|event| event.field("provider").as_deref() == Some("ReplicatedTasks"))
    .count();
    assert!(
        lost >= 1,
        "the replica that lost a contested occurrence says so, and none did"
    );

    let foreign: Vec<String> = every_key(&conn)
        .await
        .into_iter()
        .filter(|key| {
            !key.starts_with(&format!("{CLAIMS}:")) && !key.starts_with(&format!("{LEASES}:"))
        })
        .collect();
    assert!(
        foreign.is_empty(),
        "the occurrence lock writes claims and leases and nothing else: {foreign:?}"
    );
}

// --- 3. a run holds the job ----------------------------------------------------

static OVERLAP_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
static OVERLAP_MOST: AtomicUsize = AtomicUsize::new(0);
static OVERLAP_RUNS: AtomicUsize = AtomicUsize::new(0);

#[injectable]
#[derive(Default)]
struct OverlapTasks;

#[scheduled]
impl OverlapTasks {
    #[every("250ms", replicas = "one")]
    async fn purge(&self) -> anyhow::Result<()> {
        let in_flight = OVERLAP_IN_FLIGHT.fetch_add(1, Ordering::SeqCst) + 1;
        OVERLAP_MOST.fetch_max(in_flight, Ordering::SeqCst);
        OVERLAP_RUNS.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(1_200)).await;
        OVERLAP_IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    }
}

#[module(
    imports = [ScheduleModule, RedisModule::for_root(None), RedisScheduleModule],
    providers = [OverlapTasks],
)]
struct OverlapModule;

/// The audit's probe, kept: three replicas of a job whose run lasts nearly five
/// periods. Keyed per occurrence alone, each idle replica claimed the next
/// instant and three runs were in flight at once; the run lease holds the job on
/// every replica, so there is never more than one. While a run lasts its lease
/// is in Redis under the job's identity, held by the run and expiring within the
/// lease's thirty seconds, and once every replica stops no lease is left.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_outlasting_its_period_holds_the_job_on_every_replica() {
    let conn = schedule_connection().await;
    let lease_key = format!("{LEASES}:{}", job("OverlapTasks:purge"));
    let replicas = [
        schedule_replica::<OverlapModule>(schedule_config()).await,
        schedule_replica::<OverlapModule>(schedule_config()).await,
        schedule_replica::<OverlapModule>(schedule_config()).await,
    ];
    crate::wait_until(WITHIN, || OVERLAP_IN_FLIGHT.load(Ordering::SeqCst) == 1).await;
    let held: Option<String> = redis::cmd("GET")
        .arg(&lease_key)
        .query_async(&mut conn.clone())
        .await
        .expect("read the lease");
    let left: i64 = redis::cmd("PTTL")
        .arg(&lease_key)
        .query_async(&mut conn.clone())
        .await
        .expect("read the lease's expiry");
    tokio::time::sleep(Duration::from_secs(3)).await;
    for replica in replicas {
        replica.shutdown().await.expect("clean shutdown");
    }

    assert!(
        OVERLAP_RUNS.load(Ordering::SeqCst) >= 2,
        "the job ran, back to back"
    );
    assert_eq!(
        OVERLAP_MOST.load(Ordering::SeqCst),
        1,
        "no two runs of a job firing once overlap, on any replica"
    );
    assert!(
        held.as_deref().is_some_and(|holder| holder.contains('/')),
        "a running job's lease names the run holding it: {held:?}"
    );
    assert!(
        (1..=30_000).contains(&left),
        "the lease lapses within its thirty seconds unless renewed, it has {left} ms left"
    );
    let lingering: bool = redis::cmd("EXISTS")
        .arg(&lease_key)
        .query_async(&mut conn.clone())
        .await
        .expect("read the lease");
    assert!(!lingering, "the last run gave the job's lease back");
}

// --- 4. two apps ----------------------------------------------------------------

/// The API's own maintenance job.
mod api {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use nest_rs_core::{injectable, module};
    use nest_rs_redis::{RedisModule, RedisScheduleModule};
    use nest_rs_schedule::{ScheduleModule, scheduled};

    pub(super) static RUNS: AtomicUsize = AtomicUsize::new(0);

    #[injectable]
    #[derive(Default)]
    pub(super) struct MaintenanceTasks;

    #[scheduled]
    impl MaintenanceTasks {
        #[every("250ms", replicas = "one")]
        async fn sweep(&self) -> anyhow::Result<()> {
            RUNS.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[module(
        imports = [ScheduleModule, RedisModule::for_root(None), RedisScheduleModule],
        providers = [MaintenanceTasks],
    )]
    pub(super) struct ApiModule;
}

/// The worker's own maintenance job — the same provider and method names, in
/// another app of the same deployment.
mod worker {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use nest_rs_core::{injectable, module};
    use nest_rs_redis::{RedisModule, RedisScheduleModule};
    use nest_rs_schedule::{ScheduleModule, scheduled};

    pub(super) static RUNS: AtomicUsize = AtomicUsize::new(0);

    #[injectable]
    #[derive(Default)]
    pub(super) struct MaintenanceTasks;

    #[scheduled]
    impl MaintenanceTasks {
        #[every("250ms", replicas = "one")]
        async fn sweep(&self) -> anyhow::Result<()> {
            RUNS.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[module(
        imports = [ScheduleModule, RedisModule::for_root(None), RedisScheduleModule],
        providers = [MaintenanceTasks],
    )]
    pub(super) struct WorkerModule;
}

/// The instants among `claims`.
fn instants(claims: &[String]) -> std::collections::BTreeSet<u64> {
    claims
        .iter()
        .filter_map(|key| key.rsplit(':').next()?.parse().ok())
        .collect()
}

/// Two apps of one deployment share its Redis on purpose, and each declares its
/// own `MaintenanceTasks::sweep` — legal, and the naming law's own shape. Keyed
/// on `Provider:method`, they claimed each other's occurrences: the audit
/// measured twelve claims split four to eight, each app's job running on a
/// fraction of its schedule with nothing said above `debug`. Keyed on the path
/// that declared each, they are two jobs: each claims every occurrence of its
/// own, under its own key, and runs each one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_apps_declaring_same_named_jobs_each_run_every_occurrence_of_their_own() {
    let conn = schedule_connection().await;
    let api_job = identity(
        &format!("{}::api", module_path!()),
        "MaintenanceTasks:sweep",
    );
    let worker_job = identity(
        &format!("{}::worker", module_path!()),
        "MaintenanceTasks:sweep",
    );
    let started_ms = now_ms();
    let api = schedule_replica::<api::ApiModule>(schedule_config()).await;
    let worker = schedule_replica::<worker::WorkerModule>(schedule_config()).await;
    let both_up_ms = now_ms();
    await_claims(&conn, &api_job, both_up_ms, 4).await;
    await_claims(&conn, &worker_job, both_up_ms, 4).await;
    api.shutdown().await.expect("clean shutdown");
    worker.shutdown().await.expect("clean shutdown");

    let api_claims = claims_since(&conn, &api_job, started_ms).await;
    let worker_claims = claims_since(&conn, &worker_job, started_ms).await;
    assert_eq!(
        api::RUNS.load(Ordering::SeqCst),
        api_claims.len(),
        "the API ran every occurrence it claimed: {api_claims:?}"
    );
    assert_eq!(
        worker::RUNS.load(Ordering::SeqCst),
        worker_claims.len(),
        "the worker ran every occurrence it claimed: {worker_claims:?}"
    );
    let (api_since, worker_since) = (
        instants(&claims_since(&conn, &api_job, both_up_ms).await),
        instants(&claims_since(&conn, &worker_job, both_up_ms).await),
    );
    let common: Vec<_> = api_since.intersection(&worker_since).collect();
    assert!(
        common.len() >= 3,
        "with both apps up, each claimed the same instants under its own key: \
         {api_since:?} / {worker_since:?}"
    );
}

// --- 5. the claim ---------------------------------------------------------------

/// The claim itself: the lease is asked first and the claim second, in one
/// script — a lease another run holds leaves the occurrence unclaimed; the first
/// claim wins and takes the lease; each key carries its own expiry; only the run
/// holding the lease renews or releases it — so the keys are the whole of the
/// coordination.
#[tokio::test]
async fn the_claim_takes_the_lease_with_it_and_the_keys_are_the_whole_claim() {
    let conn = schedule_connection().await;
    let lock = RedisOccurrenceLock::new(conn.clone());
    let job = format!("probe:ClaimProbe:hold{}", now_ms());
    let at = |instant: u64, run: &str| Occurrence {
        job: job.clone(),
        token: format!("{job}:{instant}"),
        hold: Duration::from_secs(30),
        run: run.to_owned(),
        lease: Duration::from_secs(20),
    };
    let (first, second) = (at(1_000, "run-a"), at(1_000, "run-b"));
    let (following, following_b) = (at(2_000, "run-a"), at(2_000, "run-b"));
    let lease_key = format!("{LEASES}:{job}");
    let pttl = |key: String| {
        let conn = conn.clone();
        async move {
            redis::cmd("PTTL")
                .arg(key)
                .query_async::<i64>(&mut conn.clone())
                .await
                .expect("read an expiry")
        }
    };
    let exists = |key: String| {
        let conn = conn.clone();
        async move {
            redis::cmd("EXISTS")
                .arg(key)
                .query_async::<bool>(&mut conn.clone())
                .await
                .expect("read a key")
        }
    };

    assert!(
        !lock.claimed(&first.token).await.expect("Redis answers"),
        "nobody holds an occurrence before its first claim",
    );
    assert_eq!(
        lock.claim(&first).await.expect("Redis answers"),
        OccurrenceClaim::Claimed,
        "the first claim creates the key and takes the lease",
    );
    assert!(lock.claimed(&first.token).await.expect("Redis answers"));
    assert!(
        (1..=30_000).contains(&pttl(format!("{CLAIMS}:{}", first.token)).await),
        "the claim expires with its hold"
    );
    assert!(
        (1..=20_000).contains(&pttl(lease_key.clone()).await),
        "the lease expires with its length"
    );

    assert_eq!(
        lock.claim(&second).await.expect("Redis answers"),
        OccurrenceClaim::RunningElsewhere,
        "a run holding the lease answers first",
    );
    assert_eq!(
        lock.claim(&following_b).await.expect("Redis answers"),
        OccurrenceClaim::RunningElsewhere,
    );
    assert!(
        !lock
            .claimed(&following_b.token)
            .await
            .expect("Redis answers"),
        "an occurrence falling due while the job runs is left unclaimed, for the \
         run holding the lease to fire late",
    );

    assert!(
        !lock.renew(&second).await.expect("Redis answers"),
        "a run that does not hold the lease cannot renew it"
    );
    assert!(lock.renew(&first).await.expect("Redis answers"));
    lock.release(&second).await.expect("Redis answers");
    assert!(exists(lease_key.clone()).await, "nor release it");
    lock.release(&first).await.expect("Redis answers");
    assert!(!exists(lease_key.clone()).await, "its holder releases it");

    assert_eq!(
        lock.claim(&second).await.expect("Redis answers"),
        OccurrenceClaim::ClaimedElsewhere,
        "with the lease free, a claim on an occurrence already claimed loses",
    );
    assert!(!exists(lease_key.clone()).await, "and takes no lease");
    assert_eq!(
        lock.claim(&following).await.expect("Redis answers"),
        OccurrenceClaim::Claimed,
        "and the occurrence left unclaimed is claimed late",
    );

    redis::cmd("DEL")
        .arg(format!("{CLAIMS}:{}", first.token))
        .arg(format!("{CLAIMS}:{}", following.token))
        .arg(&lease_key)
        .query_async::<()>(&mut conn.clone())
        .await
        .expect("remove the probe's keys");
}

// --- 6. an outage ---------------------------------------------------------------

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

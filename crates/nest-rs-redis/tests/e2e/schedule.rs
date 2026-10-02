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
//! **What fired is read off the lines, never counted against the keys.** A claim
//! key exists once Redis ran the claim, which can be before the scheduler read
//! the answer; a shutdown landing between the two skips that occurrence, as the
//! port documents, and leaves its key. So each test holds every claim to its
//! ledger — fired once, or skipped by the shutdown that cut its answer — rather
//! than to a run counter, which counted that key as a run and flaked.
//!
//! Eight questions:
//!
//! 1. **Does the wiring claim in Redis?** Each occurrence fired is one key, laid
//!    out as the page states and held for the port's hold.
//! 2. **Do two replicas fire each occurrence once?** Both reach every instant,
//!    since an `#[every]` declared so ticks on the epoch, and only the replica
//!    whose claim created the key fires.
//! 3. **Does a long run hold anything?** No: a run three periods long on three
//!    replicas leaves the next occurrences to its peers, each fired once, and the
//!    runs overlap — the contract the page states.
//! 4. **Does a pinned key survive a rename?** A replica built before a rename and
//!    one built after it, pinning the identity the job had, fire each occurrence
//!    once between them.
//! 5. **Does a claim behave as the port's contract says?** First wins; each key
//!    lasts its hold and records who holds it.
//! 6. **What does an outage do?** Every occurrence whose claim Redis cannot
//!    answer is skipped with a `warn` — firing unclaimed would fire it on every
//!    replica — and the schedule claims and fires again once Redis answers.
//! 7. **What does a lost answer cost?** That occurrence and nothing more.
//! 8. **Does the ACL the page prescribes run it?** A user created exactly as the
//!    page says fires the job, and Redis refuses it nothing.

use std::collections::BTreeSet;
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
use nest_rs_testing::{CapturedEvent, LogCapture, TestApp, TransportHandle};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

/// Where every claim lives — the layout the schedule page states and an
/// operator scans, read here as written there so that moving it fails.
const CLAIMS: &str = "nestrs:schedule:claims";

/// The identity of a job this suite declares: its crate, then `provider:method`,
/// a level each — as the page states it.
fn job(provider_method: &str) -> String {
    let krate = module_path!()
        .split_once("::")
        .map_or(module_path!(), |(krate, _)| krate);
    format!("{krate}:{provider_method}")
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

/// The instant a claim key ends with.
fn instant_of(key: &str) -> Option<u64> {
    key.rsplit(':').next()?.parse().ok()
}

/// The instants the job `job` holds claims on at or after `since_ms` — a run
/// before this one leaves claims for a minute, and they are not this run's.
async fn claims_since(conn: &RedisConnection, job: &str, since_ms: u64) -> BTreeSet<u64> {
    let claims: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{CLAIMS}:{job}:*"))
        .query_async(&mut conn.clone())
        .await
        .expect("read the claims");
    claims
        .iter()
        .filter_map(|key| instant_of(key))
        .filter(|ms| *ms >= since_ms)
        .collect()
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

/// The sentence a replica files for an occurrence whose claim shutdown cut.
const SKIPPED_AT_SHUTDOWN: &str =
    "occurrence skipped: shutdown was asked for before its lock answered the claim";

/// Whether `event` was filed by one of the jobs `providers` name.
fn filed_by(event: &CapturedEvent, providers: &[&str]) -> bool {
    event
        .field("provider")
        .is_some_and(|provider| providers.contains(&provider.as_str()))
}

/// The occurrence `event` names.
fn occurrence_of(event: &CapturedEvent) -> Option<u64> {
    event.field("occurrence")?.parse().ok()
}

/// What the occurrences of the jobs `providers` name came to, read off their
/// lines: every instant a tick fired, a repeat included, and the instants a
/// shutdown skipped before their claim was answered.
struct Ledger {
    fired: Vec<u64>,
    skipped_at_shutdown: BTreeSet<u64>,
}

fn ledger(logs: &LogCapture, providers: &[&str]) -> Ledger {
    Ledger {
        fired: logs
            .find(
                nest_rs_core::operation_log::TARGET,
                nest_rs_schedule::unit::TICK.name(),
            )
            .iter()
            .filter(|line| filed_by(line, providers))
            .map(|line| {
                occurrence_of(line)
                    .unwrap_or_else(|| panic!("a job firing once names its occurrence: {line:?}"))
            })
            .collect(),
        skipped_at_shutdown: logs
            .find(nest_rs_schedule::TARGET, SKIPPED_AT_SHUTDOWN)
            .iter()
            .filter(|event| filed_by(event, providers))
            .filter_map(occurrence_of)
            .collect(),
    }
}

/// Hold every claim on `claimed` to what the jobs `providers` name did with it:
/// each one fired once, or skipped by the shutdown that cut its answer — never
/// fired twice, never fired unclaimed, never lost in silence — and no occurrence
/// of theirs skipped for any other reason.
fn assert_each_claim_fired_once(claimed: &BTreeSet<u64>, logs: &LogCapture, providers: &[&str]) {
    let Ledger {
        fired,
        skipped_at_shutdown,
    } = ledger(logs, providers);
    let once: BTreeSet<u64> = fired.iter().copied().collect();
    assert_eq!(
        once.len(),
        fired.len(),
        "an occurrence fired twice: {fired:?}"
    );
    let unclaimed: Vec<_> = once.difference(claimed).collect();
    assert!(unclaimed.is_empty(), "fired without a claim: {unclaimed:?}");
    let lost: Vec<_> = claimed
        .iter()
        .filter(|instant| !once.contains(instant) && !skipped_at_shutdown.contains(instant))
        .collect();
    assert!(
        lost.is_empty(),
        "claimed, neither fired nor skipped at shutdown: {lost:?} — fired {fired:?}, skipped \
         {skipped_at_shutdown:?}"
    );
    let other_skips: Vec<_> = logs
        .events()
        .into_iter()
        .filter(|event| {
            event.message.starts_with("occurrence skipped")
                && event.message != SKIPPED_AT_SHUTDOWN
                && filed_by(event, providers)
        })
        .collect();
    assert!(
        other_skips.is_empty(),
        "skipped otherwise: {other_skips:#?}"
    );
}

/// The keys on the schedule's database that are not claims.
async fn foreign_keys(conn: &RedisConnection) -> Vec<String> {
    every_key(conn)
        .await
        .into_iter()
        .filter(|key| !key.starts_with(&format!("{CLAIMS}:")))
        .collect()
}

// --- 1. the documented wiring --------------------------------------------------

#[injectable]
#[derive(Default)]
struct WiredTasks;

#[scheduled]
impl WiredTasks {
    #[every("250ms", replicas = "one")]
    async fn sweep(&self) -> anyhow::Result<()> {
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
/// port's token verbatim, holding who claimed it and the trace of the run that
/// fired it, for the port's hold. The boot line names the identity a `key` would
/// pin.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_documented_wiring_claims_each_occurrence_it_fires_in_redis() {
    let logs = LogCapture::install_global();
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
    assert_each_claim_fired_once(&claims, &logs, &["WiredTasks"]);
    let ticks: Vec<_> = logs
        .find(
            nest_rs_core::operation_log::TARGET,
            nest_rs_schedule::unit::TICK.name(),
        )
        .into_iter()
        .filter(|line| filed_by(line, &["WiredTasks"]))
        .collect();
    for instant in &claims {
        assert_eq!(
            instant % 250,
            0,
            "an instant on the epoch's period: {instant}"
        );
        let key = format!("{CLAIMS}:{}:{instant}", job("WiredTasks:sweep"));
        let holder: String = redis::cmd("GET")
            .arg(&key)
            .query_async(&mut conn.clone())
            .await
            .expect("read the claim");
        let (process, run) = holder
            .split_once('/')
            .unwrap_or_else(|| panic!("the claim records who holds it: {holder}"));
        assert!(!process.is_empty(), "the process: {holder}");
        assert_eq!(run.len(), 32, "the run's trace id: {holder}");
        if let Some(tick) = ticks
            .iter()
            .find(|tick| occurrence_of(tick) == Some(*instant))
        {
            assert_eq!(
                tick.trace_id.as_deref(),
                Some(run),
                "the claim records the trace of the run that fired it"
            );
        }
        let left: i64 = redis::cmd("PTTL")
            .arg(&key)
            .query_async(&mut conn.clone())
            .await
            .expect("read the claim's expiry");
        assert!(
            (1..=60_000).contains(&left),
            "the claim expires within the port's one-minute hold, it has {left} ms left"
        );
    }
    let booted = logs.expect_one(nest_rs_schedule::TARGET, "scheduled job (interval)");
    assert_eq!(
        booted.field("key"),
        Some(job("WiredTasks:sweep").replace(':', "::")),
        "the boot names the identity a key would pin"
    );
}

// --- 2. two replicas ------------------------------------------------------------

#[injectable]
#[derive(Default)]
struct ReplicatedTasks;

#[scheduled]
impl ReplicatedTasks {
    #[every("250ms", replicas = "one")]
    async fn sweep(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[module(
    imports = [ScheduleModule, RedisModule::for_root(None), RedisScheduleModule],
    providers = [ReplicatedTasks],
)]
struct ReplicatedModule;

/// Two replicas sharing one Redis fire each occurrence once between them: every
/// claim fired once, every occurrence after both are up was contested — the
/// replica that lost says so — and the database holds claims and nothing else.
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
    assert_each_claim_fired_once(&claims, &logs, &["ReplicatedTasks"]);
    let lost = logs
        .find(
            nest_rs_schedule::TARGET,
            "occurrence claimed by another replica",
        )
        .into_iter()
        .filter(|event| filed_by(event, &["ReplicatedTasks"]))
        .count();
    assert!(
        lost >= 1,
        "the replica that lost a contested occurrence says so, and none did"
    );
    let foreign = foreign_keys(&conn).await;
    assert!(
        foreign.is_empty(),
        "the occurrence lock writes claims and nothing else: {foreign:?}"
    );
}

// --- 3. a long run holds nothing ------------------------------------------------

static OVERLAP_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
static OVERLAP_MOST: AtomicUsize = AtomicUsize::new(0);

#[injectable]
#[derive(Default)]
struct OverlapTasks;

#[scheduled]
impl OverlapTasks {
    #[every("250ms", replicas = "one")]
    async fn purge(&self) -> anyhow::Result<()> {
        let in_flight = OVERLAP_IN_FLIGHT.fetch_add(1, Ordering::SeqCst) + 1;
        OVERLAP_MOST.fetch_max(in_flight, Ordering::SeqCst);
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

/// Three replicas of a job whose run lasts nearly five periods. Once per
/// occurrence is all `replicas = "one"` holds to: a run holds nothing past its
/// claim, so the idle replicas claim and fire the next occurrences while it goes
/// on — each once — and the runs overlap across replicas, as the page and the
/// port state. Holding the job as well took a run lease, and every way that
/// lease failed held the job on every replica for half a minute.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_outlasting_its_period_leaves_the_next_occurrences_to_its_peers() {
    let logs = LogCapture::install_global();
    let conn = schedule_connection().await;
    let started_ms = now_ms();
    let replicas = [
        schedule_replica::<OverlapModule>(schedule_config()).await,
        schedule_replica::<OverlapModule>(schedule_config()).await,
        schedule_replica::<OverlapModule>(schedule_config()).await,
    ];
    crate::wait_until(WITHIN, || OVERLAP_MOST.load(Ordering::SeqCst) >= 2).await;
    for replica in replicas {
        replica.shutdown().await.expect("clean shutdown");
    }

    assert!(
        OVERLAP_MOST.load(Ordering::SeqCst) >= 2,
        "a peer fired the next occurrence while a run went on"
    );
    let claims = claims_since(&conn, &job("OverlapTasks:purge"), started_ms).await;
    assert!(claims.len() >= 2, "{claims:?}");
    assert_each_claim_fired_once(&claims, &logs, &["OverlapTasks"]);
    let foreign = foreign_keys(&conn).await;
    assert!(
        foreign.is_empty(),
        "a run leaves no key behind: {foreign:?}"
    );
}

// --- 4. a rename pinned by `key` ------------------------------------------------

/// The job as the release before a rename declares it: `InvoiceTasks::close_day`,
/// whose identity is derived — this crate, the type, the method.
mod before {
    use nest_rs_core::{injectable, module};
    use nest_rs_redis::{RedisModule, RedisScheduleModule};
    use nest_rs_schedule::{ScheduleModule, scheduled};

    #[injectable]
    #[derive(Default)]
    pub(super) struct InvoiceTasks;

    #[scheduled]
    impl InvoiceTasks {
        #[every("250ms", replicas = "one")]
        async fn close_day(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[module(
        imports = [ScheduleModule, RedisModule::for_root(None), RedisScheduleModule],
        providers = [InvoiceTasks],
    )]
    pub(super) struct BeforeModule;
}

/// The same job after the rename — another type, another method, another
/// module — pinning the identity it had.
mod after {
    use nest_rs_core::{injectable, module};
    use nest_rs_redis::{RedisModule, RedisScheduleModule};
    use nest_rs_schedule::{ScheduleModule, scheduled};

    #[injectable]
    #[derive(Default)]
    pub(super) struct LedgerTasks;

    #[scheduled]
    impl LedgerTasks {
        #[every("250ms", replicas = "one", key = "e2e::InvoiceTasks::close_day")]
        async fn close(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[module(
        imports = [ScheduleModule, RedisModule::for_root(None), RedisScheduleModule],
        providers = [LedgerTasks],
    )]
    pub(super) struct AfterModule;
}

/// A rename starts a new job, and during the rolling deploy that ships it the
/// old replicas and the new would each fire every occurrence. `key` pins the
/// identity the job had: a replica of each build claims the same keys, and the
/// two fire each occurrence once between them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_renamed_job_pinning_its_key_shares_its_occurrences_with_the_old_build() {
    let logs = LogCapture::install_global();
    let conn = schedule_connection().await;
    let pinned = job("InvoiceTasks:close_day");
    let started_ms = now_ms();
    let old = schedule_replica::<before::BeforeModule>(schedule_config()).await;
    let new = schedule_replica::<after::AfterModule>(schedule_config()).await;
    let both_up_ms = now_ms();
    await_claims(&conn, &pinned, both_up_ms, 6).await;
    old.shutdown().await.expect("clean shutdown");
    new.shutdown().await.expect("clean shutdown");

    let claims = claims_since(&conn, &pinned, started_ms).await;
    assert!(
        claims_since(&conn, &pinned, both_up_ms).await.len() >= 6,
        "both builds up, within {WITHIN:?}: {claims:?}"
    );
    assert_each_claim_fired_once(&claims, &logs, &["InvoiceTasks", "LedgerTasks"]);
    assert!(
        claims_since(&conn, &job("LedgerTasks:close"), started_ms)
            .await
            .is_empty(),
        "the renamed job claims under its key, never under its new name"
    );
}

// --- 5. the claim ---------------------------------------------------------------

/// The claim itself: the first claim on an occurrence wins and every later one
/// loses while it is held; the key carries its own expiry and records the
/// holder and the run; another occurrence is a key of its own — so the keys are
/// the whole of the coordination.
#[tokio::test]
async fn the_first_claim_wins_and_the_key_is_the_whole_claim() {
    let conn = schedule_connection().await;
    let lock = RedisOccurrenceLock::new(conn.clone());
    let job = format!("probe:ClaimProbe:hold{}", now_ms());
    let at = |instant: u64, run: &str| Occurrence {
        token: format!("{job}:{instant}"),
        hold: Duration::from_secs(30),
        run: run.to_owned(),
    };
    let (first, second, following) = (at(1_000, "run-a"), at(1_000, "run-b"), at(2_000, "run-b"));

    assert!(
        !lock.claimed(&first.token).await.expect("Redis answers"),
        "nobody holds an occurrence before its first claim",
    );
    assert_eq!(
        lock.claim(&first).await.expect("Redis answers"),
        OccurrenceClaim::Claimed,
        "the first claim creates the key",
    );
    assert!(lock.claimed(&first.token).await.expect("Redis answers"));
    assert_eq!(
        lock.claim(&second).await.expect("Redis answers"),
        OccurrenceClaim::ClaimedElsewhere,
        "a claim on an occurrence already claimed loses",
    );
    let key = format!("{CLAIMS}:{}", first.token);
    let holder: String = redis::cmd("GET")
        .arg(&key)
        .query_async(&mut conn.clone())
        .await
        .expect("read the claim");
    assert!(
        holder.ends_with("/run-a"),
        "the claim records its winner, never a later claimer: {holder}"
    );
    let left: i64 = redis::cmd("PTTL")
        .arg(&key)
        .query_async(&mut conn.clone())
        .await
        .expect("read an expiry");
    assert!(
        (1..=30_000).contains(&left),
        "the claim expires with its hold"
    );
    assert_eq!(
        lock.claim(&following).await.expect("Redis answers"),
        OccurrenceClaim::Claimed,
        "the next occurrence is a key of its own",
    );

    redis::cmd("DEL")
        .arg(&key)
        .arg(format!("{CLAIMS}:{}", following.token))
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

/// A replica's Redis: the schedule's database, through a proxy at `url`, with a
/// one-second budget — so a claim Redis cannot answer is skipped within a second.
fn proxied_config(url: String) -> RedisConfig {
    RedisConfig {
        url,
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

/// The skips `provider` filed for a claim Redis could not answer.
fn unanswered(logs: &LogCapture, provider: &str) -> Vec<CapturedEvent> {
    logs.find(
        nest_rs_schedule::TARGET,
        "occurrence skipped: its lock could not be claimed",
    )
    .into_iter()
    .filter(|event| filed_by(event, &[provider]))
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
    let scheduler =
        schedule_replica::<OutageModule>(proxied_config(proxy.url_on(crate::DB_SCHEDULE))).await;
    crate::wait_until(WITHIN, || OUTAGE_RUNS.load(Ordering::SeqCst) >= 1).await;
    assert!(
        OUTAGE_RUNS.load(Ordering::SeqCst) >= 1,
        "the job fires through the proxy before the outage"
    );

    proxy.sever();
    crate::wait_until(WITHIN, || !unanswered(&logs, "OutageTasks").is_empty()).await;
    let first_skips = unanswered(&logs, "OutageTasks").len();
    assert!(first_skips >= 1, "an occurrence lost to the outage is said");
    let runs_in_outage = OUTAGE_RUNS.load(Ordering::SeqCst);
    crate::wait_until(WITHIN, || {
        unanswered(&logs, "OutageTasks").len() >= first_skips + 2
    })
    .await;
    assert!(
        unanswered(&logs, "OutageTasks").len() >= first_skips + 2,
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

    let skip = unanswered(&logs, "OutageTasks").remove(0);
    assert_eq!(skip.level, "warn");
    assert_eq!(skip.field("method").as_deref(), Some("sweep"));
    assert!(
        occurrence_of(&skip).is_some_and(|ms| ms % 250 == 0),
        "names the occurrence it skipped: {skip:?}"
    );
    assert!(
        skip.field("error").is_some_and(|error| !error.is_empty()),
        "names why Redis could not answer: {skip:?}"
    );
}

// --- 7. a lost answer -------------------------------------------------------------

/// A proxy in front of the dev container Redis that, while stalled, still carries
/// every command to Redis and holds every reply back, then lets them through in
/// order once resumed — a command that runs while its answer is lost to its
/// caller, without reordering the replies a multiplexed connection matches by
/// position.
struct StallingProxy {
    addr: SocketAddr,
    stalled: watch::Sender<bool>,
}

impl StallingProxy {
    async fn start() -> Self {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let upstream = crate::redis_address();
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the proxy");
        let addr = listener.local_addr().expect("the proxy's address");
        let stalled = watch::channel(false).0;
        let watching = stalled.subscribe();
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
                let mut resumed = watching.clone();
                tokio::spawn(async move {
                    let mut chunk = [0_u8; 16 * 1024];
                    while let Ok(read) = from_server.read(&mut chunk).await {
                        if read == 0
                            || resumed.wait_for(|stalled| !*stalled).await.is_err()
                            || to_client.write_all(&chunk[..read]).await.is_err()
                        {
                            break;
                        }
                    }
                });
            }
        });
        Self { addr, stalled }
    }

    fn url_on(&self, db: u8) -> String {
        format!("redis://{}/{db}", self.addr)
    }

    fn stall(&self) {
        self.stalled.send_replace(true);
    }

    fn resume(&self) {
        self.stalled.send_replace(false);
    }
}

static STALLED_RUNS: AtomicUsize = AtomicUsize::new(0);

#[injectable]
#[derive(Default)]
struct StalledTasks;

#[scheduled]
impl StalledTasks {
    #[every("250ms", replicas = "one")]
    async fn sweep(&self) -> anyhow::Result<()> {
        STALLED_RUNS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[module(
    imports = [ScheduleModule, RedisModule::for_root(None), RedisScheduleModule],
    providers = [StalledTasks],
)]
struct StalledModule;

/// A claim Redis ran but whose answer never reached the scheduler — a reply held
/// past the connection's budget — skips its occurrence, which nobody fires, and
/// costs nothing more: the next occurrence is claimed and fired on time. A run
/// lease taken in the same step as the claim held the job on every replica, the
/// claimer included, for the half-minute it took to lapse, while every occurrence
/// meanwhile was put down to a run that did not exist.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_claim_whose_answer_is_lost_costs_that_occurrence_alone() {
    let logs = LogCapture::install_global();
    let conn = schedule_connection().await;
    let proxy = StallingProxy::start().await;
    let scheduler =
        schedule_replica::<StalledModule>(proxied_config(proxy.url_on(crate::DB_SCHEDULE))).await;
    crate::wait_until(WITHIN, || STALLED_RUNS.load(Ordering::SeqCst) >= 2).await;
    assert!(
        STALLED_RUNS.load(Ordering::SeqCst) >= 2,
        "the job fires through the proxy"
    );

    proxy.stall();
    crate::wait_until(WITHIN, || !unanswered(&logs, "StalledTasks").is_empty()).await;
    proxy.resume();
    let resumed = std::time::Instant::now();
    let runs_at_resume = STALLED_RUNS.load(Ordering::SeqCst);
    crate::wait_until(WITHIN, || {
        STALLED_RUNS.load(Ordering::SeqCst) > runs_at_resume
    })
    .await;
    let next_run_after = resumed.elapsed();
    scheduler.shutdown().await.expect("clean shutdown");

    let lost = unanswered(&logs, "StalledTasks")
        .first()
        .and_then(occurrence_of)
        .expect("a claim whose answer was held past the budget is skipped aloud");
    let ran: bool = redis::cmd("EXISTS")
        .arg(format!("{CLAIMS}:{}:{lost}", job("StalledTasks:sweep")))
        .query_async(&mut conn.clone())
        .await
        .expect("read the claim");
    assert!(ran, "Redis ran the claim whose answer was lost");
    assert!(
        !ledger(&logs, &["StalledTasks"]).fired.contains(&lost),
        "and nobody fired that occurrence — at most once"
    );
    assert!(
        next_run_after < Duration::from_secs(3),
        "the next occurrence fires on time once Redis answers, not when a lease lapses: \
         {next_run_after:?}"
    );
}

// --- 8. the documented ACL --------------------------------------------------------

#[injectable]
#[derive(Default)]
struct ConfinedTasks;

#[scheduled]
impl ConfinedTasks {
    #[every("250ms", replicas = "one")]
    async fn sweep(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[module(
    imports = [ScheduleModule, RedisModule::for_root(None), RedisScheduleModule],
    providers = [ConfinedTasks],
)]
struct ConfinedModule;

/// The ACL user this test creates from the page, and removes — named for this
/// run, so no earlier run's denial in `ACL LOG` is read as its own.
const CONFINED_USER: &str = "nestrs-e2e-schedule";

/// The user created exactly as the schedule page prescribes boots a replica and
/// fires its job: Redis checks every command a scheduler sends — the boot's
/// proof, the database's selection, each claim and each overrun question — and
/// refuses none. The ACL the page used to give allowed the scripts and not the
/// commands inside them, and every occurrence was skipped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scheduler_runs_through_exactly_the_acl_its_page_prescribes() {
    let logs = LogCapture::install_global();
    let conn = schedule_connection().await;
    let user = crate::acl_user(CONFINED_USER);
    let config = crate::documented_user("schedule/index.mdx", &user, crate::DB_SCHEDULE).await;
    let started_ms = now_ms();
    let scheduler = schedule_replica::<ConfinedModule>(config).await;
    await_claims(&conn, &job("ConfinedTasks:sweep"), started_ms, 3).await;
    scheduler.shutdown().await.expect("clean shutdown");
    crate::forget_user(&user).await;

    let claims = claims_since(&conn, &job("ConfinedTasks:sweep"), started_ms).await;
    assert!(
        claims.len() >= 3,
        "the job claimed through the ACL: {claims:?}"
    );
    assert_each_claim_fired_once(&claims, &logs, &["ConfinedTasks"]);
    let refused: Vec<_> = logs
        .events()
        .into_iter()
        .filter(crate::refused_by_acl)
        .collect();
    assert!(refused.is_empty(), "a line carries a refusal: {refused:#?}");
    crate::assert_redis_denied_nothing_but(&user, &[]).await;
}

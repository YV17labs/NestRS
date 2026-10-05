//! Where a queue lives, against a live Redis: every key a queue holds is under
//! `nestrs:queue:{<queue>}:`, and **the queue page's ACL rules are exact**.
//!
//! A producer and a worker each run as a Redis user created exactly as the page
//! prescribes for its role — reaching `nestrs:queue:*` and nothing else — through
//! a push, a delayed push, a unique push, two cancels, a completion, a long
//! attempt renewed, a retry, a throttled method, a checkpoint and a dead
//! letter. Redis's `ACL LOG` holds no denial for either, and `MONITOR` shows
//! each sent every command its rule allows and nothing else: a rule allowing a
//! command nothing sends is an opening, and one missing a command a rare path
//! sends fails in production.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt as _;
use nest_rs_core::{injectable, module};
use nest_rs_queue::{Checkpoint, JobProducerExt, PushOptions, QueueModule, processor, queue};
use nest_rs_redis::{RedisConfig, RedisConnection, RedisModule, RedisQueueModule};
use nest_rs_testing::TestApp;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::{DB_CONFINED_TO_THE_PREFIX, Runs};

/// The page prescribing the queue's rules.
const PAGE: &str = "queue/delivery.mdx";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConfinedCommand {
    run: u64,
    act: Act,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
enum Act {
    Complete,
    Linger,
    Fail,
}

static CONFINED: Runs = Runs::new();

#[queue(name = "nestrs-e2e-acl", job = ConfinedCommand)]
struct ConfinedQueue;

#[queue(name = "nestrs-e2e-acl-throttled", job = ConfinedCommand)]
struct ThrottledQueue;

#[injectable]
#[derive(Default)]
struct ConfinedProcessor;

#[processor]
impl ConfinedProcessor {
    #[process(queue = ConfinedQueue, retries = 1, transactional = false)]
    async fn run(&self, job: ConfinedCommand, mut progress: Checkpoint<u32>) -> anyhow::Result<()> {
        CONFINED.start(job.run);
        progress.save(1).await?;
        match job.act {
            Act::Complete => {}
            Act::Linger => tokio::time::sleep(crate::LEASE * 2).await,
            Act::Fail => anyhow::bail!("the upstream is gone"),
        }
        CONFINED.finish(job.run);
        Ok(())
    }

    #[process(queue = ThrottledQueue, concurrency = 4, throttle(limit = 2, window = "1s"))]
    async fn paced(&self, job: ConfinedCommand) -> anyhow::Result<()> {
        CONFINED.start(job.run);
        CONFINED.finish(job.run);
        Ok(())
    }
}

#[module(imports = [RedisModule::for_root(None), RedisQueueModule])]
struct ProducerModule;

#[module(
    imports = [RedisModule::for_root(None), RedisQueueModule, QueueModule::for_root(None)],
    providers = [ConfinedProcessor],
)]
struct WorkerModule;

/// What `MONITOR` showed reaching the suite's confined database: each
/// command, as an ACL rule names it, with the client that sent it — `lua` for
/// a command a script ran. The producer and the worker run one after the other,
/// so what one phase saw is one role's.
type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// Start reading `MONITOR` into a list, until the returned task is aborted.
async fn monitor() -> (Seen, tokio::task::JoinHandle<()>) {
    let monitor = redis::Client::open(crate::redis_url())
        .expect("the admin URL")
        .get_async_monitor()
        .await
        .expect("a monitor connection, which sends MONITOR");
    let seen: Seen = Arc::default();
    let filling = Arc::clone(&seen);
    let reading = tokio::spawn(async move {
        let mut lines = monitor.into_on_message::<String>();
        while let Some(line) = lines.next().await {
            if let Some(command) = parse(&line) {
                filling.lock().await.push(command);
            }
        }
    });
    // MONITOR answers before it streams: give it a beat to be in place.
    tokio::time::sleep(Duration::from_millis(100)).await;
    (seen, reading)
}

/// One `MONITOR` line — `<time> [<db> <client>] "<COMMAND>" "<arg>" …` — as
/// `(client, command)` when it reached the suite's confined database: run on
/// it, or the `SELECT` that switched to it. The command is written as an ACL
/// rule names it: lowercase, with its subcommand for a container command.
fn parse(line: &str) -> Option<(String, String)> {
    let open = line.find('[')?;
    let close = line[open..].find(']')? + open;
    let mut source = line[open + 1..close].split(' ');
    let db = source.next()?;
    let client = source.next()?.to_owned();
    let mut words = line[close + 1..]
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_lowercase);
    let command = words.next()?;
    let first = words.next().unwrap_or_default();
    let confined = DB_CONFINED_TO_THE_PREFIX.to_string();
    let reached = db == confined || (command == "select" && first == confined);
    let command = match command.as_str() {
        "script" | "xgroup" | "xinfo" | "client" | "config" | "acl" => format!("{command}|{first}"),
        _ => command,
    };
    reached.then_some((client, command))
}

/// The commands sent to the confined database in `seen` by anyone but the
/// test's own administration — the clients of Redis's `default` user still
/// open — and by the scripts they ran, but the client library's `CLIENT
/// SETINFO`, which no rule names ([`crate::SETINFO`]).
async fn sent(seen: &Seen) -> BTreeSet<String> {
    let clients: String = redis::cmd("CLIENT")
        .arg("LIST")
        .query_async(&mut crate::connect().await)
        .await
        .expect("CLIENT LIST");
    let administration: BTreeSet<String> = clients
        .lines()
        .filter(|client| client.contains(" user=default "))
        .filter_map(|client| {
            client
                .split(' ')
                .find_map(|field| field.strip_prefix("addr="))
                .map(str::to_owned)
        })
        .collect();
    seen.lock()
        .await
        .iter()
        .filter(|(client, _)| !administration.contains(client))
        .map(|(_, command)| command.clone())
        .filter(|command| !crate::SETINFO.contains(&command.as_str()))
        .collect()
}

/// The commands `rule` allows, as it writes them.
fn allowed(rule: &str) -> BTreeSet<String> {
    rule.split_whitespace()
        .filter_map(|token| token.strip_prefix('+'))
        .map(str::to_owned)
        .collect()
}

/// The page's producer and worker rules, each run by its role through every
/// path the binding has, are exact: Redis denies neither user anything, and
/// each sends every command its rule allows — but `SCRIPT LOAD`, which a
/// producer sends only to a Redis that has not cached the script it calls, and
/// which is asked of it separately.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_queue_pages_rules_are_exact_for_a_producer_and_a_worker() {
    let mut admin = RedisConnection::connect(&crate::redis_config_on(DB_CONFINED_TO_THE_PREFIX))
        .await
        .expect("the admin connection");
    let _: () = redis::cmd("FLUSHDB")
        .query_async(&mut admin)
        .await
        .expect("FLUSHDB");
    // The cancel of a job no worker took reads the group, which the first
    // worker makes: made here, so the producer's run is the producer's alone.
    let _: () = redis::cmd("XGROUP")
        .arg("CREATE")
        .arg(crate::key_of(
            <ConfinedQueue as nest_rs_queue::Queue>::NAME,
            "jobs",
        ))
        .arg("workers")
        .arg("0")
        .arg("MKSTREAM")
        .query_async(&mut admin)
        .await
        .expect("XGROUP CREATE");
    let producer_user = crate::acl_user("nestrs-e2e-producer");
    let worker_user = crate::acl_user("nestrs-e2e-worker");
    let as_producer =
        crate::documented_user(PAGE, "producer", &producer_user, DB_CONFINED_TO_THE_PREFIX).await;
    let as_worker =
        crate::documented_user(PAGE, "worker", &worker_user, DB_CONFINED_TO_THE_PREFIX).await;
    crate::assert_may_load_a_script(&as_producer).await;
    let run = crate::this_run();

    // The producer alone: its boot, then every push and cancel it has.
    let (seen, reading) = monitor().await;
    let producer = producer_on(as_producer).await;
    let job = |offset: u64, act: Act| ConfinedCommand {
        run: run + offset,
        act,
    };
    let waiting = producer
        .push(
            ConfinedQueue,
            job(100, Act::Complete),
            PushOptions::default().with_unique("one"),
        )
        .await
        .expect("a unique push");
    let held = producer
        .push(
            ConfinedQueue,
            job(101, Act::Complete),
            PushOptions::default().with_delay(Duration::from_secs(60)),
        )
        .await
        .expect("a delayed push");
    assert!(producer.cancel(&waiting).await.expect("a cancel"));
    assert!(producer.cancel(&held).await.expect("a cancel"));
    producer
        .push(ConfinedQueue, job(0, Act::Complete), None)
        .await
        .expect("a push");
    producer
        .push(ConfinedQueue, job(1, Act::Linger), None)
        .await
        .expect("a push");
    producer
        .push(ConfinedQueue, job(2, Act::Fail), None)
        .await
        .expect("a push");
    for paced in 3..6 {
        producer
            .push(ThrottledQueue, job(paced, Act::Complete), None)
            .await
            .expect("a push");
    }
    let producer_sent = sent(&seen).await;
    seen.lock().await.clear();

    // The worker alone: its boot, every job to its end, and its stop.
    let worker = crate::replica_on::<WorkerModule>(as_worker).await;
    crate::wait_until(Duration::from_secs(30), || {
        [0, 1, 3, 4, 5]
            .iter()
            .all(|at| CONFINED.finished(run + at) == 1)
            && CONFINED.of(run + 2).len() == 2
    })
    .await;
    let dead: i64 = redis::cmd("XLEN")
        .arg(crate::key_of(
            <ConfinedQueue as nest_rs_queue::Queue>::NAME,
            "dead",
        ))
        .query_async(&mut admin)
        .await
        .expect("XLEN");
    worker.worker.shutdown().await.expect("clean shutdown");
    let worker_sent = sent(&seen).await;
    reading.abort();

    crate::assert_redis_denied_nothing_but(&producer_user, &[]).await;
    crate::assert_redis_denied_nothing_but(&worker_user, &[]).await;
    crate::forget_user(&producer_user).await;
    crate::forget_user(&worker_user).await;

    assert_eq!(dead, 1, "the failing job dead-lettered");
    let producer_rule = allowed(&crate::documented_acl(PAGE, "producer"));
    let worker_rule = allowed(&crate::documented_acl(PAGE, "worker"));
    let unsent: BTreeSet<_> = producer_rule.difference(&producer_sent).cloned().collect();
    assert!(
        unsent.iter().all(|command| command == "script|load"),
        "the producer's rule allows what no producer sends: {unsent:?}",
    );
    let unallowed: BTreeSet<_> = producer_sent.difference(&producer_rule).cloned().collect();
    assert!(
        unallowed.is_empty(),
        "a producer sends what its rule does not allow: {unallowed:?}"
    );
    assert_eq!(
        worker_sent, worker_rule,
        "the worker sends exactly what its rule allows"
    );
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg("*")
        .query_async(&mut admin)
        .await
        .expect("KEYS");
    assert!(
        keys.iter()
            .all(|key| key.starts_with("nestrs:queue:{nestrs-e2e-acl")),
        "every key is under the queues' own: {keys:?}",
    );
}

/// A producer-only app reaching Redis as `redis` says.
async fn producer_on(redis: RedisConfig) -> nest_rs_redis::RedisQueueProducer {
    let app = TestApp::builder()
        .provide(redis)
        .module::<ProducerModule>()
        .build_headless()
        .await
        .expect("a producer-only app boots as the page's producer");
    let producer = nest_rs_redis::RedisQueueProducer::clone(
        &app.container()
            .get::<nest_rs_redis::RedisQueueProducer>()
            .expect("the producer binding"),
    );
    Box::leak(Box::new(app));
    producer
}

/// The page's KEDA rule reads a queue's length, and nothing else.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_queue_pages_keda_rule_reads_a_length_and_writes_nothing() {
    let user = crate::acl_user("nestrs-e2e-keda");
    let config = crate::documented_user(PAGE, "KEDA", &user, 0).await;
    let mut keda = redis::Client::open(config.url.as_str())
        .expect("the KEDA user's URL")
        .get_multiplexed_async_connection()
        .await
        .expect("the KEDA user connects");
    let jobs = crate::key_of("nestrs-e2e-keda", "jobs");
    let length: i64 = redis::cmd("XLEN")
        .arg(&jobs)
        .query_async(&mut keda)
        .await
        .expect("KEDA reads a queue's length");
    assert_eq!(length, 0);
    let written: Result<String, _> = redis::cmd("XADD")
        .arg(&jobs)
        .arg("*")
        .arg("job")
        .arg("x")
        .query_async(&mut keda)
        .await;
    assert!(written.is_err(), "KEDA writes nothing");
    crate::forget_user(&user).await;
}

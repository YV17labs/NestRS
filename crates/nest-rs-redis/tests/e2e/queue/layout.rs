//! Where a queue lives, against a live Redis: every key a queue holds is under
//! `nestrs:queue:{<queue>}:`, and **the queue page's ACL rules are exact**.
//!
//! A producer and a worker each run as a Redis user created exactly as the page
//! prescribes for its role, through every path the binding has: `ACL LOG` holds
//! no denial, and `MONITOR` shows each sent every command its rule allows and
//! nothing else.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::StreamExt as _;
use nest_rs_core::{injectable, module};
use nest_rs_queue::{Checkpoint, JobProducerExt, PushOptions, QueueModule, processor, queue};
use nest_rs_redis::{RedisConnection, RedisModule, RedisQueueModule, RedisTopology};
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

#[module(
    imports = [RedisModule::for_root(None), RedisQueueModule, QueueModule::for_root(None)],
    providers = [ConfinedProcessor],
)]
struct WorkerModule;

/// What `MONITOR` showed reaching the suite's confined database. The producer
/// and the worker run one after the other, so what one phase saw is one role's.
#[derive(Default)]
struct Seen {
    /// Each command, as an ACL rule names it, with the client that sent it —
    /// `lua` for a command a script ran.
    commands: Mutex<Vec<(String, String)>>,
    /// How many [`BARRIER`]s the reader has passed.
    barriers: AtomicUsize,
}

/// What [`sent`] echoes on the confined database: Redis streams to `MONITOR`
/// in the order it runs commands, so once the reader passed it, it read every
/// command a phase ran.
const BARRIER: &str = "nestrs-e2e-monitor-barrier";

/// Start reading `MONITOR` on every primary, until the returned tasks are
/// aborted: a node streams only what it runs, and a replica what it is sent to
/// replicate. Every command run once each answers is streamed.
async fn monitor() -> (Arc<Seen>, Vec<tokio::task::JoinHandle<()>>) {
    let seen = Arc::new(Seen::default());
    let primaries = crate::primaries().await;
    let monitors =
        futures_util::future::join_all(primaries.iter().map(redis::Client::get_async_monitor))
            .await;
    let mut reading = Vec::new();
    for monitor in monitors {
        let monitor = monitor.expect("a monitor connection, which sends MONITOR");
        let filling = Arc::clone(&seen);
        reading.push(tokio::spawn(async move {
            let mut lines = monitor.into_on_message::<String>();
            while let Some(line) = lines.next().await {
                if line.contains(BARRIER) {
                    filling.barriers.fetch_add(1, Ordering::SeqCst);
                } else if let Some(command) = parse(&line) {
                    filling.commands.lock().await.push(command);
                }
            }
        }));
    }
    (seen, reading)
}

/// One `MONITOR` line — `<time> [<db> <client>] "<COMMAND>" "<arg>" …` — as
/// `(client, command)` when it reached the suite's confined database: run on
/// it, or the `SELECT` that switched to it. The command is written as an ACL
/// rule names it: lowercase, with its subcommand for a container command.
fn parse(line: &str) -> Option<(String, String)> {
    let open = line.find('[')?;
    // The source's own bracket, never one inside an IPv6 client's address.
    let close = line[open..].find("] \"")? + open;
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
        "script" | "xgroup" | "xinfo" | "client" | "config" | "acl" | "cluster" => {
            format!("{command}|{first}")
        }
        _ => command,
    };
    reached.then_some((client, command))
}

/// The commands sent to the confined database in `seen` by anyone but the
/// test's own administration — the clients of Redis's `default` user still
/// open — and by the scripts they ran, once the reader caught up with what
/// every data node ran last.
async fn sent(seen: &Seen) -> BTreeSet<String> {
    let passed = seen.barriers.load(Ordering::SeqCst);
    let echoed =
        crate::on_each::<String>(&crate::primaries().await, redis::cmd("ECHO").arg(BARRIER))
            .await
            .len();
    let caught_up = || seen.barriers.load(Ordering::SeqCst) >= passed + echoed;
    crate::wait_until(Duration::from_secs(10), caught_up).await;
    assert!(caught_up(), "MONITOR streams what ran before the barrier");
    let clients: Vec<String> = crate::on_every_node(redis::cmd("CLIENT").arg("LIST")).await;
    let administration: BTreeSet<String> = clients
        .iter()
        .flat_map(|clients| clients.lines())
        .filter(|client| client.contains(" user=default "))
        .filter_map(|client| {
            client
                .split(' ')
                .find_map(|field| field.strip_prefix("addr="))
                .map(str::to_owned)
        })
        .collect();
    seen.commands
        .lock()
        .await
        .iter()
        .filter(|(client, _)| !administration.contains(client))
        .map(|(_, command)| command.clone())
        .collect()
}

/// The commands `rule` allows, as it writes them.
fn allowed(rule: &str) -> BTreeSet<String> {
    rule.split_whitespace()
        .filter_map(|token| token.strip_prefix('+'))
        .map(str::to_owned)
        .collect()
}

/// `SCRIPT LOAD` is asked of the producer separately: it sends one only to a
/// Redis that has not cached the script it calls.
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

    let (seen, reading) = monitor().await;
    let producer = crate::producer_on(as_producer).await;
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
    seen.commands.lock().await.clear();

    let worker = crate::replica_on::<WorkerModule>(as_worker).await;
    crate::wait_until(Duration::from_secs(30), || {
        [0, 1, 3, 4, 5]
            .iter()
            .all(|at| CONFINED.finished(run + at) == 1)
            && CONFINED.of(run + 2).len() == 2
    })
    .await;
    worker.worker.shutdown().await.expect("clean shutdown");
    let dead: i64 = redis::cmd("XLEN")
        .arg(crate::key_of(
            <ConfinedQueue as nest_rs_queue::Queue>::NAME,
            "dead",
        ))
        .query_async(&mut admin)
        .await
        .expect("XLEN");
    let worker_sent = sent(&seen).await;
    for reading in reading {
        reading.abort();
    }

    crate::assert_redis_denied_nothing_but(&producer_user, &[]).await;
    crate::assert_redis_denied_nothing_but(&worker_user, &[]).await;
    crate::forget_user(&producer_user).await;
    crate::forget_user(&worker_user).await;

    assert_eq!(dead, 1, "the failing job dead-lettered");
    // Each role's rule is exact; what the topology's connection adds is
    // allowed beside it, its redirection to a moving slot is the Cluster e2e's
    // to send, and `HELLO` is no rule's: the ACL never governs it.
    let mut addition = allowed(&crate::topology_addition());
    addition.insert("hello".to_owned());
    let without_addition = |sent: &BTreeSet<String>| -> BTreeSet<String> {
        sent.difference(&addition).cloned().collect()
    };
    let producer_rule = allowed(&crate::documented_acl(PAGE, "producer"));
    let worker_rule = allowed(&crate::documented_acl(PAGE, "worker"));
    let producer_sent = without_addition(&producer_sent);
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
        without_addition(&worker_sent),
        worker_rule,
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

/// How go-redis 9.22, KEDA 2.21's client, names itself on every connection.
fn go_redis_names_itself() -> [redis::Cmd; 2] {
    let mut name = redis::cmd("CLIENT");
    name.arg("SETINFO").arg("LIB-NAME").arg("go-redis(,go1.25)");
    let mut version = redis::cmd("CLIENT");
    version.arg("SETINFO").arg("LIB-VER").arg("9.22.0");
    [name, version]
}

/// What KEDA 2.21's `redis-streams` scalers send a data node for a
/// `streamLength` trigger, as go-redis 9.22 opens it: its name, the `SELECT` of
/// `databaseIndex`, the `PING` proving the connection and, on a Cluster, the
/// slots and every command's key positions — then the `XLEN` it reads.
fn keda_sends_a_node(jobs: &str) -> Vec<redis::Cmd> {
    let mut sent = go_redis_names_itself().to_vec();
    sent.push(redis::cmd("SELECT").arg(0).clone());
    sent.push(redis::cmd("PING"));
    if crate::topology() == RedisTopology::Cluster {
        sent.push(redis::cmd("CLUSTER").arg("SLOTS").clone());
        sent.push(redis::cmd("COMMAND"));
    }
    sent.push(redis::cmd("XLEN").arg(jobs).clone());
    sent
}

/// The queue page's KEDA rule, with what the topologies page adds for KEDA,
/// lets KEDA's client send everything it sends — on every data node, and on
/// the sentinels under Sentinel, subscribing to their failover channels
/// included — and write nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_queue_pages_keda_rule_takes_what_keda_sends_and_writes_nothing() {
    let user = crate::acl_user("nestrs-e2e-keda");
    let mut rule = crate::documented_acl(PAGE, "KEDA");
    if crate::topology() == RedisTopology::Cluster {
        rule = format!(
            "{rule} {}",
            crate::documented_acl(crate::CONNECTION_PAGE, "KEDA on a Cluster")
        );
    }
    crate::forget_user(&user).await;
    let _: Vec<()> = crate::on_every_node(&crate::creating(&rule, &user)).await;
    let jobs = crate::key_of("nestrs-e2e-keda", "jobs");

    let mut read = None;
    for node in crate::data_nodes().await {
        let addr = node.get_connection_info().addr().to_string();
        let mut keda = crate::bare_client(&crate::url_as(
            &crate::node_url(&addr),
            &user,
            crate::ACL_PASSWORD,
        ))
        .get_multiplexed_async_connection()
        .await
        .expect("the KEDA user connects");
        // A replica or a node serving another slot answers a redirection,
        // which comes after the ACL's verdict; the `XLEN` comes last.
        let mut answer = None;
        for cmd in keda_sends_a_node(&jobs) {
            answer = Some(cmd.query_async::<redis::Value>(&mut keda).await);
        }
        if let Some(Ok(redis::Value::Int(length))) = answer {
            read = Some(length);
        }
        let written: Result<String, _> = redis::cmd("XADD")
            .arg(&jobs)
            .arg("*")
            .arg("job")
            .arg("x")
            .query_async(&mut keda)
            .await;
        assert!(
            written.is_err_and(|refused| refused.code() == Some("NOPERM")),
            "KEDA writes nothing on {addr}"
        );
    }

    if crate::topology() == RedisTopology::Sentinel {
        let sentinel_user = crate::acl_user("nestrs-e2e-keda-sentinels");
        let sentinel_rule = crate::documented_acl(crate::CONNECTION_PAGE, "KEDA's sentinels");
        crate::forget_user_among(&crate::sentinels(), &sentinel_user).await;
        let _: Vec<()> = crate::on_each(
            &crate::sentinels(),
            &crate::creating(&sentinel_rule, &sentinel_user),
        )
        .await;
        for addr in crate::named_hosts() {
            let as_keda =
                crate::url_as(&crate::node_url(&addr), &sentinel_user, crate::ACL_PASSWORD);
            let mut sentinel = crate::bare_client(&as_keda)
                .get_multiplexed_async_connection()
                .await
                .expect("KEDA's sentinel user connects");
            let mut sent = go_redis_names_itself().to_vec();
            for question in ["GET-MASTER-ADDR-BY-NAME", "SENTINELS"] {
                sent.push(
                    redis::cmd("SENTINEL")
                        .arg(question)
                        .arg(crate::service_name())
                        .clone(),
                );
            }
            for cmd in sent {
                cmd.query_async::<redis::Value>(&mut sentinel)
                    .await
                    .unwrap_or_else(|refused| panic!("{addr} answers {cmd:?}: {refused}"));
            }
            let mut listening = crate::bare_client(&as_keda)
                .get_async_pubsub()
                .await
                .expect("KEDA's sentinel user listens");
            listening
                .subscribe(&["+switch-master", "+replica-reconf-done"])
                .await
                .expect("KEDA's sentinel user listens for a failover");
        }
        crate::assert_denied_nothing_among(&crate::sentinels(), &sentinel_user, &[]).await;
        crate::forget_user_among(&crate::sentinels(), &sentinel_user).await;
    }

    crate::assert_redis_denied_nothing_but(&user, &["xadd"]).await;
    crate::forget_user(&user).await;
    assert_eq!(read, Some(0), "KEDA reads the queue's length");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_pages_rule_replaces_what_the_user_held_before() {
    for (page, role) in [
        (PAGE, "producer"),
        (PAGE, "worker"),
        (PAGE, "KEDA"),
        (PAGE, "operator"),
        ("rate-limiting/index.mdx", "rate limiter"),
        ("schedule/index.mdx", "schedule"),
    ] {
        let user = crate::acl_user("nestrs-e2e-replaced");
        let _: Vec<()> = crate::on_every_node(
            redis::cmd("ACL")
                .arg("SETUSER")
                .arg(&user)
                .arg("on")
                .arg(format!(">{}", crate::ACL_PASSWORD))
                .arg("~*")
                .arg("+@all"),
        )
        .await;
        let _: Vec<()> =
            crate::on_every_node(&crate::creating(&crate::documented_acl(page, role), &user)).await;
        let rules: Vec<String> =
            crate::on_every_node::<Vec<redis::Value>>(redis::cmd("ACL").arg("GETUSER").arg(&user))
                .await
                .into_iter()
                .flatten()
                .filter_map(|value| redis::from_redis_value::<String>(value).ok())
                .collect();
        crate::forget_user(&user).await;
        assert!(
            !rules
                .iter()
                .any(|rule| rule.contains("~*") || rule.contains("+@all")),
            "{page}'s {role} rule leaves what the user held before: {rules:?}",
        );
    }
}

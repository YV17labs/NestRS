//! What holds on a Cluster alone, against the suite's nodes: a queue whose
//! slot moves under it, run by the users the pages prescribe — an `ASK` while
//! the slot moves, a `MOVED` once it has — and, one at a time, a real failover
//! of the node serving a running queue.

use std::time::Duration;

use nest_rs_core::{injectable, module};
use nest_rs_queue::{JobProducerExt, Queue, QueueModule, processor, queue};
use nest_rs_redis::{RedisModule, RedisQueueModule};
use serde::{Deserialize, Serialize};

use crate::{ClusterNode, Runs};

/// The slot `queue`'s keys sit in.
async fn slot_of(queue: &str) -> u16 {
    redis::cmd("CLUSTER")
        .arg("KEYSLOT")
        .arg(crate::key_of(queue, "jobs"))
        .query_async(&mut crate::connect().await)
        .await
        .expect("CLUSTER KEYSLOT")
}

/// The primary serving `slot`.
async fn owner_of(slot: u16) -> ClusterNode {
    crate::cluster_nodes()
        .await
        .into_iter()
        .find(|node| node.serves(slot))
        .expect("a primary serves every slot")
}

/// Hand `slot` to the node `id` names, on every primary.
async fn assign(slot: u16, id: &str) {
    let _: Vec<()> = crate::on_each(
        &crate::primaries().await,
        redis::cmd("CLUSTER")
            .arg("SETSLOT")
            .arg(slot)
            .arg("NODE")
            .arg(id),
    )
    .await;
}

/// How many `ASKING` the node at `addr` has run since it started.
async fn askings(addr: &str) -> u64 {
    let stats: String = redis::cmd("INFO")
        .arg("commandstats")
        .query_async(&mut crate::node(addr).await)
        .await
        .expect("INFO commandstats");
    stats
        .lines()
        .find_map(|line| line.strip_prefix("cmdstat_asking:calls="))
        .and_then(|calls| calls.split(',').next())
        .and_then(|calls| calls.parse().ok())
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MovedCommand {
    run: u64,
}

static MOVED: Runs = Runs::new();

#[queue(name = "nestrs-e2e-cluster-moved", job = MovedCommand)]
struct MovedQueue;

#[injectable]
#[derive(Default)]
struct MovedProcessor;

#[processor]
impl MovedProcessor {
    #[process(queue = MovedQueue, retries = 5)]
    async fn run(&self, job: MovedCommand) -> anyhow::Result<()> {
        MOVED.start(job.run);
        MOVED.finish(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(None), RedisQueueModule, QueueModule::for_root(None)],
    providers = [MovedProcessor],
)]
struct MovedModule;

/// A queue's slot is moved to another primary while its worker boots, by the
/// user the queue page prescribes for a worker: its first commands — the
/// group, the blocked read — reach the slot's new node through an `ASK` while
/// it moves, its pushes through a `MOVED` once it has, and Valkey denies the
/// user nothing. A script naming keys on both nodes waits until the move ends
/// (`TRYAGAIN`), which is why the pushes come after it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_queue_runs_on_while_its_slot_moves_to_another_node() {
    let queue = <MovedQueue as Queue>::NAME;
    crate::forget(queue).await;
    let slot = slot_of(queue).await;
    let source = owner_of(slot).await;
    let target = crate::cluster_nodes()
        .await
        .into_iter()
        .find(|node| node.primary && node.id != source.id)
        .expect("a second primary");
    let asked = askings(&target.addr).await;
    let _: () = redis::cmd("CLUSTER")
        .arg("SETSLOT")
        .arg(slot)
        .arg("IMPORTING")
        .arg(&source.id)
        .query_async(&mut crate::node(&target.addr).await)
        .await
        .expect("the target imports the slot");
    let _: () = redis::cmd("CLUSTER")
        .arg("SETSLOT")
        .arg(slot)
        .arg("MIGRATING")
        .arg(&target.id)
        .query_async(&mut crate::node(&source.addr).await)
        .await
        .expect("the source migrates the slot");

    let user = crate::acl_user("nestrs-e2e-moved");
    let as_worker = crate::documented_user("queue/delivery.mdx", "worker", &user, 0).await;
    let replica = crate::replica_on::<MovedModule>(as_worker).await;
    let mut asked_while_moving = 0;
    for _ in 0..100 {
        asked_while_moving = askings(&target.addr).await - asked;
        if asked_while_moving > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assign(slot, &target.id).await;
    let run = crate::this_run();
    for offset in 0..3 {
        replica
            .producer
            .push(MovedQueue, MovedCommand { run: run + offset }, None)
            .await
            .expect("a push to a slot that moved");
    }
    crate::wait_until(Duration::from_secs(10), || {
        (0..3).all(|offset| MOVED.finished(run + offset) > 0)
    })
    .await;
    replica.worker.shutdown().await.expect("clean shutdown");

    // Restored before asserting, so a failure leaves the Cluster as it found it.
    crate::forget(queue).await;
    assign(slot, &source.id).await;
    crate::assert_redis_denied_nothing_but(&user, &[]).await;
    crate::forget_user(&user).await;

    assert!(
        asked_while_moving > 0,
        "the slot's new node ran the commands an ASK sent it while the slot moved"
    );
    for offset in 0..3 {
        assert_eq!(
            MOVED.finished(run + offset),
            1,
            "job {offset}, pushed once the slot moved, ran once"
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AskedCommand {
    run: u64,
}

static ASKED: Runs = Runs::new();

#[queue(name = "nestrs-e2e-cluster-asked", job = AskedCommand)]
struct AskedQueue;

#[injectable]
#[derive(Default)]
struct AskedProcessor;

#[processor]
impl AskedProcessor {
    #[process(queue = AskedQueue, retries = 5)]
    async fn run(&self, job: AskedCommand) -> anyhow::Result<()> {
        ASKED.start(job.run);
        ASKED.finish(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(None), RedisQueueModule, QueueModule::for_root(None)],
    providers = [AskedProcessor],
)]
struct AskedModule;

/// A queue whose keys were moved to another node, its slot not handed over
/// yet, is read where its keys are: the worker's blocked read, on a connection
/// to the slot's primary alone, follows that primary's `ASK`, so a job filed
/// before the move starts before the slot changes hands. Settling it waits for
/// the hand-over — its script names keys the new node does not hold yet
/// (`TRYAGAIN`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blocked_read_follows_the_ask_of_a_slot_being_moved() {
    let queue = <AskedQueue as Queue>::NAME;
    crate::forget(queue).await;
    let slot = slot_of(queue).await;
    let source = owner_of(slot).await;
    let target = crate::cluster_nodes()
        .await
        .into_iter()
        .find(|node| node.primary && node.id != source.id)
        .expect("a second primary");
    let run = crate::this_run();
    crate::producer()
        .await
        .push(AskedQueue, AskedCommand { run }, None)
        .await
        .expect("a push before the move");

    let mut to_source = crate::node(&source.addr).await;
    let _: () = redis::cmd("CLUSTER")
        .arg("SETSLOT")
        .arg(slot)
        .arg("IMPORTING")
        .arg(&source.id)
        .query_async(&mut crate::node(&target.addr).await)
        .await
        .expect("the target imports the slot");
    let _: () = redis::cmd("CLUSTER")
        .arg("SETSLOT")
        .arg(slot)
        .arg("MIGRATING")
        .arg(&target.id)
        .query_async(&mut to_source)
        .await
        .expect("the source migrates the slot");
    let keys: Vec<String> = redis::cmd("CLUSTER")
        .arg("GETKEYSINSLOT")
        .arg(slot)
        .arg(1000)
        .query_async(&mut to_source)
        .await
        .expect("CLUSTER GETKEYSINSLOT");
    assert!(
        !keys.is_empty(),
        "the push filed the queue's keys on the source"
    );
    let (host, port) = target.addr.rsplit_once(':').expect("host:port");
    let _: () = redis::cmd("MIGRATE")
        .arg(host)
        .arg(port)
        .arg("")
        .arg(0)
        .arg(5000)
        .arg("KEYS")
        .arg(&keys)
        .query_async(&mut to_source)
        .await
        .expect("the queue's keys move to the target");

    let replica = crate::replica_on::<AskedModule>(crate::redis_config()).await;
    let mut started_while_moving = false;
    for _ in 0..100 {
        started_while_moving = !ASKED.of(run).is_empty();
        if started_while_moving {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assign(slot, &target.id).await;
    crate::wait_until(Duration::from_secs(10), || ASKED.finished(run) > 0).await;
    replica.worker.shutdown().await.expect("clean shutdown");

    // Restored before asserting, so a failure leaves the Cluster as it found it.
    crate::forget(queue).await;
    assign(slot, &source.id).await;

    assert!(
        started_while_moving,
        "the job filed before the move started on the slot's new node before the slot changed hands"
    );
    assert_eq!(ASKED.finished(run), 1, "and ran once");
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AtomicCommand {
    run: u64,
}

static ATOMIC: Runs = Runs::new();

#[queue(name = "nestrs-e2e-cluster-atomic", job = AtomicCommand)]
struct AtomicQueue;

#[injectable]
#[derive(Default)]
struct AtomicProcessor;

#[processor]
impl AtomicProcessor {
    #[process(queue = AtomicQueue, retries = 5)]
    async fn run(&self, job: AtomicCommand) -> anyhow::Result<()> {
        ATOMIC.start(job.run);
        ATOMIC.finish(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(None), RedisQueueModule, QueueModule::for_root(None)],
    providers = [AtomicProcessor],
)]
struct AtomicModule;

/// Move `slot` whole from `from` to `to` — Valkey 9's atomic slot migration —
/// and wait until the Cluster names `to` its owner.
async fn migrate_whole(slot: u16, from: &ClusterNode, to: &ClusterNode) {
    let _: () = redis::cmd("CLUSTER")
        .arg("MIGRATESLOTS")
        .arg("SLOTSRANGE")
        .arg(slot)
        .arg(slot)
        .arg("NODE")
        .arg(&to.id)
        .query_async(&mut crate::node(&from.addr).await)
        .await
        .expect("the source moves the slot");
    crate::wait_for(Duration::from_secs(30), || async {
        owner_of(slot).await.id == to.id
    })
    .await;
}

/// How many jobs the atomic migration's test pushes once the slot has moved.
const PUSHES_AFTER: u64 = 10;

/// A queue's slot moved whole by Valkey 9's atomic slot migration, under a
/// producer pushing and a worker draining, by the user the queue page
/// prescribes: every push answers — no window refuses a script, as a slot moved
/// key by key does — every job runs once, and Valkey denies the user nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_queue_runs_on_through_an_atomic_migration_of_its_slot() {
    let queue = <AtomicQueue as Queue>::NAME;
    crate::forget(queue).await;
    let slot = slot_of(queue).await;
    let source = owner_of(slot).await;
    let target = crate::cluster_nodes()
        .await
        .into_iter()
        .find(|node| node.primary && node.id != source.id)
        .expect("a second primary");
    let user = crate::acl_user("nestrs-e2e-atomic");
    let as_worker = crate::documented_user("queue/delivery.mdx", "worker", &user, 0).await;
    let replica = crate::replica_on::<AtomicModule>(as_worker).await;
    let run = crate::this_run();

    let moved = std::sync::atomic::AtomicBool::new(false);
    let pushing = async {
        let (mut pushed, mut before) = (0, 0);
        loop {
            let moved = moved.load(std::sync::atomic::Ordering::SeqCst);
            if moved && pushed - before == PUSHES_AFTER {
                return (pushed, before);
            }
            replica
                .producer
                .push(AtomicQueue, AtomicCommand { run: run + pushed }, None)
                .await
                .expect("every push answers while the slot moves whole");
            pushed += 1;
            if !moved {
                before = pushed;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    let moving = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        migrate_whole(slot, &source, &target).await;
        moved.store(true, std::sync::atomic::Ordering::SeqCst);
    };
    let ((pushed, before), ()) = tokio::join!(pushing, moving);
    crate::wait_until(Duration::from_secs(20), || {
        (0..pushed).all(|offset| ATOMIC.finished(run + offset) > 0)
    })
    .await;
    replica.worker.shutdown().await.expect("clean shutdown");

    // Restored before asserting, so a failure leaves the Cluster as it found it.
    crate::forget(queue).await;
    migrate_whole(slot, &target, &source).await;
    crate::assert_redis_denied_nothing_but(&user, &[]).await;
    crate::forget_user(&user).await;

    assert!(
        before > 0,
        "jobs were pushed before the slot finished moving"
    );
    for offset in 0..pushed {
        assert_eq!(
            ATOMIC.finished(run + offset),
            1,
            "job {offset}, pushed while its slot moved whole, ran once"
        );
    }
}

/// Failovers move the primaries every other test runs on, so they run one at a
/// time, once every other test of the run is done.
mod failover {
    use std::time::Duration;

    use nest_rs_queue::{JobProducerExt, Queue};

    use crate::{FAILOVER, FailoverCommand, FailoverModule, FailoverQueue};

    /// The primary serving a running queue's slot freezes, the Cluster promotes
    /// its replica, and the app — its worker and its producer — sends the
    /// slot's commands to the new primary: every job pushed before, during and
    /// after runs, at least once.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_queue_runs_on_across_a_failover_of_its_node() {
        let replica = crate::replica_on::<FailoverModule>(crate::redis_config()).await;
        let run = crate::this_run();
        for offset in 0..5 {
            replica
                .producer
                .push(FailoverQueue, FailoverCommand { run: run + offset }, None)
                .await
                .expect("a push before the failover");
        }
        crate::wait_until(Duration::from_secs(10), || {
            (0..5).all(|offset| FAILOVER.finished(run + offset) > 0)
        })
        .await;

        let slot = super::slot_of(FailoverQueue::NAME).await;
        let before = super::owner_of(slot).await;
        crate::freeze(&before.addr).await;
        crate::wait_for(Duration::from_secs(30), || async {
            crate::cluster_nodes()
                .await
                .iter()
                .any(|node| node.id != before.id && node.serves(slot))
        })
        .await;

        for offset in 5..10 {
            crate::push_through_a_failover(&replica.producer, run + offset).await;
        }
        crate::wait_until(Duration::from_secs(60), || {
            (0..10).all(|offset| FAILOVER.finished(run + offset) > 0)
        })
        .await;
        replica.worker.shutdown().await.expect("clean shutdown");
    }
}

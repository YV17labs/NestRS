//! What holds on a Cluster alone, against the suite's nodes: a queue whose
//! slot moves under it, run by the users the pages prescribe — an `ASK` while
//! the slot moves, a `MOVED` once it has — and, one at a time, a real failover
//! of the node serving a running queue.

use std::time::Duration;

use nest_rs_core::{injectable, module};
use nest_rs_queue::{JobProducerExt, Queue, QueueModule, processor, queue};
use nest_rs_redis::{RedisModule, RedisQueueModule};
use serde::{Deserialize, Serialize};

use crate::Runs;

/// One node of the Cluster as `CLUSTER NODES` lists it.
struct Node {
    id: String,
    addr: String,
    primary: bool,
    /// The slot ranges a primary serves.
    slots: Vec<(u16, u16)>,
}

/// Every node the Cluster lists, asked of the first node that answers.
async fn nodes() -> Vec<Node> {
    for client in crate::data_nodes().await {
        let Ok(mut node) = client.get_multiplexed_async_connection().await else {
            continue;
        };
        let Ok(listed) = redis::cmd("CLUSTER")
            .arg("NODES")
            .query_async::<String>(&mut node)
            .await
        else {
            continue;
        };
        return listed
            .lines()
            .filter(|line| !line.contains("fail"))
            .map(|line| {
                let fields: Vec<&str> = line.split(' ').collect();
                Node {
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
    panic!("no Cluster node answers");
}

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
async fn owner_of(slot: u16) -> Node {
    nodes()
        .await
        .into_iter()
        .find(|node| {
            node.primary
                && node
                    .slots
                    .iter()
                    .any(|(from, to)| (*from..=*to).contains(&slot))
        })
        .expect("a primary serves every slot")
}

/// `cmd` sent to the node at `addr` alone.
async fn on_node<T: redis::FromRedisValue>(addr: &str, cmd: &redis::Cmd) -> T {
    let mut node = crate::bare_client(&crate::node_url(addr))
        .get_multiplexed_async_connection()
        .await
        .expect("the node answers");
    cmd.query_async(&mut node)
        .await
        .unwrap_or_else(|error| panic!("{addr} runs {cmd:?}: {error}"))
}

/// How many `ASKING` the node at `addr` has run since it started.
async fn askings(addr: &str) -> u64 {
    let stats: String = on_node(addr, redis::cmd("INFO").arg("commandstats")).await;
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
    let target = nodes()
        .await
        .into_iter()
        .find(|node| node.primary && node.id != source.id)
        .expect("a second primary");
    let asked = askings(&target.addr).await;
    let _: () = on_node(
        &target.addr,
        redis::cmd("CLUSTER")
            .arg("SETSLOT")
            .arg(slot)
            .arg("IMPORTING")
            .arg(&source.id),
    )
    .await;
    let _: () = on_node(
        &source.addr,
        redis::cmd("CLUSTER")
            .arg("SETSLOT")
            .arg(slot)
            .arg("MIGRATING")
            .arg(&target.id),
    )
    .await;

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

    for node in nodes().await.iter().filter(|node| node.primary) {
        let _: () = on_node(
            &node.addr,
            redis::cmd("CLUSTER")
                .arg("SETSLOT")
                .arg(slot)
                .arg("NODE")
                .arg(&target.id),
        )
        .await;
    }
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

    crate::forget(queue).await;
    for node in nodes().await.iter().filter(|node| node.primary) {
        let _: () = on_node(
            &node.addr,
            redis::cmd("CLUSTER")
                .arg("SETSLOT")
                .arg(slot)
                .arg("NODE")
                .arg(&source.id),
        )
        .await;
    }
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

/// Failovers move the primaries every other test runs on, so they run one at a
/// time, once every other test of the run is done.
mod failover {
    use std::time::Duration;

    use nest_rs_core::{injectable, module};
    use nest_rs_queue::{JobProducerExt, Queue, QueueModule, processor, queue};
    use nest_rs_redis::{RedisModule, RedisQueueModule};
    use serde::{Deserialize, Serialize};

    use crate::Runs;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct FailoverCommand {
        run: u64,
    }

    static FAILOVER: Runs = Runs::new();

    #[queue(name = "nestrs-e2e-cluster-failover", job = FailoverCommand)]
    struct FailoverQueue;

    #[injectable]
    #[derive(Default)]
    struct FailoverProcessor;

    #[processor]
    impl FailoverProcessor {
        #[process(queue = FailoverQueue, retries = 5)]
        async fn run(&self, job: FailoverCommand) -> anyhow::Result<()> {
            FAILOVER.start(job.run);
            FAILOVER.finish(job.run);
            Ok(())
        }
    }

    #[module(
        imports = [RedisModule::for_root(None), RedisQueueModule, QueueModule::for_root(None)],
        providers = [FailoverProcessor],
    )]
    struct FailoverModule;

    /// The primary serving a running queue's slot freezes, the Cluster promotes
    /// its replica, and the app — its worker and its producer — sends the
    /// slot's commands to the new primary: every job pushed before, during and
    /// after runs, at least once.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_queue_runs_on_across_a_failover_of_its_node() {
        let queue = <FailoverQueue as Queue>::NAME;
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

        let slot = super::slot_of(queue).await;
        let before = super::owner_of(slot).await;
        let frozen = crate::bare_client(&crate::node_url(&before.addr));
        tokio::spawn(async move {
            if let Ok(mut frozen) = frozen.get_multiplexed_async_connection().await {
                let _: Result<(), _> = redis::cmd("DEBUG")
                    .arg("SLEEP")
                    .arg(6)
                    .query_async(&mut frozen)
                    .await;
            }
        });
        let mut promoted = false;
        for _ in 0..300 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if super::nodes().await.iter().any(|node| {
                node.primary
                    && node.id != before.id
                    && node
                        .slots
                        .iter()
                        .any(|(from, to)| (*from..=*to).contains(&slot))
            }) {
                promoted = true;
                break;
            }
        }
        assert!(promoted, "the Cluster promotes the frozen node's replica");

        for offset in 5..10 {
            let job = FailoverCommand { run: run + offset };
            let mut pushed = false;
            for _ in 0..100 {
                if replica
                    .producer
                    .push(FailoverQueue, job.clone(), None)
                    .await
                    .is_ok()
                {
                    pushed = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            assert!(pushed, "a push answers again once the slot moved");
        }
        crate::wait_until(Duration::from_secs(60), || {
            (0..10).all(|offset| FAILOVER.finished(run + offset) > 0)
        })
        .await;
        replica.worker.shutdown().await.expect("clean shutdown");
        for offset in 0..10 {
            assert!(
                FAILOVER.finished(run + offset) > 0,
                "job {offset} ran at least once"
            );
        }
    }
}

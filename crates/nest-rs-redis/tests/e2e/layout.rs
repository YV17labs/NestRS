//! Where a queue lives, against a live Redis.
//!
//! **Everything a queue holds is under its namespace, and the queue page's ACL
//! runs it.** A Redis user created exactly as the page prescribes — reaching
//! `nestrs:queue:*` and nothing else, allowed the commands the page lists and
//! nothing else — runs a queue end to end: a push, a delayed push, a
//! completion, a retry and a dead letter, with every script the worker runs
//! them through. The database holds no other key afterwards, and Redis's own
//! ACL log holds no denial.

use std::sync::Mutex;
use std::time::Duration;

use nest_rs_core::{injectable, module};
use nest_rs_queue::{JobId, JobProducerExt, PushOptions, processor, queue};
use nest_rs_redis::{RedisConnection, RedisModule, RedisQueueModule, RedisWorkerModule};
use nest_rs_testing::LogCapture;
use serde::{Deserialize, Serialize};

use crate::{DB_CONFINED_TO_THE_PREFIX, Runs};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LayoutCommand {
    run: u64,
    /// Whether every attempt fails, retryably.
    fail: bool,
}

// --- a user created as the queue page prescribes --------------------------------------

/// The ACL user this test creates from the page, and removes — named for this
/// run, so no earlier run's denial in `ACL LOG` is read as its own.
const CONFINED_USER: &str = "nestrs-e2e-confined";

static CONFINED: Runs = Runs::new();

/// The job ids each outcome was reached for.
static COMPLETED: Mutex<Vec<u64>> = Mutex::new(Vec::new());

#[queue(name = "nestrs-e2e-layout-confined", job = LayoutCommand)]
struct ConfinedQueue;

#[injectable]
#[derive(Default)]
struct ConfinedProcessor;

#[processor]
impl ConfinedProcessor {
    #[process(queue = ConfinedQueue, retries = 1)]
    async fn run(&self, job: LayoutCommand) -> anyhow::Result<()> {
        CONFINED.start(job.run);
        if job.fail {
            anyhow::bail!("the upstream is gone");
        }
        COMPLETED.lock().expect("lock").push(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(None), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [ConfinedProcessor],
)]
struct ConfinedModule;

/// Whether the job `id` ended on its second attempt's failure — with
/// `retries = 1`, the attempt that spends the budget — and the port said it
/// dead-lettered a job for a spent budget.
fn dead_letters(logs: &LogCapture, id: &JobId) -> bool {
    let last_failed = logs
        .find(
            nest_rs_core::operation_log::TARGET,
            nest_rs_queue::unit::JOB.name(),
        )
        .iter()
        .any(|line| {
            crate::names(line, id)
                && line.field("attempt").as_deref() == Some("2")
                && line.field("outcome").as_deref() == Some(nest_rs_core::operation_log::ERROR)
        });
    last_failed
        && !logs
            .find(
                nest_rs_queue::TARGET,
                "job dead-lettered: retry budget spent",
            )
            .is_empty()
}

/// Every key in the confined database.
async fn every_key(admin: &mut RedisConnection) -> Vec<String> {
    let mut cursor = 0u64;
    let mut keys = Vec::new();
    loop {
        let (next, batch): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("COUNT")
            .arg(1000)
            .query_async(admin)
            .await
            .expect("SCAN");
        keys.extend(batch);
        if next == 0 {
            return keys;
        }
        cursor = next;
    }
}

/// The user the queue page prescribes runs a queue from the push to the dead
/// letter: a completion, a delayed push, a retry filed on the schedule and a
/// spent budget — every apalis script a producer and a worker send, and every
/// one of the framework's. Nothing it needs lies outside the namespace or the
/// commands the page lists — the ACL refuses anything that does, the scripts'
/// own commands included — and the database holds no other key afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_user_created_as_the_queue_page_says_runs_a_queue_from_push_to_dead_letter() {
    let logs = LogCapture::install_global();
    let mut admin = RedisConnection::connect(&crate::redis_config_on(DB_CONFINED_TO_THE_PREFIX))
        .await
        .expect("the admin connection");
    let _: () = redis::cmd("FLUSHDB")
        .query_async(&mut admin)
        .await
        .expect("FLUSHDB");
    let user = crate::acl_user(CONFINED_USER);
    let confined =
        crate::documented_user("queue/delivery.mdx", &user, DB_CONFINED_TO_THE_PREFIX).await;
    crate::assert_may_load_a_script(&confined).await;

    let run = crate::this_run();
    let replica = crate::replica_on::<ConfinedModule>(confined).await;
    let completed = replica
        .producer
        .push(ConfinedQueue, LayoutCommand { run, fail: false }, None)
        .await
        .expect("an immediate push");
    let delayed = replica
        .producer
        .push(
            ConfinedQueue,
            LayoutCommand {
                run: run + 1,
                fail: false,
            },
            PushOptions::default().with_delay(Duration::from_secs(1)),
        )
        .await
        .expect("a delayed push");
    let doomed = replica
        .producer
        .push(
            ConfinedQueue,
            LayoutCommand {
                run: run + 2,
                fail: true,
            },
            None,
        )
        .await
        .expect("a push that fails every attempt");

    crate::wait_until(Duration::from_secs(20), || {
        let done = COMPLETED.lock().expect("lock").clone();
        done.contains(&run) && done.contains(&(run + 1)) && dead_letters(&logs, doomed.id())
    })
    .await;
    replica.worker.shutdown().await.expect("clean shutdown");
    crate::forget_user(&user).await;

    let done = COMPLETED.lock().expect("lock").clone();
    assert!(
        done.contains(&run),
        "the immediate job {} completed",
        completed.id()
    );
    assert!(
        done.contains(&(run + 1)),
        "the delayed job {} completed",
        delayed.id()
    );
    assert_eq!(
        CONFINED.of(run + 2).len(),
        2,
        "the doomed job ran its two attempts"
    );
    assert!(dead_letters(&logs, doomed.id()), "and was dead-lettered");
    let refused: Vec<_> = logs
        .events()
        .into_iter()
        .filter(crate::refused_by_acl)
        .collect();
    assert!(refused.is_empty(), "nothing met the ACL: {refused:#?}");
    // Redis's own record: a refusal some path swallowed would be here even
    // without a line.
    crate::assert_redis_denied_nothing_but(&user, &[]).await;

    let outside: Vec<String> = every_key(&mut admin)
        .await
        .into_iter()
        .filter(|key| !key.split(':').take(2).eq(["nestrs", "queue"]))
        .collect();
    assert!(
        outside.is_empty(),
        "no key outside the queue's namespace: {outside:?}"
    );
}

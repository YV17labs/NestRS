//! A job's `Checkpoint`, against live workers: the progress an attempt saves is
//! what the next attempt reads — after a retry on the same replica, and after
//! the replica running it died — and it is gone once the job reaches its
//! outcome.
//!
//! The port reads the state once per delivery and clears it at the job's end;
//! what only Redis shows is that a save outlives the delivery that made it, the
//! process that made it, and nothing more than the job.

use std::sync::Mutex;
use std::time::Duration;

use nest_rs_core::{injectable, module};
use nest_rs_queue::{Checkpoint, JobProducerExt, processor, queue};
use nest_rs_redis::{RedisModule, RedisQueueModule, RedisWorkerModule};
use serde::{Deserialize, Serialize};

use crate::Runs;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ImportCommand {
    run: u64,
}

/// What each attempt at a run's job read from its checkpoint, in order.
static SEEN: Mutex<Vec<(u64, Option<u32>)>> = Mutex::new(Vec::new());

static IMPORTED: Runs = Runs::new();

fn seen(run: u64) -> Vec<Option<u32>> {
    SEEN.lock()
        .expect("seen lock")
        .iter()
        .filter(|(of, _)| *of == run)
        .map(|(_, progress)| *progress)
        .collect()
}

#[queue(name = "nestrs-e2e-checkpoint", job = ImportCommand)]
struct ImportQueue;

#[injectable]
#[derive(Default)]
struct ImportProcessor;

#[processor]
impl ImportProcessor {
    /// The first attempt saves `1` and fails retryably; the second saves `2` and
    /// holds on until its replica dies; the one after it resumes and completes.
    /// Three attempts: one fails, one dies with its replica — which spends the
    /// budget like a failure — and the third completes.
    #[process(queue = ImportQueue, retries = 2, transactional = false)]
    async fn import(
        &self,
        job: ImportCommand,
        mut progress: Checkpoint<u32>,
    ) -> anyhow::Result<()> {
        IMPORTED.start(job.run);
        let read = progress.get().copied();
        SEEN.lock().expect("seen lock").push((job.run, read));
        match read {
            None => {
                progress.save(1).await?;
                anyhow::bail!("the upstream timed out");
            }
            Some(1) => {
                progress.save(2).await?;
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
            Some(_) => {}
        }
        IMPORTED.finish(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [ImportProcessor],
)]
struct ImportModule;

/// A save outlives the attempt that failed after it and the replica that died
/// holding it, is kept for the documented bound while the job lives, and is
/// cleared when the job completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_checkpoint_survives_a_retry_and_a_dead_replica_and_is_cleared_at_the_outcome() {
    /// Six days: the checkpoint of a job still alive is kept a week, renewed by
    /// every save and delivery, and certainly for more than this.
    const MORE_THAN_SIX_DAYS_MS: i64 = 6 * 24 * 60 * 60 * 1000;
    let run = crate::this_run();
    let doomed = crate::mortal_replica::<ImportModule>().await;
    let receipt = doomed
        .producer
        .push(ImportQueue, ImportCommand { run }, None)
        .await
        .expect("enqueue");
    let saved = crate::key_of(
        "nestrs-e2e-checkpoint",
        "checkpoints",
        &receipt.id().to_string(),
    );

    // The second attempt read the first one's save, and saved its own.
    crate::wait_until(Duration::from_secs(15), || seen(run).len() == 2).await;
    let mut held = None;
    for _ in 0..40 {
        held = crate::read(&saved).await;
        if held.as_deref() == Some("2") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        held.as_deref(),
        Some("2"),
        "the second attempt's save is in Redis"
    );
    assert!(
        crate::pttl(&saved).await > MORE_THAN_SIX_DAYS_MS,
        "and kept while the job lives"
    );

    doomed.kill().await;
    let heir = crate::replica::<ImportModule>().await;
    crate::wait_until(Duration::from_secs(20), || IMPORTED.finished(run) == 1).await;
    let mut cleared = false;
    for _ in 0..40 {
        if crate::read(&saved).await.is_none() {
            cleared = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    heir.worker.shutdown().await.expect("clean shutdown");

    assert_eq!(
        seen(run),
        [None, Some(1), Some(2)],
        "each attempt resumed from the last save: after a retry, and after a dead replica",
    );
    assert_eq!(IMPORTED.finished(run), 1, "the job completed once");
    assert!(cleared, "its checkpoint went with it");
}

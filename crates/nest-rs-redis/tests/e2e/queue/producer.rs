//! A push held back until its delay ends, against a live worker.
//!
//! The backend declares delayed delivery, so a delayed push is filed on the
//! queue's schedule rather than its list, due on the second its delay ends —
//! rounded up, since apalis schedules on whole seconds. A job must therefore
//! never start before its delay has passed, and must start soon after: the
//! worker scans the schedule every second.

use std::time::{Duration, Instant};

use nest_rs_core::{injectable, module};
use nest_rs_queue::{JobProducerExt, PushOptions, processor, queue};
use nest_rs_redis::{RedisModule, RedisQueueModule, RedisWorkerModule};
use serde::{Deserialize, Serialize};

use crate::Runs;

/// How long the job is held back.
const DELAY: Duration = Duration::from_secs(2);

/// The latest it may start after its delay: the second the delay is rounded up
/// to, one scan of the schedule, and a poll of the queue.
const LATENESS: Duration = Duration::from_millis(2500);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DelayedCommand {
    run: u64,
}

static DELAYED: Runs = Runs::new();

#[queue(name = "nestrs-e2e-producer-delay", job = DelayedCommand)]
struct DelayedQueue;

#[injectable]
#[derive(Default)]
struct DelayedProcessor;

#[processor]
impl DelayedProcessor {
    #[process(queue = DelayedQueue, retries = 0)]
    async fn run(&self, job: DelayedCommand) -> anyhow::Result<()> {
        DELAYED.start(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(None)],
    providers = [DelayedProcessor],
)]
struct DelayedModule;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delayed_push_runs_once_its_delay_has_passed_and_not_before() {
    let run = crate::this_run();
    let replica = crate::replica::<DelayedModule>().await;
    let pushed = Instant::now();
    replica
        .producer
        .push(
            DelayedQueue,
            DelayedCommand { run },
            PushOptions::default().with_delay(DELAY),
        )
        .await
        .expect("a delayed push");

    crate::wait_until(DELAY + LATENESS * 2, || !DELAYED.of(run).is_empty()).await;
    replica.worker.shutdown().await.expect("clean shutdown");

    let started = DELAYED.of(run);
    assert_eq!(started.len(), 1, "the job ran, once");
    let waited = started[0] - pushed;
    assert!(waited >= DELAY, "it waited out its delay, not {waited:?}");
    assert!(
        waited < DELAY + LATENESS,
        "and ran soon after it ended, not {waited:?} after the push"
    );
}

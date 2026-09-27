//! The worker's settings at their bounds, against a live worker: every value
//! `RedisWorkerConfig` accepts is one apalis runs with.
//!
//! apalis-redis 0.7.4 converts the orphan threshold with an `unwrap` on every
//! poll, and a value past `chrono`'s range panicked its heartbeat while nothing
//! bounded it. The config's unit tests pin the arithmetic; this runs it: a
//! worker with the longest threshold, lease and drain window accepted, polling
//! at the floor so apalis converts the threshold a hundred times a second, runs
//! its jobs and stops.

use std::time::{Duration, Instant};

use nest_rs_core::{injectable, module};
use nest_rs_queue::{JobProducerExt, processor, queue};
use nest_rs_redis::{RedisModule, RedisQueueModule, RedisWorkerConfig, RedisWorkerModule};
use serde::{Deserialize, Serialize};

use crate::Runs;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProbeCommand {
    run: u64,
}

static RAN: Runs = Runs::new();

#[queue(name = "nestrs-e2e-config-ceilings", job = ProbeCommand)]
struct CeilingsQueue;

#[injectable]
#[derive(Default)]
struct CeilingsProcessor;

#[processor]
impl CeilingsProcessor {
    #[process(queue = CeilingsQueue, retries = 0)]
    async fn probe(&self, job: ProbeCommand) -> anyhow::Result<()> {
        RAN.start(job.run);
        Ok(())
    }
}

/// Every duration at its ceiling — the lease at half the threshold, the most it
/// may be — but the poll, at its floor so the threshold is converted as often
/// as the config allows.
fn ceilings() -> RedisWorkerConfig {
    RedisWorkerConfig {
        shutdown_timeout: Duration::from_secs(60 * 60),
        orphan_after: Duration::from_secs(24 * 60 * 60),
        lease: Duration::from_secs(12 * 60 * 60),
        poll_interval: Duration::from_millis(10),
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(ceilings())],
    providers = [CeilingsProcessor],
)]
struct CeilingsModule;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_worker_at_every_ceiling_runs_its_jobs_and_stops() {
    const JOBS: usize = 3;
    let run = crate::this_run();
    let replica = crate::replica::<CeilingsModule>().await;
    for _ in 0..JOBS {
        replica
            .producer
            .push(CeilingsQueue, ProbeCommand { run }, None)
            .await
            .expect("enqueue");
    }
    crate::wait_until(Duration::from_secs(15), || RAN.of(run).len() == JOBS).await;
    // A second more at the floor poll: a hundred sweeps, each converting the
    // day-long threshold, before the worker is asked to stop.
    tokio::time::sleep(Duration::from_secs(1)).await;

    let stopping = Instant::now();
    replica
        .worker
        .shutdown()
        .await
        .expect("a worker at its ceilings stops cleanly");

    assert_eq!(
        RAN.of(run).len(),
        JOBS,
        "apalis ran every job under the longest threshold, lease and window accepted",
    );
    assert!(
        stopping.elapsed() < Duration::from_secs(10),
        "a drain window of an hour is a ceiling, not a wait: nothing ran, so it stopped at once \
         ({:?})",
        stopping.elapsed(),
    );
}

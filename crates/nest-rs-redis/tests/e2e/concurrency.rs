//! A `#[process]` method runs **one job at a time**, against a live worker.
//!
//! That is the whole queue concurrency contract: nestrs targets the container,
//! so throughput comes from more replicas, not from a per-method ceiling. This
//! suite is the guard on the "one" — if anyone reintroduces in-process
//! parallelism (a concurrency knob, a wider `poll_ready`, a `tokio::spawn`
//! inside the dispatch path), the peak climbs above 1 and this fails.
//!
//! Only a live worker shows it. The in-process handler tests in
//! `tests/integration/main.rs` invoke one `JobHandler` at a time, which is
//! exactly the condition under which lost serialization stays invisible.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use nest_rs_core::{injectable, module};
use nest_rs_queue::{JobProducerExt, processor, queue};
use nest_rs_redis::{
    RedisQueueConfig, RedisQueueConnection, RedisQueueModule, RedisWorker, RedisWorkerModule,
};
use nest_rs_testing::TestApp;
use serde::{Deserialize, Serialize};

/// Enough jobs that a worker running them in parallel overshoots unmistakably.
const JOBS: usize = 6;
/// Long enough that jobs would genuinely overlap if nothing serialized them,
/// short enough to keep the suite quick.
const HOLD: Duration = Duration::from_millis(250);

/// Simultaneous handler bodies, and the peak seen. Process-wide statics: the
/// container owns the provider, and the assertion runs outside it.
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static RAN: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HoldCommand {
    seq: usize,
}

#[queue(name = "nestrs-e2e-concurrency", job = HoldCommand)]
struct ConcurrencyQueue;

#[injectable]
#[derive(Default)]
struct HoldProcessor;

#[processor]
impl HoldProcessor {
    #[process(queue = ConcurrencyQueue, retries = 0)]
    async fn hold(&self, _job: HoldCommand) -> anyhow::Result<()> {
        let now = IN_FLIGHT.fetch_add(1, Ordering::SeqCst) + 1;
        PEAK.fetch_max(now, Ordering::SeqCst);
        // Holding the slot is the measurement: unserialized, the next fetched
        // job starts here and the peak climbs past 1.
        tokio::time::sleep(HOLD).await;
        IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
        RAN.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Pinned rather than read from the env: the framework workspace ships no
/// `.env`, so `for_root(None)` would resolve to the localhost default and this
/// suite would fail on connect instead of measuring anything.
fn queue_config() -> RedisQueueConfig {
    RedisQueueConfig {
        url: std::env::var(nest_rs_config::var_name("queue", "URL"))
            .unwrap_or_else(|_| "redis://redis:6379".to_string()),
        ..Default::default()
    }
}

#[module(
    imports = [RedisQueueModule::for_root(queue_config()), RedisWorkerModule],
    providers = [HoldProcessor],
)]
struct ConcurrencyModule;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_process_method_never_runs_two_jobs_at_once() {
    let app = TestApp::builder()
        .module::<ConcurrencyModule>()
        .build_headless()
        .await
        .expect("the worker app boots against the dev container Redis");
    app.init().await.expect("init phases");

    let conn = app
        .container()
        .get::<RedisQueueConnection>()
        .expect("RedisQueueModule seeds the connection");

    let handle = app
        .spawn_transport(RedisWorker::default())
        .await
        .expect("the queue worker transport starts");

    for seq in 0..JOBS {
        conn.push_to::<ConcurrencyQueue>(HoldCommand { seq })
            .await
            .expect("enqueue");
    }

    // Serialized, the run needs JOBS waves; the slack absorbs poll latency
    // without making a parallel run look serialized.
    tokio::time::sleep(HOLD * JOBS as u32 + Duration::from_secs(3)).await;
    handle.shutdown().await.expect("clean worker shutdown");

    let peak = PEAK.load(Ordering::SeqCst);
    assert!(peak > 0, "no job ran — the worker never drained the queue");
    assert_eq!(
        peak, 1,
        "a #[process] method runs one job at a time, but {peak} ran at once — \
         in-process parallelism is back",
    );
    // The ceiling alone would also hold for a worker that never picked up a
    // second job at all. Draining every job proves serialized, not stalled.
    assert_eq!(
        RAN.load(Ordering::SeqCst),
        JOBS,
        "serialized must still mean progress: all {JOBS} jobs drain, one after another",
    );
}

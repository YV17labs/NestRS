//! A worker's shutdown is bounded by `shutdown_timeout`, whatever its jobs are
//! doing.
//!
//! A handler that never returns must neither hold the process until the
//! orchestrator's SIGKILL nor vanish in silence: the drain stops at the timeout,
//! and the jobs it abandons are counted on the framework's own target.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use nest_rs_core::{injectable, module};
use nest_rs_queue::{JobProducerExt, processor, queue};
use nest_rs_redis::{
    RedisModule, RedisQueueModule, RedisQueueProducer, RedisWorker, RedisWorkerConfig,
    RedisWorkerModule,
};
use nest_rs_testing::{LogCapture, TestApp};
use serde::{Deserialize, Serialize};

static STARTED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HangCommand {}

#[queue(name = "nestrs-e2e-shutdown", job = HangCommand)]
struct ShutdownQueue;

#[injectable]
#[derive(Default)]
struct HangProcessor;

#[processor]
impl HangProcessor {
    #[process(queue = ShutdownQueue, retries = 0)]
    async fn hang(&self, _job: HangCommand) -> anyhow::Result<()> {
        STARTED.store(true, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_secs(60)).await;
        Ok(())
    }
}

#[module(
    imports = [
        RedisModule::for_root(crate::redis_config()),
        RedisQueueModule,
        RedisWorkerModule::for_root(RedisWorkerConfig { shutdown_timeout: Duration::from_secs(1) }),
    ],
    providers = [HangProcessor],
)]
struct ShutdownModule;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_job_that_outlives_the_shutdown_timeout_is_abandoned_and_says_so() {
    // Global: the warn is emitted by the transport's own task, on a worker
    // thread of this multi-threaded runtime, which a thread-local capture misses.
    let logs = LogCapture::install_global();
    let app = TestApp::builder()
        .module::<ShutdownModule>()
        .build_headless()
        .await
        .expect("the worker app boots against the dev container Redis");
    app.init().await.expect("init phases");
    let producer = app
        .container()
        .get::<RedisQueueProducer>()
        .expect("RedisQueueModule binds the producer");
    let handle = app
        .spawn_transport(RedisWorker::default())
        .await
        .expect("the queue worker transport starts");

    producer
        .push_to::<ShutdownQueue>(HangCommand {})
        .await
        .expect("enqueue");
    crate::wait_until(|| STARTED.load(Ordering::SeqCst)).await;
    assert!(
        STARTED.load(Ordering::SeqCst),
        "the job must be running before the shutdown begins",
    );

    let began = Instant::now();
    handle.shutdown().await.expect("the worker shuts down");
    let took = began.elapsed();

    assert!(
        took < Duration::from_secs(5),
        "a one-second shutdown_timeout bounds the drain, took {took:?}",
    );
    let event = logs.expect_one(
        nest_rs_queue::TARGET,
        "worker shutdown timed out; abandoning the jobs still running",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("shutdown_timeout_secs").as_deref(), Some("1"));
    assert_eq!(
        event.field("abandoned").as_deref(),
        Some("1"),
        "the event counts the jobs it abandoned: {:?}",
        event.fields,
    );
}

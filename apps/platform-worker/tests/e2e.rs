use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nestrs_core::{injectable, module};
use nestrs_queue::{
    processor, QueueConfig, QueueConnection, QueueModule, QueueWorker,
};
use nestrs_testing::TestApp;
use serde::{Deserialize, Serialize};
use platform_worker::PlatformWorkerModule;

const PROBE_QUEUE: &str = "nestrs-e2e-probe";

fn redis_url() -> String {
    std::env::var("NESTRS_QUEUE__URL").unwrap_or_else(|_| "redis://127.0.0.1/".into())
}

fn unique_tag() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("probe-{}-{}", std::process::id(), nanos)
}

#[tokio::test]
async fn worker_app_boots_and_consumes_through_the_queue_transport() {
    let app = TestApp::builder()
        .module::<PlatformWorkerModule>()
        .build_headless()
        .await
        .expect("PlatformWorkerModule boots and connects to Redis");

    // The worker is a pure consumer now: only the QueueWorker transport, which
    // must configure against the reachable AudioJobs::transcode method.
    let queue = app
        .spawn_transport(QueueWorker::new())
        .await
        .expect("QueueWorker configures against the container");

    tokio::time::sleep(Duration::from_millis(150)).await;

    queue.shutdown().await.expect("QueueWorker stops cleanly");
}

#[derive(Clone, Serialize, Deserialize)]
struct ProbeJob {
    tag: String,
}

static PROBE_TX: OnceLock<tokio::sync::mpsc::UnboundedSender<String>> = OnceLock::new();

#[injectable]
#[derive(Default)]
struct ProbeConsumer;

#[processor]
impl ProbeConsumer {
    #[process(queue = "nestrs-e2e-probe", concurrency = 1, retries = 0)]
    async fn handle(&self, job: ProbeJob) -> anyhow::Result<()> {
        if let Some(tx) = PROBE_TX.get() {
            let _ = tx.send(job.tag);
        }
        Ok(())
    }
}

#[module(
    imports = [QueueModule::for_root(QueueConfig { url: redis_url() })],
    providers = [ProbeConsumer],
)]
struct ProbeModule;

#[tokio::test]
async fn enqueued_job_is_processed_through_real_redis() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let _ = PROBE_TX.set(tx);

    let app = TestApp::builder()
        .module::<ProbeModule>()
        .build_headless()
        .await
        .expect("ProbeModule boots and connects to Redis");

    let queue = app
        .spawn_transport(QueueWorker::new())
        .await
        .expect("QueueWorker configures");

    let tag = unique_tag();
    let conn = app
        .container()
        .get::<QueueConnection>()
        .expect("QueueModule seeded the shared QueueConnection");
    conn.of::<ProbeJob>(PROBE_QUEUE)
        .push(ProbeJob { tag: tag.clone() })
        .await
        .expect("enqueue onto the probe queue");

    let saw_our_job = tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(received) = rx.recv().await {
            if received == tag {
                return true;
            }
        }
        false
    })
    .await;

    queue.shutdown().await.expect("QueueWorker stops cleanly");

    assert!(
        matches!(saw_our_job, Ok(true)),
        "the enqueued job was consumed end-to-end via Redis",
    );
}

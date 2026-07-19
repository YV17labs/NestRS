use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nest_rs_core::{injectable, module};
use nest_rs_queue::{JobProducerExt, processor, queue};
use nest_rs_redis::{QueueConfig, QueueConnection, QueueModule, QueueWorker};
use nest_rs_testing::TestApp;
use serde::{Deserialize, Serialize};
use worker::WorkerModule;

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

#[derive(Clone, Serialize, Deserialize)]
struct ProbeCommand {
    tag: String,
}

#[queue(name = "nestrs-e2e-probe", job = ProbeCommand)]
struct ProbeQueue;

static PROBE_TX: OnceLock<tokio::sync::mpsc::UnboundedSender<String>> = OnceLock::new();

#[injectable]
#[derive(Default)]
struct ProbeConsumer;

#[processor]
impl ProbeConsumer {
    #[process(queue = ProbeQueue, concurrency = 1, retries = 0)]
    async fn handle(&self, job: ProbeCommand) -> anyhow::Result<()> {
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
async fn worker_app_boots_and_processes_an_enqueued_job_through_real_redis() {
    let worker = TestApp::builder()
        .module::<WorkerModule>()
        .build_headless()
        .await
        .expect("WorkerModule boots and connects to Redis");
    let worker_queue = worker
        .spawn_transport(QueueWorker::new())
        .await
        .expect("WorkerModule's QueueWorker configures against Redis");

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
    conn.push_to::<ProbeQueue>(ProbeCommand { tag: tag.clone() })
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
    worker_queue
        .shutdown()
        .await
        .expect("WorkerModule's QueueWorker stops cleanly");

    assert!(
        matches!(saw_our_job, Ok(true)),
        "the enqueued job was consumed end-to-end via Redis",
    );
}

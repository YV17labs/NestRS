//! One W3C trace, from the producer's process into the consumer's — and the
//! tenant and the baggage a job carries on into what it pushes: only a live
//! worker shows the context survives Redis and the dispatch.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use nest_rs_core::{injectable, module};
use nest_rs_queue::__private::consume::Delivery;
use nest_rs_queue::{
    JobProducer, JobProducerExt, PushOptions, Queue, QueueModule, QueueName, QueueWorker,
    WIRE_FORMAT_VERSION, processor, queue,
};
use nest_rs_redis::{RedisConnection, RedisModule, RedisQueueModule, RedisQueueProducer};
use nest_rs_testing::HeadlessApp;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// What the handler saw as its ambient identity: the trace it is running in,
/// the span it *is*, and who it is being served for.
#[derive(Clone, Debug)]
struct Observed {
    trace_id: String,
    span_id: Option<String>,
    actor_id: Option<String>,
}

/// What each job reported, keyed by the `seq` it carried: every run shares one
/// Redis queue, which a killed earlier run may have left jobs on.
static SEEN: Mutex<Vec<(usize, Observed)>> = Mutex::new(Vec::new());

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TraceCommand {
    seq: usize,
}

#[queue(name = "nestrs-e2e-correlation", job = TraceCommand)]
struct CorrelationQueue;

#[injectable]
#[derive(Default)]
struct CorrelationProcessor;

#[processor]
impl CorrelationProcessor {
    /// Reports the ambient id rather than asserting on it: the handler body is
    /// the only place that can answer what the consumer installed.
    #[process(queue = CorrelationQueue, retries = 0)]
    async fn record(&self, job: TraceCommand) -> anyhow::Result<()> {
        if let Some(id) = nest_rs_core::current_trace_id() {
            SEEN.lock().expect("probe lock").push((
                job.seq,
                Observed {
                    trace_id: id.to_hex(),
                    span_id: nest_rs_core::current_span_id().map(|span| span.to_hex()),
                    actor_id: nest_rs_core::current_actor_id(),
                },
            ));
        }
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, QueueModule::for_root(None)],
    providers = [CorrelationProcessor],
)]
struct CorrelationModule;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_job_runs_in_the_trace_that_enqueued_it_as_a_child_of_the_enqueue() {
    let app = HeadlessApp::builder()
        .module::<CorrelationModule>()
        .build_headless()
        .await
        .expect("a worker boots against the dev container Redis");
    app.init().await.expect("init phases");
    let worker = app
        .spawn_transport(QueueWorker::new())
        .await
        .expect("the queue worker transport starts");

    // Enqueue *under an ambient context*, the way an HTTP handler does. This id
    // is the one the consumer must end up running under.
    let conn = RedisQueueProducer::new(
        RedisConnection::connect(&crate::redis_config())
            .await
            .expect("connect"),
    );
    let correlation = nest_rs_core::Correlation::minted(None);
    // This run's own marker, so a job left behind by an earlier run cannot be
    // mistaken for it — see `SEEN`.
    let seq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after 1970")
        .subsec_nanos() as usize;
    nest_rs_core::with_request_scope(None, correlation.clone(), async {
        // Exactly what an authenticated HTTP handler's guard did before it
        // reached the service that enqueues.
        nest_rs_core::__private::set_actor_id("alice-42");
        conn.push(CorrelationQueue, TraceCommand { seq }, None)
            .await
            .expect("enqueue");
    })
    .await;

    let observed = |seq: usize| {
        SEEN.lock()
            .expect("probe lock")
            .iter()
            .find(|(s, _)| *s == seq)
            .map(|(_, o)| o.clone())
    };
    for _ in 0..100 {
        if observed(seq).is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    worker.shutdown().await.expect("clean shutdown");

    let seen = observed(seq).expect("the job ran and reported what it was running under");

    assert_eq!(
        seen.trace_id,
        correlation.trace_id().to_hex(),
        "one trace across the process boundary — the chain breaks here or nowhere",
    );
    assert_ne!(
        seen.span_id.as_deref(),
        Some(correlation.span_id().to_hex().as_str()),
        "the job is its own unit of work, not a second name for the enqueue",
    );
    assert_eq!(
        seen.actor_id.as_deref(),
        Some("alice-42"),
        "and the actor crosses too: a worker holds no credential, so what the \
         producer knew is the only answer there will ever be",
    );
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RelayCommand {
    run: u64,
}

#[queue(name = "nestrs-e2e-correlation-relay", job = RelayCommand)]
struct RelayQueue;

/// Drained by nothing: what lands here is read back off its stream.
#[queue(name = "nestrs-e2e-correlation-relayed", job = RelayCommand)]
struct RelayedQueue;

/// Pushes every job it runs on, as a handler enqueueing follow-up work does.
#[injectable]
struct RelayProcessor {
    #[inject]
    queue: Arc<dyn JobProducer>,
}

#[processor]
impl RelayProcessor {
    #[process(queue = RelayQueue, retries = 0)]
    async fn relay(&self, job: RelayCommand) -> anyhow::Result<()> {
        self.queue.push(RelayedQueue, job, None).await?;
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, QueueModule::for_root(None)],
    providers = [RelayProcessor],
)]
struct RelayModule;

/// The record `queue`'s stream holds for the job carrying `run`, when it holds
/// one.
async fn filed_for(queue: &str, run: u64) -> Option<serde_json::Value> {
    crate::entries_of(queue, "jobs")
        .await
        .into_iter()
        .flat_map(|(_, fields)| fields)
        .filter(|(field, _)| field == "record")
        .filter_map(|(_, record)| serde_json::from_str::<serde_json::Value>(&record).ok())
        .find(|record| record["payload"]["run"] == json!(run))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_job_carries_its_tenant_and_its_baggage_through_redis_into_what_it_pushes() {
    let (relay, relayed) = (RelayQueue::NAME, RelayedQueue::NAME);
    crate::forget(relay).await;
    crate::forget(relayed).await;
    let run = crate::this_run();
    let replica = crate::replica::<RelayModule>().await;

    // No push of this release writes baggage of its own, so the test producer
    // files a record carrying one, as a producer forwarding it would.
    let queue = QueueName::new(relay).expect("a valid name");
    let sealed = Delivery::new(
        JobProducer::backend(&replica.producer),
        queue.clone(),
        json!({
            "v": WIRE_FORMAT_VERSION,
            "payload": { "run": run },
            "tenant_id": "acme",
            "baggage": "region=eu;ttl=60,plan=pro",
        }),
    )
    .retry_envelope();
    JobProducer::enqueue(
        &replica.producer,
        &queue,
        vec![sealed],
        &PushOptions::default(),
    )
    .await
    .expect("the test producer files the record");

    crate::wait_for(Duration::from_secs(10), || async move {
        filed_for(relayed, run).await.is_some()
    })
    .await;
    let pushed = filed_for(relayed, run)
        .await
        .expect("the job pushed its follow-up");
    replica.worker.shutdown().await.expect("clean shutdown");
    crate::forget(relay).await;
    crate::forget(relayed).await;

    assert_eq!(
        pushed["tenant_id"],
        json!("acme"),
        "the follow-up runs for the tenant the job ran for",
    );
    assert_eq!(
        pushed["baggage"],
        json!("region=eu;ttl=60,plan=pro"),
        "the baggage is carried on as it came",
    );
}

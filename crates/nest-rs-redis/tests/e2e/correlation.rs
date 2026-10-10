//! One W3C trace, from the producer's process into the consumer's: only a live
//! worker shows the context survives Redis and the dispatch.

use std::sync::Mutex;
use std::time::Duration;

use nest_rs_core::{injectable, module};
use nest_rs_queue::{JobProducerExt, QueueModule, QueueWorker, processor, queue};
use nest_rs_redis::{RedisConnection, RedisModule, RedisQueueModule, RedisQueueProducer};
use nest_rs_testing::TestApp;
use serde::{Deserialize, Serialize};

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
    let app = TestApp::builder()
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

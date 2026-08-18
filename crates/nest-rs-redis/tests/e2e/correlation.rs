//! One W3C trace, from the producer's process into the consumer's.
//!
//! Every other propagation in the framework crosses a *task*, and a task-local
//! is enough. A queue crosses a **process**: the producer is an API pod, the
//! consumer is a worker pod, and the only thing that reaches from one to the
//! other is the payload. So the `traceparent` rides the wire envelope
//! (`nest_rs_queue::envelope`), and this is where that is measured rather than
//! asserted about.
//!
//! What it buys, concretely: "show me everything this request caused" answers
//! across the enqueue boundary, with the job appearing **under** the enqueue in
//! any conformant backend rather than merely beside it — and with no collector
//! and no shared process required for the logs to say so.
//!
//! The in-process envelope tests cover the shape; only a live worker shows the
//! context survives Redis, apalis and the dispatch.

use std::sync::Mutex;
use std::time::Duration;

use nest_rs_core::{injectable, module};
use nest_rs_queue::{JobProducerExt, processor, queue};
use nest_rs_redis::{QueueConfig, QueueConnection, QueueModule, QueueWorker, QueueWorkerModule};
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

/// A process-wide static because the container owns the provider and the
/// assertion runs outside it.
static SEEN: Mutex<Option<Observed>> = Mutex::new(None);

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
    async fn record(&self, _job: TraceCommand) -> anyhow::Result<()> {
        *SEEN.lock().expect("probe lock") = nest_rs_core::current_trace_id().map(|id| Observed {
            trace_id: id.to_hex(),
            span_id: nest_rs_core::current_span_id().map(|span| span.to_hex()),
            actor_id: nest_rs_core::current_actor_id(),
        });
        Ok(())
    }
}

/// Pinned rather than read from the env: the framework workspace ships no
/// `.env`, so `for_root(None)` would resolve to the localhost default and this
/// suite would fail on connect instead of measuring anything.
fn queue_config() -> QueueConfig {
    QueueConfig {
        url: std::env::var("NESTRS_QUEUE__URL")
            .unwrap_or_else(|_| "redis://redis:6379".to_string()),
        ..Default::default()
    }
}

#[module(
    imports = [QueueModule::for_root(queue_config()), QueueWorkerModule],
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
        .spawn_transport(QueueWorker::default())
        .await
        .expect("the queue worker transport starts");

    // Enqueue *under an ambient context*, the way an HTTP handler does. This id
    // is the one the consumer must end up running under.
    let conn = QueueConnection::connect(&queue_config().url)
        .await
        .expect("connect");
    let correlation = nest_rs_core::Correlation::mint();
    nest_rs_core::with_request_scope(None, correlation.clone(), None, async {
        // Exactly what an authenticated HTTP handler's guard did before it
        // reached the service that enqueues.
        nest_rs_core::set_actor_id("alice-42");
        conn.push_to::<CorrelationQueue>(TraceCommand { seq: 0 })
            .await
            .expect("enqueue");
    })
    .await;

    for _ in 0..100 {
        if SEEN.lock().expect("probe lock").is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    worker.shutdown().await.expect("clean shutdown");

    let seen = SEEN
        .lock()
        .expect("probe lock")
        .clone()
        .expect("the job ran and reported what it was running under");

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

//! [`RedisWorker`] — the queue port's consumer half over the shared
//! [`RedisConnection`]: a `Transport` running one oxana runtime that drains
//! every `#[process]` method this app serves.
//!
//! **This file is the transport and nothing else.** What a job attempt *is* —
//! the envelope, the trace, the `queue.job` span, the panic catch, the outcome
//! classes, the events and the operation line — is the port's
//! (`nest_rs_queue::consume::attempt`), and discovery is the port's too
//! (`consume::discover`). What stays here is what the backend alone knows: the
//! queues the runtime drains, how often it polls, the retry budget, one job at a
//! time per queue, the drain at shutdown, and how an [`Attempt`] settles a job.
//!
//! Every queue is drained by **one** oxana worker, which routes each record by
//! the queue it names to the method serving it. A queue this app does not serve
//! is never fetched.
//!
//! **One job at a time per `#[process]` method.** That is the whole contract,
//! and it is deliberately not configurable: nestrs targets the container, so
//! throughput comes from running more replicas of the worker — the unit the
//! platform already schedules, meters and restarts. A per-method ceiling would
//! be a second, in-process scheduler competing with the first, and the number
//! that makes it correct depends on the pod's CPU share rather than on anything
//! the code can know. Serialized-per-method is the behaviour a reader can
//! predict from the source, and it makes every replica's load identical. Each
//! queue is registered at a concurrency of one, and `discover` refuses a second
//! method on a queue, so one queue is one method.
//!
//! **Retries run inside the attempt that failed.** oxana settles a job's retry
//! budget before the job runs, so it cannot dead-letter a payload
//! `consume::attempt` classifies as undeliverable without first spending that
//! budget on it — re-running the handler, side effects included, on a payload
//! that cannot succeed. So oxana is told the budget is zero and this file keeps
//! it: a retryable failure runs again at once, same `job_id` and `attempt` one
//! higher, while the queue's single permit is still held; a dead letter, or the
//! last retry failing, fails the job and oxana moves it to the dead list.
//!
//! **Delivery is exclusive but at-least-once.** A job is claimed by one atomic
//! `LMOVE` into the claiming process's own processing list, so two replicas
//! never receive the same job. A replica that dies mid-job leaves it there, and
//! once its heartbeat has been silent for oxana's dead-process threshold a live
//! replica puts it back on the queue, where it runs again — so a `#[process]`
//! handler must be idempotent. Starting a replica disturbs nothing: only a
//! process whose heartbeat stopped is swept.
//!
//! **A process is identified by its hostname and pid, and a container restarted
//! in place keeps both.** Replicas of a deployment differ by hostname, so the
//! sweep above holds between them. A container the platform restarts where it
//! stood comes back with the same hostname and pid 1, heartbeats for its crashed
//! predecessor's processing list, and never sweeps it: the job that was running
//! when the container died is not run again. That is oxana's identity, not this
//! file's to change.

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use nest_rs_core::{Container, Transport};
use nest_rs_queue::ProcessMethod;
use nest_rs_queue::consume::{self, Attempt};
use oxana::{FromContext, JobContext, QueueConfig};
use tokio_util::sync::CancellationToken;

use crate::RedisConnection;
use crate::connection::CONNECTION_REMEDY;
use crate::error::Undelivered;
use crate::job::{Received, RedisJob};

/// How long a dispatcher waits before polling a queue it found empty — the
/// pickup latency of a job pushed onto an idle queue. oxana's default is ten
/// seconds, which is a batch system's number: a job a request enqueued is
/// expected to start while its caller is still looking. The price is paid while
/// idle: a dequeue per served queue, per replica, every interval.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The consecutive Redis failures oxana tolerates before stopping the runtime:
/// all of them. oxana counts failures across every loop it runs, so its default
/// of thirty is about three seconds of outage — and a stopped runtime took the
/// whole app down with it. Every failure is still a `warn` on
/// `oxana::storage_internal`, and the loops resume when Redis answers.
const OUTLAST_EVERY_REDIS_OUTAGE: u32 = u32::MAX;

/// What the drain may take past `shutdown_timeout` before this transport stops
/// waiting: oxana stops counting its workers at the timeout, then removes its
/// process record from Redis.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

/// The consumer-side transport: drains the `#[processor]` inventory and runs
/// each job's process method against the Redis queue. Attached by
/// [`RedisWorkerModule`](crate::RedisWorkerModule).
pub struct RedisWorker {
    methods: Vec<&'static ProcessMethod>,
    container: Option<Container>,
}

impl RedisWorker {
    /// An empty worker; process methods and the container are wired at boot.
    pub fn new() -> Self {
        Self {
            methods: Vec::new(),
            container: None,
        }
    }
}

impl Default for RedisWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Transport for RedisWorker {
    async fn configure(&mut self, container: &Container) -> Result<()> {
        // Which `#[process]` methods this app serves is the port's answer —
        // module-gated, duplicate-checked, announced — not this backend's.
        self.methods = consume::discover(container)?;

        // A queue this backend cannot file is a boot error, not a worker that
        // drains the bindings' own keys.
        for method in &self.methods {
            RedisJob::check_queue(method.queue)
                .with_context(|| format!("RedisWorker cannot serve {}", method.name))?;
        }

        // Fail fast at boot if methods exist but no connection is seeded.
        if !self.methods.is_empty() {
            container.get::<RedisConnection>().with_context(|| {
                format!("RedisWorker found #[processor]s but {CONNECTION_REMEDY}")
            })?;
        }

        self.container = Some(container.clone());
        Ok(())
    }

    async fn serve(self: Box<Self>, cancel: CancellationToken) -> Result<()> {
        let Self { methods, container } = *self;
        // No methods: idle until shutdown so this transport doesn't race
        // the app down when it is the only one attached.
        if methods.is_empty() {
            cancel.cancelled().await;
            return Ok(());
        }

        let container = container.expect("RedisWorker::configure must run before serve");
        let connection = container
            .get::<RedisConnection>()
            .expect("RedisConnection presence is verified in configure");

        // Bound the post-signal drain so a hung `#[process]` can't block SIGTERM
        // until the orchestrator SIGKILLs the pod (QUEUE-I5). The config is a
        // factory output `RedisWorkerModule::for_root` resolved.
        let shutdown_timeout = container
            .get::<crate::RedisWorkerConfig>()
            .map(|cfg| cfg.shutdown_timeout)
            .unwrap_or_else(|| crate::RedisWorkerConfig::default().shutdown_timeout);

        let storage = RedisJob::storage(&connection)
            .context("RedisWorker could not open the queue storage")?;
        let in_flight = Arc::new(AtomicUsize::new(0));
        let served = Served {
            container,
            methods: Arc::new(
                methods
                    .iter()
                    .map(|method| (method.queue, *method))
                    .collect(),
            ),
            in_flight: Arc::clone(&in_flight),
        };
        let signal = cancel.clone();
        let mut runtime = storage
            .runtime(served)
            .worker::<Deliver, Received>()
            // The app owns the shutdown. oxana's default listens for SIGTERM on
            // its own, which would start this drain before the app decided to.
            .shutdown_on(async move {
                signal.cancelled_owned().await;
                Ok(())
            })
            .shutdown_timeout(shutdown_timeout)
            .dequeue_timeout(POLL_INTERVAL)
            .redis_failure_tolerance(OUTLAST_EVERY_REDIS_OUTAGE)
            .error_formatter(describe);
        for method in &methods {
            runtime = runtime.queue_with(QueueConfig::as_static(method.queue).concurrency(1));
        }

        // Whether oxana returns once its own `shutdown_timeout` elapses or keeps
        // awaiting a job that never ends, the drain stops here: the deadline is
        // this transport's, so a hung handler cannot hold the process until
        // SIGKILL. Either way a job still running when the drain stops is
        // abandoned, and oxana's own line on that is an `error` naming no job —
        // so the count is kept here, and said here.
        let run = std::pin::pin!(runtime.run());
        let deadline = async {
            cancel.cancelled().await;
            tokio::time::sleep(shutdown_timeout + SHUTDOWN_GRACE).await;
        };
        let outcome = tokio::select! {
            outcome = run => Some(outcome),
            () = deadline => None,
        };
        let abandoned = in_flight.load(Ordering::SeqCst);
        if cancel.is_cancelled() && abandoned > 0 {
            tracing::warn!(
                target: nest_rs_queue::TARGET,
                abandoned,
                shutdown_timeout_secs = shutdown_timeout.as_secs(),
                "worker shutdown timed out; abandoning the jobs still running",
            );
        }
        if let Some(outcome) = outcome {
            outcome.context("the Redis queue runtime stopped with an error")?;
        }
        Ok(())
    }
}

/// What oxana hands the worker it builds for every job: the app's container,
/// the method serving each queue, and the count a shutdown reports.
#[derive(Clone)]
struct Served {
    container: Container,
    methods: Arc<HashMap<&'static str, &'static ProcessMethod>>,
    /// Jobs started and not finished. A job the runtime drops mid-attempt
    /// never finishes, so a job abandoned at shutdown stays counted whether it
    /// was dropped or is still running.
    in_flight: Arc<AtomicUsize>,
}

impl Served {
    /// Decode one fetched record and find the method its queue names — or fail
    /// it, with the event that says which of the two went wrong. Only a record
    /// written around the producer fails here: the producer writes a decodable
    /// record naming the queue it pushed to, and the runtime drains the queues
    /// `methods` was built from.
    fn route(
        &self,
        record: serde_json::Value,
        job_id: &str,
    ) -> Result<(&'static ProcessMethod, serde_json::Value), Undelivered> {
        let job: RedisJob = serde_json::from_value(record).map_err(|error| {
            tracing::error!(
                target: nest_rs_queue::TARGET,
                job_id,
                error = %error,
                "job dead-lettered: its Redis record does not decode",
            );
            Undelivered(Box::new(error))
        })?;
        let Some(method) = self.methods.get(job.queue.as_str()).copied() else {
            tracing::error!(
                target: nest_rs_queue::TARGET,
                queue = %job.queue,
                job_id,
                "job dead-lettered: no #[process] method serves its queue",
            );
            return Err(Undelivered(
                format!("no #[process] method serves queue `{}`", job.queue).into(),
            ));
        };
        Ok((method, job.message))
    }
}

/// The one oxana worker, routing each job to the method its queue names.
struct Deliver(Served);

impl Deliver {
    /// Route the record and run its method's attempts until one settles it.
    async fn deliver(&self, record: Received, ctx: &JobContext) -> Result<(), Undelivered> {
        let (method, mut message) = self.0.route(record.0, &ctx.meta.id)?;
        let mut attempt = 1;
        loop {
            // Only an attempt another may follow needs its own copy.
            let payload = if attempt <= method.retries {
                message.clone()
            } else {
                std::mem::take(&mut message)
            };
            let outcome = consume::attempt(
                method,
                payload,
                ctx.meta.id.clone(),
                attempt,
                self.0.container.clone(),
            )
            .await;
            match settle(outcome, attempt, method.retries) {
                ControlFlow::Continue(()) => attempt += 1,
                ControlFlow::Break(result) => return result,
            }
        }
    }
}

impl FromContext<Served> for Deliver {
    fn from_context(served: &Served) -> Self {
        Self(served.clone())
    }
}

#[async_trait]
impl oxana::Worker<Received> for Deliver {
    type Error = Undelivered;

    async fn process(&self, record: Received, ctx: &JobContext) -> Result<(), Undelivered> {
        self.0.in_flight.fetch_add(1, Ordering::SeqCst);
        let settled = self.deliver(record, ctx).await;
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
        settled
    }

    /// Zero, always: `process` keeps the method's budget — see the module docs.
    fn max_retries(&self, _job: &Received) -> u32 {
        0
    }
}

/// The one thing this backend decides about an outcome: whether the job runs
/// again. A retryable failure does while the method's budget lasts — `retries`
/// re-runs after the first attempt — and a dead letter never does, so a payload
/// that cannot succeed is not re-run on its way to the dead list.
fn settle(
    outcome: Attempt,
    attempt: usize,
    retries: usize,
) -> ControlFlow<Result<(), Undelivered>> {
    match outcome {
        Attempt::Ok => ControlFlow::Break(Ok(())),
        Attempt::Retry(_) if attempt <= retries => ControlFlow::Continue(()),
        Attempt::Retry(error) | Attempt::DeadLetter(error) => {
            ControlFlow::Break(Err(Undelivered(error.source)))
        }
    }
}

/// The error a dead-list record carries: the message and every cause beneath
/// it, where oxana's default is the `Debug` form of the outermost wrapper.
fn describe(error: &(dyn std::error::Error + Send + Sync + 'static)) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(inner) = cause {
        text.push_str(": ");
        text.push_str(&inner.to_string());
        cause = inner.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;

    use nest_rs_queue::JobError;
    use serde_json::json;

    use super::*;

    type Handler = Pin<Box<dyn Future<Output = Result<(), JobError>> + Send>>;

    fn noop(_job: serde_json::Value, _container: Container) -> Handler {
        Box::pin(async { Ok(()) })
    }

    static AUDIO: ProcessMethod = ProcessMethod {
        origin: module_path!(),
        name: "AudioProcessor::transcode",
        queue: "audio",
        retries: 0,
        provider_type_id: || std::any::TypeId::of::<()>(),
        handler: noop,
    };

    fn served() -> Served {
        Served {
            container: Container::builder().build(),
            methods: Arc::new(HashMap::from([("audio", &AUDIO)])),
            in_flight: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// The settlement is a few lines and nothing else exercises it without a
    /// live Redis: a swap of its arms would pass every in-process suite, and a
    /// dead letter would re-run a payload that cannot succeed for the whole
    /// budget.
    #[test]
    fn a_dead_letter_never_runs_again_and_a_retry_runs_while_the_budget_lasts() {
        assert!(
            matches!(
                settle(Attempt::DeadLetter(JobError::abort("bad payload")), 1, 3),
                ControlFlow::Break(Err(_)),
            ),
            "a dead letter fails the job at once, whatever budget is left",
        );
        assert!(
            matches!(
                settle(Attempt::Retry(JobError::retry("upstream timed out")), 1, 1),
                ControlFlow::Continue(()),
            ),
            "a retryable failure runs again while the budget lasts",
        );
        assert!(
            matches!(
                settle(Attempt::Retry(JobError::retry("upstream timed out")), 2, 1),
                ControlFlow::Break(Err(_)),
            ),
            "`retries = 1` is two attempts, and the second failing ends the job",
        );
        assert!(matches!(
            settle(Attempt::Ok, 1, 0),
            ControlFlow::Break(Ok(()))
        ));
    }

    #[test]
    fn a_record_reaches_the_method_its_queue_names_with_its_message() {
        let (method, message) = served()
            .route(json!({ "queue": "audio", "message": { "v": 1 } }), "job-1")
            .unwrap_or_else(|error| panic!("a producer's record routes: {error}"));
        assert_eq!(method.name, "AudioProcessor::transcode");
        assert_eq!(message, json!({ "v": 1 }));
    }

    /// A job the runtime fetched and nothing here can run must not vanish into
    /// the dead list with only the runtime's own line: the operator reading
    /// `nest_rs::queue` needs the queue that has no method.
    #[test]
    fn a_record_naming_a_queue_no_method_serves_is_dead_lettered_with_an_event() {
        let logs = nest_rs_testing::LogCapture::install();
        let Err(error) = served().route(json!({ "queue": "reports", "message": {} }), "job-2")
        else {
            panic!("a queue with no method cannot route")
        };
        assert!(
            describe(&error).contains("reports"),
            "the dead list's error names the queue: {error}",
        );

        let event = logs.expect_one(
            nest_rs_queue::TARGET,
            "job dead-lettered: no #[process] method serves its queue",
        );
        assert_eq!(event.level, "error");
        assert_eq!(event.field("queue").as_deref(), Some("reports"));
        assert_eq!(event.field("job_id").as_deref(), Some("job-2"));
    }

    /// The record oxana used to decode before the worker saw it: a failure
    /// there logged a fieldless line on the runtime's own target and nothing on
    /// the framework's.
    #[test]
    fn a_record_that_does_not_decode_is_dead_lettered_with_an_event() {
        let logs = nest_rs_testing::LogCapture::install();
        assert!(
            served()
                .route(json!({ "payload": "written around the producer" }), "job-3")
                .is_err(),
            "a record with no queue cannot route",
        );

        let event = logs.expect_one(
            nest_rs_queue::TARGET,
            "job dead-lettered: its Redis record does not decode",
        );
        assert_eq!(event.level, "error");
        assert_eq!(event.field("job_id").as_deref(), Some("job-3"));
        assert!(event.field("error").is_some(), "{:?}", event.fields);
    }

    #[test]
    fn a_dead_list_error_reads_as_its_message_and_every_cause() {
        #[derive(Debug, thiserror::Error)]
        #[error("audio storage failed")]
        struct StorageFailure(#[source] std::io::Error);

        let failure = Undelivered(Box::new(StorageFailure(std::io::Error::other(
            "bucket unreachable",
        ))));
        assert_eq!(
            describe(&failure),
            "audio storage failed: bucket unreachable"
        );
    }
}

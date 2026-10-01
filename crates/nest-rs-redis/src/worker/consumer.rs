//! apalis-redis `JobConsumer` exposed as a `Transport`: one apalis worker per
//! discovered `#[process]` method on a shared [`Monitor`].
//!
//! **This file is the transport and nothing else.** What a job attempt *is* —
//! the envelope, the trace, the `queue.job` span, the panic catch, the outcome
//! classes, the retry budget, the events and the operation line — is the port's
//! (`nest_rs_queue::consume::attempt`), and discovery is the port's too
//! (`consume::discover`, which refuses every declaration this backend does not
//! honour). What one delivery does with a fetched job — its lease, its attempt,
//! how it settles — is [`Deliveries`]'s. What stays here is what apalis alone
//! knows: the storage handle and its settings, each replica's identity, a
//! method's `concurrency`, what apalis says about itself, and the drain at
//! shutdown.
//!
//! Every queue is consumed as `RedisStorage<serde_json::Value>` — the
//! backend-agnostic wire format — under the queue's namespace,
//! `nestrs:queue:<queue>` ([`crate::layout`]).
//!
//! **Concurrency is per method, per replica.** `#[process(concurrency = N)]`
//! bounds how many attempts of that method one worker replica runs at once
//! (default 1); throughput beyond it comes from running more replicas, which the
//! platform schedules and meters. Each method is its own apalis worker, so one
//! method's jobs never wait on another's permits, and it fetches up to `N` jobs
//! per poll while a permit is free — see `storage`.
//!
//! **Every replica is its own consumer.** apalis names a worker's in-flight set
//! after the worker's id, and hands a worker's jobs to its peers once the worker
//! has not proved it is alive for the orphan threshold. So each method of each
//! replica consumes under an id of its own — the host, then a UUID v7 — and the
//! threshold is ten of its heartbeats ([`RedisWorkerConfig::orphan_after`]),
//! never *now*: a live peer's jobs are never taken on the periodic sweep, and a
//! crashed one's are once the threshold passes.
//!
//! **apalis's startup sweep is the one that takes a live peer's jobs**, and it
//! cannot be configured away: a worker starting calls `reenqueue_orphaned` with
//! a cutoff of *now*, which matches every registered consumer, alive or not, and
//! puts up to ten times its fetch size — the method's `concurrency` — of their
//! in-flight jobs back on the queue. The delivery guard
//! is what makes that harmless — the job's lease is held by the delivery running
//! it, so the second delivery hands it back until the first has settled it, then
//! acknowledges it without running. Measured in `tests/e2e/worker/`.
//!
//! **Due records reach the queue at the fetch's pace.** A record held back —
//! a delayed push, a retry's next attempt, a job handed back — waits on the
//! queue's schedule until a scan moves it onto `active`, where the worker fetches
//! it and an autoscaler counts it. Each method's worker runs a [`Promotion`] of
//! its own, every second, moving up to a hundred due records or its
//! `concurrency`, whichever is more — 799 at the most — so a burst of due retries
//! or hand-backs reaches `active` within seconds rather than one fetch's worth a
//! second. apalis's own scan, which moves one fetch's worth, is left at its
//! default thirty seconds, a backstop.
//!
//! **A shutdown drains within its window.** The worker stops fetching at once;
//! attempts running get [`RedisWorkerConfig::shutdown_timeout`] less a reserve to
//! finish, and whatever still runs when that closes is interrupted and handed
//! back to the queue inside the reserve — so the orchestrator's SIGKILL never
//! lands on a job the worker still holds. The reserve is one of the connection's
//! budgets, the time Redis is given to answer a call, so the start an interrupted
//! attempt gives back is waited for as any call is (`hand_back_reserve`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use apalis::layers::ErrorHandlingLayer;
use apalis::layers::WorkerBuilderExt;
use apalis::layers::catch_panic::CatchPanicLayer;
use apalis::prelude::{Attempt, Event, Monitor, TaskId, Worker, WorkerBuilder, WorkerFactoryFn};
use apalis_redis::{Config, RedisPollError, RedisStorage};
use async_trait::async_trait;
use nest_rs_core::{Container, Transport};
use nest_rs_queue::consume;
use nest_rs_queue::{ProcessMethod, QueueName};
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;

use super::delivery::{Deliveries, Task};
use super::gate::ThrottleGate;
use super::lease::Leases;
use crate::backend::{BACKEND, uncapped_context};
use crate::connection::CONNECTION_REMEDY;
use crate::error::LegacyLayoutError;
use crate::legacy_layout::{self, LegacyLayout};
use crate::promotion::{self, Promotion};
use crate::{RedisConnection, RedisWorkerConfig, layout};

/// How often apalis's own scan moves due records onto the queue, one fetch's
/// worth each time: its default, half a minute. The worker's [`Promotion`]
/// moves them every second, a larger batch at a time, so this scan only backs
/// it up.
const APALIS_SCAN: Duration = Duration::from_secs(30);

/// The least a drain keeps back from its window to hand interrupted jobs back
/// in, when the connection's budget is shorter: five seconds, which a healthy
/// Redis spends in milliseconds.
const HAND_BACK_RESERVE: Duration = Duration::from_secs(5);

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
        // module-gated, duplicate-checked, refused where this backend lacks a
        // capability a method declares, announced — not this backend's.
        self.methods = consume::discover(container, &BACKEND)?;

        // Fail fast at boot if methods exist but no connection is seeded.
        if !self.methods.is_empty() {
            let connection = container.get::<RedisConnection>().with_context(|| {
                format!("RedisWorker found #[processor]s but {CONNECTION_REMEDY}")
            })?;
            refuse_legacy_jobs(&connection, &self.methods).await?;
        }

        self.container = Some(container.clone());
        Ok(())
    }

    async fn serve(self: Box<Self>, cancel: CancellationToken) -> Result<()> {
        // No methods: idle until shutdown so this transport doesn't race
        // the app down when it is the only one attached.
        if self.methods.is_empty() {
            cancel.cancelled().await;
            return Ok(());
        }

        let container = self
            .container
            .context("RedisWorker::configure must run before serve")?;
        let connection = container
            .get::<RedisConnection>()
            .with_context(|| format!("RedisWorker found #[processor]s but {CONNECTION_REMEDY}"))?;
        // A factory output `RedisWorkerModule::for_root` resolved; a worker
        // attached without the module runs on the defaults.
        let config = container
            .get::<RedisWorkerConfig>()
            .map(|config| (*config).clone())
            .unwrap_or_default();

        let interrupt = CancellationToken::new();
        let underway = Arc::new(AtomicUsize::new(0));
        // Each method's promotion, for as long as `serve` runs — the drain
        // included, where a job handed back is due at once.
        let mut promoting = Vec::new();
        let mut built = Vec::new();
        for method in &self.methods {
            let queue = QueueName::new(method.queue())?;
            built.push((method, queue, worker_id()));
        }
        let mut workers = HashMap::new();
        let mut monitor = Monitor::new();
        for (method, queue, id) in built {
            promoting.push(AbortOnDropHandle::new(tokio::spawn(promote(
                Promotion::new(&connection, &queue, promoted_per_scan(concurrency(method))),
            ))));
            let filing = uncapped_context(Some(&id)).with_context(|| {
                format!(
                    "RedisWorker could not build the apalis context queue `{queue}`'s records are \
                     handed back under"
                )
            })?;
            let deliveries = Arc::new(Deliveries {
                method,
                queue: queue.clone(),
                conn: (*connection).clone(),
                worker: id.clone(),
                container: container.clone(),
                storage: storage(&connection, &queue, &config, concurrency(method)),
                filing,
                leases: Leases::new(
                    (*connection).clone(),
                    queue,
                    config.lease,
                    config.orphan_after,
                    method.options().throttle(),
                ),
                gate: ThrottleGate::new(cancel.clone()),
                interrupt: interrupt.clone(),
                underway: Arc::clone(&underway),
            });
            let fetching = storage(
                &connection.without_budget(),
                &deliveries.queue,
                &config,
                concurrency(method),
            );
            workers.insert(id.clone(), deliveries.queue.clone());
            monitor = register(monitor, &id, method, fetching, deliveries);
        }
        let reporter = Reporter::new(workers);
        let monitor = monitor.on_event(move |event| reporter.report(&event));

        let signal = cancel.clone();
        let run = monitor.run_with_signal(async move {
            signal.cancelled().await;
            Ok(())
        });
        tokio::pin!(run);
        let stopped = tokio::select! {
            finished = &mut run => Some(finished),
            () = cancel.cancelled() => None,
        };
        match stopped {
            Some(finished) => finished.map_err(anyhow::Error::from),
            None => {
                let window = config.shutdown_timeout;
                let reserve = hand_back_reserve(window, connection.budget());
                drain(run, &interrupt, window, reserve, &underway).await
            }
        }
    }
}

/// Refuse to serve a queue that still holds jobs under the 6.x layout, naming
/// every such queue and what to do — a worker started beside them would leave
/// them waiting with nothing to say so.
async fn refuse_legacy_jobs(conn: &RedisConnection, methods: &[&ProcessMethod]) -> Result<()> {
    let mut refused = Vec::new();
    for method in methods {
        let queue = QueueName::new(method.queue())?;
        match LegacyLayout::read(conn, &queue).await {
            Ok(found) if found.is_empty() => {}
            Ok(found) => refused.push(
                LegacyLayoutError {
                    queue: queue.to_string(),
                    keys: found.held().join(", "),
                    namespace: layout::namespace(&queue),
                }
                .to_string(),
            ),
            // A user scoped to the framework's prefix cannot read the root, and
            // could not have written the 6.x layout either — but another could
            // have, so the gap is said rather than passed over.
            Err(error) if legacy_layout::outside_the_acl(&error) => tracing::warn!(
                target: nest_rs_queue::TARGET,
                queue = %queue,
                error = %nest_rs_core::error_message(&error),
                "6.x key layout not checked: the connection's ACL does not reach it; drain any 6.x \
                 jobs on this database before relying on this worker",
            ),
            Err(error) => {
                return Err(anyhow::Error::new(error).context(format!(
                    "RedisWorker could not check queue `{queue}` for jobs under the 6.x key layout"
                )));
            }
        }
    }
    if refused.is_empty() {
        Ok(())
    } else {
        Err(anyhow::anyhow!("{}", refused.join("\n")))
    }
}

/// What a drain keeps back from its `window` to hand interrupted jobs back in:
/// one of the connection's `budget`s — an interrupted attempt gives its start
/// back in one call, and a reserve shorter than the time Redis is given to
/// answer one gives up on a Redis that is only slow — or [`HAND_BACK_RESERVE`]
/// when the budget is shorter, and never more than half the window, which the
/// running attempts keep.
fn hand_back_reserve(window: Duration, budget: Option<Duration>) -> Duration {
    HAND_BACK_RESERVE
        .max(budget.unwrap_or_default())
        .min(window / 2)
}

/// Let the running attempts finish within the window, then interrupt the rest
/// and give them the reserve to hand their jobs back. A worker still not
/// stopped once the whole window has passed is said, with how many deliveries
/// it cut — `underway`, every method's — and left: each of those jobs stays in
/// flight until a replica sweeps it.
async fn drain<F>(
    mut run: std::pin::Pin<&mut F>,
    interrupt: &CancellationToken,
    window: Duration,
    reserve: Duration,
    underway: &AtomicUsize,
) -> Result<()>
where
    F: std::future::Future<Output = std::io::Result<()>>,
{
    let patience = window.saturating_sub(reserve);
    if let Ok(finished) = tokio::time::timeout(patience, run.as_mut()).await {
        return Ok(finished?);
    }
    tracing::warn!(
        target: nest_rs_queue::TARGET,
        shutdown_timeout_ms = millis(window),
        reserve_ms = millis(reserve),
        "queue jobs still running as the shutdown window closes; interrupting them to hand their \
         jobs back",
    );
    interrupt.cancel();
    match tokio::time::timeout(reserve, run.as_mut()).await {
        Ok(finished) => Ok(finished?),
        Err(_) => {
            tracing::error!(
                target: nest_rs_queue::TARGET,
                shutdown_timeout_ms = millis(window),
                reserve_ms = millis(reserve),
                cut = underway.load(Ordering::Relaxed),
                "queue workers did not stop within the shutdown window; a delivery cut here leaves \
                 its job in flight until a replica sweeps it, its attempt spent unless the \
                 give-back reached Redis",
            );
            Ok(())
        }
    }
}

/// The storage one method's worker reads, under its queue's namespace.
///
/// apalis's worker runs it on a connection [`without_budget`]: its fetch claims
/// the jobs it answers with, so a fetch cut at the budget still runs and leaves
/// them in this replica's flight, where nothing ever runs them while it lives.
/// Its heartbeat and its acknowledgements ride the same handle — apalis gives a
/// storage one connection — and answer late rather than not at all. They share
/// its loop too, one call at a time, so a stall past the orphan threshold holds
/// the heartbeat as long, and a sweep — a peer's, or this replica's own — puts
/// the jobs in flight back on the queue while the fetch still delivers them:
/// a second delivery the guard answers, where a cut fetch strands them. A
/// delivery's own hand-backs go through a storage on the budgeted connection:
/// each is safe to cut (`hand_back`).
///
/// [`without_budget`]: RedisConnection::without_budget
fn storage(
    conn: &RedisConnection,
    queue: &QueueName,
    config: &RedisWorkerConfig,
    concurrency: usize,
) -> RedisStorage<serde_json::Value, RedisConnection> {
    RedisStorage::new_with_config(conn.clone(), fetching(queue, config, concurrency))
}

/// How many values Redis's Lua hands to one command through `unpack`: 7,999,
/// its stack's 8,000 slots less the command itself. Pinned against every Redis
/// the e2e suite runs on.
const LUA_UNPACK_LIMIT: usize = 7_999;

/// How many fetches' worth of a silent peer's in-flight jobs apalis's sweep
/// moves in one script.
const FETCHES_PER_SWEEP: usize = 10;

/// The most jobs one fetch claims, whatever the method's `concurrency`.
/// apalis's scripts hand a fetch's ids to Redis in one Lua `unpack`, and its
/// sweep hands over ten fetches' worth of a peer's jobs the same way. Past the
/// limit a fetch fails on every poll for as long as that many jobs wait, and a
/// sweep fails *after* popping the jobs it could not push — losing them. Ten
/// fetches of 799 stay under it.
const MOST_PER_FETCH: usize = LUA_UNPACK_LIMIT / FETCHES_PER_SWEEP;

/// The settings one method's storage reads Redis with, on top of its queue's
/// namespace.
///
/// **A fetch takes up to the method's `concurrency`, once per poll** — 799 at
/// the most (`MOST_PER_FETCH`). apalis asks Redis for jobs every
/// [`RedisWorkerConfig::poll_interval`], and only while the worker is ready —
/// which the method's permits decide, a worker with every permit taken being
/// not ready. One script then claims up to `buffer_size` jobs, handed to the
/// worker through a channel where each waits for a permit. Sized at
/// `concurrency`, one poll fills every permit a method has, so a method whose
/// jobs are short runs up to `concurrency` jobs per poll on one replica, not
/// one. The price is what a busy replica holds: a fetch needs one free permit
/// and may bring `concurrency` jobs, so a replica keeps up to `concurrency - 1`
/// jobs it fetched and has not started — `concurrency` when Redis takes longer
/// than a poll to answer, since apalis then fetches again before the worker has
/// seen the first batch. They wait in its in-flight set for its next free
/// permits, invisible to its peers and to an autoscaler reading the queue, and a
/// replica that dies hands them over with its running jobs.
///
/// The fetch size bounds apalis's two other loops the same way. A record
/// scheduled for later — a delayed push, a retry, a job handed back — becomes
/// available on apalis's `enqueue_scheduled` scan, which moves up to
/// `buffer_size` due records per scan: one due record a second, at the default
/// concurrency, were it the one moving them. The worker's [`Promotion`] moves
/// them instead, every second and a hundred at the least (`promoted_per_scan`),
/// so apalis's scan stays at its default half a minute, a backstop. And every
/// poll, as at start, apalis sweeps up to ten times `buffer_size` of the jobs
/// silent peers held back onto the queue.
///
/// The heartbeat and the orphan threshold are the config's: a worker proves it
/// is alive every tenth of the threshold, so a peer only ever sweeps one that
/// missed ten in a row.
fn fetching(queue: &QueueName, config: &RedisWorkerConfig, concurrency: usize) -> Config {
    layout::config(queue)
        .set_buffer_size(concurrency.min(MOST_PER_FETCH))
        .set_poll_interval(config.poll_interval)
        .set_enqueue_scheduled(APALIS_SCAN)
        .set_keep_alive(config.heartbeat())
        .set_reenqueue_orphaned_after(config.orphan_after)
}

/// How many due records a method's promotion moves per scan: a hundred, or its
/// `concurrency` when that is more — a burst of due jobs then reaches `active`
/// at least as fast as one replica fetches it — and never more than one fetch
/// may claim, the most apalis's scripts hand Lua at once.
fn promoted_per_scan(concurrency: usize) -> usize {
    concurrency.clamp(promotion::BATCH, MOST_PER_FETCH)
}

// `clamp` panics when its floor passes its ceiling; both are constants, so the
// order is checked where they are compiled rather than on every worker's start.
const _: () = assert!(promotion::BATCH <= MOST_PER_FETCH);

/// Move `promotion`'s due records onto its queue, a scan a tick, until the task
/// is aborted — when `serve` returns.
async fn promote(mut promotion: Promotion) {
    loop {
        tokio::time::sleep(promotion.wait()).await;
        promotion.scan().await;
    }
}

/// How many attempts of `method` one replica runs at once — its permits, and
/// the most one fetch of its takes.
fn concurrency(method: &ProcessMethod) -> usize {
    usize::try_from(method.options().concurrency().get()).unwrap_or(usize::MAX)
}

/// Register one apalis worker for a `ProcessMethod` on `monitor`. The wire
/// payload is always `serde_json::Value`; the port's `attempt` opens the
/// envelope and the macro-emitted handler deserializes it to the method's job
/// type, so this builder never names it.
fn register(
    monitor: Monitor,
    id: &str,
    method: &'static ProcessMethod,
    storage: RedisStorage<serde_json::Value, RedisConnection>,
    deliveries: Arc<Deliveries>,
) -> Monitor {
    // Position is load-bearing for the panic layer: a delivery runs on a task of
    // its own and `consume::attempt` catches a handler panic itself (so the event
    // lands inside the per-job span), which leaves this layer as the **backstop**
    // for a panic outside both — in apalis's own fetch/deserialize path, or in
    // the closure prologue. It turns one into apalis's `Abort`, which
    // dead-letters rather than re-queues, so one bad job cannot take down the
    // queue's consumer.
    let worker = WorkerBuilder::new(id)
        // The throttle's gate, outermost: while the method's window is full the
        // worker is not ready, so apalis fetches nothing and takes no permit —
        // the backlog waits on the queue, not round the schedule.
        .layer(deliveries.gate.layer())
        // The method's permits, so a permit covers a job's whole delivery.
        // apalis delegates `poll_ready` to the inner service, so with every
        // permit held the fetch loop backs off rather than piling work into
        // memory: the next job stays in Redis, where another replica can take
        // it.
        .concurrency(concurrency(method))
        .layer(ErrorHandlingLayer::new())
        .layer(CatchPanicLayer::new())
        .backend(storage)
        .build_fn(
            move |job: serde_json::Value, task_id: TaskId, attempt: Attempt| {
                let task = Task {
                    id: task_id,
                    attempt,
                };
                Arc::clone(&deliveries).deliver(job, task)
            },
        );
    monitor.register(worker)
}

/// This replica's id for one method's worker: the host it runs on, when the
/// platform names it, then a UUID v7 — unique per replica and per queue, so no
/// two replicas share an in-flight set, and the sweep that follows a crash
/// finds the crashed replica's jobs and only those.
fn worker_id() -> String {
    let id = uuid::Uuid::now_v7();
    match std::env::var("HOSTNAME") {
        Ok(host)
            if !host.is_empty()
                && host
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) =>
        {
            format!("{host}:{id}")
        }
        _ => id.to_string(),
    }
}

/// How often a worker unregistered by a peer's sweep is said to be, at most:
/// apalis reports it on every poll until the worker registers again.
const UNREGISTERED_REPEAT: Duration = Duration::from_secs(30);

/// Says what apalis says about the workers of one replica, at the level each
/// event deserves. apalis logs nothing through `tracing` itself, so an event
/// dropped here is a failure nobody hears of.
struct Reporter {
    /// Each worker's queue, by the id apalis names it with.
    workers: HashMap<String, QueueName>,
    /// When each worker was last said to be unregistered.
    unregistered: Mutex<HashMap<String, Instant>>,
}

impl Reporter {
    fn new(workers: HashMap<String, QueueName>) -> Self {
        Self {
            workers,
            unregistered: Mutex::default(),
        }
    }

    fn report(&self, event: &Worker<Event>) {
        let worker = event.id().name();
        let queue = self
            .workers
            .get(worker)
            .map(QueueName::as_str)
            .unwrap_or_default();
        match event.inner() {
            Event::Start => tracing::info!(
                target: nest_rs_queue::TARGET,
                queue,
                worker,
                "queue worker started",
            ),
            Event::Exit => tracing::info!(
                target: nest_rs_queue::TARGET,
                queue,
                worker,
                "queue worker stopped",
            ),
            Event::Stop | Event::Engage(_) | Event::Idle | Event::Custom(_) => {}
            Event::Error(error) if unregistered(error.as_ref()) => {
                self.report_unregistered(queue, worker);
            }
            Event::Error(error) => report_error(queue, worker, error.as_ref()),
        }
    }

    /// A starting peer's sweep unregistered this worker along with the jobs it
    /// took, and apalis fails every fetch until it registers again — at once
    /// against Redis 7 and later, whose script errors carry the words apalis
    /// looks for, and at the worker's next heartbeat against Redis 6.2, whose
    /// errors do not. Said once per stretch, since apalis repeats it every poll.
    fn report_unregistered(&self, queue: &str, worker: &str) {
        let now = Instant::now();
        let fresh = {
            let mut said = self
                .unregistered
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let fresh = said
                .get(worker)
                .is_none_or(|last| now.duration_since(*last) >= UNREGISTERED_REPEAT);
            if fresh {
                said.insert(worker.to_owned(), now);
            }
            fresh
        };
        if fresh {
            tracing::info!(
                target: nest_rs_queue::TARGET,
                queue,
                worker,
                "queue worker unregistered by a starting peer's sweep; it fetches again once \
                 registered, at its next heartbeat at the latest",
            );
        }
    }
}

/// Whether `error` is a fetch refused because a peer's sweep unregistered the
/// worker.
fn unregistered(error: &(dyn std::error::Error + 'static)) -> bool {
    matches!(
        error.downcast_ref::<RedisPollError>(),
        Some(RedisPollError::PollNextError(cause)) if cause.to_string().contains("consumer not registered")
    )
}

/// An error a worker met, classified by where it came from.
fn report_error(queue: &str, worker: &str, error: &(dyn std::error::Error + 'static)) {
    let message = nest_rs_core::error_message(error);
    // A delivery's own answer — a dead letter, a hand-back Redis refused — is
    // reported where it was decided; apalis echoing it back is not a second
    // event.
    if error.downcast_ref::<apalis::prelude::Error>().is_some() {
        tracing::debug!(target: nest_rs_queue::TARGET, queue, worker, error = %message, "queue delivery answered with an error");
        return;
    }
    match error.downcast_ref::<RedisPollError>() {
        Some(poll) => report_trouble(Trouble::of(poll), queue, worker, &message),
        None => {
            tracing::warn!(target: nest_rs_queue::TARGET, queue, worker, error = %message, "queue worker error")
        }
    }
}

/// Which of apalis's own calls a worker's heartbeat failed at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Trouble {
    /// Fetching the next job: the connection failed under it, or Redis refused
    /// it.
    Fetch,
    /// Fetching the next job: Redis answered, and one record it handed back is
    /// not one apalis can decode.
    Undecodable,
    /// Proving the worker is alive.
    Heartbeat,
    /// Moving due records from the schedule onto the queue.
    Promotion,
    /// Taking a silent peer's jobs back.
    Reclaim,
    /// Handing a fetched job to the worker's own loop.
    Handoff,
    /// Recording a job's outcome.
    Acknowledgement,
}

impl Trouble {
    fn of(error: &RedisPollError) -> Self {
        match error {
            RedisPollError::PollNextError(cause) if undecodable(cause) => Self::Undecodable,
            RedisPollError::PollNextError(_) => Self::Fetch,
            RedisPollError::KeepAliveError(_) => Self::Heartbeat,
            RedisPollError::EnqueueScheduledError(_) => Self::Promotion,
            RedisPollError::ReenqueueOrphanedError(_) => Self::Reclaim,
            RedisPollError::EnqueueError(_) => Self::Handoff,
            RedisPollError::AckError(_) => Self::Acknowledgement,
        }
    }
}

/// Whether a fetch failed on a record apalis could not decode, rather than on
/// the connection or a refusal.
///
/// apalis-redis 0.7.4 decodes a fetch's records after the script that claimed
/// them has run, and reports the first it cannot read as an `InvalidData` io
/// error of its own making. The connection reports two errors of that kind as
/// well, both TLS: a refused negotiation, which carries rustls's error, and a
/// connection `redis` failed to reopen, which it words `Reconnecting failed: …`
/// — neither is a record.
fn undecodable(error: &redis::RedisError) -> bool {
    std::error::Error::source(error)
        .and_then(|source| source.downcast_ref::<std::io::Error>())
        .is_some_and(|io| {
            io.kind() == std::io::ErrorKind::InvalidData
                && !crate::tls::negotiation_failed(error)
                && !io.to_string().starts_with(REOPEN_FAILED)
        })
}

/// How `redis` 0.32 words a connection it could not reopen, before the cause.
const REOPEN_FAILED: &str = "Reconnecting failed";

/// The line for `trouble`: `warn` for a call apalis makes again on its own, and
/// `error` where jobs wait that nothing will run while this replica lives — a
/// lost acknowledgement, a fetch that met a record apalis cannot decode.
fn report_trouble(trouble: Trouble, queue: &str, worker: &str, error: &str) {
    match trouble {
        // A fetch that failed because its connection did may have run: the jobs
        // it claimed then wait in this replica's flight, which only a starting
        // replica's sweep empties. A fetch Redis refused claimed nothing.
        Trouble::Fetch => tracing::warn!(
            target: nest_rs_queue::TARGET,
            queue,
            worker,
            error,
            "queue fetch failed; retrying at the next poll, and any job it claimed before failing \
             waits in flight until a replica starts",
        ),
        Trouble::Undecodable => tracing::error!(
            target: nest_rs_queue::TARGET,
            queue,
            worker,
            error,
            "queue fetch met a record apalis cannot decode; it and every job fetched beside it \
             wait in flight until a replica starts",
        ),
        Trouble::Heartbeat => tracing::warn!(
            target: nest_rs_queue::TARGET,
            queue,
            worker,
            error,
            "queue worker heartbeat failed; its peers take its jobs if it keeps failing",
        ),
        Trouble::Promotion => tracing::warn!(
            target: nest_rs_queue::TARGET,
            queue,
            worker,
            error,
            "scheduled jobs not moved onto the queue; retrying at the next scan",
        ),
        Trouble::Reclaim => tracing::warn!(
            target: nest_rs_queue::TARGET,
            queue,
            worker,
            error,
            "orphaned jobs not reclaimed; retrying at the next sweep",
        ),
        Trouble::Handoff => tracing::warn!(
            target: nest_rs_queue::TARGET,
            queue,
            worker,
            error,
            "fetched job not handed to its worker; it stays in flight until swept",
        ),
        Trouble::Acknowledgement => tracing::error!(
            target: nest_rs_queue::TARGET,
            queue,
            worker,
            error,
            "job acknowledgement lost; the job stays in flight until a replica sweeps it, and runs \
             again then if its settled mark has lapsed",
        ),
    }
}

/// `duration` in whole milliseconds, for a line's field.
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use nest_rs_testing::LogCapture;

    use super::*;

    /// One fetch takes the method's `concurrency` — up to the most apalis's
    /// scripts can hand Redis's Lua, sweep included — at the poll and the
    /// liveness the config sets.
    #[test]
    fn a_fetch_takes_the_methods_concurrency_up_to_what_lua_can_unpack() {
        let queue = QueueName::new("audio").expect("a valid name");
        let config = RedisWorkerConfig {
            poll_interval: Duration::from_millis(25),
            ..Default::default()
        };
        for (concurrency, fetched) in [(1, 1), (16, 16), (799, 799), (800, 799), (100_000, 799)] {
            let settings = fetching(&queue, &config, concurrency);
            assert_eq!(
                settings.get_buffer_size(),
                fetched,
                "concurrency {concurrency}"
            );
            assert!(
                settings.get_buffer_size() * FETCHES_PER_SWEEP <= LUA_UNPACK_LIMIT,
                "a sweep of ten fetches unpacks within Lua's limit"
            );
        }
        let settings = fetching(&queue, &config, 4);
        assert_eq!(settings.get_poll_interval(), &Duration::from_millis(25));
        assert_eq!(settings.get_keep_alive(), &config.heartbeat());
        assert_eq!(settings.reenqueue_orphaned_after(), config.orphan_after);
        assert_eq!(settings.get_enqueue_scheduled(), &APALIS_SCAN);
        assert_eq!(settings.get_namespace(), &layout::namespace(&queue));
    }

    /// A method's promotion moves a hundred due records a scan, or its
    /// `concurrency` when that is more, and never more than a fetch may claim —
    /// the Lua bound a scan's single push of ids meets too.
    #[test]
    fn a_promotion_moves_a_hundred_or_the_methods_concurrency_up_to_a_fetchs_most() {
        for (concurrency, moved) in [
            (1, 100),
            (100, 100),
            (250, 250),
            (799, 799),
            (800, 799),
            (100_000, 799),
        ] {
            assert_eq!(
                promoted_per_scan(concurrency),
                moved,
                "concurrency {concurrency}"
            );
        }
    }

    /// Two workers in one process are two consumers: an id is never reused, so
    /// no two replicas — nor two methods of one — share an in-flight set.
    #[test]
    fn every_worker_consumes_under_an_id_of_its_own() {
        let (first, second) = (worker_id(), worker_id());
        assert_ne!(first, second);
        assert!(!first.contains(char::is_whitespace), "{first}");
    }

    /// apalis's failures reach the log at `warn` or above with the queue, the
    /// worker and the cause — a lost acknowledgement at `error`, since its job
    /// then waits in flight with nothing to run it while the replica lives, and
    /// runs again once swept if its settled mark has lapsed — while a delivery's
    /// own answer, reported where it was decided, is not said twice.
    #[test]
    fn apalis_failures_are_reported_with_fields_and_a_deliverys_answer_is_not_repeated() {
        let logs = LogCapture::install();
        let failure = || redis::RedisError::from(std::io::Error::other("connection reset"));
        for (error, trouble) in [
            (RedisPollError::PollNextError(failure()), Trouble::Fetch),
            (
                RedisPollError::KeepAliveError(failure()),
                Trouble::Heartbeat,
            ),
            (
                RedisPollError::EnqueueScheduledError(failure()),
                Trouble::Promotion,
            ),
            (
                RedisPollError::ReenqueueOrphanedError(failure()),
                Trouble::Reclaim,
            ),
            (
                RedisPollError::AckError(failure()),
                Trouble::Acknowledgement,
            ),
        ] {
            assert_eq!(Trouble::of(&error), trouble);
        }
        report_error("audio", "host:01", &RedisPollError::AckError(failure()));
        let lost = logs.expect_one(
            nest_rs_queue::TARGET,
            "job acknowledgement lost; the job stays in flight until a replica sweeps it, and runs \
             again then if its settled mark has lapsed",
        );
        assert_eq!(lost.level, "error");
        assert_eq!(lost.field("queue").as_deref(), Some("audio"));
        assert_eq!(lost.field("worker").as_deref(), Some("host:01"));
        assert!(
            lost.field("error")
                .is_some_and(|error| error.contains("connection reset")),
            "{lost:?}"
        );

        for trouble in [
            Trouble::Fetch,
            Trouble::Heartbeat,
            Trouble::Promotion,
            Trouble::Reclaim,
            Trouble::Handoff,
        ] {
            report_trouble(trouble, "audio", "host:01", "connection reset");
        }
        let fetch = logs.expect_one(
            nest_rs_queue::TARGET,
            "queue fetch failed; retrying at the next poll, and any job it claimed before failing \
             waits in flight until a replica starts",
        );
        let heartbeat = logs.expect_one(
            nest_rs_queue::TARGET,
            "queue worker heartbeat failed; its peers take its jobs if it keeps failing",
        );
        let promotion = logs.expect_one(
            nest_rs_queue::TARGET,
            "scheduled jobs not moved onto the queue; retrying at the next scan",
        );
        let reclaim = logs.expect_one(
            nest_rs_queue::TARGET,
            "orphaned jobs not reclaimed; retrying at the next sweep",
        );
        let handoff = logs.expect_one(
            nest_rs_queue::TARGET,
            "fetched job not handed to its worker; it stays in flight until swept",
        );
        for event in [fetch, heartbeat, promotion, reclaim, handoff] {
            assert_eq!(event.level, "warn", "{event:?}");
            assert_eq!(event.field("error").as_deref(), Some("connection reset"));
            assert_eq!(event.field("worker").as_deref(), Some("host:01"));
        }

        let answered =
            apalis::prelude::Error::Abort(Arc::new(Box::new(std::io::Error::other("bad payload"))));
        report_error("audio", "host:01", &answered);
        logs.expect_none(nest_rs_queue::TARGET, "queue worker error");
    }

    /// A record apalis cannot decode strands the jobs fetched beside it, so it
    /// is its own line, at `error` — and the connection's own `InvalidData`
    /// errors, a refused or failed TLS reopening, are not mistaken for one.
    #[test]
    fn a_fetch_meeting_a_record_apalis_cannot_decode_is_its_own_error() {
        let logs = LogCapture::install();
        let poison = RedisPollError::PollNextError(redis::RedisError::from(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "expected value at line 1 column 1",
        )));
        assert_eq!(Trouble::of(&poison), Trouble::Undecodable);
        let reopening =
            RedisPollError::PollNextError(redis::RedisError::from(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{REOPEN_FAILED}: invalid peer certificate: UnknownIssuer"),
            )));
        assert_eq!(Trouble::of(&reopening), Trouble::Fetch);
        let refused = RedisPollError::PollNextError(redis::RedisError::from(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer),
        )));
        assert_eq!(Trouble::of(&refused), Trouble::Fetch);

        report_error("audio", "host:01", &poison);
        let said = logs.expect_one(
            nest_rs_queue::TARGET,
            "queue fetch met a record apalis cannot decode; it and every job fetched beside it \
             wait in flight until a replica starts",
        );
        assert_eq!(said.level, "error");
        assert_eq!(said.field("queue").as_deref(), Some("audio"));
    }

    /// apalis repeats a sweep's unregistration on every poll until the worker
    /// registers again — against Redis 6.2, for a whole heartbeat — so it is
    /// said once per stretch, and a failing fetch that is something else stays
    /// a `warn`.
    #[test]
    fn a_worker_unregistered_by_a_peers_sweep_is_said_once_per_stretch() {
        let logs = LogCapture::install();
        let reporter = Reporter::new(HashMap::new());
        let refused = RedisPollError::PollNextError(redis::RedisError::from((
            redis::ErrorKind::ResponseError,
            "An error was signalled by the server",
            "user_script:1: consumer not registered script: 9608…".to_owned(),
        )));
        assert!(unregistered(&refused));
        for _ in 0..3 {
            reporter.report_unregistered("audio", "host:01");
        }
        let said = logs.find(
            nest_rs_queue::TARGET,
            "queue worker unregistered by a starting peer's sweep; it fetches again once \
             registered, at its next heartbeat at the latest",
        );
        assert_eq!(said.len(), 1, "{said:#?}");

        let other = RedisPollError::PollNextError(redis::RedisError::from(std::io::Error::other(
            "connection reset",
        )));
        assert!(!unregistered(&other));
    }

    /// A worker still running once the whole window has passed is left to the
    /// process's end, and said at `error`, with how many deliveries it cut:
    /// each of their jobs stays in flight, its attempt spent unless given back.
    #[tokio::test]
    async fn a_worker_outlasting_the_whole_window_is_said_with_what_it_cut_and_left() {
        let logs = LogCapture::install();
        let interrupt = CancellationToken::new();
        let run = std::future::pending::<std::io::Result<()>>();
        tokio::pin!(run);
        let window = Duration::from_millis(200);
        let underway = AtomicUsize::new(2);
        drain(run, &interrupt, window, window / 2, &underway)
            .await
            .expect("the drain returns");
        assert!(
            interrupt.is_cancelled(),
            "the running attempts were interrupted"
        );
        let left = logs.expect_one(
            nest_rs_queue::TARGET,
            "queue workers did not stop within the shutdown window; a delivery cut here leaves \
             its job in flight until a replica sweeps it, its attempt spent unless the give-back \
             reached Redis",
        );
        assert_eq!(left.level, "error");
        assert_eq!(left.field("cut").as_deref(), Some("2"));
        assert_eq!(left.field("reserve_ms").as_deref(), Some("100"));
    }

    /// The reserve is one connection budget — the time an interrupted
    /// attempt's give-back is owed — or five seconds when the budget is
    /// shorter, and never more than half the window.
    #[test]
    fn the_reserve_covers_one_budgeted_call_within_half_the_window() {
        let secs = Duration::from_secs;
        for (window, budget, reserve) in [
            (secs(30), Some(secs(10)), secs(10)),
            (secs(30), Some(secs(1)), secs(5)),
            (secs(30), None, secs(5)),
            (secs(30), Some(secs(60)), secs(15)),
            (secs(2), Some(secs(10)), secs(1)),
        ] {
            assert_eq!(
                hand_back_reserve(window, budget),
                reserve,
                "window {window:?}, budget {budget:?}"
            );
        }
    }

    /// A drain keeps back a reserve to hand interrupted jobs back in, and
    /// interrupts what still runs once the rest of its window has passed.
    #[tokio::test]
    async fn a_drain_interrupts_what_still_runs_and_keeps_within_its_window() {
        let logs = LogCapture::install();
        let interrupt = CancellationToken::new();
        let window = Duration::from_millis(400);
        let started = tokio::time::Instant::now();
        let watched = interrupt.clone();
        let run = async move {
            watched.cancelled().await;
            Ok(())
        };
        tokio::pin!(run);
        drain(run, &interrupt, window, window / 2, &AtomicUsize::new(0))
            .await
            .expect("drained");
        let took = started.elapsed();
        assert!(
            took >= window / 2 && took < window,
            "interrupted once half the window passed, not {took:?}"
        );
        logs.expect_one(
            nest_rs_queue::TARGET,
            "queue jobs still running as the shutdown window closes; interrupting them to hand \
             their jobs back",
        );
    }
}

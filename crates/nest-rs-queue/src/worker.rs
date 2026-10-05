//! [`QueueWorker`] — the transport that runs an app's `#[process]` methods over
//! the queue backend its binding bound ([`BoundConsumer`]).
//!
//! **The port runs the loop; the backend the transport.** Each method keeps a
//! pool of `concurrency` permits and asks the backend for as many deliveries as
//! permits are free, so a replica never holds a job it has no permit to run.
//! Each delivery runs on a task of its own holding one permit, through the
//! port's attempt, and its outcome becomes one [`Disposition`] the backend
//! settles, fenced on the delivery's lease.
//!
//! **A lease is renewed while its attempt runs**, every third of its length.
//! When the backend says another delivery took it, or no renewal was confirmed
//! for a whole lease, the attempt is cut — its outcome could not land — and the
//! job handed back, fenced, so the delivery that holds it now decides.
//!
//! **A shutdown stays inside the window.** The worker stops asking for jobs and
//! hands back what a receive brought after the stop, lets running attempts
//! finish for the window less a reserve, then cuts the rest and hands each back
//! within the reserve; leases are renewed until the last delivery ends. What
//! still runs then is said at `error` and left to its lease.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroU32;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures_util::FutureExt as _;
use futures_util::future::BoxFuture;
use nest_rs_core::{Container, SHUTDOWN_SETTLE_TIMEOUT, Transport, error_message};
use serde_json::Value;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::consume::{self, AttemptOutcome};
use crate::{
    Ask, BACKEND_REMEDY, BACKEND_TIMEOUT, BoundConsumer, Capability, Delivery, Disposition,
    JobConsumer, JobError, JobId, JobProducer, LeaseHold, Prepared, ProcessMethod, QueueBackend,
    QueueConfig, QueueError, QueueName, Received, TARGET,
};

/// The longest a receive waits for a job before it asks again: a push wakes a
/// receive the backend blocks, so the wait only bounds what an idle method
/// costs — one receive a second — and how long a stopping one waits for its
/// last.
const RECEIVE_WAIT: Duration = Duration::from_secs(1);

/// The first wait after a call failed, doubling up to [`LONGEST_RETRY`].
const FIRST_RETRY: Duration = Duration::from_millis(250);
const LONGEST_RETRY: Duration = Duration::from_secs(5);

/// The least a drain keeps back from its window to hand cut jobs back in, when
/// the backend answers faster: a healthy backend spends milliseconds of it.
const HAND_BACK_RESERVE: Duration = Duration::from_secs(5);

/// How often the leases are looked at while none is held.
const IDLE_RENEWAL: Duration = Duration::from_secs(1);

/// The least time between two renewals of a method's leases, however their
/// due instants fall.
const RENEWAL_SPACING: Duration = Duration::from_millis(10);

/// The longest dead-letter reason a backend is handed, in bytes: the failure as
/// its line renders it, cut on a character boundary.
const REASON_LIMIT: usize = 1024;

/// The transport [`QueueModule`](crate::QueueModule) attaches: every reachable
/// `#[process]` method, run over the [`BoundConsumer`] a backend's binding
/// declared.
pub struct QueueWorker {
    methods: Vec<&'static ProcessMethod>,
    container: Option<Container>,
    config: QueueConfig,
    consumer: Option<Arc<BoundConsumer>>,
    prepared: Option<Prepared>,
}

impl QueueWorker {
    /// A worker with nothing to run yet: the methods, the consumer and the
    /// settings are read from the container at boot.
    pub fn new() -> Self {
        Self {
            methods: Vec::new(),
            container: None,
            config: QueueConfig::default(),
            consumer: None,
            prepared: None,
        }
    }
}

impl Default for QueueWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Transport for QueueWorker {
    async fn configure(&mut self, container: &Container) -> anyhow::Result<()> {
        if let Some(config) = container.get::<QueueConfig>() {
            self.config = (*config).clone();
        }
        let reachable = consume::reachable(container);
        let Some(consumer) = container.get::<BoundConsumer>() else {
            if let Some(method) = reachable.first() {
                anyhow::bail!(
                    "`{}` drains queue `{}`, but no queue backend is bound to run it. \
                     {BACKEND_REMEDY}",
                    method.name(),
                    method.queue(),
                );
            }
            return Ok(());
        };
        let backend = consumer.backend();
        self.methods = consume::check(reachable, backend)?;
        if let Some(producer) = container.get_dyn::<dyn JobProducer>()
            && producer.backend().name() != backend.name()
        {
            anyhow::bail!(
                "the queue's producer runs on `{}` and its consumer on `{}`. {BACKEND_REMEDY}",
                producer.backend().name(),
                backend.name(),
            );
        }
        if !self.methods.is_empty() {
            let prepared = match within(BACKEND_TIMEOUT, consumer.prepare(&self.methods)).await {
                Ok(Ok(prepared)) => prepared,
                Ok(Err(error)) => {
                    return Err(anyhow::Error::new(error).context(format!(
                        "the `{}` queue backend could not prepare its consumer",
                        backend.name()
                    )));
                }
                Err(Elapsed) => anyhow::bail!(
                    "the `{}` queue backend did not prepare its consumer within the port's net \
                     of {BACKEND_TIMEOUT:?}",
                    backend.name()
                ),
            };
            self.prepared = Some(prepared);
        }
        self.consumer = Some(consumer);
        self.container = Some(container.clone());
        Ok(())
    }

    async fn serve(self: Box<Self>, cancel: CancellationToken) -> anyhow::Result<()> {
        let (Some(consumer), Some(container), Some(prepared)) =
            (self.consumer, self.container, self.prepared)
        else {
            // Nothing to run: idle until shutdown, so this transport does not
            // race the app down when it is the only one attached.
            cancel.cancelled().await;
            return Ok(());
        };
        consumer
            .serve(Serving {
                methods: self.methods,
                container,
                window: self.config.shutdown_timeout,
                prepared,
                cancel,
            })
            .await;
        Ok(())
    }

    /// The drain window holds its reserve for handing cut jobs back; the settle
    /// is for the backend letting go of what it opened.
    fn stop_bound(&self) -> Duration {
        self.config.shutdown_timeout + SHUTDOWN_SETTLE_TIMEOUT
    }
}

/// What a worker hands the loop of its consumer at `serve`.
pub(crate) struct Serving {
    methods: Vec<&'static ProcessMethod>,
    container: Container,
    window: Duration,
    prepared: Prepared,
    cancel: CancellationToken,
}

/// A [`JobConsumer`] behind the one type the container holds — the only call
/// that crosses it per worker is the loop's start, so no job pays for it.
pub(crate) trait Run: Send + Sync {
    fn backend(&self) -> &'static QueueBackend;
    fn prepare<'a>(
        &'a self,
        methods: &'a [&'static ProcessMethod],
    ) -> BoxFuture<'a, Result<Prepared, QueueError>>;
    fn serve(&self, serving: Serving) -> BoxFuture<'_, ()>;
}

pub(crate) struct Erased<C>(Arc<C>);

impl<C: JobConsumer> Erased<C> {
    pub(crate) fn new(consumer: C) -> Self {
        Self(Arc::new(consumer))
    }
}

impl<C: JobConsumer> Run for Erased<C> {
    fn backend(&self) -> &'static QueueBackend {
        self.0.backend()
    }

    fn prepare<'a>(
        &'a self,
        methods: &'a [&'static ProcessMethod],
    ) -> BoxFuture<'a, Result<Prepared, QueueError>> {
        Box::pin(self.0.prepare(methods))
    }

    fn serve(&self, serving: Serving) -> BoxFuture<'_, ()> {
        Box::pin(serve(Arc::clone(&self.0), serving))
    }
}

/// What every method's loop of one worker shares.
struct Shared<C: JobConsumer> {
    consumer: Arc<C>,
    container: Container,
    /// How long one receive waits for a job.
    wait: Duration,
    /// Fired at the shutdown signal: no receive, no start, waits end.
    stop: CancellationToken,
    /// Fired when the window less its reserve has passed: running attempts are
    /// cut and their jobs handed back.
    interrupt: CancellationToken,
    /// Every delivery's task, the drain's to wait on.
    deliveries: TaskTracker,
    /// Fired when the worker's `serve` ends or is dropped: every delivery task
    /// stops where it stands, settling nothing, as a killed process would.
    killed: CancellationToken,
}

/// Run every method of `serving` over `consumer` until the shutdown signal,
/// then drain within the window.
async fn serve<C: JobConsumer>(consumer: Arc<C>, serving: Serving) {
    let Serving {
        methods,
        container,
        window,
        prepared,
        cancel,
    } = serving;
    let reserve = HAND_BACK_RESERVE.max(prepared.answer_bound).min(window / 2);
    let shared = Arc::new(Shared {
        consumer,
        container,
        wait: RECEIVE_WAIT.min(window / 4),
        stop: CancellationToken::new(),
        interrupt: CancellationToken::new(),
        deliveries: TaskTracker::new(),
        killed: CancellationToken::new(),
    });
    let _dies_with_serve = shared.killed.clone().drop_guard();
    let renewing = CancellationToken::new();
    let mut loops = JoinSet::new();
    let mut renewals = JoinSet::new();
    for method in methods {
        let queue = match QueueName::new(method.queue()) {
            Ok(queue) => queue,
            // Discovery refused every name outside the rule at boot.
            Err(refused) => {
                tracing::error!(
                    target: TARGET,
                    processor = method.name(),
                    error = %error_message(&refused),
                    "queue processor not run: its queue's name is refused",
                );
                continue;
            }
        };
        let run = Arc::new(MethodRun {
            delays: shared
                .consumer
                .backend()
                .capabilities()
                .contains(Capability::DelayedPush),
            shared: Arc::clone(&shared),
            method,
            queue,
            permits: Arc::new(Semaphore::new(permits_of(method))),
            held: Mutex::new(HashMap::new()),
            next_token: AtomicU64::new(0),
            holding: Notify::new(),
        });
        loops.spawn(Arc::clone(&run).receive());
        loops.spawn(Arc::clone(&run).maintain());
        renewals.spawn(Arc::clone(&run).renew(renewing.clone()));
    }

    cancel.cancelled().await;
    shared.stop.cancel();
    let patience = window.saturating_sub(reserve);
    let drained = tokio::time::timeout(patience, async {
        while loops.join_next().await.is_some() {}
        shared.deliveries.close();
        shared.deliveries.wait().await;
    })
    .await;
    if drained.is_err() {
        tracing::warn!(
            target: TARGET,
            shutdown_timeout_ms = millis(window),
            reserve_ms = millis(reserve),
            "queue jobs still running as the shutdown window closes; interrupting them to hand \
             their jobs back",
        );
        shared.interrupt.cancel();
        loops.abort_all();
        shared.deliveries.close();
        if tokio::time::timeout(reserve, shared.deliveries.wait())
            .await
            .is_err()
        {
            tracing::error!(
                target: TARGET,
                shutdown_timeout_ms = millis(window),
                reserve_ms = millis(reserve),
                cut = shared.deliveries.len(),
                "queue workers did not stop within the shutdown window; a delivery cut here \
                 leaves its job to its lease, after which another worker runs it",
            );
        }
    }
    renewing.cancel();
    renewals.abort_all();
    match tokio::time::timeout(SHUTDOWN_SETTLE_TIMEOUT, shared.consumer.close()).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(
            target: TARGET,
            error = %error_message(&error),
            "queue backend did not close cleanly",
        ),
        Err(_) => tracing::warn!(
            target: TARGET,
            settle_ms = millis(SHUTDOWN_SETTLE_TIMEOUT),
            "queue backend did not close within the settle; left as it was",
        ),
    }
}

/// How many attempts of `method` run at once on this replica.
fn permits_of(method: &ProcessMethod) -> usize {
    usize::try_from(method.options().concurrency().get())
        .unwrap_or(usize::MAX)
        .min(Semaphore::MAX_PERMITS)
}

/// One method's loop on one replica.
struct MethodRun<C: JobConsumer> {
    shared: Arc<Shared<C>>,
    method: &'static ProcessMethod,
    queue: QueueName,
    /// Whether the backend files a record due later, so a retry's wait is the
    /// backend's rather than a held permit's.
    delays: bool,
    permits: Arc<Semaphore>,
    /// Every lease a delivery of this method holds, by its local token.
    held: Mutex<HashMap<u64, Held<C::Lease>>>,
    next_token: AtomicU64,
    /// Woken when a delivery takes a lease, so its first renewal is timed from
    /// its own start.
    holding: Notify,
}

/// One delivery's hold on its job, as the renewal sees it.
struct Held<L> {
    lease: Arc<L>,
    leased_for: Duration,
    /// When a renewal last confirmed the lease, or the delivery began.
    confirmed: Instant,
    /// Whether its outcome is being settled: a renewal answering `Lost` then
    /// means the settle landed, not that another delivery took it.
    settling: bool,
    /// Fired when the lease is lost; the delivery's attempt is then cut.
    lost: CancellationToken,
    job: Option<JobId>,
}

/// How an attempt's run ended, from where the delivery stands.
enum Ran {
    Answered(AttemptOutcome),
    Cut,
    Lost,
}

/// How an in-process wait ended.
enum Waited {
    Elapsed,
    Stopped,
    Lost,
}

/// A delivery's lease, taken off the renewed ones however its task ends — a
/// panic included — so no lease is renewed for a delivery that is gone, holding
/// its job for as long as the worker lives.
struct Released<C: JobConsumer> {
    run: Arc<MethodRun<C>>,
    token: u64,
}

impl<C: JobConsumer> Drop for Released<C> {
    fn drop(&mut self) {
        self.run.lock().remove(&self.token);
    }
}

impl<C: JobConsumer> MethodRun<C> {
    /// Ask for as many deliveries as permits are free, until the stop.
    async fn receive(self: Arc<Self>) {
        let mut retry = FIRST_RETRY;
        loop {
            let first = tokio::select! {
                biased;
                () = self.shared.stop.cancelled() => return,
                permit = Arc::clone(&self.permits).acquire_owned() => match permit {
                    Ok(permit) => permit,
                    Err(_) => return,
                },
            };
            let mut permits = vec![first];
            while let Ok(more) = Arc::clone(&self.permits).try_acquire_owned() {
                permits.push(more);
            }
            let max = u32::try_from(permits.len())
                .ok()
                .and_then(NonZeroU32::new)
                .unwrap_or(NonZeroU32::MIN);
            let wait = self.shared.wait;
            let net = wait + BACKEND_TIMEOUT;
            let answer = within(
                net,
                self.shared
                    .consumer
                    .receive(self.method, Ask::new(max, wait)),
            )
            .await;
            match answer {
                Ok(Ok(Received {
                    deliveries,
                    throttled_for,
                })) => {
                    retry = FIRST_RETRY;
                    let stopped = self.shared.stop.is_cancelled();
                    if deliveries.len() > permits.len() {
                        tracing::error!(
                            target: TARGET,
                            queue = %self.queue,
                            asked = permits.len(),
                            handed = deliveries.len(),
                            "queue backend handed over more deliveries than asked; the extra are \
                             handed back",
                        );
                    }
                    for delivery in deliveries {
                        match permits.pop() {
                            Some(permit) if !stopped => {
                                self.spawn(Arc::clone(&self).deliver(delivery, permit));
                            }
                            _ => {
                                self.spawn(Arc::clone(&self).hand_back(delivery));
                            }
                        }
                    }
                    drop(permits);
                    if stopped {
                        return;
                    }
                    if let Some(window) = throttled_for {
                        tracing::debug!(
                            target: TARGET,
                            queue = %self.queue,
                            window_left_ms = millis(window),
                            "queue receive waits for its throttle's next window",
                        );
                        tokio::select! {
                            () = self.shared.stop.cancelled() => return,
                            () = tokio::time::sleep(window) => {}
                        }
                    }
                }
                failed => {
                    drop(permits);
                    tracing::warn!(
                        target: TARGET,
                        queue = %self.queue,
                        error = %failure(failed, net),
                        retry_in_ms = millis(retry),
                        "queue receive failed; retrying",
                    );
                    tokio::select! {
                        () = self.shared.stop.cancelled() => return,
                        () = tokio::time::sleep(retry) => {}
                    }
                    retry = (retry * 2).min(LONGEST_RETRY);
                }
            }
        }
    }

    /// Run `delivery` on a task of its own, which dies with the worker.
    fn spawn(&self, delivery: impl Future<Output = ()> + Send + 'static) {
        let killed = self.shared.killed.clone();
        let queue = self.queue.clone();
        self.shared.deliveries.spawn(async move {
            tokio::select! {
                biased;
                () = killed.cancelled() => {}
                ended = AssertUnwindSafe(delivery).catch_unwind() => {
                    // The attempt catches its handler's panics, so this one is
                    // the backend's or the worker's own.
                    if let Err(panic) = ended {
                        nest_rs_core::contained_panic!(
                            target: TARGET,
                            panic.as_ref(),
                            "queue delivery panicked outside its attempt; its job is left to its \
                             lease",
                            queue = %queue,
                        );
                    }
                }
            }
        });
    }

    /// The backend's upkeep of the queue, as often as it asks, until the stop.
    async fn maintain(self: Arc<Self>) {
        let mut retry = FIRST_RETRY;
        loop {
            let next =
                match within(BACKEND_TIMEOUT, self.shared.consumer.maintain(self.method)).await {
                    Ok(Ok(None)) => return,
                    Ok(Ok(Some(next))) => {
                        retry = FIRST_RETRY;
                        next
                    }
                    failed => {
                        tracing::warn!(
                            target: TARGET,
                            queue = %self.queue,
                            error = %failure(failed, BACKEND_TIMEOUT),
                            retry_in_ms = millis(retry),
                            "queue upkeep failed; retrying",
                        );
                        let now = retry;
                        retry = (retry * 2).min(LONGEST_RETRY);
                        now
                    }
                };
            tokio::select! {
                () = self.shared.stop.cancelled() => return,
                () = tokio::time::sleep(next) => {}
            }
        }
    }

    /// Renew the leases this method's deliveries hold — each a third of its
    /// length after its last confirmation, every lease held renewed together
    /// when one falls due — until `until`, after the last delivery ended.
    async fn renew(self: Arc<Self>, until: CancellationToken) {
        let mut retry = FIRST_RETRY;
        let mut not_before = Instant::now();
        loop {
            let due = self.next_renewal().max(not_before);
            tokio::select! {
                () = until.cancelled() => return,
                () = self.holding.notified() => continue,
                () = tokio::time::sleep_until(due.into()) => {}
            }
            let held: Vec<(u64, Arc<C::Lease>)> = self
                .lock()
                .iter()
                .map(|(token, held)| (*token, Arc::clone(&held.lease)))
                .collect();
            if !held.is_empty() {
                let leases: Vec<&C::Lease> = held.iter().map(|(_, lease)| &**lease).collect();
                let asked = Instant::now();
                match within(
                    BACKEND_TIMEOUT,
                    self.shared.consumer.renew(self.method, &leases),
                )
                .await
                {
                    Ok(Ok(answers)) => {
                        retry = FIRST_RETRY;
                        not_before = Instant::now() + RENEWAL_SPACING;
                        if answers.len() != held.len() {
                            tracing::error!(
                                target: TARGET,
                                queue = %self.queue,
                                asked = held.len(),
                                answered = answers.len(),
                                "queue backend answered a renewal with as many answers as it \
                                 chose; the leases left unanswered count as unconfirmed",
                            );
                        }
                        for ((token, _), hold) in held.iter().zip(answers) {
                            match hold {
                                LeaseHold::Held => self.confirm(*token, asked),
                                LeaseHold::Lost => self.lose(*token),
                            }
                        }
                    }
                    failed => {
                        tracing::warn!(
                            target: TARGET,
                            queue = %self.queue,
                            leases = held.len(),
                            error = %failure(failed, BACKEND_TIMEOUT),
                            retry_in_ms = millis(retry),
                            "job leases not renewed; retrying",
                        );
                        not_before = Instant::now() + retry;
                        retry = (retry * 2).min(LONGEST_RETRY);
                    }
                }
            }
            self.lapse_unconfirmed();
        }
    }

    /// Run one delivery to the end of its job's delivery.
    async fn deliver(self: Arc<Self>, delivery: Delivery<C::Lease>, _permit: OwnedSemaphorePermit) {
        let Delivery {
            record,
            lease,
            leased_for,
            delivery_count,
            backend_id,
            deferred_for,
        } = delivery;
        let lease = Arc::new(lease);
        let (token, lost) = self.hold(Arc::clone(&lease), leased_for);
        let _released = Released {
            run: Arc::clone(&self),
            token,
        };
        let record = match record {
            Ok(record) => record,
            Err(why) => {
                self.refuse(
                    token,
                    &lease,
                    backend_id.as_deref(),
                    JobError::abort(why),
                    &[],
                )
                .await;
                return;
            }
        };
        let message: Value = match serde_json::from_slice(&record) {
            Ok(message) => message,
            Err(error) => {
                let refused = JobError::undecodable(self.queue.as_str(), &error);
                self.refuse(token, &lease, backend_id.as_deref(), refused, &record)
                    .await;
                return;
            }
        };
        let backend = self.shared.consumer.backend();
        let mut state = consume::Delivery::new(backend, self.queue.clone(), message)
            .with_delivery_count(delivery_count);
        if let Some(backend_id) = backend_id {
            state = state.with_backend_id(backend_id);
        }
        if let Some(waited) = deferred_for {
            state = state.with_deferred_for(waited);
        }
        if self.method.options().checkpoint()
            && let Some(store) = self
                .shared
                .consumer
                .checkpoint(self.method, &lease, state.id())
        {
            state = state.with_checkpoint(store);
        }
        let job = state.id().clone();
        self.name(token, &job);
        loop {
            let ran = tokio::select! {
                biased;
                () = self.shared.interrupt.cancelled() => Ran::Cut,
                () = lost.cancelled() => Ran::Lost,
                outcome = consume::attempt(self.method, &mut state, self.shared.container.clone()) => {
                    Ran::Answered(outcome)
                }
            };
            let outcome = match ran {
                Ran::Answered(outcome) => outcome,
                Ran::Cut => {
                    tracing::info!(
                        target: TARGET,
                        queue = %self.queue,
                        job_id = %job,
                        attempt = state.attempt(),
                        reason = "shutdown",
                        "job handed back to the queue",
                    );
                    self.end(
                        token,
                        &lease,
                        Disposition::Requeue { record: &record },
                        Some(&job),
                        false,
                    )
                    .await;
                    return;
                }
                Ran::Lost => {
                    self.end(
                        token,
                        &lease,
                        Disposition::Requeue { record: &record },
                        Some(&job),
                        true,
                    )
                    .await;
                    return;
                }
            };
            match outcome {
                AttemptOutcome::Ok => {
                    self.end(token, &lease, Disposition::Complete, Some(&job), false)
                        .await;
                    return;
                }
                AttemptOutcome::DeadLetter(error) => {
                    let reason = reason(&error);
                    self.end(
                        token,
                        &lease,
                        Disposition::DeadLetter {
                            reason: &reason,
                            record: &record,
                        },
                        Some(&job),
                        false,
                    )
                    .await;
                    return;
                }
                AttemptOutcome::Retry { after } => {
                    if !self.delays && !after.is_zero() {
                        match self.wait(after, &lost).await {
                            Waited::Elapsed => continue,
                            Waited::Stopped | Waited::Lost => {}
                        }
                    }
                    let after = if self.delays { after } else { Duration::ZERO };
                    let next = state.retry_envelope().into_json().to_string();
                    self.end(
                        token,
                        &lease,
                        Disposition::Retry {
                            after,
                            record: next.as_bytes(),
                        },
                        Some(&job),
                        lost.is_cancelled(),
                    )
                    .await;
                    return;
                }
                AttemptOutcome::Defer { after } => {
                    let disposition = if self.delays {
                        Disposition::Defer {
                            after,
                            record: &record,
                        }
                    } else {
                        // The wait is this worker's; whatever ends it, the job
                        // goes back as it was stored.
                        let _waited = self.wait(after, &lost).await;
                        Disposition::Requeue { record: &record }
                    };
                    self.end(token, &lease, disposition, Some(&job), lost.is_cancelled())
                        .await;
                    return;
                }
            }
        }
    }

    /// Hand back, unstarted, a delivery a receive brought after the stop, or
    /// one past the deliveries asked for.
    async fn hand_back(self: Arc<Self>, delivery: Delivery<C::Lease>) {
        let Delivery {
            record,
            lease,
            leased_for,
            backend_id,
            ..
        } = delivery;
        let lease = Arc::new(lease);
        let (token, _lost) = self.hold(Arc::clone(&lease), leased_for);
        match record {
            Ok(record) => {
                self.end(
                    token,
                    &lease,
                    Disposition::Requeue { record: &record },
                    None,
                    false,
                )
                .await;
            }
            Err(why) => {
                self.refuse(
                    token,
                    &lease,
                    backend_id.as_deref(),
                    JobError::abort(why),
                    &[],
                )
                .await;
            }
        }
    }

    /// Dead-letter a delivery the port cannot run — a record the backend could
    /// not hand over, or one that is not JSON — as a unit of work of its own,
    /// keeping `record` as stored.
    async fn refuse(
        &self,
        token: u64,
        lease: &C::Lease,
        backend_id: Option<&str>,
        error: JobError,
        record: &[u8],
    ) {
        let error = consume::refuse(
            self.shared.consumer.backend(),
            Some(self.queue.as_str()),
            None,
            backend_id,
            error,
        )
        .await;
        let reason = reason(&error);
        self.end(
            token,
            lease,
            Disposition::DeadLetter {
                reason: &reason,
                record,
            },
            None,
            false,
        )
        .await;
    }

    /// Settle `disposition`, retrying a call that erred until the lease would
    /// lapse, and let the lease go. `expected_loss` says the lease was already
    /// known lost, so a `Lost` answer is the expected one.
    async fn end(
        &self,
        token: u64,
        lease: &C::Lease,
        disposition: Disposition<'_>,
        job: Option<&JobId>,
        expected_loss: bool,
    ) {
        self.settling(token);
        let mut retry = FIRST_RETRY;
        let mut erred = false;
        loop {
            match within(
                BACKEND_TIMEOUT,
                self.shared.consumer.settle(self.method, lease, disposition),
            )
            .await
            {
                Ok(Ok(LeaseHold::Held)) => break,
                Ok(Ok(LeaseHold::Lost)) => {
                    if erred || expected_loss {
                        tracing::debug!(
                            target: TARGET,
                            queue = %self.queue,
                            job_id = job.map(tracing::field::display),
                            disposition = disposition.name(),
                            "job outcome not written: the delivery is no longer this worker's",
                        );
                    } else {
                        tracing::warn!(
                            target: TARGET,
                            queue = %self.queue,
                            job_id = job.map(tracing::field::display),
                            disposition = disposition.name(),
                            "job outcome dropped: its lease lapsed and another worker holds its \
                             delivery",
                        );
                    }
                    break;
                }
                failed => {
                    erred = true;
                    let error = failure(failed, BACKEND_TIMEOUT);
                    if Instant::now() + retry >= self.holds_until(token) {
                        tracing::error!(
                            target: TARGET,
                            queue = %self.queue,
                            job_id = job.map(tracing::field::display),
                            disposition = disposition.name(),
                            error = %error,
                            "job outcome not confirmed; unless it was written, the job runs again once its lease lapses",
                        );
                        break;
                    }
                    tracing::debug!(
                        target: TARGET,
                        queue = %self.queue,
                        job_id = job.map(tracing::field::display),
                        error = %error,
                        retry_in_ms = millis(retry),
                        "job outcome not recorded yet; retrying",
                    );
                    tokio::time::sleep(retry).await;
                    retry = (retry * 2).min(LONGEST_RETRY);
                }
            }
        }
        self.lock().remove(&token);
    }

    /// Wait `after` in process — a backend that cannot file a record due later —
    /// renewing the lease meanwhile; the stop or the lease's loss ends it early.
    async fn wait(&self, after: Duration, lost: &CancellationToken) -> Waited {
        tokio::select! {
            biased;
            () = lost.cancelled() => Waited::Lost,
            () = self.shared.stop.cancelled() => Waited::Stopped,
            () = tokio::time::sleep(after) => Waited::Elapsed,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Held<C::Lease>>> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Start renewing `lease`, held from now for `leased_for`.
    fn hold(&self, lease: Arc<C::Lease>, leased_for: Duration) -> (u64, CancellationToken) {
        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        let lost = CancellationToken::new();
        self.lock().insert(
            token,
            Held {
                lease,
                leased_for,
                confirmed: Instant::now(),
                settling: false,
                lost: lost.clone(),
                job: None,
            },
        );
        self.holding.notify_one();
        (token, lost)
    }

    fn name(&self, token: u64, job: &JobId) {
        if let Some(held) = self.lock().get_mut(&token) {
            held.job = Some(job.clone());
        }
    }

    /// Mark the lease as being settled.
    fn settling(&self, token: u64) {
        if let Some(held) = self.lock().get_mut(&token) {
            held.settling = true;
        }
    }

    /// Until when the lease holds without another renewal: its last
    /// confirmation, plus its length.
    fn holds_until(&self, token: u64) -> Instant {
        self.lock()
            .get(&token)
            .map_or_else(Instant::now, |held| held.confirmed + held.leased_for)
    }

    fn confirm(&self, token: u64, at: Instant) {
        if let Some(held) = self.lock().get_mut(&token) {
            held.confirmed = at;
        }
    }

    /// The backend says another delivery holds this lease's job now.
    fn lose(&self, token: u64) {
        let guard = self.lock();
        let Some(held) = guard.get(&token) else {
            return;
        };
        if held.settling || held.lost.is_cancelled() {
            return;
        }
        tracing::warn!(
            target: TARGET,
            queue = %self.queue,
            job_id = held.job.as_ref().map(tracing::field::display),
            "job lease taken by another delivery; its attempt is cut, and the delivery holding \
             the job decides it",
        );
        held.lost.cancel();
    }

    /// Cut every attempt whose lease no renewal confirmed for a whole lease.
    fn lapse_unconfirmed(&self) {
        let now = Instant::now();
        for held in self.lock().values() {
            if held.settling
                || held.lost.is_cancelled()
                || now.saturating_duration_since(held.confirmed) < held.leased_for
            {
                continue;
            }
            tracing::warn!(
                target: TARGET,
                queue = %self.queue,
                job_id = held.job.as_ref().map(tracing::field::display),
                lease_ms = millis(held.leased_for),
                "job lease not renewed for a whole lease; its attempt is cut and the job handed \
                 back",
            );
            held.lost.cancel();
        }
    }

    /// When the next lease falls due for renewal — a third of its length after
    /// its last confirmation, so two renewals can fail before it lapses — or
    /// [`IDLE_RENEWAL`] from now when none is held.
    fn next_renewal(&self) -> Instant {
        self.lock()
            .values()
            .map(|held| held.confirmed + held.leased_for / 3)
            .min()
            .unwrap_or_else(|| Instant::now() + IDLE_RENEWAL)
    }
}

/// `answer`, unless the backend took longer than `net` to give it.
async fn within<T>(
    net: Duration,
    answer: impl Future<Output = Result<T, QueueError>>,
) -> Result<Result<T, QueueError>, Elapsed> {
    tokio::time::timeout(net, answer)
        .await
        .map_err(|_elapsed| Elapsed)
}

/// A backend call the port stopped waiting for at its net.
#[derive(Debug)]
struct Elapsed;

/// What failed in a call the port made, as a line says it.
fn failure<T>(failed: Result<Result<T, QueueError>, Elapsed>, net: Duration) -> String {
    match failed {
        Ok(Err(error)) => error_message(&error),
        Err(Elapsed) => format!("no answer within the port's net of {net:?}"),
        Ok(Ok(_)) => String::from("answered"),
    }
}

/// The dead-letter reason a backend keeps: the failure as its line renders it,
/// cut to [`REASON_LIMIT`] bytes on a character boundary.
fn reason(error: &JobError) -> String {
    let mut reason = error_message(error);
    if reason.len() > REASON_LIMIT {
        let mut end = REASON_LIMIT;
        while !reason.is_char_boundary(end) {
            end -= 1;
        }
        reason.truncate(end);
    }
    reason
}

/// `duration` in whole milliseconds, for a line's field.
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

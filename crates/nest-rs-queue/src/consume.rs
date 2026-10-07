//! What a job attempt *is*, written once for every backend and run by the
//! port's [`QueueWorker`](crate::QueueWorker): discovery, which drains the
//! `#[process]` inventory module-gated and refuses two methods on one queue and
//! any declaration the backend cannot honour, and [`attempt`](crate::consume::attempt), which opens the
//! envelope, continues or mints the trace, opens the `queue.job` span and the
//! ambient scope, catches a panic, spends or ends the retry budget and says how
//! long to wait before the next attempt, and files the events and the
//! `nest_rs::operation` line.
//!
//! Not a driver's seam: a backend implements
//! [`JobConsumer`](crate::JobConsumer), and the worker calls these. Public, and
//! hidden, so this crate's suite drives an attempt without a worker.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::FutureExt;
use nest_rs_core::{
    Container, Correlation, ReachableProviders, RequestScope, panic_message, with_request_scope,
};
use serde_json::Value;
use tracing::Instrument;

use crate::capability::unsupported;
use crate::checkpoint::CheckpointCell;
use crate::envelope::{self, Opened, Unusable};
use crate::inventory::{HandlerContext, JobHandler};
use crate::{
    CheckpointStore, Envelope, JobError, JobId, ProcessMethod, QueueBackend, QueueName, TARGET,
    Throttle, backoff, unit,
};

/// A job span's `messaging.operation.name` and `messaging.operation.type`:
/// OpenTelemetry's messaging conventions call the consumer's unit of work
/// `process`.
const PROCESS: &str = "process";

/// How long a job sealed by a newer release waits before it is delivered again,
/// when a consumer of this release hands it back — [`AttemptOutcome::Defer`].
///
/// Long enough that a backlog of such jobs is not fetched and handed back
/// continuously while old replicas outnumber new ones; short enough that, once a
/// rolling deploy has put a consumer of the newer release in place, the jobs
/// reach it within a minute.
pub const NEWER_RELEASE_WAIT: Duration = Duration::from_secs(60);

/// How long a job sealed by a newer release may go unread before a consumer of
/// this release dead-letters it: a day, counted from the first time it was
/// handed back unread ([`Delivery::with_deferred_for`]) — or from its push, on a
/// backend that keeps no such record.
///
/// A rolling deploy mixes releases for minutes. A day of a newer release's jobs
/// handed back every minute is a rollout that stopped — its producers rolled
/// back, its consumers never coming — and without a bound those jobs would wait
/// forever, each warned about once a minute. Dead-lettered, each is said once,
/// naming both versions, and its record is kept with the dead letters — a week,
/// on Redis — for a consumer of that release.
pub const NEWER_RELEASE_PATIENCE: Duration = Duration::from_secs(24 * 60 * 60);

/// How many deliveries of one record may end without an answer before the
/// port dead-letters the job without running it.
///
/// An answer ends a record — the job completes, is dead-lettered, or is filed
/// again for its next attempt — so every earlier delivery of the record a
/// backend hands over ended without one: its worker was killed, froze, or lost
/// the backend past its lease. The retry budget counts attempts that answered;
/// this counts the ones that never did, so a job that takes its worker down
/// stops after three tries instead of forever, and a job whose delivery merely
/// went missing — a reply lost, a worker restarted — still runs.
pub const STALL_LIMIT: u32 = 3;

/// The boot's refusal of each declaration on `method` that `backend` does not
/// honour — every one, so a method declaring two is refused once for each.
pub(crate) fn unsupported_by<'a>(
    method: &'a ProcessMethod,
    backend: &'a QueueBackend,
) -> impl Iterator<Item = String> + 'a {
    method
        .required_capabilities()
        .iter()
        .filter(|capability| !backend.capabilities().contains(*capability))
        .map(|capability| unsupported(capability, backend.name(), Some(method.name())))
}

/// The `#[process]` methods this app serves on `backend`: every entry whose
/// provider is reachable from the running app's root, with a boot `warn` for
/// each that is linked but unreachable.
///
/// The boot fails — naming every offender at once — when a reachable method
/// drains a queue whose name breaks the rule, declares something `backend`
/// cannot honour, or claims a queue another method already drains. Checked after
/// module-gating, so a method another app owns cannot fail this app's boot.
///
/// Called once by an adapter's `Transport::configure`; what it returns is what
/// that adapter subscribes to.
pub fn discover(
    container: &Container,
    backend: &QueueBackend,
) -> anyhow::Result<Vec<&'static ProcessMethod>> {
    check(reachable(container), backend)
}

/// The `#[process]` methods whose provider is reachable from the running app's
/// root, with a boot `warn` for each that is linked but unreachable.
pub(crate) fn reachable(container: &Container) -> Vec<&'static ProcessMethod> {
    let reachable = container.get::<ReachableProviders>();
    let mut methods: Vec<&'static ProcessMethod> = Vec::new();
    for entry in nest_rs_core::inventory::iter::<ProcessMethod>() {
        if !ReachableProviders::reaches(reachable.as_deref(), entry.provider_type_id()) {
            ::nest_rs_core::report_inert_host!(
                target: TARGET,
                what: "#[process] method",
                origin: entry.origin(),
                host: entry.provider_type_id(),
                container: container,
                processor = entry.name(),
                queue = entry.queue(),
            );
            continue;
        }
        methods.push(entry);
    }
    methods
}

/// `methods`, once each is one `backend` can serve: the boot fails, naming every
/// offender at once, on a queue name outside the rule, a throttle window under
/// a millisecond, a declaration `backend` cannot honour, or two methods on one
/// queue. Each method kept is announced.
pub(crate) fn check(
    methods: Vec<&'static ProcessMethod>,
    backend: &QueueBackend,
) -> anyhow::Result<Vec<&'static ProcessMethod>> {
    let mut refusals = Vec::new();
    for method in &methods {
        // A decorator's literals were checked at compile time; an entry built by
        // hand reaches the boot unchecked, so the boot checks what the decorator
        // would have — in the sentence the port already words.
        if let Err(refused) = QueueName::new(method.queue()) {
            refusals.push(format!(
                "`{}` drains a queue whose name is refused: {refused}",
                method.name()
            ));
        }
        // A window under a millisecond is zero to a store counting in
        // milliseconds, and a zero window limits nothing.
        if let Some(throttle) = method.options().throttle()
            && throttle.window() < Throttle::MIN_WINDOW
        {
            refusals.push(format!(
                "`{}` declares a throttle window under a millisecond, which would limit nothing: \
                 a window is at least {:?}",
                method.name(),
                Throttle::MIN_WINDOW,
            ));
        }
        refusals.extend(unsupported_by(method, backend));
    }
    if !refusals.is_empty() {
        anyhow::bail!("{}", refusals.join("\n"));
    }

    // Aggregating a queue is like aggregating a mount: the one failure mode it
    // adds is two contributions claiming one addressable name, and that is a
    // boot error naming both.
    check_duplicate_queue_claims(&methods).map_err(anyhow::Error::msg)?;
    for method in &methods {
        let options = method.options();
        let throttle = options.throttle();
        tracing::info!(
            target: TARGET,
            processor = method.name(),
            queue = method.queue(),
            retries = options.retries(),
            concurrency = options.concurrency().get(),
            throttle_limit = throttle.map(|throttle| throttle.limit().get()),
            throttle_window_ms = throttle.map(|throttle| throttle.window().as_millis() as u64),
            checkpoint = options.checkpoint().then_some(true),
            "registered queue processor",
        );
    }
    Ok(methods)
}

/// Two `#[process]` methods may not drain one queue.
///
/// A queue is addressed by name and carries exactly one job type
/// (`#[process(queue = Q)]` asserts the handler's payload is `Q::Job`), so
/// draining it twice is never the shape a developer meant: each job would go to
/// whichever handler a backend happened to hand it to, and the retry budget would
/// fork with it. The way to run more jobs at once is `concurrency`, not a second
/// handler.
fn check_duplicate_queue_claims(methods: &[&ProcessMethod]) -> Result<(), String> {
    let mut claimants: BTreeMap<&'static str, Vec<&'static str>> = BTreeMap::new();
    for method in methods {
        claimants
            .entry(method.queue())
            .or_default()
            .push(method.name());
    }

    let clashes: Vec<String> = claimants
        .into_iter()
        .filter(|(_, names)| names.len() > 1)
        .map(|(queue, names)| format!("queue {queue:?} ({})", names.join(" and ")))
        .collect();

    if clashes.is_empty() {
        return Ok(());
    }
    Err(format!(
        "duplicate queue claim: {} — a queue is drained by one `#[process]` method, so a second \
         one would take an unpredictable share of its jobs. Give the other method its own queue, \
         or fold the two bodies into one and raise its `concurrency`.",
        clashes.join(", "),
    ))
}

/// How one attempt ended, in the port's vocabulary. The adapter translates it
/// into its backend's — and nothing else about the outcome is its to decide.
#[derive(Debug)]
pub enum AttemptOutcome {
    /// The handler returned `Ok`, and its data context settled. The job is done:
    /// acknowledge it.
    Ok,
    /// The handler failed in a way another attempt could clear, and the
    /// method's retry budget holds another attempt. Run the job again once
    /// `after` has passed.
    ///
    /// `after` is `min(5 min, 1 s · 2^(n − 1))` for failed attempt `n`, scaled by
    /// a jitter between 0.8 and 1.2 derived from the job's id and `n` — the same
    /// wait on every backend, and for any job reproducible from its id.
    ///
    /// The [`Delivery`] already counts the next attempt. A backend declaring
    /// [`DelayedPush`](crate::Capability::DelayedPush) re-files
    /// [`Delivery::retry_envelope`] to become available once `after` has passed,
    /// then acknowledges this delivery. One without it waits `after` itself —
    /// giving up the wait, never the job, when the worker shuts down — and calls
    /// [`attempt`] again with the same delivery.
    Retry {
        /// How long the job waits before its next attempt.
        after: Duration,
    },
    /// The job is done failing — deterministically (an undeserializable
    /// payload, a pipe rejection, a missing provider, a panic, an envelope of
    /// an older version), or retryably on its last attempt: dead-letter it.
    ///
    /// The record an adapter keeps of the failure is
    /// [`error_message`](fn@nest_rs_core::error_message)'s rendering — the sentence
    /// the dead-letter line carries, every cause and each decode failure said
    /// without its value — never the error's own `Display` or its `source`
    /// alone: the first may spell a decode failure in serde's words, and the
    /// second drops what a `.context(…)` wrapped.
    DeadLetter(JobError),
    /// The job was sealed by a newer release than this consumer's, which cannot
    /// read it: nothing ran and no attempt is spent. Hand the stored record
    /// back **as it was stored** — [`Delivery::retry_envelope`] answers it
    /// unchanged — to be delivered again once `after` has passed, and
    /// acknowledge this delivery; a backend that counted an attempt start for
    /// it takes the start back, as for any delivery handed back without an
    /// answer, and one that keeps how long a job has waited unread records the
    /// first such hand-back ([`Delivery::with_deferred_for`]).
    ///
    /// A rolling deploy is the case: an older replica meets a newer producer's
    /// job, and the job waits for a replica of the newer release instead of
    /// being dead-lettered by one that is leaving — for
    /// [`NEWER_RELEASE_PATIENCE`] at the most. `after` is
    /// [`NEWER_RELEASE_WAIT`].
    Defer {
        /// How long the job waits before it is delivered again.
        after: Duration,
    },
}

/// One job as its backend delivered it — what every attempt at it reads.
///
/// Built once per delivery by the adapter and handed to each [`attempt`], so
/// what the delivery holds — the stored value, the job's id, the attempt it is
/// at, the checkpoint read so far — is shared by the attempts a retry budget
/// runs inside it.
#[non_exhaustive]
pub struct Delivery {
    backend: &'static QueueBackend,
    queue: QueueName,
    id: JobId,
    /// The attempt the next call to [`attempt`] runs.
    attempt: u32,
    /// How many earlier deliveries of this record ended without an answer.
    stalls: u32,
    /// How long the backend has handed the job back unread, when it keeps that
    /// — see [`Delivery::with_deferred_for`].
    deferred_for: Option<Duration>,
    unique_key: Option<String>,
    backend_id: Option<String>,
    message: Value,
    checkpoints: Option<Arc<CheckpointCell>>,
    /// The trace the first attempt minted, when the stored value carried none
    /// to continue: every later attempt at this delivery runs inside it, so the
    /// attempts at one job are one trace whether or not the envelope had one.
    minted: Option<Correlation>,
    /// Whether the lines said once per delivery — a legacy payload, an
    /// unusable key — have been filed.
    announced: bool,
}

impl Delivery {
    /// The job `backend` fetched from `queue`, as it stored it: the sealed
    /// envelope, or any value a foreign producer wrote.
    ///
    /// The job's [`JobId`], the attempt it is at and its unique key are read out
    /// of the envelope here. A value naming no usable id — one no push of this
    /// release wrote — runs under an id minted for this delivery, as attempt 1.
    pub fn new(backend: &'static QueueBackend, queue: QueueName, message: Value) -> Self {
        let identity = envelope::identify(&message);
        Self {
            backend,
            queue,
            id: identity.id.unwrap_or_else(JobId::mint),
            attempt: identity.attempt.unwrap_or(1),
            stalls: 0,
            deferred_for: None,
            unique_key: identity.unique_key,
            backend_id: None,
            message,
            checkpoints: None,
            minted: None,
            announced: false,
        }
    }

    /// The backend's own id for the stored record — reported beside the job's
    /// [`JobId`] as `backend_id`, never in its place.
    pub fn with_backend_id(mut self, backend_id: impl Into<String>) -> Self {
        self.backend_id = Some(backend_id.into());
        self
    }

    /// How many times the backend handed this record over, this delivery
    /// included: every earlier one ended without an answer, and past
    /// [`STALL_LIMIT`] of them the job is dead-lettered without running.
    pub(crate) fn with_delivery_count(mut self, count: u32) -> Self {
        self.stalls = count.saturating_sub(1);
        self
    }

    /// How long the backend has handed this job back unread for a newer release
    /// — since the first of an unbroken run of [`AttemptOutcome::Defer`]
    /// answers, zero when there is none — for a backend that can keep it per
    /// job.
    ///
    /// A job a newer release sealed is dead-lettered once it has waited unread
    /// past [`NEWER_RELEASE_PATIENCE`]. Counted from its first hand-back, a job
    /// pushed with a delay is not charged for the delay; without this, the port
    /// counts from the job's push — the instant its id was minted — which
    /// charges a delayed job its delay too. A backend keeps the instant of the
    /// first hand-back answering `Defer`, clears it once a delivery reads the job
    /// or the job settles, and passes how long ago that was.
    pub fn with_deferred_for(mut self, waited: Duration) -> Self {
        self.deferred_for = Some(waited);
        self
    }

    /// Keep this job's checkpoint in `store` — for a method whose options
    /// declare a checkpoint. The store is the job's: keyed by [`id`](Self::id).
    pub fn with_checkpoint(mut self, store: Arc<dyn CheckpointStore>) -> Self {
        self.checkpoints = Some(Arc::new(CheckpointCell::new(store, self.queue.clone())));
        self
    }

    /// The queue the job was fetched from.
    pub fn queue(&self) -> &QueueName {
        &self.queue
    }

    /// The job's id — the one its push returned, the same across its attempts.
    pub fn id(&self) -> &JobId {
        &self.id
    }

    /// The attempt the next call to [`attempt`] runs, from 1: the envelope's —
    /// or the backend's count of attempts started, when that is later — then
    /// one more after each [`AttemptOutcome::Retry`].
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// The unique key the job was pushed under, which the backend releases when
    /// the job reaches its terminal outcome.
    pub fn unique_key(&self) -> Option<&str> {
        self.unique_key.as_deref()
    }

    /// The record to re-file for the attempt an [`AttemptOutcome::Retry`]
    /// announced: the stored envelope, stamped with the job's id and the next
    /// attempt's number, carrying the trace every attempt at the job shares.
    ///
    /// For a job a newer release sealed — [`AttemptOutcome::Defer`] — it is the
    /// stored record unchanged, which this consumer cannot re-seal.
    pub fn retry_envelope(&self) -> Envelope {
        envelope::retry(
            &self.message,
            &self.id,
            self.attempt,
            self.minted.as_ref(),
            self.announced,
        )
    }
}

/// Run the attempt `delivery` is at (1 for the first) of `method` — the whole of
/// what an attempt is, from the envelope to the line that reports it.
///
/// The job's id and the attempt ride the span and the operation line, so the
/// attempts at one job are one `job_id` and distinct `attempt`s. The budget is
/// the port's: a retryable failure is [`AttemptOutcome::Retry`] while the
/// attempt is within `retries` re-runs of the first — and the delivery counts
/// the next one — and [`AttemptOutcome::DeadLetter`] on the last, so an adapter
/// never counts, and every backend spends a budget alike. A delivery past
/// [`STALL_LIMIT`] deliveries that ended without an answer is dead-lettered
/// without running.
///
/// A job sealed by a newer release runs nothing: the delivery says so once, at
/// `warn`, naming both versions, and the answer is [`AttemptOutcome::Defer`] —
/// until the job has waited unread past [`NEWER_RELEASE_PATIENCE`], or at once
/// when it names no id this release reads, since nothing can then follow it
/// from one delivery to the next: it is then dead-lettered, as a unit of work
/// with its line. Both are filed in the newer envelope's trace when it spells
/// one as this release does.
///
/// **An attempt its driver drops still files its line.** A driver stops an
/// attempt by dropping this future — a drain whose window closed on it, a
/// worker torn down — and the handler is dropped where it waits. That attempt
/// was a unit of work all the same, so it files its `queue.job` line with
/// [`CANCELLED`](nest_rs_core::operation_log::CANCELLED) and the time it ran,
/// in the job's own trace; what the driver does with the job next — hand it
/// back, take the attempt back — is the driver's to say.
pub async fn attempt(
    method: &'static ProcessMethod,
    delivery: &mut Delivery,
    container: Container,
) -> AttemptOutcome {
    let newer = envelope::newer_version(&delivery.message);
    if let Some(version) = newer
        && let Some(waited) = waited_unread(delivery)
        && waited < NEWER_RELEASE_PATIENCE
    {
        return defer_newer(delivery, version, waited).await;
    }
    let attempt = delivery.attempt;
    let retries = method.options().retries();
    let last = attempt > retries;
    // The last attempt takes the stored value; one another may follow reads it.
    let message = if last {
        Cow::Owned(std::mem::take(&mut delivery.message))
    } else {
        Cow::Borrowed(&delivery.message)
    };
    let (payload, inherited, minted_trace, unversioned, unusable) = match newer {
        // Past its patience: nothing is opened, and the job's own trace is
        // what its dead letter is filed in.
        Some(_) => (
            Ok(Cow::Owned(Value::Null)),
            envelope::newer_trace(&message),
            false,
            false,
            Unusable::default(),
        ),
        None => match envelope::open(message, delivery.queue.as_str()) {
            Ok(Opened::Sealed {
                payload,
                correlation,
                minted,
                unusable,
            }) => (Ok(payload), correlation, minted, false, unusable),
            Ok(Opened::Unversioned(value)) => (Ok(value), None, false, true, Unusable::default()),
            Err(refused) => (Err(refused), None, false, false, Unusable::default()),
        },
    };
    let input = match (newer, payload) {
        (Some(version), _) => Input::Unread {
            version,
            waited: waited_unread(delivery),
        },
        _ if delivery.stalls >= STALL_LIMIT => Input::Stalled {
            deliveries: delivery.stalls.saturating_add(1),
        },
        (None, Ok(payload)) => Input::Payload(payload),
        (None, Err(refused)) => Input::Refused(refused),
    };
    // The producer sealed its W3C trace context into the envelope, because a
    // queue is the one hop the framework crosses that is a *process* boundary
    // rather than a task one. Continuing it here is what makes the HTTP request
    // that enqueued, and this worker minutes later in another binary, one trace
    // with the job a child of the enqueue — every attempt a child of it, so the
    // attempts at one job share its trace. A value carrying nothing to continue
    // gets a trace minted at the first attempt (for the actor it names, when it
    // names one), and every later attempt runs as a child of that first one:
    // one trace per delivery either way — and a driver runs every attempt of a
    // budget inside its delivery — which is what an operator following a retried
    // job by its `trace_id` is promised. A backend re-filing each attempt carries
    // the first attempt's trace in the record, marked as minted: continued, it
    // joins the job's own trace and is still not the producer's.
    let continued_trace = !minted_trace
        && inherited
            .as_ref()
            .is_some_and(Correlation::parent_is_remote);
    let correlation = match (inherited, &delivery.minted) {
        (Some(continued), _) if continued.parent_is_remote() => continued,
        (_, Some(first)) => first.child(),
        (minted, None) => {
            let first = minted.unwrap_or_else(|| Correlation::minted(None));
            delivery.minted = Some(first.clone());
            first
        }
    };
    let first_opening = !delivery.announced;
    delivery.announced = true;
    let queue = delivery.queue.as_str();
    // One span per attempt; `.instrument` (not an entered guard held across
    // `.await`) keeps it current for the whole poll. Through `operation_span!` so
    // a job declares the canonical fields every edge does, and in OpenTelemetry's
    // messaging vocabulary so a messaging view recognises it: a `process` span of
    // `messaging.system` on `messaging.destination.name`, named as the
    // conventions name it.
    let span = nest_rs_core::operation_span!(
        unit::JOB,
        &correlation,
        otel.name = %format_args!("{PROCESS} {queue}"),
        messaging.system = delivery.backend.name(),
        messaging.operation.name = PROCESS,
        messaging.operation.type = PROCESS,
        messaging.destination.name = queue,
        messaging.message.id = %delivery.id,
        backend_id = delivery.backend_id.as_deref(),
        processor = method.name(),
        attempt,
        // Whether this job is traceable back to what enqueued it, or starts a
        // trace of its own. An operator chasing a lost request needs to tell
        // the two apart.
        continued_trace,
    );
    // What the job's own line reports it *was*. The span carries the same facts
    // for the export; a log line renders no span state, so the line that names
    // the work has to carry them as event attributes of its own.
    let identity = JobIdentity {
        queue: delivery.queue.clone(),
        processor: method.name(),
        job_id: delivery.id.clone(),
        backend_id: delivery.backend_id.clone(),
        attempt,
    };
    let checkpoints = delivery.checkpoints.clone();
    let context = HandlerContext {
        container: container.clone(),
        checkpoints: checkpoints.clone(),
    };
    let retry_after = backoff::retry_after(&delivery.id, attempt);
    // The ambient context too, not just the span: a `#[process]` body that
    // enqueues a follow-up job must seal *this* trace, not mint a third one and
    // break the chain — and every line below, the attempt's own included, reads
    // its trace ids off the ambient context, not off the span.
    let scope = Arc::new(RequestScope::new(container));
    let outcome = with_request_scope(Some(scope), correlation, async move {
        tracing::debug!(target: TARGET, attempt, "job started");
        // Said once per delivery: every attempt reopens the same stored value,
        // and a retry is not a second legacy job.
        if first_opening {
            if unversioned {
                tracing::warn!(
                    target: TARGET,
                    queue = %identity.queue,
                    job_id = %identity.job_id,
                    hint = "producer predates the wire envelope; drain the queue to clear legacy jobs",
                    "processed an unversioned job payload",
                );
            }
            if unusable.any() {
                // The keys are listed in a field of their own: a field called
                // `actor_id` would read as the actor the line names.
                tracing::warn!(
                    target: TARGET,
                    queue = %identity.queue,
                    job_id = %identity.job_id,
                    unusable = %unusable.keys(),
                    hint = %unusable.hint(),
                    "job envelope carries keys this consumer cannot use",
                );
            }
        }
        let timeout = method.options().timeout();
        run(method.handler(), input, context, identity, last, retry_after, timeout).await
    })
    .instrument(span)
    .await;
    if matches!(outcome, AttemptOutcome::Retry { .. }) {
        delivery.attempt = attempt.saturating_add(1);
    }
    outcome
}

/// How long a job a newer release sealed has waited unread: the backend's
/// record when it keeps one, its push's age otherwise — and `None` when it
/// names no id this release reads, since nothing then follows it from one
/// delivery to the next and no wait can be counted.
fn waited_unread(delivery: &Delivery) -> Option<Duration> {
    let id = envelope::identify(&delivery.message).id?;
    Some(delivery.deferred_for.unwrap_or_else(|| id.age()))
}

/// What to do about a job a newer release sealed that this release keeps
/// handing back — the remedy every line about one names.
const NEWER_RELEASE_REMEDY: &str = "a newer release sealed this job and only its consumers can \
     run it: finish rolling them forward, or, rolling back, keep one running until the queue \
     holds none of its jobs — a job still unread a day after it was first handed back is \
     dead-lettered, naming both versions, and kept with the dead letters for a consumer of that \
     release";

/// Hand back a job a newer release sealed: nothing runs, and the delivery says
/// once, at `warn` and in the job's own trace when its envelope spells one as
/// this release does, which release sealed it, which this one reads, and how
/// long it has waited.
async fn defer_newer(delivery: &mut Delivery, version: u64, waited: Duration) -> AttemptOutcome {
    if !delivery.announced {
        delivery.announced = true;
        let correlation =
            envelope::newer_trace(&delivery.message).unwrap_or_else(|| Correlation::minted(None));
        let (queue, job_id) = (&delivery.queue, &delivery.id);
        with_request_scope(None, correlation, async {
            tracing::warn!(
                target: TARGET,
                queue = %queue,
                job_id = %job_id,
                version,
                supported = crate::WIRE_FORMAT_VERSION,
                retry_after_ms = millis(NEWER_RELEASE_WAIT),
                waited_ms = millis(waited),
                patience_ms = millis(NEWER_RELEASE_PATIENCE),
                hint = NEWER_RELEASE_REMEDY,
                "job sealed by a newer release handed back unread",
            );
        })
        .await;
    }
    AttemptOutcome::Defer {
        after: NEWER_RELEASE_WAIT,
    }
}

/// `duration` in whole milliseconds, for a line's field.
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Settle a job `backend` fetched and can deliver to no method — a record it
/// cannot read, a queue name outside the rule, a queue no method serves.
///
/// Still a unit of work, so still the port's: it opens the `queue.job` span, files
/// the dead-letter event and the `nest_rs::operation` line exactly as an attempt
/// does, and hands back the error the adapter dead-letters the job with. The
/// adapter keeps only `error`'s sentence — *why* its own storage could not route
/// the record — and never logs the outcome itself.
///
/// `queue` is the name the record carried and `message` the value stored under
/// it, when the record could be read that far; `backend_id` is the backend's own
/// id for the record, when it has one. The job's id and the trace a sealed
/// envelope carries are read from it, as an attempt reads them, so following the
/// push's trace — or its receipt — reaches the dead-letter. Nothing here trusts
/// `queue`: the span is named for it only when it is a name a push could have
/// written, and the raw string reaches the event alone, where the formatter
/// escapes it.
///
/// **Only a driver that holds the record can call it.** One whose library
/// decodes inside its own fetch never sees the record that failed, nor the
/// batch dropped with it, and says so in its own line instead of routing here.
pub async fn refuse(
    backend: &'static QueueBackend,
    queue: Option<&str>,
    message: Option<&Value>,
    backend_id: Option<&str>,
    error: JobError,
) -> JobError {
    let started = Instant::now();
    let named = queue.and_then(|raw| QueueName::new(raw.to_owned()).ok());
    let job_id = message.and_then(|message| envelope::identify(message).id);
    let continued = match (message, named.as_ref()) {
        (Some(message), Some(name)) => {
            match envelope::open(Cow::Borrowed(message), name.as_str()) {
                Ok(Opened::Sealed { correlation, .. }) => correlation,
                _ => None,
            }
        }
        _ => None,
    };
    let correlation = continued.unwrap_or_else(|| Correlation::minted(None));
    let destination = named.as_ref().map_or("unknown", QueueName::as_str);
    let job_id = job_id.as_ref().map(ToString::to_string);
    let span = nest_rs_core::operation_span!(
        unit::JOB,
        &correlation,
        otel.name = %format_args!("{PROCESS} {destination}"),
        messaging.system = backend.name(),
        messaging.operation.name = PROCESS,
        messaging.operation.type = PROCESS,
        messaging.destination.name = named.as_ref().map(QueueName::as_str),
        messaging.message.id = job_id.as_deref(),
        backend_id,
        attempt = 1u32,
    );
    let line_span = span.clone();
    with_request_scope(None, correlation, async move {
        tracing::error!(
            target: TARGET,
            queue,
            job_id = job_id.as_deref(),
            backend_id,
            error = %nest_rs_core::error_message(&error),
            "job dead-lettered: undeliverable",
        );
        nest_rs_core::operation_line!(
            unit::JOB,
            span: &line_span,
            outcome: nest_rs_core::operation_log::ERROR,
            started: started,
            queue,
            job_id = job_id.as_deref(),
            backend_id,
            attempt = 1u32,
        );
        error
    })
    .instrument(span)
    .await
}

/// What an attempt runs on.
enum Input<'a> {
    /// The payload the envelope carried, for the handler.
    Payload(Cow<'a, Value>),
    /// Nothing: the envelope was refused, and this is why.
    Refused(JobError),
    /// Nothing: the record was handed over `deliveries` times, and every
    /// delivery before this one ended without an answer — past [`STALL_LIMIT`].
    Stalled {
        /// How many times the record was handed over, this delivery included.
        deliveries: u32,
    },
    /// Nothing: a newer release sealed the job, and it has waited unread past
    /// [`NEWER_RELEASE_PATIENCE`] — or names no id, so no wait can be counted.
    Unread {
        /// The wire-format version that sealed it.
        version: u64,
        /// How long it waited unread, when that can be counted.
        waited: Option<Duration>,
    },
}

/// Why an attempt ran nothing.
enum Unrun {
    /// The record's deliveries past [`STALL_LIMIT`] ended without an answer.
    Stalled { deliveries: u32 },
    /// A newer release sealed the job and it was not read in time.
    Unread {
        version: u64,
        waited: Option<Duration>,
    },
}

/// What one job attempt is, for the line that reports it ran.
///
/// Held as a value rather than read back off the span: `tracing` gives no way to
/// read a span's fields, and a log line renders none of them anyway.
struct JobIdentity {
    queue: QueueName,
    processor: &'static str,
    job_id: JobId,
    backend_id: Option<String>,
    attempt: u32,
}

/// An attempt's `queue.job` line, filed exactly once: with the outcome the
/// attempt settled on, or — dropped before it settled — with
/// [`CANCELLED`](nest_rs_core::operation_log::CANCELLED).
///
/// Dropped inside the attempt's span and ambient scope, since both wrappers
/// drop the future they wrap inside what they install, so the cancelled line
/// carries the job's trace like every other line of the attempt.
struct JobLine {
    identity: JobIdentity,
    started: Instant,
    /// The attempt's span, which the outcome is recorded on for the export.
    span: tracing::Span,
    filed: bool,
}

impl JobLine {
    /// Opened first thing inside the attempt, so the span current here is the
    /// attempt's own.
    fn open(identity: JobIdentity) -> Self {
        Self {
            identity,
            started: Instant::now(),
            span: tracing::Span::current(),
            filed: false,
        }
    }

    /// File the line for an attempt that settled on `outcome`.
    fn file(mut self, outcome: &'static str) {
        self.emit(outcome);
        self.filed = true;
    }

    fn emit(&self, outcome: &'static str) {
        let identity = &self.identity;
        nest_rs_core::operation_line!(
            unit::JOB,
            span: &self.span,
            outcome: outcome,
            started: self.started,
            queue = %identity.queue,
            processor = identity.processor,
            job_id = %identity.job_id,
            backend_id = identity.backend_id.as_deref(),
            attempt = identity.attempt,
        );
    }
}

impl Drop for JobLine {
    fn drop(&mut self) {
        if self.filed {
            return;
        }
        // Unwinding out of the port's own code rather than dropped by a driver:
        // the handler's panics are caught before here, so this one is ours, and
        // the unit still ended in one.
        let outcome = if std::thread::panicking() {
            nest_rs_core::operation_log::PANIC
        } else {
            nest_rs_core::operation_log::CANCELLED
        };
        self.emit(outcome);
    }
}

/// Run one job handler — or refuse the envelope it would have run on — and turn
/// the outcome into the event it logs plus the [`AttemptOutcome`] the adapter
/// translates. Lifted out of [`attempt`] so every terminal state is reachable
/// from a test.
///
/// | outcome | event | [`AttemptOutcome`] |
/// | --- | --- | --- |
/// | `Ok(())` | *(the operation line alone)* | `Ok` |
/// | non-retryable `Err` | `job dead-lettered: non-retryable failure` (`error`) | `DeadLetter` |
/// | retryable `Err`, budget left | `job failed; will retry within the budget` (`warn`) | `Retry` |
/// | retryable `Err`, last attempt | `job dead-lettered: retry budget spent` (`error`) | `DeadLetter` |
/// | **panic** | `job dead-lettered: handler panicked` (`error`) | `DeadLetter` |
/// | deliveries past [`STALL_LIMIT`] ended without an answer | `job dead-lettered: its deliveries ended without an answer past the stall limit` (`error`) | `DeadLetter` |
/// | a newer release's, unread past the patience | `job dead-lettered: a newer release sealed it, and none of its consumers ran it in time` (`error`) | `DeadLetter` |
///
/// The panic is caught **here** rather than left to a backend's panic layer,
/// which would contain it correctly and unwind past this function — skipping the
/// per-job span and every event below.
async fn run(
    handler: JobHandler,
    input: Input<'_>,
    context: HandlerContext,
    identity: JobIdentity,
    last: bool,
    retry_after: Duration,
    timeout: Duration,
) -> AttemptOutcome {
    let attempt = identity.attempt;
    let line = JobLine::open(identity);
    // `Err` when nothing ran, saying why.
    let outcome = match input {
        // Cut at its timeout, an attempt fails retryably: the worker awaits
        // developer code no edge deadline bounds, and a call that never answers
        // would hold its permit for as long as the process lives.
        Input::Payload(payload) => Ok(
            match tokio::time::timeout(
                timeout,
                AssertUnwindSafe(handler(payload, context)).catch_unwind(),
            )
            .await
            {
                Ok(caught) => caught,
                Err(_) => Ok(Err(JobError::retry(nest_rs_worker::JobTimedOut {
                    timeout,
                }))),
            },
        ),
        // An envelope of another version never reaches the handler: the refusal
        // is the attempt's outcome.
        Input::Refused(refused) => Ok(Ok(Err(refused))),
        Input::Stalled { deliveries } => Err(Unrun::Stalled { deliveries }),
        Input::Unread { version, waited } => Err(Unrun::Unread { version, waited }),
    };
    // Every terminal state, one detail event and one line. The detail says
    // *why* and stays on `nest_rs::queue`; the line says the job ran, and is the
    // family's. Neither restates the other's fields.
    let (settled, result) = match outcome {
        Err(Unrun::Unread { version, waited }) => {
            let supported = crate::WIRE_FORMAT_VERSION;
            tracing::error!(
                target: TARGET,
                version,
                supported,
                waited_ms = waited.map(millis),
                patience_ms = millis(NEWER_RELEASE_PATIENCE),
                hint = NEWER_RELEASE_REMEDY,
                "job dead-lettered: a newer release sealed it, and none of its consumers ran it \
                 in time",
            );
            let why = match waited {
                Some(_) => "waited unread past the day this release gives a newer one's job",
                None => "names no id this release reads, so no wait for it can be counted",
            };
            (
                nest_rs_core::operation_log::ERROR,
                AttemptOutcome::DeadLetter(JobError::abort(format!(
                    "job sealed by wire-format version {version}, which this consumer (version \
                     {supported}) cannot read, {why}; its record is kept with the dead letters \
                     for a consumer of that release"
                ))),
            )
        }
        Err(Unrun::Stalled { deliveries }) => {
            let unanswered = deliveries.saturating_sub(1);
            tracing::error!(
                target: TARGET,
                deliveries,
                stall_limit = STALL_LIMIT,
                "job dead-lettered: its deliveries ended without an answer past the stall limit",
            );
            (
                nest_rs_core::operation_log::ERROR,
                AttemptOutcome::DeadLetter(JobError::abort(format!(
                    "{unanswered} deliveries of the job ended without an answer — the process \
                     running each was stopped, froze or lost the queue backend past its lease — \
                     so it is dead-lettered without running"
                ))),
            )
        }
        Ok(Ok(Ok(()))) => (nest_rs_core::operation_log::OK, AttemptOutcome::Ok),
        Ok(Ok(Err(error))) if !error.retryable => {
            // `errors` carries the rejection's per-field detail when it had any —
            // the member name HTTP and the WebSocket error frame use, so one query
            // shape finds a validation failure on any transport.
            //
            // `error` is the failure's own sentence *and every cause beneath it*:
            // a wrapper names none of them — `the queue backend failed` is what a
            // checkpoint save finding no record displays — and the cause is what an
            // operator acts on. The dead-letter record is rendered the same way.
            tracing::error!(
                target: TARGET,
                error = %nest_rs_core::error_message(&error),
                errors = error.details.as_ref().map(tracing::field::display),
                "job dead-lettered: non-retryable failure",
            );
            (
                nest_rs_core::operation_log::ERROR,
                AttemptOutcome::DeadLetter(error),
            )
        }
        Ok(Ok(Err(error))) if last => {
            tracing::error!(
                target: TARGET,
                error = %nest_rs_core::error_message(&error),
                attempts = attempt,
                "job dead-lettered: retry budget spent",
            );
            (
                nest_rs_core::operation_log::ERROR,
                AttemptOutcome::DeadLetter(error),
            )
        }
        Ok(Ok(Err(error))) => {
            tracing::warn!(
                target: TARGET,
                error = %nest_rs_core::error_message(&error),
                retry_after_ms = retry_after.as_millis() as u64,
                "job failed; will retry within the budget",
            );
            (
                nest_rs_core::operation_log::ERROR,
                AttemptOutcome::Retry { after: retry_after },
            )
        }
        Ok(Err(panic)) => {
            nest_rs_core::contained_panic!(
                target: TARGET,
                panic.as_ref(),
                "job dead-lettered: handler panicked",
            );
            // A panic is deterministic as far as the queue can tell — the same
            // payload panics again — so it dead-letters rather than burning the
            // retry budget.
            (
                nest_rs_core::operation_log::PANIC,
                AttemptOutcome::DeadLetter(JobError::abort(panic_message(panic.as_ref()))),
            )
        }
    };

    line.file(settled);
    result
}

#[cfg(test)]
mod tests {
    use std::future::Future;

    use nest_rs_testing::LogCapture;

    use super::*;

    fn identity(attempt: u32) -> JobIdentity {
        JobIdentity {
            queue: QueueName::new("audio").expect("a valid name"),
            processor: "AudioProcessor",
            job_id: JobId::mint(),
            backend_id: None,
            attempt,
        }
    }

    fn context() -> HandlerContext {
        HandlerContext {
            container: Container::builder().build(),
            checkpoints: None,
        }
    }

    type Handler<'a> = std::pin::Pin<Box<dyn Future<Output = Result<(), JobError>> + Send + 'a>>;

    async fn run_payload(handler: JobHandler, last: bool) -> AttemptOutcome {
        run(
            handler,
            Input::Payload(Cow::Owned(serde_json::json!({}))),
            context(),
            identity(1),
            last,
            Duration::from_secs(1),
            nest_rs_worker::JOB_TIMEOUT,
        )
        .await
    }

    /// A backend's panic layer dead-letters a panicking job correctly, and
    /// `nest_rs::queue` used to say **nothing** about it. The panic branch emits
    /// the same shape a deserialization failure does.
    #[tokio::test]
    async fn a_panicking_handler_is_dead_lettered_with_an_event() {
        fn boom(_job: Cow<'_, Value>, _context: HandlerContext) -> Handler<'_> {
            Box::pin(async { panic!("deliberate panic for panic-2") })
        }

        let logs = LogCapture::install();
        // The default hook would print the panic to stderr and drown the test
        // output; the event under test is the structured one.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let result = run_payload(boom, false).await;
        std::panic::set_hook(previous);

        assert!(
            matches!(result, AttemptOutcome::DeadLetter(_)),
            "a panic is deterministic — it dead-letters instead of burning the retry budget",
        );

        let event = logs.expect_one(TARGET, "job dead-lettered: handler panicked");
        assert_eq!(
            event.level, "error",
            "at the docs' own production filter (`nest_rs::queue=warn`) it has to show",
        );
        assert_eq!(
            event.field(nest_rs_core::panic::FIELD).as_deref(),
            Some("deliberate panic for panic-2"),
            "the panic message rides on the shared `panic` field: {event:#?}",
        );
        let ran = logs.expect_one(nest_rs_core::operation_log::TARGET, unit::JOB.name());
        assert_eq!(
            ran.field("outcome").as_deref(),
            Some(nest_rs_core::operation_log::PANIC),
            "a panic is its own outcome, not a plain error: {ran:#?}",
        );
        assert!(ran.field("duration_ms").is_some());
    }

    /// A dead-lettered job is read from a log, days later, by someone who cannot
    /// re-run it; the rejection's per-field detail rides the event as `errors`.
    #[tokio::test]
    async fn a_dead_lettered_pipe_rejection_logs_its_field_errors() {
        fn rejected(_job: Cow<'_, Value>, _context: HandlerContext) -> Handler<'_> {
            Box::pin(async {
                Err(
                    JobError::abort("validation failed").with_details(Some(serde_json::json!({
                        "slug": [{ "code": "length" }],
                    }))),
                )
            })
        }

        let logs = LogCapture::install();
        let result = run_payload(rejected, false).await;
        // The classification, not merely the failure: a non-retryable error that
        // stopped dead-lettering would spend the whole budget re-running a payload
        // that cannot succeed, and every assertion below would still pass.
        assert!(
            matches!(result, AttemptOutcome::DeadLetter(_)),
            "a non-retryable failure dead-letters so the budget is never spent on it",
        );

        let event = logs.expect_one(TARGET, "job dead-lettered: non-retryable failure");
        let errors = event
            .field("errors")
            .unwrap_or_else(|| panic!("the dead-letter event carries `errors`: {event:#?}"));
        assert!(
            errors.contains("slug"),
            "and it names the offending field: {errors}"
        );
    }

    /// A failure with nothing structured to say must not invent an `errors`
    /// field — an empty one reads as "checked, nothing found".
    #[tokio::test]
    async fn a_dead_letter_without_detail_logs_no_errors_field() {
        fn bare(_job: Cow<'_, Value>, _context: HandlerContext) -> Handler<'_> {
            Box::pin(async { Err(JobError::abort("missing field `id`")) })
        }

        let logs = LogCapture::install();
        assert!(matches!(
            run_payload(bare, false).await,
            AttemptOutcome::DeadLetter(_)
        ));
        let event = logs.expect_one(TARGET, "job dead-lettered: non-retryable failure");
        assert!(
            event.field("errors").is_none(),
            "no detail ⇒ no field: {event:#?}"
        );
    }

    /// The outcomes that are not panics, pinned against each other.
    #[tokio::test]
    async fn every_other_outcome_keeps_its_own_event() {
        fn ok(_job: Cow<'_, Value>, _context: HandlerContext) -> Handler<'_> {
            Box::pin(async { Ok(()) })
        }
        fn fatal(_job: Cow<'_, Value>, _context: HandlerContext) -> Handler<'_> {
            Box::pin(async { Err(JobError::abort("missing field `id`")) })
        }
        fn transient(_job: Cow<'_, Value>, _context: HandlerContext) -> Handler<'_> {
            Box::pin(async { Err(JobError::retry("upstream timed out")) })
        }

        let logs = LogCapture::install();
        assert!(matches!(run_payload(ok, false).await, AttemptOutcome::Ok));
        assert!(matches!(
            run_payload(fatal, false).await,
            AttemptOutcome::DeadLetter(_)
        ));
        assert!(matches!(
            run_payload(transient, false).await,
            AttemptOutcome::Retry { .. }
        ));

        // Success is said once, and it is the family's line that says it.
        assert!(
            logs.find(TARGET, "job ok").is_empty(),
            "a successful job reports through `nest_rs::operation`, not twice",
        );
        assert_eq!(
            logs.expect_one(TARGET, "job dead-lettered: non-retryable failure")
                .level,
            "error",
        );
        assert_eq!(
            logs.expect_one(TARGET, "job failed; will retry within the budget")
                .level,
            "warn",
        );
    }

    /// The retryable half: a transient failure that eventually succeeds leaves
    /// the queue looking healthy, and this `warn` is the only signal before a
    /// job succeeding on attempt four every time falls over.
    #[tokio::test]
    async fn a_retryable_failure_with_budget_left_is_reported_before_it_runs_again() {
        fn flaky(_job: Cow<'_, Value>, _context: HandlerContext) -> Handler<'_> {
            Box::pin(async { Err(JobError::retry("the upstream API timed out")) })
        }

        let logs = LogCapture::install();
        let result = run_payload(flaky, false).await;
        assert!(
            matches!(result, AttemptOutcome::Retry { .. }),
            "a retryable failure with budget left is a `Retry`",
        );

        let event = logs.expect_one(TARGET, "job failed; will retry within the budget");
        assert_eq!(event.level, "warn");
        assert!(
            event
                .field("error")
                .is_some_and(|e| e.contains("upstream API")),
            "the event carries the cause the retry will hit again, got {:?}",
            event.fields,
        );
        let ran = logs.expect_one(nest_rs_core::operation_log::TARGET, unit::JOB.name());
        assert_eq!(
            ran.field("outcome").as_deref(),
            Some(nest_rs_core::operation_log::ERROR),
        );
        assert_eq!(ran.field("queue").as_deref(), Some("audio"));
        assert!(ran.field("duration_ms").is_some());
    }

    /// A wrapper names no cause — `the queue backend failed` is what a checkpoint
    /// save finding no record displays — so a failure event carries the sentence
    /// and every cause beneath it, as the dead-letter record already did. It
    /// carried the wrapper alone, and the cause reached the console only from the
    /// backend's own line, on another target.
    #[tokio::test]
    async fn a_failure_event_carries_every_cause_beneath_the_error() {
        fn wrapped(_job: Cow<'_, Value>, _context: HandlerContext) -> Handler<'_> {
            Box::pin(async {
                Err(JobError::retry(crate::QueueError::backend(
                    std::io::Error::other("Job not found"),
                )))
            })
        }

        let logs = LogCapture::install();
        let outcome = run_payload(wrapped, false).await;
        assert!(matches!(outcome, AttemptOutcome::Retry { .. }));

        let event = logs.expect_one(TARGET, "job failed; will retry within the budget");
        assert_eq!(
            event.field("error").as_deref(),
            Some("the queue backend failed: Job not found"),
            "the wrapper alone names nothing an operator acts on: {:?}",
            event.fields,
        );
    }

    /// The last attempt of a budget: the job dead-letters, and the line saying
    /// so is not the one promising a retry that will never come — which is what
    /// every backend logged while each counted the budget for itself.
    #[tokio::test]
    async fn a_retryable_failure_on_the_last_attempt_dead_letters_and_says_the_budget_is_spent() {
        fn flaky(_job: Cow<'_, Value>, _context: HandlerContext) -> Handler<'_> {
            Box::pin(async { Err(JobError::retry("the upstream API timed out")) })
        }

        let logs = LogCapture::install();
        let result = run(
            flaky,
            Input::Payload(Cow::Owned(serde_json::json!({}))),
            context(),
            identity(4),
            true,
            Duration::from_secs(1),
            nest_rs_worker::JOB_TIMEOUT,
        )
        .await;
        assert!(matches!(result, AttemptOutcome::DeadLetter(_)));

        let spent = logs.expect_one(TARGET, "job dead-lettered: retry budget spent");
        assert_eq!(spent.level, "error");
        assert_eq!(spent.field("attempts").as_deref(), Some("4"));
        assert!(
            logs.find(TARGET, "job failed; will retry within the budget")
                .is_empty(),
            "no retry is promised on the attempt that has none left",
        );
    }

    /// An envelope of another version is refused before the handler runs, and
    /// dead-letters like any deterministic failure.
    #[tokio::test]
    async fn a_refused_envelope_never_reaches_the_handler() {
        fn unreachable_handler(_job: Cow<'_, Value>, _context: HandlerContext) -> Handler<'_> {
            Box::pin(async { panic!("the handler must not run on a refused envelope") })
        }

        let logs = LogCapture::install();
        let result = run(
            unreachable_handler,
            Input::Refused(JobError::abort("unsupported job wire-format version 99")),
            context(),
            identity(1),
            false,
            Duration::from_secs(1),
            nest_rs_worker::JOB_TIMEOUT,
        )
        .await;
        assert!(matches!(result, AttemptOutcome::DeadLetter(_)));
        logs.expect_one(TARGET, "job dead-lettered: non-retryable failure");
    }
}

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use croner::Cron;
use futures_util::FutureExt;
use nest_rs_core::{
    Container, Correlation, Discovery, ReachableProviders, Transport, inventory, panic_message,
};
use nest_rs_worker::{JobContext, JobTransaction, Unhonoured, run_in_job_context};
use tokio::task::JoinSet;
use tokio::time::{Instant, MissedTickBehavior, interval, sleep};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::{
    BACKEND_REMEDY, CronJobMeta, Occurrence, OccurrenceClaim, OccurrenceLock, Replicas, RunFn,
    ScheduledMethod, Trigger,
};

/// The shortest time an occurrence stays claimed. Keys are distinct per
/// instant, so a hold only has to outlast the skew between two replicas'
/// clocks — and, since a replica held past the following occurrence asks who
/// fired the ones it overran, that following occurrence too ([`claim_hold`]): a
/// minute is far past what NTP leaves, and short enough that the backend forgets
/// a short job's keys soon after.
const MIN_HOLD: Duration = Duration::from_secs(60);

/// How far two replicas' clocks may disagree while a stalled replica still skips,
/// rather than fires a second time, an occurrence a peer claimed. The replica
/// measures how late it is on its own clock, while the peer's claim runs out on
/// the backend's timer from an instant the peer read on its own: a replica
/// reaching an occurrence this close to its hold's end skips it. NTP keeps
/// clocks within milliseconds; this is the margin past it.
const MAX_SKEW: Duration = Duration::from_secs(10);

/// The most overrun occurrences one report asks the lock about, all at once;
/// past this many they are counted unchecked.
const CHECKED: u64 = 100;

/// Cron expressions and timezones are parsed in `configure` so a bad value
/// fails boot (not the first fire); each tick is cheap. The occurrence lock is
/// read there too, so a job declaring `replicas = "one"` with none bound fails
/// the boot rather than its first occurrence.
pub struct Scheduler {
    jobs: Vec<Job>,
    container: Option<Container>,
    lock: Option<Arc<dyn OccurrenceLock>>,
}

enum Job {
    Interval {
        id: JobId,
        period: Duration,
        task: Task,
    },
    Timeout {
        id: JobId,
        delay: Duration,
        task: Task,
    },
    Cron {
        id: JobId,
        // Boxed because a parsed Cron is ~330 bytes (large_enum_variant).
        schedule: Box<Cron>,
        tz: Option<Tz>,
        task: Task,
    },
}

/// What one fire runs, how its data-layer work is settled, and how many
/// replicas fire it — travelling together because they are decided together,
/// at the `#[every]` / `#[cron]` / `#[after]` that declares the job.
#[derive(Clone, Copy)]
struct Task {
    run: RunFn,
    transaction: JobTransaction,
    replicas: Replicas,
}

/// A job, kept split (declaring module, host struct, method) so logs filter on
/// the provider or the method alone instead of parsing a baked
/// `Provider::method` string — and the `key` that pins what it claims under.
#[derive(Clone, Copy)]
struct JobId {
    origin: &'static str,
    provider: &'static str,
    method: &'static str,
    key: Option<&'static str>,
}

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::{}", self.provider, self.method)
    }
}

impl JobId {
    /// What a job firing once claims its occurrences under, as levels: **its
    /// crate, its host struct and its method** — or the path its `key` pins.
    ///
    /// **The crate is in it**, because the lock is shared by every app of a
    /// deployment — an API and a worker on one Redis — and the boot sees one app's
    /// jobs only. Keyed on `Provider:method`, two apps each declaring their own
    /// `MaintenanceTasks::sweep` claimed each other's occurrences, and each job
    /// ran on some of them with nothing said above `debug`; their crates tell them
    /// apart, while one job declared in a crate both apps link is still one job,
    /// coordinating across both.
    ///
    /// **Only the crate**, because a module is what a refactor moves. Keyed on
    /// the declaring module's whole path, a file moved or a module renamed inside
    /// its crate split the job during the rolling deploy that shipped it: old
    /// replicas and new each claimed and fired every occurrence, with nothing
    /// said. Renaming the type, the method or the crate still starts a new job,
    /// and `key` is how a job outlives one: it pins the identity the job had.
    fn levels(&self) -> Vec<&'static str> {
        match self.key {
            Some(key) => key.split("::").collect(),
            None => vec![crate_of(self.origin), self.provider, self.method],
        }
    }

    /// The identity as a lock holds it — its levels, one `:`-separated level
    /// each: `features:NotificationsTasks:purge_expired`.
    ///
    /// A level per `::`, never the `::` form itself: the identity becomes a
    /// member of whatever key the bound lock writes, and `::` inside a
    /// `:`-delimited name leaves an empty segment no operator can glob.
    /// [`levels_refusal`](Self::levels_refusal) keeps it injective.
    fn identity(&self) -> String {
        self.levels().join(":")
    }

    /// The identity as `key = "…"` spells it — `features::NotificationsTasks::purge_expired`
    /// — which the boot line carries, so pinning a job before renaming it is
    /// copying what its boot said.
    fn key(&self) -> String {
        self.levels().join("::")
    }

    /// The token an occurrence is claimed under and asked about — **one
    /// derivation, because two sites read it.**
    ///
    /// It is a method rather than a `format!` at each site because the claim and
    /// the overrun check must agree exactly: built twice, they disagreed the
    /// moment one was edited, and the lock then answered "unclaimed" for an
    /// occurrence it had just granted — which fires a job every replica already
    /// fired.
    fn occurrence(&self, instant_ms: u64) -> String {
        format!("{}:{instant_ms}", self.identity())
    }

    /// Why this job cannot be identified, if it cannot: a level of its identity
    /// that is empty or carries a `:`, whitespace or a control character.
    ///
    /// Joined by `:`, the levels are one string, and two jobs whose levels differ
    /// could then spell one token — a provider `A:B` with a method `c` and a
    /// provider `A` with a method `B:c` — and split one occurrence between them,
    /// in two apps where no boot sees both. `#[scheduled]` writes idents,
    /// `module_path!()` and a `key` its grammar checked, which can do neither; a
    /// job attached by hand, whose parts are plain strings, is held to the same
    /// shape at the boot.
    fn levels_refusal(&self) -> Option<String> {
        let broken = |level: &str| !is_level(level);
        if self.origin.split("::").any(broken) {
            return Some(format!(
                "its origin `{}` is not a module path — `module_path!()` of the code declaring \
                 it, whose crate opens the key its occurrences are claimed under",
                self.origin
            ));
        }
        if let Some(key) = self.key {
            return (!is_key(key)).then(|| format!("{key:?} is not a job key: {KEY_RULE}"));
        }
        [("provider", self.provider), ("method", self.method)]
            .into_iter()
            .find(|(_, level)| broken(level))
            .map(|(part, level)| {
                format!(
                    "its {part} {level:?} is empty or carries a `:`, whitespace or a control \
                     character, and it is one level of the key its occurrences are claimed under"
                )
            })
    }
}

/// The crate `origin` — a `module_path!()` — was declared in: its first segment.
fn crate_of(origin: &str) -> &str {
    origin.split_once("::").map_or(origin, |(krate, _)| krate)
}

/// What a `key` is — the boot's copy of the rule `#[every]` and `#[cron]` check
/// their literal against, since a job attached by hand reaches the boot with a
/// plain string. Written twice and pinned once: the suite runs both copies over
/// one corpus and holds both refusals to [`KEY_RULE`] and [`KEY_NEEDS_ONE`].
fn is_key(value: &str) -> bool {
    value.split("::").all(is_level)
}

/// Whether `level` can be one level of an identity: non-empty, and free of the
/// `:` that separates levels, of whitespace and of control characters — what an
/// ident and a `module_path!()` segment always are, and a string attached by hand
/// may not be.
fn is_level(level: &str) -> bool {
    !level.is_empty()
        && !level
            .chars()
            .any(|c| c == ':' || c.is_whitespace() || c.is_control())
}

/// Why a string is not a key — the fact the decorator's refusal states too.
const KEY_RULE: &str = "it takes one or more levels joined by `::`, each non-empty and free of `:`, \
     whitespace and control characters — `:` separates the levels of the key a backend claims \
     under, and whitespace would reach a log field";

/// Why a job firing on every replica cannot carry a key — the decorator's fact
/// too.
const KEY_NEEDS_ONE: &str = "a key pins what a job firing once claims its occurrences under, and \
     this job fires on every replica and claims none — declare `replicas = \"one\"` beside it, or \
     remove the key";

impl From<&CronJobMeta> for Task {
    fn from(meta: &CronJobMeta) -> Self {
        Self {
            run: meta.run,
            transaction: meta.transaction,
            replicas: meta.replicas,
        }
    }
}

impl Job {
    fn id(&self) -> JobId {
        match self {
            Job::Interval { id, .. } | Job::Timeout { id, .. } | Job::Cron { id, .. } => *id,
        }
    }

    fn task(&self) -> Task {
        match self {
            Job::Interval { task, .. } | Job::Timeout { task, .. } | Job::Cron { task, .. } => {
                *task
            }
        }
    }
}

impl Scheduler {
    /// How long a tick still running when shutdown is asked for is given to
    /// finish before the scheduler stops it: the shutdown hooks' budget,
    /// [`SHUTDOWN_HOOKS_TIMEOUT`](nest_rs_core::SHUTDOWN_HOOKS_TIMEOUT).
    ///
    /// **A tick is developer code on the way down, as a hook is, so it gets what
    /// a hook gets** — and it gets it *before* the hooks rather than beside them:
    /// `serve` returns only once every tick has ended or been stopped, so no tick
    /// runs through the shutdown hooks it may depend on. A tick still running at
    /// the bound is dropped where it waits, files its `schedule.tick` line
    /// `outcome = "cancelled"`, and is named in one `warn`; `serve` then waits
    /// [`SHUTDOWN_SETTLE_TIMEOUT`](nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT) at most
    /// for it to unwind. What a dropped tick had not done stays undone, and a
    /// transaction it held open is rolled back once its statement drains, which
    /// `nest_rs_seaorm` reports at `warn`.
    ///
    /// Before this bound a tick that never returned held the scheduler's stop,
    /// and with it the whole way down, until the orchestrator killed the process
    /// with nothing said.
    pub const SHUTDOWN_TIMEOUT: Duration = nest_rs_core::SHUTDOWN_HOOKS_TIMEOUT;

    /// An empty scheduler with no container bound — jobs are added at
    /// `configure` from the inventory. Prefer this over relying on `Default`.
    pub fn new() -> Self {
        Self {
            jobs: Vec::new(),
            container: None,
            lock: None,
        }
    }

    fn resolve(meta: &Arc<CronJobMeta>) -> Result<Job> {
        let id = JobId {
            origin: meta.origin,
            provider: meta.provider,
            method: meta.method,
            key: meta.key,
        };
        if let Some(why) = id.levels_refusal() {
            anyhow::bail!("scheduled job `{id}` cannot be identified: {why}");
        }
        // The decorator refuses this at compile time; a job attached by hand is
        // held to the same rule here, since an ignored key is a declaration that
        // says something nothing does.
        if let (Some(key), Replicas::Each) = (meta.key, meta.replicas) {
            anyhow::bail!("scheduled job `{id}` pins `key = {key:?}`: {KEY_NEEDS_ONE}");
        }
        Ok(match meta.trigger {
            Trigger::Interval(period) => {
                // The duration grammar refuses this at compile time; a job
                // registered by hand reaches here, and is held to the same floor
                // whatever its `replicas`. A zero period has no next tick — the
                // interval timer panics on it — a shorter one than a millisecond is
                // finer than that timer resolves, and a claimed tick is keyed by
                // its millisecond, so one period must hold at least one.
                if period.as_millis() == 0 {
                    anyhow::bail!(
                        "scheduled job `{id}` has an interval of {period:?} — an interval is at \
                         least one millisecond"
                    );
                }
                Job::Interval {
                    id,
                    period,
                    task: Task::from(&**meta),
                }
            }
            Trigger::Timeout(delay) => {
                if meta.replicas == Replicas::One {
                    anyhow::bail!(
                        "scheduled job `{id}` is a one-shot declaring `replicas = \"one\"` — it \
                         fires once after this process boots, so each replica's boot is its own \
                         event and there is no shared occurrence to claim"
                    );
                }
                Job::Timeout {
                    id,
                    delay,
                    task: Task::from(&**meta),
                }
            }
            Trigger::Cron { expr, tz } => {
                let schedule = Cron::from_str(expr).with_context(|| {
                    format!("cron job `{id}` has an invalid cron expression `{expr}`")
                })?;
                let tz = tz
                    .map(|name_str| {
                        name_str.parse::<Tz>().map_err(|e| {
                            anyhow::anyhow!(
                                "cron job `{id}` has an invalid timezone `{name_str}`: {e}"
                            )
                        })
                    })
                    .transpose()?;
                Job::Cron {
                    id,
                    schedule: Box::new(schedule),
                    tz,
                    task: Task::from(&**meta),
                }
            }
        })
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Transport for Scheduler {
    async fn configure(&mut self, container: &Container) -> Result<()> {
        let discovery = Discovery::new(container);
        // Path 1: `attach_meta::<…, CronJobMeta>` — direct registration the
        // crate's own tests use; also a hand-written escape hatch for an app
        // that wants to register a job without going through the macros.
        let mut jobs: Vec<Job> = discovery
            .meta::<CronJobMeta>()
            .iter()
            .map(|d| Scheduler::resolve(&d.meta))
            .collect::<Result<Vec<_>>>()?;
        // Path 2: link-time inventory from `#[scheduled]` — module-gated by
        // `ReachableProviders` so a job whose provider lives in an unimported
        // module compiles in but does not fire.
        let reachable = container.get::<ReachableProviders>();
        for entry in inventory::iter::<ScheduledMethod>() {
            if !ReachableProviders::reaches(reachable.as_deref(), (entry.provider_type_id)()) {
                ::nest_rs_core::report_inert_host!(
                    target: crate::TARGET,
                    what: "scheduled method",
                    origin: entry.origin,
                    host: (entry.provider_type_id)(),
                    container: container,
                    provider = entry.provider,
                    method = entry.method,
                );
                continue;
            }
            let synthesized = Arc::new(CronJobMeta {
                origin: entry.origin,
                provider: entry.provider,
                method: entry.method,
                trigger: entry.trigger,
                run: entry.run,
                transaction: entry.transaction,
                replicas: entry.replicas,
                key: entry.key,
            });
            jobs.push(Scheduler::resolve(&synthesized)?);
        }

        // A job's `Provider::method` is what its lines carry and are filtered
        // on. Two jobs under one name — two `Tasks` structs in two modules, each
        // with a `sweep` — would file lines nobody can tell apart. Named at boot,
        // both origins.
        refuse_shared_names(&jobs.iter().map(Job::id).collect::<Vec<_>>())?;
        // Two jobs firing once under one identity would split its occurrences,
        // each firing the ones it won. Distinct names have distinct derived
        // identities, so only a `key` can bring two together.
        refuse_shared_identities(
            &jobs
                .iter()
                .filter(|job| job.task().replicas == Replicas::One)
                .map(Job::id)
                .collect::<Vec<_>>(),
        )?;

        // The earliest site that sees both facts: which jobs declared one replica
        // is the code's, and whether a lock is bound is the app's imports.
        let lock = container.get_dyn::<dyn OccurrenceLock>();
        if lock.is_none() {
            let unclaimable: Vec<String> = jobs
                .iter()
                .filter(|job| job.task().replicas == Replicas::One)
                .map(|job| format!("`{}`", job.id()))
                .collect();
            if !unclaimable.is_empty() {
                anyhow::bail!(
                    "scheduled job(s) {} declare `replicas = \"one\"`, and no occurrence lock is \
                     bound to claim their occurrences. {BACKEND_REMEDY}",
                    unclaimable.join(", "),
                );
            }
        }

        self.jobs = jobs;
        self.lock = lock;
        for job in &self.jobs {
            let replicas = job.task().replicas;
            // What a job firing once claims under, spelled as `key = "…"` takes it,
            // so a developer about to rename the job reads the pin off this line.
            let key = (replicas == Replicas::One).then(|| job.id().key());
            let key = key.as_deref();
            let replicas = replicas.as_str();
            match job {
                Job::Interval { id, period, .. } => tracing::info!(
                    target: crate::TARGET,
                    provider = id.provider,
                    method = id.method,
                    interval_ms = period.as_millis() as u64,
                    replicas,
                    key,
                    "scheduled job (interval)",
                ),
                Job::Timeout { id, delay, .. } => tracing::info!(
                    target: crate::TARGET,
                    provider = id.provider,
                    method = id.method,
                    delay_ms = delay.as_millis() as u64,
                    replicas,
                    "scheduled job (one-shot)",
                ),
                Job::Cron { id, tz, .. } => tracing::info!(
                    target: crate::TARGET,
                    provider = id.provider,
                    method = id.method,
                    timezone = tz.map(|t| t.name()).unwrap_or("UTC"),
                    replicas,
                    key,
                    "scheduled job (cron)",
                ),
            }
        }
        self.container = Some(container.clone());
        Ok(())
    }

    async fn serve(self: Box<Self>, cancel: CancellationToken) -> Result<()> {
        #[expect(
            clippy::expect_used,
            reason = "the transport lifecycle runs configure before serve"
        )]
        let container = self
            .container
            .expect("Scheduler::configure must run before serve");
        // No jobs: idle until shutdown so this transport doesn't race the app
        // down when it is the only one attached.
        if self.jobs.is_empty() {
            cancel.cancelled().await;
            return Ok(());
        }

        // Resolve once: a database module's `WorkerDbContext` binds this so
        // each tick runs with an executor and the job queries through Repo.
        let runner = Runner {
            ctx: container.get_dyn::<dyn JobContext>(),
            container,
            lock: self.lock,
        };

        let mut tasks = JoinSet::new();
        // Which job each task runs, so a task's end is named by its job.
        let mut spawned: HashMap<tokio::task::Id, JobId> = HashMap::new();
        for job in self.jobs {
            let id = job.id();
            let handle = tasks.spawn(run_job(job, runner.clone(), cancel.clone()));
            spawned.insert(handle.id(), id);
        }
        // A loop returns only once shutdown is asked for, so a task ending before
        // it ended early: a panic the loop does not catch ends the job for good,
        // and its task's end is the one place left that can say so.
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                joined = tasks.join_next_with_id() => match joined {
                    Some(joined) => loop_ended(joined, &mut spawned, false),
                    // Every loop has ended, each in a panic named above: a
                    // schedule left with nothing to run idles until shutdown, as
                    // one with no job does, rather than ending an app it is the
                    // only transport of.
                    None => {
                        cancel.cancelled().await;
                        return Ok(());
                    }
                },
            }
        }
        stop(tasks, spawned, Scheduler::SHUTDOWN_TIMEOUT).await;
        Ok(())
    }

    /// A running tick gets [`Scheduler::SHUTDOWN_TIMEOUT`], then the settle
    /// once it is stopped.
    fn stop_bound(&self) -> Duration {
        Self::SHUTDOWN_TIMEOUT + nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT
    }
}

/// Account for a loop that ended, and name it unless it ended the ordinary way.
///
/// A loop returns only once shutdown is asked for, so before it a task's end is
/// an early one: a panic the loop does not catch ends the job for good, and its
/// task's end is the one place left that can say so. A cancelled task is its
/// runtime going down under it — the end of the process, not of the job — or,
/// once `stopping`, a tick [`stop`] dropped at its bound, whose line was filed
/// as it was dropped.
fn loop_ended(
    joined: std::result::Result<(tokio::task::Id, ()), tokio::task::JoinError>,
    spawned: &mut HashMap<tokio::task::Id, JobId>,
    stopping: bool,
) {
    let ended = match joined {
        Ok((task, ())) => {
            spawned.remove(&task);
            return;
        }
        Err(ended) => ended,
    };
    let id = spawned.remove(&ended.id());
    match ended.try_into_panic() {
        Ok(payload) => tracing::error!(
            target: crate::TARGET,
            provider = id.map(|id| id.provider),
            method = id.map(|id| id.method),
            panic = panic_message(&*payload),
            "scheduled job stopped: its schedule panicked, and the job will not run again",
        ),
        Err(_) if stopping => {}
        Err(cancelled) => tracing::debug!(
            target: crate::TARGET,
            provider = id.map(|id| id.provider),
            method = id.map(|id| id.method),
            error = %cancelled,
            "scheduled job cancelled with its runtime",
        ),
    }
}

/// Stop the scheduler once shutdown is asked for: no loop starts a tick from
/// here on, a tick already running gets `bound` —
/// [`Scheduler::SHUTDOWN_TIMEOUT`] — to finish, and one still running then is
/// stopped, dropped where it waits, and given
/// [`SHUTDOWN_SETTLE_TIMEOUT`](nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT) to unwind.
/// Bounded either way, so `serve` returns within the two.
async fn stop(
    mut tasks: JoinSet<()>,
    mut spawned: HashMap<tokio::task::Id, JobId>,
    bound: Duration,
) {
    let deadline = Instant::now() + bound;
    while let Ok(Some(joined)) = tokio::time::timeout_at(deadline, tasks.join_next_with_id()).await
    {
        loop_ended(joined, &mut spawned, true);
    }
    if tasks.is_empty() {
        return;
    }
    tracing::warn!(
        target: crate::TARGET,
        jobs = running_jobs(&spawned).as_str(),
        running = tasks.len(),
        shutdown_timeout_ms = millis(bound),
        "scheduled ticks still running at the shutdown bound are stopped; each is dropped where it \
         waits and files its line cancelled",
    );
    tasks.abort_all();
    let settled = tokio::time::timeout(nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT, async {
        while let Some(joined) = tasks.join_next_with_id().await {
            loop_ended(joined, &mut spawned, true);
        }
    })
    .await;
    if settled.is_err() {
        tracing::error!(
            target: crate::TARGET,
            jobs = running_jobs(&spawned).as_str(),
            still_running = tasks.len(),
            settle_timeout_ms = millis(nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT),
            "stopped ticks did not unwind within their bound; they run on through the shutdown \
             hooks",
        );
        // Dropping the set is what would abort them, and they are aborted
        // already: detach them instead, so the end of this function is not one
        // more wait on a tick that blocks its thread.
        tasks.detach_all();
    }
}

/// The jobs whose loops still run, as `Provider::method`, in a stable order.
fn running_jobs(spawned: &HashMap<tokio::task::Id, JobId>) -> String {
    let mut jobs: Vec<String> = spawned.values().map(JobId::to_string).collect();
    jobs.sort();
    jobs.join(", ")
}

/// Whole milliseconds, saturating — what a duration field on a line carries.
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// What every job's loop fires through: the container its method resolves
/// from, the data context its tick runs inside, and the lock a job declared
/// `replicas = "one"` claims its occurrences with.
#[derive(Clone)]
struct Runner {
    container: Container,
    ctx: Option<Arc<dyn JobContext>>,
    lock: Option<Arc<dyn OccurrenceLock>>,
}

/// Each variant computes its own waits; all return only when `token` is
/// cancelled (one-shot idles after its single run so the transport doesn't
/// race the app down).
///
/// **Once `token` is cancelled, no occurrence starts.** Every wait checks it
/// before the timer — a tick falling due in the very poll shutdown is asked for
/// is not fired — and a lock call in flight is abandoned rather than awaited
/// ([`bounded`]). A run already started is not raced against the token: it
/// finishes and the loop ends after it, unless it outlasts
/// [`Scheduler::SHUTDOWN_TIMEOUT`], when `serve` stops the loop and the run with
/// it.
async fn run_job(job: Job, runner: Runner, token: CancellationToken) {
    let id = job.id();
    match job {
        Job::Interval { period, task, .. } if task.replicas == Replicas::Each => {
            let mut ticker = interval(period);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            // Drop the immediate first tick — semantics is "every N", not
            // "now, then every N".
            let mut previous = ticker.tick().await;
            loop {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => break,
                    deadline = ticker.tick() => {
                        // The timer hands back each tick's own deadline, a late
                        // tick's included, and moves past the ticks a long run
                        // overran rather than firing them late in a burst: the gap
                        // between two deadlines counts exactly the ones never
                        // fired — and they are said, as a job firing once says it.
                        let skipped = ticks_skipped_between(previous, deadline, period);
                        if skipped > 0 {
                            let now_ms = epoch_millis(SystemTime::now());
                            let since_ms =
                                u64::try_from(previous.elapsed().as_millis()).unwrap_or(u64::MAX);
                            let overrun = Overrun {
                                first: Vec::new(),
                                count: skipped,
                                capped: false,
                            };
                            report_skipped(id, now_ms.saturating_sub(since_ms), now_ms, &overrun);
                        }
                        previous = deadline;
                        runner.fire(id, task, Correlation::minted(None), None).await;
                    }
                }
            }
        }
        Job::Interval { period, task, .. } => {
            let period_ms = u64::try_from(period.as_millis()).unwrap_or(u64::MAX);
            let hold = claim_hold(period);
            // The last instant this loop reached: at boot, the multiple of the
            // period at or before the clock, which is not due, so the first one
            // reached is the next.
            let mut last_ms = aligned_at_or_before(epoch_millis(SystemTime::now()), period_ms);
            loop {
                let wait = Duration::from_millis(
                    last_ms
                        .saturating_add(period_ms)
                        .saturating_sub(epoch_millis(SystemTime::now())),
                );
                tokio::select! {
                    biased;
                    _ = token.cancelled() => break,
                    _ = sleep(wait) => {}
                }
                // Chosen on the clock the timer woke to, as a cron job's is below.
                let now_ms = epoch_millis(SystemTime::now());
                let instant_ms = interval_reached(last_ms, now_ms, period_ms);
                let skipped = (instant_ms.saturating_sub(last_ms) / period_ms).saturating_sub(1);
                let claim = runner.claim_then_fire(id, task, instant_ms, hold, &token);
                if skipped > 0 {
                    let overrun = interval_overrun(last_ms, period_ms, skipped);
                    tokio::join!(
                        runner.report_overrun(
                            id,
                            last_ms,
                            now_ms,
                            overrun,
                            stale_at(instant_ms, hold),
                            &token,
                        ),
                        claim
                    );
                } else {
                    claim.await;
                }
                last_ms = instant_ms;
            }
        }
        Job::Timeout { delay, task, .. } => {
            tokio::select! {
                biased;
                _ = token.cancelled() => return,
                _ = sleep(delay) => runner.fire(id, task, Correlation::minted(None), None).await,
            }
            token.cancelled().await;
        }
        Job::Cron {
            schedule, tz, task, ..
        } => {
            // The last occurrence this loop reached: at boot, the clock, since no
            // occurrence before the boot is due.
            let mut last = Utc::now();
            loop {
                let Some(target) = next_occurrence(&schedule, tz, last) else {
                    tracing::warn!(
                        target: crate::TARGET,
                        provider = id.provider,
                        method = id.method,
                        "cron job has no future occurrence; it will not run again",
                    );
                    token.cancelled().await;
                    break;
                };
                let wait = (target - Utc::now()).to_std().unwrap_or(Duration::ZERO);
                tokio::select! {
                    biased;
                    _ = token.cancelled() => break,
                    _ = sleep(wait) => {}
                }
                // Chosen on the clock the timer woke to, walking from the last one
                // reached: whatever held the loop past the next occurrence — a slow
                // claim, a long run, a stalled replica, a clock stepped forward — it
                // reaches the latest one due, once, and names the ones before it,
                // rather than the stale one it slept for and the latest right after.
                // The report runs beside the fire, so neither delays the other.
                let now = Utc::now();
                let (instant, overrun) = match cron_due(&schedule, tz, last, now) {
                    CronDue::Latest(instant, overrun) => (Some(instant), overrun),
                    // A timer waking before the wall clock reached the occurrence
                    // it slept for fires that one all the same.
                    CronDue::NoneYet => (Some(target), Overrun::default()),
                    CronDue::PastCap(overrun) => (None, overrun),
                };
                // The instant reached and how long its claim holds, read off the
                // gap to the following occurrence ([`claim_hold`]).
                let reached = instant.map(|instant| {
                    let hold = next_occurrence(&schedule, tz, instant)
                        .and_then(|following| (following - instant).to_std().ok())
                        .map_or(MIN_HOLD, claim_hold);
                    (u64::try_from(instant.timestamp_millis()).unwrap_or(0), hold)
                });
                let fire = async {
                    let Some((instant_ms, hold)) = reached else {
                        return;
                    };
                    match task.replicas {
                        Replicas::Each => {
                            runner.fire(id, task, Correlation::minted(None), None).await
                        }
                        Replicas::One => {
                            runner
                                .claim_then_fire(id, task, instant_ms, hold, &token)
                                .await
                        }
                    }
                };
                if overrun.count > 0 {
                    let from_ms = u64::try_from(last.timestamp_millis()).unwrap_or(0);
                    let now_ms = u64::try_from(now.timestamp_millis()).unwrap_or(0);
                    match task.replicas {
                        Replicas::Each => {
                            report_skipped(id, from_ms, now_ms, &overrun);
                            fire.await;
                        }
                        Replicas::One => {
                            // Bounded by the occurrence it runs beside — past the
                            // cap there is none, and an occurrence reached now at the
                            // shortest hold stands in for it.
                            let stale = reached.map_or(stale_at(now_ms, MIN_HOLD), |(ms, hold)| {
                                stale_at(ms, hold)
                            });
                            tokio::join!(
                                runner.report_overrun(id, from_ms, now_ms, overrun, stale, &token),
                                fire
                            );
                        }
                    }
                } else {
                    fire.await;
                }
                last = instant.unwrap_or(now);
            }
        }
    }
}

/// How long the claim on an occurrence holds, `gap` being the time to the job's
/// following occurrence: **twice the gap**, and never less than [`MIN_HOLD`].
///
/// A replica held past the following occurrence — by a slow claim, a stall —
/// reaches the latest one due and asks the lock who claimed the ones it overran,
/// and it can only ask once the following one is due: a gap after this one at
/// the earliest. Held for the gap alone, the claim on the occurrence it overran
/// had expired by the time it asked, and for every job whose period is a minute
/// or more — past the floor, where the hold *was* the gap — each occurrence a
/// peer fired was reported skipped at `warn`, claimed nowhere. Twice the gap
/// answers the question for the occurrence overrun most often, the one just
/// before the one reached; an overrun longer than a gap reports its earliest
/// unclaimed, as the port says.
///
/// It costs keys, never correctness: tokens are distinct per instant, so a claim
/// still held when the next occurrence is claimed stands beside it, and a job
/// holds two claims where it held one.
fn claim_hold(gap: Duration) -> Duration {
    gap.saturating_mul(2).max(MIN_HOLD)
}

/// The latest multiple of `period_ms` at or before `at_ms`. Aligned on the epoch
/// rather than on this process's boot, so a replica started a second after
/// another still reaches the same instants — and so computes the same key for the
/// same occurrence.
fn aligned_at_or_before(at_ms: u64, period_ms: u64) -> u64 {
    at_ms - at_ms % period_ms
}

/// The occurrence of an interval a loop reaches on waking, `last_ms` being the one
/// it reached before and `now_ms` the clock its timer woke to: the latest multiple
/// of `period_ms` due by then — one a slow claim, a long run or a stalled replica
/// held the loop past, reached late, and only the latest of them — or else the one
/// after `last_ms`, which a timer waking a hair early reaches all the same, and
/// never `last_ms` again.
fn interval_reached(last_ms: u64, now_ms: u64, period_ms: u64) -> u64 {
    last_ms
        .saturating_add(period_ms)
        .max(aligned_at_or_before(now_ms, period_ms))
}

/// The ticks a timer skipping missed ticks moved past between two ticks it handed
/// back: their deadlines stand one period apart per tick, fired or not.
fn ticks_skipped_between(previous: Instant, deadline: Instant, period: Duration) -> u64 {
    let periods =
        deadline.saturating_duration_since(previous).as_nanos() / period.as_nanos().max(1);
    u64::try_from(periods).unwrap_or(u64::MAX).saturating_sub(1)
}

fn epoch_millis(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH).map_or(0, |since| {
        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
    })
}

/// The first occurrence strictly after `after`, as a UTC instant; `None` if the
/// schedule has none.
fn next_occurrence(schedule: &Cron, tz: Option<Tz>, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    match tz {
        Some(tz) => schedule
            .find_next_occurrence(&after.with_timezone(&tz), false)
            .ok()
            .map(|dt| dt.with_timezone(&Utc)),
        None => schedule.find_next_occurrence(&after, false).ok(),
    }
}

/// The occurrences a slow claim or run overran: the instants of the first
/// [`CHECKED`] — what a job firing once asks the lock about — how many there
/// were, and whether counting them stopped at its cap.
#[derive(Default)]
struct Overrun {
    first: Vec<u64>,
    count: u64,
    capped: bool,
}

/// The `count` occurrences of an interval after `instant_ms`, `period_ms` apart.
fn interval_overrun(instant_ms: u64, period_ms: u64, count: u64) -> Overrun {
    Overrun {
        first: (1..=count.min(CHECKED))
            .map(|n| instant_ms.saturating_add(n.saturating_mul(period_ms)))
            .collect(),
        count,
        capped: false,
    }
}

/// What a cron loop reaches on waking.
enum CronDue {
    /// The latest occurrence due by the clock, and those before it the loop
    /// moves past.
    Latest(DateTime<Utc>, Overrun),
    /// None is due: the timer woke before the wall clock reached the occurrence
    /// it slept for.
    NoneYet,
    /// More fell due than a walk counts: none of them fires late, and every one
    /// is moved past.
    PastCap(Overrun),
}

/// Walk the occurrences of `schedule` after `last`, up to and including `now`,
/// with the forward search alone — the same search the loop's own sleep already
/// uses, so the walk and the sleep name one set of instants rather than two. The
/// latest is what the loop reaches, and the ones before it are counted up to a
/// cap: a job an hour long on a per-second cron has thousands, and the number
/// past the cap is not what an operator acts on.
///
/// **The direction settles nothing about daylight saving.** croner names the end
/// of a spring-forward gap as an occurrence in *either* direction, so no choice
/// of search filters it, and nothing here tries: a fixed-time job whose declared
/// time falls inside the gap fires once at the gap's end, which is what `cron`
/// has always done with the skipped hour and better than skipping a daily job.
/// Stated as a limit on the schedule page, where a reader looks for it.
fn cron_due(schedule: &Cron, tz: Option<Tz>, last: DateTime<Utc>, now: DateTime<Utc>) -> CronDue {
    const CAP: u64 = 10_000;
    let mut overrun = Overrun::default();
    let mut latest: Option<DateTime<Utc>> = None;
    let mut cursor = last;
    while let Some(next) = next_occurrence(schedule, tz, cursor).filter(|next| *next <= now) {
        if let Some(passed) = latest {
            if overrun.count == CAP {
                overrun.capped = true;
                return CronDue::PastCap(overrun);
            }
            if overrun.count < CHECKED {
                overrun
                    .first
                    .push(u64::try_from(passed.timestamp_millis()).unwrap_or(0));
            }
            overrun.count += 1;
        }
        latest = Some(next);
        cursor = next;
    }
    match latest {
        Some(latest) => CronDue::Latest(latest, overrun),
        None => CronDue::NoneYet,
    }
}

/// How late a replica reaching the occurrence at `instant_ms` at `now_ms` is,
/// once that is within [`MAX_SKEW`] of the `hold_ms` a peer's claim lasts, or
/// past it — from where the replica can no longer tell whether the occurrence
/// fired, its own clock possibly behind the peer's. `None` while a claim can
/// still be answered by the key.
fn reached_after_hold(now_ms: u64, instant_ms: u64, hold_ms: u64) -> Option<u64> {
    let late_ms = now_ms.saturating_sub(instant_ms);
    let skew_ms = u64::try_from(MAX_SKEW.as_millis()).unwrap_or(u64::MAX);
    (late_ms >= hold_ms.saturating_sub(skew_ms)).then_some(late_ms)
}

/// When the occurrence at `instant_ms`, claimed for `hold`, goes **stale**: the
/// instant from which [`reached_after_hold`] skips it, since a peer's claim on it
/// may be forgotten by then.
///
/// It is also where waiting on the occurrence's lock stops being worth anything,
/// and that is what makes it the bound on every lock call: an answer arriving
/// later could not be acted on, so the call is abandoned there and the occurrence
/// skipped. Without the bound a lock that never answers — a backend holding a
/// command, a network dropping it without a reset — held its job's loop for as
/// long, and the job stopped firing with nothing said, while the process stayed
/// healthy.
fn stale_at(instant_ms: u64, hold: Duration) -> u64 {
    let hold_ms = u64::try_from(hold.as_millis()).unwrap_or(u64::MAX);
    let skew_ms = u64::try_from(MAX_SKEW.as_millis()).unwrap_or(u64::MAX);
    instant_ms.saturating_add(hold_ms.saturating_sub(skew_ms))
}

/// How long a lock call may still be awaited: from the wall clock now to
/// `stale_at_ms`, and nothing once it has passed.
fn left_until(stale_at_ms: u64) -> Duration {
    Duration::from_millis(stale_at_ms.saturating_sub(epoch_millis(SystemTime::now())))
}

/// What a lock call came to when it was awaited no longer than its budget, and
/// no longer than its loop ran.
enum Bounded<T> {
    /// It answered within its budget — or panicked, and this is the payload.
    Answered(std::thread::Result<T>),
    /// Its occurrence went stale first: an answer could no longer be acted on.
    Stale,
    /// Shutdown was asked for first: the loop waiting on it is being torn down.
    Cancelled,
}

/// Await `call` — a claim, or a question about a claim — for `budget` and no
/// longer, and not past `cancel`, containing a panic the way every call into a
/// lock is contained.
///
/// **The loop's cancellation is the second bound, and it is checked first.** The
/// budget runs to the occurrence's stale threshold, which for a daily cron is
/// most of a day: a lock that stopped answering held its loop that long, and the
/// scheduler's shutdown with it, since `serve` returns once every loop has. So
/// shutdown abandons the call at once, and a call answering in the very poll
/// shutdown is asked for is abandoned too rather than acted on — the order is
/// what keeps an occurrence from starting after shutdown began.
///
/// A caller hands in the lock's call wrapped in an `async` block, so the call is
/// made inside the catch: a lock that panics before it hands back its future is
/// contained the same way as one that panics while it runs. A call abandoned
/// before it was first polled is never made at all.
async fn bounded<T>(
    budget: Duration,
    cancel: &CancellationToken,
    call: impl std::future::Future<Output = T>,
) -> Bounded<T> {
    tokio::select! {
        biased;
        () = cancel.cancelled() => Bounded::Cancelled,
        answered = tokio::time::timeout(budget, AssertUnwindSafe(call).catch_unwind()) => {
            answered.map_or(Bounded::Stale, Bounded::Answered)
        }
    }
}

/// Say that the occurrences a job firing on every replica overran were skipped:
/// they were this replica's own, so no lock is asked about them.
fn report_skipped(id: JobId, from_ms: u64, now_ms: u64, overrun: &Overrun) {
    tracing::warn!(
        target: crate::TARGET,
        provider = id.provider,
        method = id.method,
        occurrence = from_ms,
        skipped = overrun.count,
        capped = overrun.capped.then_some(true),
        overrun_ms = now_ms.saturating_sub(from_ms),
        "occurrences skipped: they fell due while the previous one was claimed or run",
    );
}

/// Refuse two jobs sharing one `Provider::method`, naming where each was
/// declared.
///
/// The name is what a job's lines carry, so two jobs under one name in one app
/// cannot be told apart in its logs. It is **not** what an occurrence lock keys
/// on — that is [`JobId::identity`], which [`refuse_shared_identities`] holds
/// apart — so this check is about the reader.
fn refuse_shared_names(ids: &[JobId]) -> Result<()> {
    let mut by_name: std::collections::BTreeMap<String, Vec<&'static str>> =
        std::collections::BTreeMap::new();
    for id in ids {
        by_name.entry(id.to_string()).or_default().push(id.origin);
    }
    let shared: Vec<String> = by_name
        .into_iter()
        .filter(|(_, declared)| declared.len() > 1)
        .map(|(name, mut declared)| {
            // Several jobs at one site share its path: named once, and counted.
            declared.sort_unstable();
            let sites: Vec<String> = declared
                .chunk_by(|a, b| a == b)
                .map(|jobs| {
                    let times = match jobs.len() {
                        1 => "once".to_owned(),
                        2 => "twice".to_owned(),
                        n => format!("{n} times"),
                    };
                    format!("{times} in {}", jobs[0])
                })
                .collect();
            format!("`{name}` (declared {})", sites.join(" and "))
        })
        .collect();
    if shared.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "scheduled jobs share one name: {}. `provider` and `method` are what a job's lines carry \
         and are filtered by, so two jobs under one name cannot be told apart in them: rename \
         one provider, or fold the two methods into one",
        shared.join(", "),
    )
}

/// Refuse two jobs firing once under one identity, naming the identity and each
/// job by its name.
///
/// Every replica claims an occurrence under the identity alone, so two jobs
/// sharing one would take turns at its occurrences — each firing the ones it won,
/// both on a fraction of the schedule. Jobs with distinct names derive distinct
/// identities, so only a `key` can bring two together: one pinning the identity
/// another derives, or two pinning one path. The boot that sees both is the only
/// one that can say so; two apps' jobs pinning one key are one job by design.
fn refuse_shared_identities(ids: &[JobId]) -> Result<()> {
    let mut by_identity: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for id in ids {
        by_identity
            .entry(id.key())
            .or_default()
            .push(format!("`{id}` (declared in {})", id.origin));
    }
    let shared: Vec<String> = by_identity
        .into_iter()
        .filter(|(_, jobs)| jobs.len() > 1)
        .map(|(key, jobs)| format!("`{key}` by {}", jobs.join(" and ")))
        .collect();
    if shared.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "scheduled jobs firing once claim under one identity: {}. Each occurrence would fire \
         one of them only: give each its own `key`, or remove the one pinning the other's identity",
        shared.join(", "),
    )
}

/// A tick's `schedule.tick` line, filed exactly once: with the outcome its run
/// settled on, or — dropped before it settled — with
/// [`CANCELLED`](nest_rs_core::operation_log::CANCELLED).
///
/// One line per tick, whatever happened: the clock is not a caller, so this is
/// the only place a tick says it ran at all. A tick is dropped before it settles
/// when the scheduler stops one still running at its shutdown bound
/// ([`Scheduler::SHUTDOWN_TIMEOUT`]); the run's own panics are caught before
/// here, so one unwinding through the guard is the scheduler's.
struct TickLine {
    id: JobId,
    replicas: Replicas,
    occurrence: Option<u64>,
    started: std::time::Instant,
    span: tracing::Span,
    filed: bool,
}

impl TickLine {
    fn open(id: JobId, replicas: Replicas, occurrence: Option<u64>, span: tracing::Span) -> Self {
        Self {
            id,
            replicas,
            occurrence,
            started: std::time::Instant::now(),
            span,
            filed: false,
        }
    }

    /// File the line for a tick whose run settled on `outcome`.
    fn file(mut self, outcome: &'static str) {
        self.emit(outcome);
        self.filed = true;
    }

    fn emit(&self, outcome: &'static str) {
        nest_rs_core::operation_log::record_outcome(&self.span, outcome);
        tracing::info!(
            name: crate::unit::TICK,
            target: nest_rs_core::operation_log::TARGET,
            message = crate::unit::TICK,
            provider = self.id.provider,
            method = self.id.method,
            replicas = self.replicas.as_str(),
            occurrence = self.occurrence,
            outcome,
            duration_ms = nest_rs_core::operation_log::duration_ms(self.started),
        );
    }
}

impl Drop for TickLine {
    fn drop(&mut self) {
        if self.filed {
            return;
        }
        let outcome = if std::thread::panicking() {
            nest_rs_core::operation_log::PANIC
        } else {
            nest_rs_core::operation_log::CANCELLED
        };
        self.emit(outcome);
    }
}

impl Runner {
    /// Fire the occurrence at `instant_ms` only if this replica claims it.
    ///
    /// **Once per occurrence, and nothing about the run.** The claim holds the
    /// occurrence, never the job: a run outlasting its period on this replica
    /// leaves the next occurrence to whichever replica claims it, so two runs of
    /// the job may overlap across replicas, as the port states. Holding the job
    /// as well needs a lease renewed while the run lasts, and every way a lease
    /// fails — a renewal left unanswered, a reply lost after the claim took it, a
    /// deploy landing in a claim's window — holds the job on every replica, the
    /// claimer included, until it lapses, each occurrence meanwhile put down to a
    /// run that does not exist. Without one, a lost answer costs the one
    /// occurrence it was about.
    ///
    /// Fail closed: a claim that errors — or no lock at all, which `configure`
    /// already refused — skips the occurrence, because firing unclaimed would
    /// fire it on every replica. So does a claim reached within [`MAX_SKEW`] of
    /// its hold's end, or later: a replica stalled that long — paused by its
    /// platform, starved of CPU — cannot tell whether a peer fired the
    /// occurrence, since the peer's key may already be gone; firing could be the
    /// second time. And so does a claim answered that late, however early it was
    /// sent: the key it set may be one a peer's forgotten claim left room for.
    ///
    /// The claim itself is awaited until the occurrence goes stale and no longer
    /// ([`stale_at`]): a lock that has not answered by then is abandoned, and the
    /// occurrence skipped with a `warn`. At most once still holds — an abandoned
    /// claim may have taken the key, and then nobody fires the occurrence — which
    /// is the side the port's rule chooses.
    ///
    /// Nor past `cancel`, the loop's: once shutdown is asked for, a claim still
    /// unanswered is abandoned at once and said once — the same at-most-once
    /// side, taken because the replica is leaving. A claim answered before that
    /// is an occurrence already started, and fires, as a tick that won its wait
    /// does on a job firing on every replica.
    async fn claim_then_fire(
        &self,
        id: JobId,
        task: Task,
        instant_ms: u64,
        hold: Duration,
        cancel: &CancellationToken,
    ) {
        let hold_ms = u64::try_from(hold.as_millis()).unwrap_or(u64::MAX);
        if let Some(late_ms) =
            reached_after_hold(epoch_millis(SystemTime::now()), instant_ms, hold_ms)
        {
            tracing::warn!(
                target: crate::TARGET,
                provider = id.provider,
                method = id.method,
                occurrence = instant_ms,
                late_ms,
                hold_ms,
                max_skew_ms = MAX_SKEW.as_millis() as u64,
                "occurrence skipped: reached too near its hold's end, when a peer's claim may \
                 already be forgotten, so firing it could fire it twice",
            );
            return;
        }
        // A tick is a unit of work with no upstream — nothing enqueued it and no
        // caller is waiting — so it mints its own trace, here rather than when it
        // fires: the claim records it, so an operator reading an occurrence's key
        // finds the trace of the run that fired it.
        let correlation = Correlation::minted(None);
        let occurrence = Occurrence {
            token: id.occurrence(instant_ms),
            hold,
            run: correlation.trace_id().to_string(),
        };
        let budget = left_until(stale_at(instant_ms, hold));
        let sent = Instant::now();
        let claimed = match &self.lock {
            Some(lock) => {
                match bounded(budget, cancel, async { lock.claim(&occurrence).await }).await {
                    Bounded::Answered(Ok(claimed)) => claimed,
                    // Not answered while an answer could still be acted on: the call
                    // is dropped, the occurrence skipped, and the loop goes on to the
                    // next one rather than waiting on a lock that may never answer.
                    Bounded::Stale => {
                        tracing::warn!(
                            target: crate::TARGET,
                            provider = id.provider,
                            method = id.method,
                            occurrence = instant_ms,
                            waited_ms = u64::try_from(budget.as_millis()).unwrap_or(u64::MAX),
                            hold_ms,
                            "occurrence skipped: its lock did not answer the claim before the \
                             occurrence went stale",
                        );
                        return;
                    }
                    // Shutdown does not wait on a lock: the call is dropped where it
                    // stands, and the occurrence it was claiming is named once,
                    // since nobody may fire it now — this replica is leaving, and
                    // the key may have been taken.
                    Bounded::Cancelled => {
                        tracing::warn!(
                            target: crate::TARGET,
                            provider = id.provider,
                            method = id.method,
                            occurrence = instant_ms,
                            waited_ms =
                                u64::try_from(sent.elapsed().as_millis()).unwrap_or(u64::MAX),
                            "occurrence skipped: shutdown was asked for before its lock \
                             answered the claim",
                        );
                        return;
                    }
                    // A lock that panics answers nothing: the occurrence is skipped,
                    // the panic named, and the schedule goes on.
                    Bounded::Answered(Err(payload)) => {
                        tracing::error!(
                            target: crate::TARGET,
                            provider = id.provider,
                            method = id.method,
                            occurrence = instant_ms,
                            panic = panic_message(&*payload),
                            "occurrence skipped: its lock panicked while claiming it",
                        );
                        return;
                    }
                }
            }
            None => Err(crate::OccurrenceLockError::new(
                "no occurrence lock is bound",
            )),
        };
        match claimed {
            Ok(OccurrenceClaim::Claimed) => {
                // Judged again on the clock after the answer, which can come a
                // connect budget after the check above. The claim is then left to
                // expire, unfired.
                if let Some(late_ms) =
                    reached_after_hold(epoch_millis(SystemTime::now()), instant_ms, hold_ms)
                {
                    tracing::warn!(
                        target: crate::TARGET,
                        provider = id.provider,
                        method = id.method,
                        occurrence = instant_ms,
                        late_ms,
                        hold_ms,
                        max_skew_ms = MAX_SKEW.as_millis() as u64,
                        "occurrence skipped: its claim was answered too near its hold's end, when a \
                         peer's claim may already be forgotten, so firing it could fire it twice",
                    );
                    return;
                }
                self.fire(id, task, correlation, Some(instant_ms)).await
            }
            Ok(OccurrenceClaim::ClaimedElsewhere) => tracing::debug!(
                target: crate::TARGET,
                provider = id.provider,
                method = id.method,
                occurrence = instant_ms,
                "occurrence claimed by another replica",
            ),
            Err(error) => tracing::warn!(
                target: crate::TARGET,
                provider = id.provider,
                method = id.method,
                occurrence = instant_ms,
                error = %nest_rs_core::error_message(&error),
                "occurrence skipped: its lock could not be claimed",
            ),
        }
    }

    /// Say what a slow claim or run overran on a job firing once, from the
    /// occurrence at `from_ms`.
    ///
    /// A peer may have claimed and fired the overrun occurrences while this
    /// replica was busy, so the lock is asked about the first [`CHECKED`]: only
    /// those nobody claimed — with those it could not answer, and those past the
    /// ones asked — are reported skipped, at `warn`; an overrun whose every
    /// occurrence a peer claimed is a `debug`. Each question is awaited until
    /// `stale_at_ms`, the stale threshold of the occurrence the report runs beside,
    /// and a question not answered by then counts as unanswered: the loop reaching
    /// that occurrence is not held past the point its own claim is abandoned at —
    /// nor past `cancel`, the loop's, at which every question still out is
    /// abandoned at once, counted unanswered and named once.
    async fn report_overrun(
        &self,
        id: JobId,
        from_ms: u64,
        now_ms: u64,
        overrun: Overrun,
        stale_at_ms: u64,
        cancel: &CancellationToken,
    ) {
        let overrun_ms = now_ms.saturating_sub(from_ms);
        let capped = overrun.capped.then_some(true);
        // No lock: `configure` refused the boot already, so nothing is asked and
        // every overrun occurrence is this replica's to report.
        let Some(lock) = &self.lock else {
            report_skipped(id, from_ms, now_ms, &overrun);
            return;
        };
        let budget = left_until(stale_at_ms);
        let sent = Instant::now();
        let ask = |instant: &u64| {
            let occurrence = id.occurrence(*instant);
            async move { bounded(budget, cancel, async { lock.claimed(&occurrence).await }).await }
        };
        let answers = futures_util::future::join_all(overrun.first.iter().map(ask)).await;
        // A question the lock left unanswered past the threshold counts as
        // unanswered, and the abandoned ones are named once for the whole report.
        let abandoned = answers
            .iter()
            .filter(|answer| matches!(answer, Bounded::Stale))
            .count() as u64;
        if abandoned > 0 {
            tracing::warn!(
                target: crate::TARGET,
                provider = id.provider,
                method = id.method,
                occurrence = from_ms,
                abandoned,
                waited_ms = u64::try_from(budget.as_millis()).unwrap_or(u64::MAX),
                "occurrence lock did not answer whether an overrun occurrence was claimed before \
                 the occurrence reached went stale",
            );
        }
        // The same for the questions shutdown cut short, under their own cause: a
        // lock that went quiet and a replica that is leaving are two things to act on.
        let abandoned = answers
            .iter()
            .filter(|answer| matches!(answer, Bounded::Cancelled))
            .count() as u64;
        if abandoned > 0 {
            tracing::warn!(
                target: crate::TARGET,
                provider = id.provider,
                method = id.method,
                occurrence = from_ms,
                abandoned,
                waited_ms = u64::try_from(sent.elapsed().as_millis()).unwrap_or(u64::MAX),
                "occurrence lock had not answered whether an overrun occurrence was claimed when \
                 shutdown was asked for",
            );
        }
        // A lock that panicked answering counts as unanswered, and the first panic
        // is named once for the whole report rather than once per occurrence asked.
        if let Some(payload) = answers.iter().find_map(|answer| match answer {
            Bounded::Answered(Err(payload)) => Some(payload),
            _ => None,
        }) {
            tracing::error!(
                target: crate::TARGET,
                provider = id.provider,
                method = id.method,
                occurrence = from_ms,
                panic = panic_message(&**payload),
                "occurrence lock panicked answering whether an overrun occurrence was claimed",
            );
        }
        // A lock that *answered* with an error counts as unanswered too, and
        // `unanswered` alone says how many and never why — the half an operator
        // acts on.
        if let Some(error) = answers.iter().find_map(|answer| match answer {
            Bounded::Answered(Ok(Err(error))) => Some(error),
            _ => None,
        }) {
            tracing::warn!(
                target: crate::TARGET,
                provider = id.provider,
                method = id.method,
                occurrence = from_ms,
                error = %nest_rs_core::error_message(error),
                "occurrence lock could not answer whether an overrun occurrence was claimed",
            );
        }
        let checked = answers.len() as u64;
        let claimed_elsewhere = answers
            .iter()
            .filter(|answer| matches!(answer, Bounded::Answered(Ok(Ok(true)))))
            .count() as u64;
        let unanswered = answers
            .iter()
            .filter(|answer| !matches!(answer, Bounded::Answered(Ok(Ok(_)))))
            .count() as u64;
        let skipped = checked - claimed_elsewhere - unanswered;
        let unchecked = overrun.count.saturating_sub(checked);
        if skipped == 0 && unanswered == 0 && unchecked == 0 {
            tracing::debug!(
                target: crate::TARGET,
                provider = id.provider,
                method = id.method,
                occurrence = from_ms,
                overrun = overrun.count,
                overrun_ms,
                "occurrences overrun on this replica, each claimed by another",
            );
            return;
        }
        tracing::warn!(
            target: crate::TARGET,
            provider = id.provider,
            method = id.method,
            occurrence = from_ms,
            skipped,
            claimed_elsewhere,
            unanswered,
            unchecked,
            capped,
            overrun_ms,
            "occurrences skipped: they fell due while the previous one was claimed or run",
        );
    }

    /// Fire one tick under `correlation`, the trace it mints. `occurrence` is the
    /// instant a job firing once across replicas claimed — carried on the span and
    /// the line, so the fire and the claim it followed name one thing — and a job
    /// firing on every replica claims nothing and carries none.
    async fn fire(&self, id: JobId, task: Task, correlation: Correlation, occurrence: Option<u64>) {
        let span = nest_rs_core::operation_span!(
            target: crate::TARGET,
            // No caller and no wire: the clock is not a producer.
            kind: nest_rs_core::operation_log::kind::INTERNAL,
            crate::unit::TICK,
            &correlation,
            provider = id.provider,
            method = id.method,
            occurrence,
        );
        let scope = Arc::new(nest_rs_core::RequestScope::new(self.container.clone()));
        // Opened outside the run and filed inside it: a tick the scheduler stops at
        // its shutdown bound is dropped with this future, and the line is what
        // says so — inside the span and the scope, since both wrappers drop the
        // future they wrap inside what they install.
        let line = TickLine::open(id, task.replicas, occurrence, span.clone());
        nest_rs_core::with_request_scope(Some(scope), correlation, self.fire_inner(id, task, line))
            .instrument(span)
            .await
    }

    async fn fire_inner(&self, id: JobId, task: Task, line: TickLine) {
        // Isolate the fire: a panic in the user method (or the `.expect` the
        // `#[scheduled]` macro emits when the provider is missing) would otherwise
        // unwind this job's task and stop its schedule while the process still
        // reports healthy. Catch it, log at `error`, and let the loop schedule the
        // next occurrence — the run starts inside the catch as well, so a run
        // function panicking before it hands back its future is caught the same way.
        let outcome = AssertUnwindSafe(run_in_job_context(
            self.ctx.as_ref(),
            task.transaction,
            async { (task.run)(&self.container).await },
            Result::is_ok,
            // A job that ran fine but whose transaction could not be settled has
            // written nothing. Reporting success would hide that; the schedule logs
            // it as a failed job and fires again at the next occurrence.
            //
            // The classification the context carried is *reported*, not acted on: a
            // schedule has no retry budget to spend and no dead-letter to reach, so
            // its next occurrence is the same whichever way the answer went. That is
            // the one site of the four job decorators where "retryable" changes
            // nothing, and it owes the sentence rather than a mechanism.
            |why| Err(anyhow::Error::new(why)),
        ))
        .catch_unwind()
        .await;
        line.file(match &outcome {
            Ok(Ok(())) => nest_rs_core::operation_log::OK,
            Ok(Err(_)) => nest_rs_core::operation_log::ERROR,
            Err(_) => nest_rs_core::operation_log::PANIC,
        });
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(err)) => tracing::error!(
                target: crate::TARGET,
                provider = id.provider,
                method = id.method,
                // The failure's own sentence and every cause beneath it: a tick's
                // error is usually wrapped — a context line over the error that says
                // what actually went wrong — and the wrapper alone names none of it.
                error = %nest_rs_core::error_message(&*err),
                // The classification, as a field rather than a decision. A schedule
                // has no budget to spend on it, but the operator reading this line
                // wants to know whether the next occurrence is likely to work — and
                // saying "reported, not acted on" while reporting nothing was the
                // gap an audit found in the sentence above.
                retryable = err.downcast_ref::<Unhonoured>().map(|why| why.retryable),
                "scheduled job failed",
            ),
            Err(panic) => tracing::error!(
                target: crate::TARGET,
                provider = id.provider,
                method = id.method,
                // `panic.as_ref()`, never `&panic`: a `Box<dyn Any + Send>` is itself
                // `Any`, so the borrow unsizes to a trait object *of the box* and every
                // downcast inside `panic_message` misses — the operator reads
                // `<non-string panic payload>` whatever the job said.
                panic = panic_message(panic.as_ref()),
                "scheduled job panicked; the schedule continues",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tick that blocks its thread cannot be dropped where it waits, since it
    /// never yields to be dropped: once stopped it is given the settle bound to
    /// unwind and no more, and named at `error`, because it runs on through the
    /// shutdown hooks. `stop` still returns — a thread that blocks never holds the
    /// way down past its two bounds.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the test's receiver may already be gone"
    )]
    async fn a_stopped_tick_that_blocks_its_thread_is_named_at_error_and_left() {
        let logs = nest_rs_testing::LogCapture::install();
        let mut tasks = JoinSet::new();
        let mut spawned = HashMap::new();
        let (started, blocked) = std::sync::mpsc::channel();
        let handle = tasks.spawn(async move {
            let _ = started.send(());
            std::thread::sleep(Duration::from_secs(1));
        });
        spawned.insert(
            handle.id(),
            JobId {
                origin: module_path!(),
                provider: "BlockingHost",
                method: "blocks",
                key: None,
            },
        );
        blocked.recv().expect("the tick started");

        let asked = Instant::now();
        stop(tasks, spawned, Duration::from_millis(50)).await;
        let took = asked.elapsed();

        assert!(
            took < Duration::from_millis(50)
                + nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT
                + Duration::from_millis(500),
            "stop returned within its two bounds: {took:?}",
        );
        let stopped = logs.expect_one(
            crate::TARGET,
            "scheduled ticks still running at the shutdown bound are stopped; each is dropped \
             where it waits and files its line cancelled",
        );
        assert_eq!(
            stopped.field("jobs").as_deref(),
            Some("BlockingHost::blocks")
        );
        let stuck = logs.expect_one(
            crate::TARGET,
            "stopped ticks did not unwind within their bound; they run on through the shutdown \
             hooks",
        );
        assert_eq!(stuck.level, "error");
        assert_eq!(stuck.field("jobs").as_deref(), Some("BlockingHost::blocks"));
        assert_eq!(stuck.field("still_running").as_deref(), Some("1"));
    }

    /// Two replicas booted at different moments must reach the same instants,
    /// or they compute different keys for one occurrence and both fire it.
    #[test]
    fn an_aligned_instant_is_the_latest_multiple_of_the_period_whenever_it_is_asked() {
        assert_eq!(aligned_at_or_before(0, 1_000), 0);
        assert_eq!(aligned_at_or_before(999, 1_000), 0);
        assert_eq!(aligned_at_or_before(1_000, 1_000), 1_000);
        let booted_early = aligned_at_or_before(1_789_000_000_123, 5_000);
        let booted_later = aligned_at_or_before(1_789_000_003_999, 5_000);
        assert_eq!(booted_early, 1_789_000_000_000);
        assert_eq!(booted_early, booted_later);
    }

    #[test]
    fn a_reached_interval_occurrence_saturates_rather_than_wrapping() {
        assert_eq!(interval_reached(u64::MAX, u64::MAX, 1), u64::MAX);
    }

    /// A replica stalled near the end of the hold — paused by its platform,
    /// starved for most of a minute — may wake to a key the peer's claim no
    /// longer holds, its clock possibly behind the peer's, and would fire the
    /// occurrence a second time; it skips it instead. Short of that, the key
    /// still answers, however late the timer fired.
    #[test]
    fn an_occurrence_reached_near_its_holds_end_is_stale_and_one_before_is_not() {
        let hold_ms = 60_000;
        assert_eq!(reached_after_hold(1_000, 1_000, hold_ms), None, "on time");
        assert_eq!(
            reached_after_hold(50_999, 1_000, hold_ms),
            None,
            "late, but further from the hold's end than two clocks may disagree",
        );
        assert_eq!(
            reached_after_hold(51_000, 1_000, hold_ms),
            Some(50_000),
            "within the clock skew of the hold's end: the peer's key may be gone",
        );
        assert_eq!(
            reached_after_hold(66_000, 1_000, hold_ms),
            Some(65_000),
            "and every later wake, by how late",
        );
        assert_eq!(
            reached_after_hold(500, 1_000, hold_ms),
            None,
            "a timer waking early is not late",
        );
    }

    /// A lock that panics answering whether an overrun occurrence was claimed
    /// answers nothing: every occurrence asked counts as unanswered, and the panic
    /// is named once for the report, under the field every contained panic is
    /// filed under.
    #[tokio::test]
    async fn a_lock_panicking_as_it_answers_an_overrun_is_named_once_and_counted_unanswered() {
        struct PanicsAnswering;
        #[async_trait]
        impl OccurrenceLock for PanicsAnswering {
            async fn claim(
                &self,
                _occurrence: &Occurrence,
            ) -> Result<OccurrenceClaim, crate::OccurrenceLockError> {
                Ok(OccurrenceClaim::Claimed)
            }

            async fn claimed(&self, _occurrence: &str) -> Result<bool, crate::OccurrenceLockError> {
                panic!("the lock store's client panicked")
            }
        }

        let logs = nest_rs_testing::LogCapture::install();
        let runner = Runner {
            container: Container::builder().build(),
            ctx: None,
            lock: Some(Arc::new(PanicsAnswering)),
        };
        let id = JobId {
            origin: "features::tasks",
            provider: "OverrunTasks",
            method: "sweep",
            key: None,
        };
        let stale = stale_at(epoch_millis(SystemTime::now()), MIN_HOLD);
        runner
            .report_overrun(
                id,
                0,
                1_000,
                interval_overrun(0, 100, 3),
                stale,
                &CancellationToken::new(),
            )
            .await;

        let panicked = logs.find(
            crate::TARGET,
            "occurrence lock panicked answering whether an overrun occurrence was claimed",
        );
        assert_eq!(panicked.len(), 1, "named once: {:#?}", logs.events());
        assert_eq!(panicked[0].level, "error");
        assert_eq!(
            panicked[0].field("panic").as_deref(),
            Some("the lock store's client panicked")
        );
        let skipped = logs.expect_one(
            crate::TARGET,
            "occurrences skipped: they fell due while the previous one was claimed or run",
        );
        assert_eq!(skipped.field("unanswered").as_deref(), Some("3"));
        assert_eq!(skipped.field("skipped").as_deref(), Some("0"));
    }

    /// The claim the stale check guards: a granting lock, an occurrence a
    /// minute and a second in the past, and the job must not run — a peer's key
    /// for it is already gone, so the only thing firing could do is fire it
    /// twice — while the skip is said aloud, naming how late.
    #[tokio::test]
    async fn a_claim_reached_after_its_hold_skips_the_occurrence_and_says_so() {
        struct Granting;
        #[async_trait]
        impl OccurrenceLock for Granting {
            async fn claim(
                &self,
                _occurrence: &Occurrence,
            ) -> Result<OccurrenceClaim, crate::OccurrenceLockError> {
                Ok(OccurrenceClaim::Claimed)
            }

            async fn claimed(&self, _occurrence: &str) -> Result<bool, crate::OccurrenceLockError> {
                Ok(false)
            }
        }
        static RAN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        fn run(
            _: &Container,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>> {
            Box::pin(async {
                RAN.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })
        }

        let logs = nest_rs_testing::LogCapture::install();
        let runner = Runner {
            container: Container::builder().build(),
            ctx: None,
            lock: Some(Arc::new(Granting)),
        };
        let id = JobId {
            origin: "features::tasks",
            provider: "StalledTasks",
            method: "sweep",
            key: None,
        };
        let task = Task {
            run,
            transaction: JobTransaction::Pool,
            replicas: Replicas::One,
        };
        let instant_ms = epoch_millis(SystemTime::now()) - 61_000;

        runner
            .claim_then_fire(id, task, instant_ms, MIN_HOLD, &CancellationToken::new())
            .await;

        assert!(
            !RAN.load(std::sync::atomic::Ordering::SeqCst),
            "the occurrence is not fired"
        );
        let skipped = logs.expect_one(
            crate::TARGET,
            "occurrence skipped: reached too near its hold's end, when a peer's claim may \
             already be forgotten, so firing it could fire it twice",
        );
        assert_eq!(skipped.level, "warn");
        assert_eq!(skipped.field("provider").as_deref(), Some("StalledTasks"));
        assert_eq!(
            skipped.field("occurrence").as_deref(),
            Some(instant_ms.to_string().as_str())
        );
        assert!(
            skipped
                .field("late_ms")
                .is_some_and(|late| late.parse::<u64>().is_ok_and(|ms| ms >= 61_000)),
            "{skipped:?}"
        );
        assert_eq!(skipped.field("hold_ms").as_deref(), Some("60000"));
        assert_eq!(skipped.field("max_skew_ms").as_deref(), Some("10000"));
    }

    /// The following occurrence is what bounds a cron hold, and it has to be
    /// strictly later than the one being claimed.
    #[test]
    fn the_next_occurrence_is_strictly_after_the_instant_asked_about() {
        let every_second = Cron::from_str("* * * * * *").expect("parses");
        let at: DateTime<Utc> = "2026-03-11T14:23:45Z".parse().expect("parses");
        let next = next_occurrence(&every_second, None, at).expect("has one");
        assert_eq!(
            next,
            "2026-03-11T14:23:46Z".parse::<DateTime<Utc>>().unwrap()
        );
    }

    /// The check before a claim reads the clock before the claim is answered, and
    /// the claim's bound cannot cover a replica that stalls while it waits —
    /// paused by its platform, starved of CPU — so that the stall and the answer
    /// end together, past the threshold the wait was bounded at. A key set that
    /// late may be a fresh one a peer's forgotten claim left room for, so the
    /// answer is judged again once it arrives.
    ///
    /// The double blocks its thread to stand for the stall: the answer is ready
    /// the first time the bound polls it, as it is when a stalled process resumes.
    #[tokio::test]
    async fn a_claim_answered_too_near_its_holds_end_is_not_fired() {
        struct GrantingAfterAStall;
        #[async_trait]
        impl OccurrenceLock for GrantingAfterAStall {
            async fn claim(
                &self,
                _occurrence: &Occurrence,
            ) -> Result<OccurrenceClaim, crate::OccurrenceLockError> {
                std::thread::sleep(Duration::from_millis(1_200));
                Ok(OccurrenceClaim::Claimed)
            }

            async fn claimed(&self, _occurrence: &str) -> Result<bool, crate::OccurrenceLockError> {
                Ok(false)
            }
        }
        static RAN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        fn run(
            _: &Container,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>> {
            Box::pin(async {
                RAN.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })
        }

        let logs = nest_rs_testing::LogCapture::install();
        let runner = Runner {
            container: Container::builder().build(),
            ctx: None,
            lock: Some(Arc::new(GrantingAfterAStall)),
        };
        let id = JobId {
            origin: "features::tasks",
            provider: "SlowTasks",
            method: "sweep",
            key: None,
        };
        let task = Task {
            run,
            transaction: JobTransaction::Pool,
            replicas: Replicas::One,
        };
        // Late, but further from the hold's end than two clocks may disagree —
        // until the claim is answered.
        let instant_ms = epoch_millis(SystemTime::now()) - 49_000;

        runner
            .claim_then_fire(id, task, instant_ms, MIN_HOLD, &CancellationToken::new())
            .await;

        assert!(
            !RAN.load(std::sync::atomic::Ordering::SeqCst),
            "the occurrence is not fired"
        );
        logs.expect_none(
            crate::TARGET,
            "occurrence skipped: reached too near its hold's end, when a peer's claim may \
             already be forgotten, so firing it could fire it twice",
        );
        let skipped = logs.expect_one(
            crate::TARGET,
            "occurrence skipped: its claim was answered too near its hold's end, when a peer's \
             claim may already be forgotten, so firing it could fire it twice",
        );
        assert_eq!(skipped.level, "warn");
        assert!(
            skipped
                .field("late_ms")
                .is_some_and(|late| late.parse::<u64>().is_ok_and(|ms| ms >= 50_000)),
            "{skipped:?}"
        );
    }

    /// The bound on a lock call and the check on a late reach are one threshold:
    /// the millisecond an occurrence goes stale is the first one the check skips
    /// it at, so a call is never abandoned while its answer could still be acted
    /// on, nor awaited once it could not.
    #[test]
    fn an_occurrence_goes_stale_exactly_where_reaching_it_is_judged_too_late() {
        let instant_ms = 1_000;
        let stale = stale_at(instant_ms, MIN_HOLD);
        assert_eq!(
            stale, 51_000,
            "the hold, less the clock skew two replicas may carry"
        );
        assert_eq!(reached_after_hold(stale - 1, instant_ms, 60_000), None);
        assert_eq!(reached_after_hold(stale, instant_ms, 60_000), Some(50_000));
        assert_eq!(
            stale_at(u64::MAX - 1, MIN_HOLD),
            u64::MAX,
            "saturates rather than wrapping"
        );
    }

    /// A lock whose calls never answer — a backend holding every command, a
    /// network dropping them without a reset — and whose every call is counted.
    struct NeverAnswering {
        claims: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl OccurrenceLock for NeverAnswering {
        async fn claim(
            &self,
            _occurrence: &Occurrence,
        ) -> Result<OccurrenceClaim, crate::OccurrenceLockError> {
            self.claims
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            std::future::pending().await
        }

        async fn claimed(&self, _occurrence: &str) -> Result<bool, crate::OccurrenceLockError> {
            std::future::pending().await
        }
    }

    /// A lock written without `#[async_trait]` can panic in the method itself,
    /// before it hands back a future, and that panic is contained like one inside
    /// the future: the occurrence is skipped and named, and nothing unwinds the
    /// job's loop.
    #[tokio::test]
    async fn a_lock_panicking_before_it_hands_back_a_future_is_contained() {
        struct PanicsWhenCalled;
        impl OccurrenceLock for PanicsWhenCalled {
            fn claim<'a, 'b, 'c>(
                &'a self,
                _occurrence: &'b Occurrence,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<
                            Output = Result<OccurrenceClaim, crate::OccurrenceLockError>,
                        > + Send
                        + 'c,
                >,
            >
            where
                'a: 'c,
                'b: 'c,
            {
                panic!("the lock panicked before handing back its claim")
            }

            fn claimed<'a, 'b, 'c>(
                &'a self,
                _occurrence: &'b str,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = Result<bool, crate::OccurrenceLockError>>
                        + Send
                        + 'c,
                >,
            >
            where
                'a: 'c,
                'b: 'c,
            {
                panic!("the lock panicked before handing back its answer")
            }
        }
        fn run(
            _: &Container,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>> {
            Box::pin(async { Ok(()) })
        }

        let logs = nest_rs_testing::LogCapture::install();
        let runner = Runner {
            container: Container::builder().build(),
            ctx: None,
            lock: Some(Arc::new(PanicsWhenCalled)),
        };
        let id = JobId {
            origin: "features::tasks",
            provider: "EagerTasks",
            method: "sweep",
            key: None,
        };
        let task = Task {
            run,
            transaction: JobTransaction::Pool,
            replicas: Replicas::One,
        };
        let now_ms = epoch_millis(SystemTime::now());

        runner
            .claim_then_fire(id, task, now_ms, MIN_HOLD, &CancellationToken::new())
            .await;
        runner
            .report_overrun(
                id,
                0,
                1_000,
                interval_overrun(0, 100, 2),
                stale_at(now_ms, MIN_HOLD),
                &CancellationToken::new(),
            )
            .await;

        let claiming = logs.expect_one(
            crate::TARGET,
            "occurrence skipped: its lock panicked while claiming it",
        );
        assert_eq!(
            claiming.field("panic").as_deref(),
            Some("the lock panicked before handing back its claim")
        );
        let answering = logs.expect_one(
            crate::TARGET,
            "occurrence lock panicked answering whether an overrun occurrence was claimed",
        );
        assert_eq!(
            answering.field("panic").as_deref(),
            Some("the lock panicked before handing back its answer")
        );
    }

    /// A claim the lock never answers is abandoned when its occurrence goes
    /// stale: the occurrence is not fired — the key may have been taken, so
    /// firing could be the second time — and the skip is said, naming the job,
    /// the occurrence and how long the claim was waited on.
    ///
    /// On paused time, so the minute a real claim would wait passes at once.
    #[tokio::test(start_paused = true)]
    async fn a_claim_the_lock_never_answers_is_abandoned_once_its_occurrence_is_stale() {
        static RAN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        fn run(
            _: &Container,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>> {
            Box::pin(async {
                RAN.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })
        }

        let logs = nest_rs_testing::LogCapture::install();
        let runner = Runner {
            container: Container::builder().build(),
            ctx: None,
            lock: Some(Arc::new(NeverAnswering {
                claims: std::sync::atomic::AtomicUsize::new(0),
            })),
        };
        let id = JobId {
            origin: "features::tasks",
            provider: "HungTasks",
            method: "sweep",
            key: None,
        };
        let task = Task {
            run,
            transaction: JobTransaction::Pool,
            replicas: Replicas::One,
        };
        let instant_ms = epoch_millis(SystemTime::now());

        runner
            .claim_then_fire(id, task, instant_ms, MIN_HOLD, &CancellationToken::new())
            .await;

        assert!(
            !RAN.load(std::sync::atomic::Ordering::SeqCst),
            "the occurrence is not fired"
        );
        let skipped = logs.expect_one(
            crate::TARGET,
            "occurrence skipped: its lock did not answer the claim before the occurrence went \
             stale",
        );
        assert_eq!(skipped.level, "warn");
        assert_eq!(skipped.field("provider").as_deref(), Some("HungTasks"));
        assert_eq!(skipped.field("method").as_deref(), Some("sweep"));
        assert_eq!(
            skipped.field("occurrence").as_deref(),
            Some(instant_ms.to_string().as_str())
        );
        assert!(
            skipped.field("waited_ms").is_some_and(|waited| waited
                .parse::<u64>()
                .is_ok_and(|ms| ms > 40_000 && ms <= 50_000)),
            "waited until the hold less the skew: {skipped:?}"
        );
    }

    /// The same bound on the question an overrun asks: an unanswered one is
    /// abandoned at the threshold of the occurrence the report runs beside,
    /// counted unanswered rather than skipped, and named once for the report.
    #[tokio::test(start_paused = true)]
    async fn an_overrun_question_the_lock_never_answers_is_abandoned_and_counted_unanswered() {
        let logs = nest_rs_testing::LogCapture::install();
        let runner = Runner {
            container: Container::builder().build(),
            ctx: None,
            lock: Some(Arc::new(NeverAnswering {
                claims: std::sync::atomic::AtomicUsize::new(0),
            })),
        };
        let id = JobId {
            origin: "features::tasks",
            provider: "HungTasks",
            method: "sweep",
            key: None,
        };
        let now_ms = epoch_millis(SystemTime::now());
        let stale = stale_at(now_ms, MIN_HOLD);

        runner
            .report_overrun(
                id,
                0,
                1_000,
                interval_overrun(0, 100, 3),
                stale,
                &CancellationToken::new(),
            )
            .await;

        let abandoned = logs.expect_one(
            crate::TARGET,
            "occurrence lock did not answer whether an overrun occurrence was claimed before the \
             occurrence reached went stale",
        );
        assert_eq!(abandoned.level, "warn");
        assert_eq!(abandoned.field("provider").as_deref(), Some("HungTasks"));
        assert_eq!(abandoned.field("occurrence").as_deref(), Some("0"));
        assert_eq!(abandoned.field("abandoned").as_deref(), Some("3"));
        let skipped = logs.expect_one(
            crate::TARGET,
            "occurrences skipped: they fell due while the previous one was claimed or run",
        );
        assert_eq!(skipped.field("unanswered").as_deref(), Some("3"));
        assert_eq!(skipped.field("skipped").as_deref(), Some("0"));
    }

    /// What the bound is for: a lock that never answers holds no loop. Without
    /// it the job's first claim held its loop for good and the job never fired
    /// again, with nothing said; with it every occurrence is claimed in turn,
    /// each abandoned and each named, until shutdown.
    #[tokio::test(start_paused = true)]
    async fn a_lock_that_never_answers_stops_no_schedule() {
        fn run(
            _: &Container,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>> {
            Box::pin(async { Ok(()) })
        }

        let logs = nest_rs_testing::LogCapture::install();
        let lock = Arc::new(NeverAnswering {
            claims: std::sync::atomic::AtomicUsize::new(0),
        });
        let runner = Runner {
            container: Container::builder().build(),
            ctx: None,
            lock: Some(Arc::clone(&lock) as Arc<dyn OccurrenceLock>),
        };
        let job = Job::Interval {
            id: JobId {
                origin: "features::tasks",
                provider: "HungTasks",
                method: "sweep",
                key: None,
            },
            period: Duration::from_millis(100),
            task: Task {
                run,
                transaction: JobTransaction::Pool,
                replicas: Replicas::One,
            },
        };
        let token = CancellationToken::new();
        let looping = tokio::spawn(run_job(job, runner, token.clone()));

        let deadline = Instant::now() + Duration::from_secs(3_600);
        while lock.claims.load(std::sync::atomic::Ordering::SeqCst) < 3 {
            assert!(
                Instant::now() < deadline,
                "the loop claimed {} occurrences in an hour of a lock that never answers",
                lock.claims.load(std::sync::atomic::Ordering::SeqCst),
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        token.cancel();
        looping.await.expect("the loop ends at shutdown");

        assert!(
            logs.find(
                crate::TARGET,
                "occurrence skipped: its lock did not answer the claim before the occurrence \
                 went stale",
            )
            .len()
                >= 2,
            "each abandoned claim is named: {:#?}",
            logs.events()
        );
    }

    /// A lock that answers the claim — granting it — at the instant shutdown is
    /// asked for: both are ready in one poll, and cancellation is checked first,
    /// so the answer is dropped and the occurrence is not fired. Without the
    /// order this fired a job while the app was going down, whenever the answer
    /// happened to be polled first.
    #[tokio::test]
    async fn a_claim_answered_as_shutdown_is_asked_for_is_abandoned_and_not_fired() {
        struct GrantingAtShutdown {
            shutdown: CancellationToken,
        }
        #[async_trait]
        impl OccurrenceLock for GrantingAtShutdown {
            async fn claim(
                &self,
                _occurrence: &Occurrence,
            ) -> Result<OccurrenceClaim, crate::OccurrenceLockError> {
                self.shutdown.cancelled().await;
                Ok(OccurrenceClaim::Claimed)
            }

            async fn claimed(&self, _occurrence: &str) -> Result<bool, crate::OccurrenceLockError> {
                Ok(false)
            }
        }
        static RAN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        fn run(
            _: &Container,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>> {
            Box::pin(async {
                RAN.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })
        }

        let logs = nest_rs_testing::LogCapture::install();
        let shutdown = CancellationToken::new();
        let runner = Runner {
            container: Container::builder().build(),
            ctx: None,
            lock: Some(Arc::new(GrantingAtShutdown {
                shutdown: shutdown.clone(),
            })),
        };
        let id = JobId {
            origin: "features::tasks",
            provider: "LeavingTasks",
            method: "sweep",
            key: None,
        };
        let task = Task {
            run,
            transaction: JobTransaction::Pool,
            replicas: Replicas::One,
        };
        let instant_ms = epoch_millis(SystemTime::now());

        let claiming = runner.claim_then_fire(id, task, instant_ms, MIN_HOLD, &shutdown);
        let asking = async {
            tokio::task::yield_now().await;
            shutdown.cancel();
        };
        tokio::join!(claiming, asking);

        assert!(
            !RAN.load(std::sync::atomic::Ordering::SeqCst),
            "nothing fires once shutdown is asked for"
        );
        let abandoned = logs.expect_one(
            crate::TARGET,
            "occurrence skipped: shutdown was asked for before its lock answered the claim",
        );
        assert_eq!(abandoned.level, "warn");
        assert_eq!(abandoned.field("provider").as_deref(), Some("LeavingTasks"));
        assert_eq!(abandoned.field("method").as_deref(), Some("sweep"));
        assert_eq!(
            abandoned.field("occurrence").as_deref(),
            Some(instant_ms.to_string().as_str())
        );
    }

    /// The overrun questions are bounded by the loop's shutdown as the claim is:
    /// a lock that never answers them holds the report until shutdown is asked
    /// for and not a moment past — not until the occurrence goes stale, most of
    /// a minute later — and each question still out counts as unanswered, named
    /// once under shutdown's own cause rather than staleness's.
    #[tokio::test(start_paused = true)]
    async fn overrun_questions_in_flight_at_shutdown_are_abandoned_at_once() {
        let logs = nest_rs_testing::LogCapture::install();
        let runner = Runner {
            container: Container::builder().build(),
            ctx: None,
            lock: Some(Arc::new(NeverAnswering {
                claims: std::sync::atomic::AtomicUsize::new(0),
            })),
        };
        let id = JobId {
            origin: "features::tasks",
            provider: "HungTasks",
            method: "sweep",
            key: None,
        };
        let stale = stale_at(epoch_millis(SystemTime::now()), MIN_HOLD);
        let shutdown = CancellationToken::new();
        const ASKED_AFTER: Duration = Duration::from_secs(1);

        let started = Instant::now();
        let reporting =
            runner.report_overrun(id, 0, 1_000, interval_overrun(0, 100, 3), stale, &shutdown);
        let asking = async {
            sleep(ASKED_AFTER).await;
            shutdown.cancel();
        };
        tokio::join!(reporting, asking);

        assert_eq!(
            started.elapsed(),
            ASKED_AFTER,
            "the report ends when shutdown is asked for, not when the occurrence goes stale"
        );
        let abandoned = logs.expect_one(
            crate::TARGET,
            "occurrence lock had not answered whether an overrun occurrence was claimed when \
             shutdown was asked for",
        );
        assert_eq!(abandoned.level, "warn");
        assert_eq!(abandoned.field("provider").as_deref(), Some("HungTasks"));
        assert_eq!(abandoned.field("occurrence").as_deref(), Some("0"));
        assert_eq!(abandoned.field("abandoned").as_deref(), Some("3"));
        assert_eq!(abandoned.field("waited_ms").as_deref(), Some("1000"));
        logs.expect_none(
            crate::TARGET,
            "occurrence lock did not answer whether an overrun occurrence was claimed before the \
             occurrence reached went stale",
        );
        let skipped = logs.expect_one(
            crate::TARGET,
            "occurrences skipped: they fell due while the previous one was claimed or run",
        );
        assert_eq!(skipped.field("unanswered").as_deref(), Some("3"));
    }

    /// A timer skipping missed ticks hands back deadlines a whole number of
    /// periods apart; one period apart skips nothing, however late the tick fired.
    #[test]
    fn the_ticks_skipped_between_two_deadlines_are_the_periods_between_them_less_one() {
        let period = Duration::from_millis(200);
        let start = Instant::now();
        assert_eq!(ticks_skipped_between(start, start + period, period), 0);
        assert_eq!(ticks_skipped_between(start, start + period * 3, period), 2);
        assert_eq!(ticks_skipped_between(start, start, period), 0);
    }

    /// Whatever held a loop past the next occurrence — a slow claim, a long run,
    /// a stalled replica — it reaches the latest one due by the clock it woke to,
    /// never the stale one it slept for first. With none due it reaches the one it
    /// slept for, and never the last one again.
    #[test]
    fn an_interval_reaches_the_latest_occurrence_due_by_the_clock_it_woke_to() {
        let period_ms = 200;
        assert_eq!(interval_reached(1_000, 1_200, period_ms), 1_200, "on time");
        assert_eq!(
            interval_reached(1_000, 1_250, period_ms),
            1_200,
            "one due: reached late",
        );
        assert_eq!(
            interval_reached(1_000, 1_750, period_ms),
            1_600,
            "several due: the latest of them",
        );
        assert_eq!(
            interval_reached(1_000, 1_150, period_ms),
            1_200,
            "a timer waking a hair early: the one it slept for",
        );
        assert_eq!(
            interval_reached(1_000, 999, period_ms),
            1_200,
            "a clock behind the last one reached never reaches it again",
        );
    }

    /// The same for a cron job, walked forward from the last occurrence reached:
    /// the occurrences its expression names stand for the multiples of a period —
    /// in its time zone when it declares one — and those before the latest are the
    /// overrun.
    #[test]
    fn a_cron_reaches_the_latest_occurrence_due_by_the_clock_it_woke_to() {
        let at = |instant: &str| instant.parse::<DateTime<Utc>>().expect("parses");
        let ms = |instant: &str| u64::try_from(at(instant).timestamp_millis()).expect("in range");
        let reach = |expression: &str, tz: Option<Tz>, last: &str, now: &str| {
            let schedule = Cron::from_str(expression).expect("parses");
            match cron_due(&schedule, tz, at(last), at(now)) {
                CronDue::Latest(instant, overrun) => Some((instant, overrun.first)),
                CronDue::NoneYet => None,
                CronDue::PastCap(_) => panic!("under the cap"),
            }
        };
        let every_minute = "0 * * * * *";
        assert_eq!(
            reach(
                every_minute,
                None,
                "2026-03-11T12:01:00Z",
                "2026-03-11T12:02:06Z"
            ),
            Some((at("2026-03-11T12:02:00Z"), vec![])),
            "one due: reached late",
        );
        assert_eq!(
            reach(
                every_minute,
                None,
                "2026-03-11T12:01:00Z",
                "2026-03-11T12:04:30Z"
            ),
            Some((
                at("2026-03-11T12:04:00Z"),
                vec![ms("2026-03-11T12:02:00Z"), ms("2026-03-11T12:03:00Z")],
            )),
            "several due: the latest of them, and the two before it moved past",
        );
        assert_eq!(
            reach(
                every_minute,
                None,
                "2026-03-11T12:02:00Z",
                "2026-03-11T12:02:59Z"
            ),
            None,
            "none due yet",
        );
        assert_eq!(
            reach(
                "0 0 9 * * *",
                Some(chrono_tz::Europe::Paris),
                "2026-03-10T08:00:00Z",
                "2026-03-11T08:30:00Z",
            ),
            Some((at("2026-03-11T08:00:00Z"), vec![])),
            "the occurrence due is the zone's nine o'clock",
        );
        // A time the spring-forward gap leaves alone — London skips 01:00 to 01:59
        // local, so 02:30 BST exists and is 01:30 UTC — is simply still ahead at
        // 02:10 BST.
        assert_eq!(
            reach(
                "0 30 2 * * *",
                Some(chrono_tz::Europe::London),
                "2026-03-28T02:30:00Z",
                "2026-03-29T01:10:00Z",
            ),
            None,
            "at 02:10 BST on the day clocks go forward, that day's 02:30 is still ahead",
        );
        // The tripwire for the limit `cron_due` describes: this asserts croner's
        // answer for a time the gap swallows, so it fails the day croner changes
        // it — which is when the limit can come off the schedule page.
        assert_eq!(
            reach(
                "0 30 1 * * *",
                Some(chrono_tz::Europe::London),
                "2026-03-28T01:30:00Z",
                "2026-03-29T02:00:00Z",
            ),
            Some((at("2026-03-29T01:00:00Z"), vec![])),
            "the gap's end is what croner names for a time the gap swallowed",
        );
    }

    /// Counting a long cron overrun stops at its cap, and says it stopped; past
    /// the cap, none of the overrun fires late.
    #[test]
    fn a_cron_overrun_is_counted_up_to_its_cap_and_past_it_none_fires_late() {
        let every_second = Cron::from_str("* * * * * *").expect("parses");
        let after: DateTime<Utc> = "2026-03-11T00:00:00Z".parse().expect("parses");
        let seconds = |n| chrono::TimeDelta::try_seconds(n).expect("in range");
        let CronDue::Latest(latest, within) =
            cron_due(&every_second, None, after, after + seconds(10_001))
        else {
            panic!("ten thousand overrun is within the cap");
        };
        assert_eq!(latest, after + seconds(10_001));
        assert_eq!(
            (within.count, within.capped, within.first.len()),
            (10_000, false, 100)
        );
        let CronDue::PastCap(past) = cron_due(&every_second, None, after, after + seconds(10_002))
        else {
            panic!("ten thousand and one overrun is past the cap");
        };
        assert_eq!((past.count, past.capped), (10_000, true));
    }

    /// Several jobs at one site are that site, counted; each other site is named
    /// with its own count; a name no second job shares is not named.
    #[test]
    fn jobs_sharing_a_name_are_named_by_site_and_counted() {
        let job = |origin, method| JobId {
            origin,
            provider: "Tasks",
            method,
            key: None,
        };
        let refusal = refuse_shared_names(&[
            job("features::billing", "sweep"),
            job("features::audit", "sweep"),
            job("features::audit", "sweep"),
            job("features::health", "heartbeat"),
        ])
        .expect_err("one name, three jobs")
        .to_string();
        assert!(
            refusal.contains(
                "`Tasks::sweep` (declared twice in features::audit and once in features::billing)"
            ),
            "{refusal}"
        );
        assert!(!refusal.contains("Tasks::heartbeat"), "{refusal}");
    }

    /// Two apps of one deployment share its lock, and the boot sees one app. Each
    /// declaring its own `MaintenanceTasks::sweep` — legal, and the naming law's
    /// own shape — they claimed each other's occurrences under one token, and
    /// each job ran on some of them with nothing said above `debug`. Their crates
    /// are the token's first level, so they are two jobs; the job one shared crate
    /// declares has one crate, so one token, in every app.
    ///
    /// And only the crate: keyed on the declaring module's whole path, a module
    /// moved or renamed inside its crate split the job during the rolling deploy
    /// that shipped the move, old replicas and new each firing every occurrence.
    #[test]
    fn a_jobs_identity_is_its_crate_its_type_and_its_method() {
        let api = JobId {
            origin: "api::maintenance::schedule::tasks",
            provider: "MaintenanceTasks",
            method: "sweep",
            key: None,
        };
        assert_eq!(api.identity(), "api:MaintenanceTasks:sweep");
        assert_eq!(api.key(), "api::MaintenanceTasks::sweep");
        assert_eq!(
            api.occurrence(1_789_000_000_000),
            "api:MaintenanceTasks:sweep:1789000000000"
        );
        let worker = JobId {
            origin: "worker::maintenance::schedule::tasks",
            ..api
        };
        assert_ne!(
            api.occurrence(1_789_000_000_000),
            worker.occurrence(1_789_000_000_000),
            "two apps' same-named jobs in their own crates claim apart"
        );
        for moved in ["api", "api::tasks", "api::housekeeping::schedule::tasks"] {
            assert_eq!(
                JobId {
                    origin: moved,
                    ..api
                }
                .occurrence(1_789_000_000_000),
                api.occurrence(1_789_000_000_000),
                "moving the job to `{moved}` inside its crate keeps it one job"
            );
        }
    }

    /// A rename of the type, the method or the crate starts a new job, and `key`
    /// is how a job outlives one: it pins the identity the job had, so a replica
    /// built before the rename and one built after claim the same occurrences
    /// through the rolling deploy that ships it.
    #[test]
    fn a_key_pins_the_identity_the_job_had_before_a_rename() {
        let before = JobId {
            origin: "billing::schedule::tasks",
            provider: "InvoiceTasks",
            method: "close_day",
            key: None,
        };
        let renamed = JobId {
            origin: "ledger::schedule::tasks",
            provider: "LedgerTasks",
            method: "close",
            key: None,
        };
        assert_ne!(
            renamed.identity(),
            before.identity(),
            "a rename is a new job"
        );
        let pinned = JobId {
            key: Some("billing::InvoiceTasks::close_day"),
            ..renamed
        };
        assert_eq!(pinned.identity(), before.identity());
        assert_eq!(
            pinned.occurrence(1_789_000_000_000),
            before.occurrence(1_789_000_000_000)
        );
        assert_eq!(
            JobId {
                key: Some("nightly_close"),
                ..renamed
            }
            .identity(),
            "nightly_close",
            "a key is any path, one level included"
        );
    }

    /// The levels are joined by `:`, so a level carrying one could spell another
    /// job's token — a provider `A:B` with a method `c` and a provider `A` with a
    /// method `B:c` — and split its occurrences, in two apps no boot sees
    /// together. Only a job attached by hand can, and the boot refuses it.
    #[test]
    fn a_level_that_is_empty_or_carries_a_colon_cannot_identify_a_job() {
        let job = |origin, provider, method, key| JobId {
            origin,
            provider,
            method,
            key,
        };
        assert!(
            job("features::tasks", "Tasks", "sweep", None)
                .levels_refusal()
                .is_none()
        );
        assert!(
            job("attached_metadata", "Tasks", "sweep", None)
                .levels_refusal()
                .is_none(),
            "a level is any non-empty string without a `:`, whitespace or a control character"
        );
        assert!(
            job("features::tasks", "Tasks", "sweep", Some("billing::close"))
                .levels_refusal()
                .is_none()
        );
        for broken in [
            job("features::tasks", "A:B", "c", None),
            job("features::tasks", "A", "B:c", None),
            job("features::tasks", "", "c", None),
            job("features:tasks", "A", "c", None),
            job("features::", "A", "c", None),
            job("", "A", "c", None),
            job("attached metadata", "A", "c", None),
            job("features::tasks", "A b", "c", None),
            job("features::tasks", "A", "c\n", None),
            job("features::tasks", "A", "c", Some("")),
            job("features::tasks", "A", "c", Some("billing::")),
            job("features::tasks", "A", "c", Some("billing:::close")),
            job("features::tasks", "A", "c", Some("billing:close")),
        ] {
            assert!(
                broken.levels_refusal().is_some(),
                "`{}` from `{}` is refused",
                broken.identity(),
                broken.origin
            );
        }
    }

    /// Two jobs firing once under one identity would take turns at its
    /// occurrences. Only a `key` brings two together, and the boot that sees both
    /// names the identity and each job; a job firing on every replica claims
    /// nothing and is not asked.
    #[test]
    fn jobs_firing_once_under_one_identity_are_refused_naming_each() {
        let pinning = JobId {
            origin: "features::billing::schedule::tasks",
            provider: "LedgerTasks",
            method: "close",
            key: Some("features::InvoiceTasks::close_day"),
        };
        let deriving = JobId {
            origin: "features::invoices::schedule::tasks",
            provider: "InvoiceTasks",
            method: "close_day",
            key: None,
        };
        let refusal = refuse_shared_identities(&[pinning, deriving])
            .expect_err("one identity, two jobs")
            .to_string();
        assert!(
            refusal.contains(
                "`features::InvoiceTasks::close_day` by `LedgerTasks::close` (declared in \
                 features::billing::schedule::tasks) and `InvoiceTasks::close_day` (declared in \
                 features::invoices::schedule::tasks)"
            ),
            "{refusal}"
        );
        assert!(refuse_shared_identities(&[pinning]).is_ok());
    }

    /// A replica held past the following occurrence asks who claimed the one it
    /// overran once it reaches the next — a gap after that occurrence at the
    /// earliest, and up to two gaps after it. Held for the gap alone, as it was
    /// past the one-minute floor, the claim had lapsed by then, and every
    /// occurrence a peer fired was reported skipped.
    #[test]
    fn a_claim_outlives_the_occurrence_after_the_one_it_holds() {
        for gap in [
            Duration::from_millis(1),
            Duration::from_secs(1),
            Duration::from_secs(59),
            Duration::from_secs(60),
            Duration::from_secs(3_600),
            Duration::from_secs(86_400),
        ] {
            let hold = claim_hold(gap);
            assert!(hold >= MIN_HOLD, "{gap:?}: never under the floor");
            assert!(
                hold >= gap * 2,
                "{gap:?}: held {hold:?}, lapsing before the occurrence two gaps on"
            );
        }
        assert_eq!(claim_hold(Duration::MAX), Duration::MAX, "saturates");
    }
}

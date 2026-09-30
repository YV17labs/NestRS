//! [`OccurrenceLock`] — the port a job declared `replicas = "one"` fires through.
//!
//! **As if one replica ran the job.** `replicas = "one"` promises two things, and
//! the port carries both: each occurrence fires **at most once** across the
//! replicas — the occurrence's *claim* — and no two runs of the job overlap, on
//! one replica or several — the job's *run lease*, taken with the claim and kept
//! while the run lasts. A run that outlasts its period therefore holds the job on
//! every replica, as it holds a single replica's loop: the occurrences falling due
//! meanwhile are left unclaimed, and the replica running it reports them skipped
//! and fires the latest one late, once its run ends.
//!
//! **Fail closed, at most once per occurrence.** The scheduler claims an
//! occurrence at its instant and fires it only when the claim says this replica
//! won it. A claim that errors skips the occurrence with a `warn`, and so does a
//! claim still unanswered when the occurrence goes stale or when shutdown is
//! asked for: firing unclaimed would fire it on every replica, which is exactly
//! what the declaration exists to prevent. So a replica that crashes — or stops
//! — after claiming loses that occurrence, and its lease holds the job until it
//! lapses; work that must not be lost belongs in a queue job the tick enqueues —
//! a queue delivers at least once.
//!
//! **Selected by import**, like the throttler's store. A backend binds
//! `Arc<dyn OccurrenceLock>` as a declared factory carrying [`BACKEND_REMEDY`]
//! (`ContainerBuilder::provide_declared_factory_after`, after the connection it
//! claims over), so importing two fails the boot naming both; with none bound, a
//! reachable job declaring `replicas = "one"` fails the boot naming the job. A
//! binding is therefore one factory and nothing else: the instants, the hold, the
//! lease, the tokens and every decision about firing stay here.

use std::time::Duration;

use async_trait::async_trait;

use crate::OccurrenceLockError;

/// One occurrence of a job, as a replica claims it — and the run that fires it
/// when the claim is won.
///
/// Every string is the port's and a backend never parses one: it prefixes each
/// with its own structure. Every replica computes the same `job` and `token` for
/// the same occurrence, so the keys built from them are the whole of the
/// coordination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occurrence {
    /// The job's identity: the path of the module that declared it, the host
    /// struct and the method, one `:`-separated level each —
    /// `features:notifications:schedule:tasks:NotificationsTasks:purge_expired`.
    /// The declaring path is what makes it the job's rather than a name's: two
    /// apps each declaring their own `MaintenanceTasks::sweep` are two jobs,
    /// while one job declared in a crate both apps link is one. The run lease is
    /// held under it.
    pub job: String,
    /// The occurrence's token: `job`, then the instant in milliseconds since the
    /// Unix epoch — `…:NotificationsTasks:purge_expired:1789002000000`. The claim
    /// is held under it.
    pub token: String,
    /// How long the claim is held: twice the gap to the job's following
    /// occurrence and never less than a minute, so it outlasts the skew between
    /// replica clocks and a replica that overran it can still ask who fired it.
    pub hold: Duration,
    /// This attempt to fire it, unique to the attempt — the trace id the tick
    /// fires under. What the run lease is held as, so only the run that took the
    /// lease renews or releases it.
    pub run: String,
    /// How long the run lease lasts unless it is renewed.
    pub lease: Duration,
}

/// What a claim on an [`Occurrence`] came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OccurrenceClaim {
    /// This replica claimed the occurrence and took the job's run lease: it
    /// fires the occurrence, renews the lease while the run lasts, then releases
    /// it.
    Claimed,
    /// Another replica claimed this occurrence first.
    ClaimedElsewhere,
    /// Another replica holds the job's run lease — it is running the job, this
    /// occurrence or an earlier one — so this occurrence was left unclaimed, for
    /// that replica to fire late or report skipped once its run ends.
    RunningElsewhere,
}

/// Claims occurrences of a scheduled job for this replica, and holds the job's
/// run while one fires.
///
/// **Every method answers or fails in bounded time.** The scheduler waits on a
/// claim only while it could still act on it — until the occurrence goes stale,
/// its hold less the clock skew two replicas may carry — then abandons the call
/// and skips the occurrence with a `warn`, so a call that never returns costs
/// occurrences rather than stopping its job; a renewal and a release are bounded
/// by the renewal's own beat. That bound is the scheduler's net, not a backend's
/// budget: a backend bounds each command it sends, as the connection it claims
/// over already does for every other caller, so an outage reaches the `warn` as
/// the backend's own error and promptly, rather than as a minute of silence per
/// occurrence.
///
/// **A call may be dropped at any `.await`** — at that threshold, and the moment
/// the scheduler shuts down, which waits on no claim. A claim dropped after its
/// command reached the backend may still have taken the key and the lease, and
/// that is the at-most-once side the port already chooses: nobody fires the
/// occurrence, the key expires with its hold and the lease with its length. So a
/// backend keeps no state a dropped call would have had to undo.
#[async_trait]
pub trait OccurrenceLock: Send + Sync + 'static {
    /// Claim `occurrence` and take its job's run lease, **atomically**.
    ///
    /// In that order of questions: a lease held by another run answers
    /// [`RunningElsewhere`](OccurrenceClaim::RunningElsewhere) and claims
    /// nothing; a claim a replica already holds answers
    /// [`ClaimedElsewhere`](OccurrenceClaim::ClaimedElsewhere); otherwise the
    /// claim is held for `occurrence.hold`, the lease for `occurrence.lease` as
    /// `occurrence.run`, and the answer is
    /// [`Claimed`](OccurrenceClaim::Claimed). No replica may see the one written
    /// without the other.
    ///
    /// `Err` — nobody can tell, and the scheduler skips the occurrence rather than
    /// risk firing it twice.
    async fn claim(&self, occurrence: &Occurrence) -> Result<OccurrenceClaim, OccurrenceLockError>;

    /// Whether a replica holds the claim on the occurrence `token` names — asked
    /// about the occurrences a replica overran while it claimed or ran the one
    /// before, so the ones a peer fired are told apart from the ones nobody did.
    ///
    /// `Ok(false)` once the claim's hold has ended, whoever held it. A hold is
    /// twice the gap to the following occurrence, so an overrun of one
    /// occurrence is always answered; a longer one reports its earliest
    /// occurrences unclaimed.
    async fn claimed(&self, token: &str) -> Result<bool, OccurrenceLockError>;

    /// Extend the job's run lease by `occurrence.lease`, if `occurrence.run`
    /// still holds it: `Ok(true)` renewed, `Ok(false)` lost — it lapsed, and
    /// another run may hold it now.
    async fn renew(&self, occurrence: &Occurrence) -> Result<bool, OccurrenceLockError>;

    /// Release the job's run lease, if `occurrence.run` still holds it, so the
    /// job's next occurrence need not wait for it to lapse.
    async fn release(&self, occurrence: &Occurrence) -> Result<(), OccurrenceLockError>;
}

/// The remedy the boot names when a job needs a lock and none is bound, or when
/// two backends bind one — shared with every backend's binding, so the two
/// halves of the rule cannot drift.
pub const BACKEND_REMEDY: &str = "Import exactly one occurrence lock binding beside \
     `ScheduleModule` — `nest_rs::redis::RedisScheduleModule` claims each occurrence in Redis — \
     or let every replica fire the job with `replicas = \"each\"`, the default.";

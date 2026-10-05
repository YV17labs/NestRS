//! [`OccurrenceLock`] — the port a job declared `replicas = "one"` fires through.
//!
//! **Once per occurrence, and nothing about runs.** `replicas = "one"` promises
//! that each occurrence of a job fires **at most once** across the replicas
//! sharing the lock, and the port carries exactly that: the occurrence's
//! *claim*. A run holds nothing past its claim, so a run that outlasts its
//! period does not hold the job: the next occurrence is claimed and fired by
//! another replica while the first run goes on, and two runs of the job may
//! overlap. One replica never overlaps its own — its loop fires one occurrence
//! at a time — so the overlap is across replicas only. A job whose runs must
//! not overlap keeps its work shorter than its period, or guards it where the
//! work is — a lock its service takes on the rows it changes — since nothing a
//! scheduler holds can outlive a replica that stalls mid-run.
//!
//! **Fail closed, at most once per occurrence.** The scheduler claims an
//! occurrence at its instant and fires it only when the claim says this replica
//! won it. A claim that errors skips the occurrence with a `warn`, and so does a
//! claim still unanswered when the occurrence goes stale or when shutdown is
//! asked for: firing unclaimed would fire it on every replica, which is exactly
//! what the declaration exists to prevent. So a replica that crashes — or stops
//! — after claiming loses that occurrence, and that occurrence only; work that
//! must not be lost belongs in a queue job the tick enqueues — a queue delivers
//! at least once.
//!
//! **Selected by import**, like the throttler's store. A backend binds
//! `Arc<dyn OccurrenceLock>` as a declared factory carrying [`BACKEND_REMEDY`]
//! (`ContainerBuilder::provide_declared_factory_after`, after the connection it
//! claims over), so importing two fails the boot naming both; with none bound, a
//! reachable job declaring `replicas = "one"` fails the boot naming the job. A
//! binding is therefore one factory and nothing else: the instants, the hold, the
//! token and every decision about firing stay here.

use std::time::Duration;

use async_trait::async_trait;

use crate::OccurrenceLockError;
use crate::scheduler::{MAX_SKEW, MIN_HOLD};

/// One occurrence of a job, as a replica claims it.
///
/// Every string is the port's and a backend never parses one: it prefixes the
/// token with its own structure. Every replica computes the same `token` for the
/// same occurrence, so the key built from it is the whole of the coordination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occurrence {
    /// The occurrence's token: the job's identity, then the instant in
    /// milliseconds since the Unix epoch, one `:`-separated level each —
    /// `features:NotificationsTasks:purge_expired:1789002000000`.
    ///
    /// The identity is the job's crate, its host struct and its method, or the
    /// path its `key = "…"` pins: two apps declaring a same-named job in their own
    /// crates are two jobs, one job declared in a crate both apps link is one, and
    /// moving a job's module inside its crate keeps it.
    pub token: String,
    /// How long the claim is held: twice the gap to the job's following
    /// occurrence and never less than a minute, so it outlasts the skew between
    /// replica clocks and a replica that overran it can still ask who fired it.
    pub hold: Duration,
    /// The attempt to fire it — the trace id the tick fires under. A backend
    /// records it beside the claim for an operator, who finds the trace of the run
    /// an occurrence fired from its key; the claim decides on nothing but the
    /// token.
    pub run: String,
}

/// What a claim on an [`Occurrence`] came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OccurrenceClaim {
    /// This replica claimed the occurrence, and fires it.
    Claimed,
    /// Another replica claimed this occurrence first, and fires it.
    ClaimedElsewhere,
}

/// Claims occurrences of a scheduled job for this replica.
///
/// **Both methods answer or fail in bounded time.** The scheduler waits on an
/// answer only while it could still act on it — until the occurrence goes stale,
/// its hold less the clock skew two replicas may carry — then abandons the call
/// and skips the occurrence with a `warn`, so a call that never returns costs
/// occurrences rather than stopping its job. That bound is the scheduler's net,
/// not a backend's budget: a backend bounds each command it sends, as the
/// connection it claims over already does for every other caller, so an outage
/// reaches the `warn` as the backend's own error and promptly, rather than as a
/// minute of silence per occurrence.
///
/// **A call may be dropped at any `.await`** — at that threshold, and the moment
/// the scheduler shuts down, which waits on no claim. A claim dropped after its
/// command reached the backend may still have taken the key, and that is the
/// at-most-once side the port already chooses: nobody fires that occurrence, and
/// the key expires with its hold. The next occurrence is a key of its own, so a
/// lost answer costs one occurrence and never the job: a backend keeps no state a
/// dropped call would have had to undo.
#[async_trait]
pub trait OccurrenceLock: Send + Sync + 'static {
    /// Claim `occurrence` for `occurrence.hold`, **atomically**: of every
    /// replica claiming one token, exactly one is answered
    /// [`Claimed`](OccurrenceClaim::Claimed), and every other
    /// [`ClaimedElsewhere`](OccurrenceClaim::ClaimedElsewhere) while the claim is
    /// held.
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
}

/// The shortest the scheduler waits on an [`OccurrenceLock`] call: **50 seconds**,
/// from the instant of an occurrence claimed for the shortest hold — a minute —
/// to its going stale, the clock skew two replicas may carry (ten seconds)
/// before the hold ends.
///
/// **The scheduler's net, never a backend's budget**, and a backend's budget sits
/// below it: a command still answering when the net gives up is abandoned, and
/// its occurrence skipped with the bare fact that it went stale rather than the
/// backend's own cause. A binding refuses the boot on a budget at or past it.
pub const LOCK_TIMEOUT: Duration = MIN_HOLD.saturating_sub(MAX_SKEW);

/// The remedy the boot names when a job needs a lock and none is bound, or when
/// two backends bind one — shared with every backend's binding, so the two
/// halves of the rule cannot drift.
pub const BACKEND_REMEDY: &str = "Import exactly one occurrence lock binding beside \
     `ScheduleModule` — `nest_rs::redis::RedisScheduleModule` claims each occurrence in Redis — \
     or let every replica fire the job with `replicas = \"each\"`, the default.";

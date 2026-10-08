//! [`OccurrenceLock`] — the port a job declared `replicas = "one"` fires through.
//!
//! **Once per occurrence, and nothing about runs.** Each occurrence fires **at
//! most once** across the replicas sharing the lock; a run holds nothing past
//! its claim, so runs outlasting their period overlap across replicas (never on
//! one replica). A job whose runs must not overlap guards it where the work is.
//!
//! **Fail closed.** A claim that errors, or is still unanswered when the
//! occurrence goes stale or shutdown is asked for, skips the occurrence with a
//! `warn`. A replica that stops after claiming loses that occurrence; work that
//! must not be lost belongs in a queue job the tick enqueues.
//!
//! **Selected by import.** A backend binds `Arc<dyn OccurrenceLock>` as a
//! declared factory carrying [`BACKEND_REMEDY`]
//! (`ContainerBuilder::provide_declared_factory_after`, after the connection it
//! claims over): two fail the boot, and none fails it for a reachable job
//! declaring `replicas = "one"`. The instants, the hold, the token and every
//! decision about firing stay here.

use std::time::Duration;

use async_trait::async_trait;

use crate::OccurrenceLockError;
use crate::scheduler::{MAX_SKEW, MIN_HOLD};

/// One occurrence of a job, as a replica claims it.
///
/// A backend never parses a string here: it prefixes the token with its own
/// structure. Every replica computes the same `token` for the same occurrence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occurrence {
    /// The occurrence's token: the job's identity (its crate, host struct and
    /// method, or the path its `key = "…"` pins), then the instant in
    /// milliseconds since the Unix epoch, one `:`-separated level each —
    /// `features:NotificationsTasks:purge_expired:1789002000000`.
    pub token: String,
    /// How long the claim is held: twice the gap to the job's following
    /// occurrence and never less than a minute, so it outlasts the skew between
    /// replica clocks and a replica that overran it can still ask who fired it.
    pub hold: Duration,
    /// The trace id the tick fires under, recorded beside the claim for an
    /// operator; the claim decides on nothing but the token.
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
/// **Both methods answer or fail in bounded time**: a backend bounds each
/// command it sends. The scheduler's own net abandons a call once the
/// occurrence goes stale and skips it with a `warn`.
///
/// **A call may be dropped at any `.await`** — at that net, and at shutdown. A
/// backend keeps no state a dropped call would have had to undo.
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

    /// Whether a replica holds the claim on the occurrence `token` names, asked
    /// about the occurrences this replica overran.
    ///
    /// `Ok(false)` once the claim's hold has ended, whoever held it.
    async fn claimed(&self, token: &str) -> Result<bool, OccurrenceLockError>;
}

/// The shortest the scheduler waits on an [`OccurrenceLock`] call: **50 seconds**,
/// from the instant of an occurrence claimed for the shortest hold — a minute —
/// to its going stale, the clock skew two replicas may carry (ten seconds)
/// before the hold ends.
///
/// **The scheduler's net, never a backend's budget**: a binding refuses the boot
/// on a budget at or past it.
pub const LOCK_TIMEOUT: Duration = MIN_HOLD.saturating_sub(MAX_SKEW);

/// The remedy the boot names when a job needs a lock and none is bound, or when
/// two backends bind one.
pub const BACKEND_REMEDY: &str = "Import exactly one occurrence lock binding beside \
     `ScheduleModule` — `nest_rs::redis::RedisScheduleModule` claims each occurrence in Redis — \
     or let every replica fire the job with `replicas = \"each\"`, the default.";

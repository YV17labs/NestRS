//! [`OccurrenceLock`] — the port a job declared `replicas = "one"` fires through.
//!
//! **Fail closed, at most once per occurrence.** The scheduler claims an
//! occurrence at its instant and fires it only when the claim says this replica
//! won it. A claim that errors skips the occurrence with a `warn`, and so does a
//! claim still unanswered when the occurrence goes stale or when shutdown is
//! asked for: firing unclaimed would fire it on every replica, which is exactly
//! what the declaration exists to prevent. So a replica that crashes — or stops
//! — after claiming loses that occurrence, and work that must not be lost
//! belongs in a queue job the tick enqueues — a queue delivers at least once.
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

/// Claims one occurrence of a scheduled job for this replica.
///
/// A backend holds the claim under a key built from `occurrence` for `hold` and
/// answers whether this call created it. Every replica computes the same
/// `occurrence` for the same instant — the job's provider, its method and the
/// instant in milliseconds since the Unix epoch, one `:`-separated level each
/// (`AudioTasks:sweep:1789000000000`) — so the key is the whole of the
/// coordination, and `hold` only has to outlast the skew between replica clocks.
/// The token is the port's: a backend prefixes it with its own structure and
/// never parses it.
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
/// the scheduler shuts down, which waits on no lock. A claim dropped after its
/// command reached the backend may still have taken the key, and that is the
/// at-most-once side the port already chooses: nobody fires the occurrence, and
/// the key expires with its hold. So a backend keeps no state a dropped call
/// would have had to undo.
#[async_trait]
pub trait OccurrenceLock: Send + Sync + 'static {
    /// Claim `occurrence` for `hold`.
    ///
    /// `Ok(true)` — this replica claimed it and fires it. `Ok(false)` — another
    /// replica did. `Err` — nobody can tell, and the scheduler skips the
    /// occurrence rather than risk firing it twice.
    async fn claim(&self, occurrence: &str, hold: Duration) -> Result<bool, OccurrenceLockError>;

    /// Whether a replica holds `occurrence`'s claim — asked about the
    /// occurrences a replica overran while it claimed or ran the one before, so
    /// the ones a peer fired are told apart from the ones nobody did.
    ///
    /// `Ok(false)` once the claim's hold has ended, whoever held it. A hold is
    /// twice the gap to the following occurrence, so an overrun of one
    /// occurrence is always answered; a longer one reports its earliest
    /// occurrences unclaimed.
    async fn claimed(&self, occurrence: &str) -> Result<bool, OccurrenceLockError>;
}

/// The remedy the boot names when a job needs a lock and none is bound, or when
/// two backends bind one — shared with every backend's binding, so the two
/// halves of the rule cannot drift.
pub const BACKEND_REMEDY: &str = "Import exactly one occurrence lock binding beside \
     `ScheduleModule` — `nest_rs::redis::RedisScheduleModule` claims each occurrence in Redis — \
     or let every replica fire the job with `replicas = \"each\"`, the default.";

//! [`ScheduleView`] — what a layer reads of the tick it runs around, and how it
//! skips one.

use std::fmt;
use std::sync::OnceLock;
use std::time::SystemTime;

use nest_rs_core::{Settled, UnitView};

use crate::Replicas;
use crate::scheduler::JobId;

/// The schedule edge's view of one tick, reached through
/// [`UnitContext::switch_to`](nest_rs_core::UnitContext::switch_to): the job it
/// runs, the occurrence it claimed and how many replicas fire it.
///
/// A layer settles the tick without running it through [`skip`](Self::skip) —
/// a maintenance window, a feature switched off:
///
/// ```
/// use std::sync::atomic::{AtomicBool, Ordering};
///
/// use nest_rs_core::{Interceptor, Layer, Next, Outcome, UnitContext, async_trait};
/// use nest_rs_schedule::ScheduleView;
///
/// struct Maintenance {
///     on: AtomicBool,
/// }
///
/// impl Layer for Maintenance {}
///
/// #[async_trait]
/// impl Interceptor for Maintenance {
///     async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
///         if let Some(tick) = cx.switch_to_mut::<ScheduleView>()
///             && self.on.load(Ordering::Relaxed)
///         {
///             return Ok(tick.skip());
///         }
///         next.run(cx).await
///     }
/// }
/// ```
pub struct ScheduleView {
    id: JobId,
    /// The identity [`job`](Self::job) spells, on its first read: a tick no
    /// layer asks pays nothing for it.
    job: OnceLock<String>,
    occurrence: Option<SystemTime>,
    replicas: Replicas,
}

impl ScheduleView {
    pub(crate) const fn new(id: JobId, occurrence: Option<SystemTime>, replicas: Replicas) -> Self {
        Self {
            id,
            job: OnceLock::new(),
            occurrence,
            replicas,
        }
    }

    /// The job, as `key = "…"` spells its identity: its crate, its host struct
    /// and its method (`features::NotificationsTasks::purge_expired`), or the
    /// key it pins.
    pub fn job(&self) -> &str {
        self.job.get_or_init(|| self.id.key())
    }

    /// The occurrence a job firing once claimed — the instant it fell due;
    /// `None` for a job firing on every replica, which claims none.
    pub fn occurrence(&self) -> Option<SystemTime> {
        self.occurrence
    }

    /// How many replicas fire the job.
    pub fn replicas(&self) -> Replicas {
        self.replicas
    }

    /// Settle the tick without running the rest of it. A tick has no retry:
    /// the next occurrence runs as scheduled.
    ///
    /// From an exception filter, it answers a failed tick: the failure is said
    /// at `warn` rather than `error`, and its unit of work still rolls back.
    pub fn skip(&mut self) -> Settled {
        nest_rs_core::__private::settled()
    }
}

impl fmt::Debug for ScheduleView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScheduleView")
            .field("job", &self.job())
            .field("occurrence", &self.occurrence)
            .field("replicas", &self.replicas)
            .finish()
    }
}

impl UnitView for ScheduleView {}

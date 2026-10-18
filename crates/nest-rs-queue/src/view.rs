//! [`QueueView`] — what a layer reads of the job attempt it runs around, and
//! how it settles one without running it.

use std::sync::Arc;

use nest_rs_core::{Settled, UnitView};

use crate::Attempt;

/// The queue edge's view of one job attempt, reached through
/// [`UnitContext::switch_to`](nest_rs_core::UnitContext::switch_to): the
/// attempt the method runs, the method running it, and the backend's own
/// record id.
///
/// A layer settles the job without running it through
/// [`acknowledge`](Self::acknowledge) — the inbox pattern, keyed on the job's
/// id, the one name it keeps across every redelivery:
///
/// ```
/// use nest_rs_core::{Interceptor, Layer, Next, Outcome, UnitContext, async_trait};
/// use nest_rs_queue::{JobId, QueueView};
///
/// # struct Inbox;
/// # impl Inbox { async fn first_time(&self, _: &JobId) -> bool { true } }
/// struct Deduplicating {
///     inbox: Inbox,
/// }
///
/// impl Layer for Deduplicating {}
///
/// #[async_trait]
/// impl Interceptor for Deduplicating {
///     async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
///         if let Some(job) = cx.switch_to_mut::<QueueView>()
///             && !self.inbox.first_time(job.attempt().job_id()).await
///         {
///             return Ok(job.acknowledge());
///         }
///         next.run(cx).await
///     }
/// }
/// ```
#[derive(Debug)]
pub struct QueueView {
    attempt: Attempt,
    processor: &'static str,
    backend_id: Option<Arc<str>>,
}

impl QueueView {
    pub(crate) fn new(
        attempt: Attempt,
        processor: &'static str,
        backend_id: Option<Arc<str>>,
    ) -> Self {
        Self {
            attempt,
            processor,
            backend_id,
        }
    }

    /// The attempt this is: the job's id, its number and its queue.
    pub fn attempt(&self) -> &Attempt {
        &self.attempt
    }

    /// The method running it, as `Provider::method`.
    pub fn processor(&self) -> &'static str {
        self.processor
    }

    /// The backend's own id for the stored record, when it keeps one — beside
    /// the job's id, never in its place.
    pub fn backend_id(&self) -> Option<&str> {
        self.backend_id.as_deref()
    }

    /// Settle the job as done without running the rest of the attempt: the
    /// backend acknowledges it, and no retry follows.
    ///
    /// From an exception filter, it answers a failed attempt: the job is
    /// acknowledged, its unit of work still rolls back, and the failure is
    /// said at `warn`.
    pub fn acknowledge(&mut self) -> Settled {
        nest_rs_core::__private::settled()
    }
}

impl UnitView for QueueView {}

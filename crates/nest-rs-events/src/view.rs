//! [`EventsView`] — what a layer reads of the listener call it runs around,
//! and how it skips one.

use nest_rs_core::{Settled, UnitView};

/// The events edge's view of one listener call, reached through
/// [`UnitContext::switch_to`](nest_rs_core::UnitContext::switch_to): the event
/// it was emitted for and the listener it calls.
///
/// A layer settles the call without running it through [`skip`](Self::skip):
///
/// ```
/// use nest_rs_core::{Interceptor, Layer, Next, Outcome, UnitContext, async_trait};
/// use nest_rs_events::EventsView;
///
/// /// Keeps the audit listeners quiet while a backfill replays history.
/// struct Backfilling;
///
/// impl Layer for Backfilling {}
///
/// #[async_trait]
/// impl Interceptor for Backfilling {
///     async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
///         if let Some(call) = cx.switch_to_mut::<EventsView>()
///             && call.listener().starts_with("AuditListeners::")
///         {
///             return Ok(call.skip());
///         }
///         next.run(cx).await
///     }
/// }
/// ```
#[derive(Debug)]
pub struct EventsView {
    event: &'static str,
    listener: &'static str,
}

impl EventsView {
    pub(crate) const fn new(event: &'static str, listener: &'static str) -> Self {
        Self { event, listener }
    }

    /// The event's type, as `std::any::type_name` spells it.
    pub fn event(&self) -> &'static str {
        self.event
    }

    /// The listener called, as `Provider::method`.
    pub fn listener(&self) -> &'static str {
        self.listener
    }

    /// Settle the call without running the listener; the dispatch goes on with
    /// the next one.
    ///
    /// From an exception filter, it answers a failed call: the failure is said
    /// at `warn` rather than `error`, and its unit of work still rolls back.
    pub fn skip(&mut self) -> Settled {
        nest_rs_core::__private::settled()
    }
}

impl UnitView for EventsView {}

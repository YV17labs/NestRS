//! [`Disposition`] — how the port ends a delivery, in the words a backend's
//! [`settle`](crate::JobConsumer::settle) translates into its storage's.

use std::fmt;
use std::time::Duration;

/// How the port ends one delivery. The backend writes it in one step with the
/// check that it still holds the delivery
/// ([`settle`](crate::JobConsumer::settle)).
///
/// Every way but [`Complete`](Self::Complete) and
/// [`DeadLetter`](Self::DeadLetter) files the job again, and a record filed
/// again starts its delivery count over. Non-exhaustive: a way added later is
/// sent only to a backend declaring the capability that names it, and a
/// driver's catch-all arm answers the driver defect it is.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub enum Disposition<'a> {
    /// The job is done: every record of it goes, its unique key and its
    /// checkpoint with it.
    Complete,
    /// An attempt failed with budget left: file `record` — the job's next
    /// attempt, as the port sealed it — due once `after` has passed. An `after`
    /// above zero is sent only to a backend declaring
    /// [`DelayedPush`](crate::Capability::DelayedPush).
    #[non_exhaustive]
    Retry {
        /// How long the job waits before its next attempt.
        after: Duration,
        /// The record of the next attempt.
        record: &'a [u8],
    },
    /// A newer release sealed the job and this one cannot read it: file `record`
    /// — as it was stored — due once `after` has passed, keeping the instant of
    /// the first such hand-back when the backend keeps one
    /// ([`Delivery::with_deferred_for`](crate::Delivery::with_deferred_for)).
    /// Sent only to a backend declaring
    /// [`DelayedPush`](crate::Capability::DelayedPush).
    #[non_exhaustive]
    Defer {
        /// How long the job waits before it is delivered again.
        after: Duration,
        /// The record as stored.
        record: &'a [u8],
    },
    /// No attempt answered — the drain cut it, or it never started: file
    /// `record`, as it was stored, due at once.
    #[non_exhaustive]
    Requeue {
        /// The record as stored.
        record: &'a [u8],
    },
    /// The job is done failing: keep `record`, as stored, in the dead letters
    /// with `reason` — the failure as its line rendered it, which names no
    /// payload value — and let every other record of the job go, its unique key
    /// and its checkpoint with them.
    #[non_exhaustive]
    DeadLetter {
        /// Why the job ended.
        reason: &'a str,
        /// The record as stored — empty when the backend refused it
        /// ([`Delivery::refused`](crate::Delivery::refused)).
        record: &'a [u8],
    },
}

impl Disposition<'_> {
    /// The way, as a line names it.
    pub(crate) const fn name(&self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Retry { .. } => "retry",
            Self::Defer { .. } => "defer",
            Self::Requeue { .. } => "requeue",
            Self::DeadLetter { .. } => "dead_letter",
        }
    }
}

/// The way, never the record (`payload-never-quoted`).
impl fmt::Debug for Disposition<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Complete => f.write_str("Complete"),
            Self::Retry { after, record } => f
                .debug_struct("Retry")
                .field("after", after)
                .field("record", &record.len())
                .finish(),
            Self::Defer { after, record } => f
                .debug_struct("Defer")
                .field("after", after)
                .field("record", &record.len())
                .finish(),
            Self::Requeue { record } => f
                .debug_struct("Requeue")
                .field("record", &record.len())
                .finish(),
            Self::DeadLetter { reason, record } => f
                .debug_struct("DeadLetter")
                .field("reason", reason)
                .field("record", &record.len())
                .finish(),
        }
    }
}

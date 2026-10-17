//! [`Delivery`] — one job as a backend's [`receive`](crate::JobConsumer::receive)
//! hands it to the port: the record as stored, the lease that holds it, and the
//! facts the backend keeps about it.

use std::fmt;
use std::time::Duration;

/// One job a backend handed to the port, holding its lease.
///
/// Built by the backend's [`receive`](crate::JobConsumer::receive): the record
/// as the producer filed it, or [`refused`](Self::refused) when the backend
/// cannot hand one over, plus what the backend knows of this delivery.
#[non_exhaustive]
pub struct Delivery<L> {
    pub(crate) record: Result<Vec<u8>, &'static str>,
    pub(crate) lease: L,
    pub(crate) leased_for: Duration,
    pub(crate) delivery_count: u32,
    pub(crate) backend_id: Option<String>,
    pub(crate) deferred_for: Option<Duration>,
}

impl<L> Delivery<L> {
    /// `record`, as the producer filed it, held by `lease` for `leased_for` from
    /// the instant it was handed over — the first time the record is.
    pub fn new(record: Vec<u8>, lease: L, leased_for: Duration) -> Self {
        Self {
            record: Ok(record),
            lease,
            leased_for,
            delivery_count: 1,
            backend_id: None,
            deferred_for: None,
        }
    }

    /// A delivery whose record the backend cannot hand over — the stored entry
    /// is not one a producer files — saying `why` in words of the storage's,
    /// never with the value. The port dead-letters it as a unit of work of its
    /// own, and its neighbours run.
    pub fn refused(lease: L, leased_for: Duration, why: &'static str) -> Self {
        Self {
            record: Err(why),
            ..Self::new(Vec::new(), lease, leased_for)
        }
    }

    /// How many times the backend handed this record over, this delivery
    /// included; past [`STALL_LIMIT`](crate::STALL_LIMIT) the port dead-letters
    /// the job without running it.
    #[must_use]
    pub fn with_delivery_count(mut self, count: u32) -> Self {
        self.delivery_count = count.max(1);
        self
    }

    /// The backend's own id for the stored record — reported beside the job's
    /// [`JobId`](crate::JobId) as `backend_id`, never in its place.
    #[must_use]
    pub fn with_backend_id(mut self, backend_id: impl Into<String>) -> Self {
        self.backend_id = Some(backend_id.into());
        self
    }

    /// How long ago the backend first filed this job back unread for a newer
    /// release ([`Disposition::Defer`](crate::Disposition::Defer)), for a
    /// backend that keeps that instant per job: the port's patience with a
    /// newer release's job counts from it, not from the push.
    #[must_use]
    pub fn with_deferred_for(mut self, waited: Duration) -> Self {
        self.deferred_for = Some(waited);
        self
    }

    /// The lease that holds this delivery.
    pub fn lease(&self) -> &L {
        &self.lease
    }
}

/// Never the record: it is somebody's data (`payload-never-quoted`).
impl<L> fmt::Debug for Delivery<L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Delivery")
            .field(
                "record",
                &self
                    .record
                    .as_ref()
                    .map(|record| format!("{} bytes", record.len())),
            )
            .field("leased_for", &self.leased_for)
            .field("delivery_count", &self.delivery_count)
            .field("backend_id", &self.backend_id)
            .field("deferred_for", &self.deferred_for)
            .finish_non_exhaustive()
    }
}

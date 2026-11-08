//! [`KitLease`] — which of its backend's leases a case's apps hold deliveries
//! for.

/// Which of its backend's leases a case's apps hold deliveries for: the one a
/// case renewing a lease waits out, or one no stall of the runner outlasts.
///
/// A runner frozen past a lease between a delivery and its settle sees the job
/// run again, as at-least-once allows, so a case counting a job's runs holds
/// the lasting lease, and lapses one only through
/// [`KitBackend::lapse`](super::KitBackend::lapse) or
/// [`KitBackend::take`](super::KitBackend::take).
///
/// The two are the whole set: a driver names each, since each asks a lease
/// and bounds of its own of the backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[expect(
    clippy::exhaustive_enums,
    reason = "closed by its definition, and every driver gains from naming each arm: each asks a lease and bounds of its own"
)]
pub enum KitLease {
    /// A few seconds: a case whose subject is the lease's renewal waits on
    /// it, and a renewal goes a third into it, so on a networked backend a
    /// runner stalled past the other two thirds sees it lapse and the job run
    /// again, the behaviour right and the count wrong.
    Brief,
    /// Longer than a runner stalls on a loaded machine — seconds at a time —
    /// with every bound of the backend's connection as wide: no case on it
    /// waits one out.
    Lasting,
}

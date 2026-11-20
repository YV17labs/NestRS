//! [`PolicyStats`] — what a policy has seen since its process started.

use std::sync::atomic::{AtomicU64, Ordering};

/// A snapshot of a policy's counters since the process started: what a
/// metrics exporter or a health page reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PolicyStats {
    /// Calls made through the policy, refused ones included.
    pub calls: u64,
    /// Calls that answered `Ok`.
    pub succeeded: u64,
    /// Calls that ended on the call's own error, after their last attempt.
    pub failed: u64,
    /// Attempts cut by the timeout or the deadline.
    pub timed_out: u64,
    /// Attempts made past a call's first.
    pub retried: u64,
    /// Calls refused because the breaker was open.
    pub short_circuited: u64,
    /// Calls refused because the bulkhead was full.
    pub bulkhead_rejected: u64,
    /// Times the breaker opened.
    pub opened: u64,
}

#[derive(Debug, Default)]
pub(crate) struct Counters {
    pub(crate) calls: AtomicU64,
    pub(crate) succeeded: AtomicU64,
    pub(crate) failed: AtomicU64,
    pub(crate) timed_out: AtomicU64,
    pub(crate) retried: AtomicU64,
    pub(crate) short_circuited: AtomicU64,
    pub(crate) bulkhead_rejected: AtomicU64,
    pub(crate) opened: AtomicU64,
}

impl Counters {
    pub(crate) fn bump(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> PolicyStats {
        let read = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        PolicyStats {
            calls: read(&self.calls),
            succeeded: read(&self.succeeded),
            failed: read(&self.failed),
            timed_out: read(&self.timed_out),
            retried: read(&self.retried),
            short_circuited: read(&self.short_circuited),
            bulkhead_rejected: read(&self.bulkhead_rejected),
            opened: read(&self.opened),
        }
    }
}

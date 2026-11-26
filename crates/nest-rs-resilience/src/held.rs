//! [`Held`] — what a run's answer holds of its policy once the run is over,
//! while a part of it is still to come: a response's body.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use crate::breaker::HeldAttempt;
use crate::bulkhead::HeldSlot;
use crate::error::{Refusal, ResilienceError, TIMED_OUT};
use crate::mechanisms::Mechanisms;
use crate::verdict::PANICKED;

/// What a run's answer holds of its policy once the run is over, while a part
/// of it is still to come — a response's body: the run's slot of the bulkhead,
/// its last attempt in the breaker when its answer was judged a success, and
/// what that attempt's bound had left as the run ended. It borrows nothing, so
/// it lives as long as what it covers; dropped before it is settled, the
/// answer stands as it was judged.
pub(crate) struct Held {
    policy: Arc<Mechanisms>,
    attempt: Option<HeldAttempt>,
    slot: Option<HeldSlot>,
    left: Duration,
    bound: Duration,
}

impl Held {
    /// What `policy`'s run handed on: its last `attempt`, sent under `bound`,
    /// with `left` of it, and its `slot`.
    pub(crate) const fn new(
        policy: Arc<Mechanisms>,
        attempt: Option<HeldAttempt>,
        slot: Option<HeldSlot>,
        left: Duration,
        bound: Duration,
    ) -> Self {
        Self {
            policy,
            attempt,
            slot,
            left,
            bound,
        }
    }

    /// What the attempt's bound, counted from its send, had left as the run
    /// ended: how long the rest of the answer may still be waited for.
    pub(crate) const fn left(&self) -> Duration {
        self.left
    }

    /// The answer's outcome now that it is whole, or will never be: `failure`
    /// saying what it failed on, or `None` for the success it was judged. The
    /// breaker counts it and the slot is freed, once: settled again, it
    /// changes nothing.
    pub(crate) fn settle(&mut self, failure: Option<&str>) {
        let policy = &*self.policy;
        // Freed after the breaker, as a run's drop hands a probe's turn on before
        // its slot — and as the breaker's telling unwinds, should a line panic.
        let _slot = self
            .slot
            .take()
            .zip(policy.bulkhead.as_ref())
            .map(|(slot, bulkhead)| bulkhead.resume(slot));
        if let Some((attempt, breaker)) = self.attempt.take().zip(policy.breaker.as_ref()) {
            breaker.resume(attempt).settle(failure);
        }
    }

    /// The answer given up, its reader having waited for the rest of it all
    /// its attempt's bound had left: a failure, timed out, and the refusal its
    /// reader is told, counted.
    pub(crate) fn timed_out(&mut self) -> ResilienceError<Infallible> {
        self.settle(Some(TIMED_OUT));
        let teller = &self.policy.teller;
        let refusal = teller.refused(None, Refusal::TimedOut(self.bound));
        ResilienceError::refused(refusal, teller.instance())
    }

    /// Reading the rest of the answer panicked: a failure, as a panic in an
    /// attempt's call is.
    pub(crate) fn panicked(&mut self) {
        self.settle(Some(PANICKED));
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.settle(None);
    }
}

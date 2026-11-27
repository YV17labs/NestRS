//! [`Held`] — what a run's answer holds of its policy once the run is over,
//! while a part of it is still to come: a response's body.

use std::convert::Infallible;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use std::borrow::Cow;

use crate::attempting::{SharedSent, SharedSlot};
use crate::breaker::{Failure, HeldAttempt, Unwound};
use crate::bulkhead::HeldSlot;
use crate::error::{PANICKED, ResilienceError, ResilienceRefusal, TIMED_OUT};
use crate::mechanisms::Mechanisms;

/// What a run's answer holds of its policy once the run is over, while a part
/// of it is still to come — a response's body: the run's slot of the bulkhead,
/// its last attempt in the breaker when its answer was judged a success, and
/// what that attempt's bound had left as the run ended. It borrows nothing, so
/// it lives as long as what it covers; dropped before it is settled, the
/// answer stands as it was judged.
pub(crate) struct Held {
    policy: Arc<Mechanisms>,
    attempt: Option<Attempt>,
    slot: Option<Slot>,
    left: Duration,
    bound: Duration,
}

/// The attempt a held answer counts in: the run's own, or the turn it shares
/// of the attempt it was made in, whose failure the answer's is.
pub(crate) enum Attempt {
    Own(HeldAttempt),
    Shared(SharedSent),
}

/// The slot a held answer holds: the run's own, or the one it shares of the
/// attempt it was made in, given back as the answer ends.
pub(crate) enum Slot {
    Own(HeldSlot),
    Shared(SharedSlot),
}

impl Held {
    /// What `policy`'s run handed on: its last `attempt`, sent under `bound`,
    /// with `left` of it, and its `slot`.
    pub(crate) const fn new(
        policy: Arc<Mechanisms>,
        attempt: Option<Attempt>,
        slot: Option<Slot>,
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
    pub(crate) fn settle(&mut self, failure: Option<&'static str>) {
        let said = self.ended(failure.map(Failure::Said));
        debug_assert!(said.is_ok(), "the policy's own words never panic");
    }

    /// A read of the answer failed on `error`: a failure, put in words only
    /// if it opens the breaker, words that panic handed back once it is
    /// counted.
    pub(crate) fn failed(&mut self, error: &(dyn Error + 'static)) -> Result<(), Unwound> {
        self.ended(Some(Failure::Error(error)))
    }

    /// [`settle`](Self::settle), whatever the failure.
    fn ended(&mut self, failure: Option<Failure<'_>>) -> Result<(), Unwound> {
        let policy = &*self.policy;
        // Freed after the breaker, as a run's drop hands a probe's turn on before
        // its slot — and as the breaker's telling unwinds, should a line panic.
        let _slot = self.slot.take().map(|slot| match slot {
            Slot::Own(slot) => policy
                .bulkhead
                .as_ref()
                .map(|bulkhead| bulkhead.resume(slot)),
            Slot::Shared(shared) => {
                drop(shared);
                None
            }
        });
        match self.attempt.take() {
            Some(Attempt::Own(attempt)) => match &policy.breaker {
                Some(breaker) => breaker.resume(attempt).ended(failure),
                None => Ok(()),
            },
            Some(Attempt::Shared(turn)) => match failure.map(Failure::said).transpose() {
                Ok(said) => {
                    turn.ended(said);
                    Ok(())
                }
                Err(payload) => {
                    turn.ended(Some(Cow::Borrowed(PANICKED)));
                    Err(payload)
                }
            },
            None => Ok(()),
        }
    }

    /// The answer given up, its reader having waited for the rest of it all
    /// its attempt's bound had left: a failure, timed out, and the refusal its
    /// reader is told, counted.
    pub(crate) fn timed_out(&mut self) -> ResilienceError<Infallible> {
        self.settle(Some(TIMED_OUT));
        let teller = &self.policy.teller;
        let refusal = teller.refused(
            None,
            ResilienceRefusal::TimedOut {
                timeout: self.bound,
            },
        );
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

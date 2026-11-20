//! [`Retry`] — how often an idempotent call is attempted, and how long it
//! waits between attempts.

use std::num::NonZeroU32;
use std::time::Duration;

use nest_rs_core::Backoff;

/// How many times an idempotent call is attempted in all, the first included,
/// and the jittered [`Backoff`] it waits on between two attempts.
///
/// A call its caller does not declare idempotent is attempted once whatever
/// the policy's retry says: replaying a payment because its answer was lost is
/// the failure this rule exists to prevent.
///
/// ```
/// use std::num::NonZeroU32;
/// use std::time::Duration;
/// use nest_rs_core::Backoff;
/// use nest_rs_resilience::Retry;
///
/// let retry = Retry::new(
///     NonZeroU32::new(4).expect("non-zero"),
///     Backoff::new(Duration::from_millis(100), Duration::from_secs(1)),
/// );
/// assert_eq!(retry.attempts().get(), 4);
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Retry {
    attempts: NonZeroU32,
    backoff: Backoff,
}

impl Retry {
    /// Three attempts in all, waiting about 200 ms, then 400 ms, between them
    /// (doubling up to 2 s, ±20 % jitter).
    pub const DEFAULT: Self = Self::new(
        NonZeroU32::MIN.saturating_add(2),
        Backoff::new(Duration::from_millis(200), Duration::from_secs(2)),
    );

    /// `attempts` in all, the first included, waiting on `backoff` between
    /// two of them.
    pub const fn new(attempts: NonZeroU32, backoff: Backoff) -> Self {
        Self { attempts, backoff }
    }

    /// How many times the call is attempted in all, the first included.
    pub const fn attempts(&self) -> NonZeroU32 {
        self.attempts
    }

    /// The curve the waits between two attempts follow.
    pub const fn backoff(&self) -> Backoff {
        self.backoff
    }
}

impl Default for Retry {
    fn default() -> Self {
        Self::DEFAULT
    }
}

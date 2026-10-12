//! [`Backoff`] — how long a loop waits after a failed attempt — [`Retries`], one
//! loop's count of them, and [`process_seed`], what sets two replicas' loops
//! apart.

use std::num::NonZeroU32;
use std::sync::OnceLock;
use std::time::Duration;

/// The jitter's range, in thousandths of the curve: 800 to 1200.
const JITTER_LOW_PERMILLE: u128 = 800;
const JITTER_SPAN_PERMILLE: u64 = 400;

/// FNV-1a's 64-bit offset basis and prime: a hash its own specification fixes,
/// so a wait never moves with the standard library's unspecified
/// `DefaultHasher`.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

const NANOS_PER_SEC: u128 = 1_000_000_000;

/// Exponential backoff with ±20 % jitter — the shape of gRPC's
/// connection-backoff specification (`JITTER = 0.2`) and of the queue's job
/// retries. The wait after failed attempt `n` is `min(ceiling, first ·
/// 2^(n − 1))`, scaled by a factor in `[0.8, 1.2]` drawn from a seed and `n`
/// alone: the same for one seed, apart for two, so loops that failed together
/// do not come back together.
///
/// ```
/// use std::num::NonZeroU32;
/// use std::time::Duration;
/// use nest_rs_core::Backoff;
///
/// let backoff = Backoff::new(Duration::from_millis(250), Duration::from_secs(5));
/// let wait = backoff.wait(NonZeroU32::MIN, Backoff::seed(b"orders"));
/// assert!(wait >= Duration::from_millis(200) && wait <= Duration::from_millis(300));
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Backoff {
    first: Duration,
    ceiling: Duration,
}

impl Backoff {
    /// The backoff waiting about `first` after a first failure, doubling up to
    /// `ceiling`. A `first` past `ceiling` is held to it: neither panics nor
    /// errs, so a value read at runtime is safe.
    pub const fn new(first: Duration, ceiling: Duration) -> Self {
        let first = if first.as_nanos() > ceiling.as_nanos() {
            ceiling
        } else {
            first
        };
        Self { first, ceiling }
    }

    /// The wait after failed attempt number `attempt`, drawn from `seed`.
    pub fn wait(&self, attempt: NonZeroU32, seed: u64) -> Duration {
        let ceiling = self.ceiling.as_nanos();
        let curve = 1u128
            .checked_shl(attempt.get() - 1)
            .map_or(u128::MAX, |factor| {
                self.first.as_nanos().saturating_mul(factor)
            })
            .min(ceiling);
        let jitter = fnv1a(seed, &attempt.get().to_le_bytes()) % (JITTER_SPAN_PERMILLE + 1);
        let nanos = curve.saturating_mul(JITTER_LOW_PERMILLE + u128::from(jitter)) / 1000;
        u64::try_from(nanos / NANOS_PER_SEC).map_or(Duration::MAX, |secs| {
            // The remainder of a division by a billion fits a `u32`.
            Duration::new(secs, (nanos % NANOS_PER_SEC) as u32)
        })
    }

    /// The seed a loop keyed by `key` draws its waits from — a job's id, a
    /// queue's name: its FNV-1a.
    pub fn seed(key: &[u8]) -> u64 {
        fnv1a(FNV_OFFSET_BASIS, key)
    }
}

/// FNV-1a over `bytes`, from the state `from`.
fn fnv1a(from: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(from, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

/// One loop's retries: its [`Backoff`], its seed and how many attempts failed
/// in a row, back to none once a call answers.
///
/// ```
/// use std::time::Duration;
/// use nest_rs_core::{Backoff, Retries, process_seed};
///
/// let mut retries = Retries::new(
///     Backoff::new(Duration::from_millis(250), Duration::from_secs(2)),
///     process_seed(),
/// );
/// let first = retries.next_wait();
/// assert!(first <= Duration::from_millis(300));
/// retries.reset();
/// assert_eq!(retries.next_wait(), first);
/// ```
#[derive(Clone, Debug)]
pub struct Retries {
    backoff: Backoff,
    seed: u64,
    failed: u32,
}

impl Retries {
    /// No failure yet, under `backoff`, drawing from `seed`.
    pub const fn new(backoff: Backoff, seed: u64) -> Self {
        Self {
            backoff,
            seed,
            failed: 0,
        }
    }

    /// The wait after one more failed attempt.
    pub fn next_wait(&mut self) -> Duration {
        self.failed = self.failed.saturating_add(1);
        let attempt = NonZeroU32::new(self.failed).unwrap_or(NonZeroU32::MIN);
        self.backoff.wait(attempt, self.seed)
    }

    /// A call answered: the next failure waits as a first one.
    pub const fn reset(&mut self) {
        self.failed = 0;
    }
}

/// A seed drawn once per process, from a random UUID's bits — what sets two
/// replicas' reconnect loops apart.
pub fn process_seed() -> u64 {
    static SEED: OnceLock<u64> = OnceLock::new();
    *SEED.get_or_init(|| {
        let (high, low) = uuid::Uuid::new_v4().as_u64_pair();
        high ^ low
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    const FIRST: Duration = Duration::from_secs(1);
    const CEILING: Duration = Duration::from_secs(5 * 60);
    const JOBS: Backoff = Backoff::new(FIRST, CEILING);

    fn nth(attempt: u32) -> NonZeroU32 {
        NonZeroU32::new(attempt).expect("attempts count from one")
    }

    /// `min(ceiling, first · 2^(attempt − 1))`, computed apart from the code
    /// under test.
    fn unjittered(backoff: Backoff, attempt: u32) -> Duration {
        let mut wait = backoff.first;
        for _ in 1..attempt.min(64) {
            wait = wait.saturating_mul(2).min(backoff.ceiling);
        }
        wait.min(backoff.ceiling)
    }

    #[test]
    fn every_wait_sits_within_a_fifth_of_its_curve_without_overflow() {
        let seed = Backoff::seed(b"a job");
        let attempts = (1..=64).chain([1_000, u32::MAX / 2, u32::MAX - 1, u32::MAX]);
        for attempt in attempts {
            let curve = unjittered(JOBS, attempt);
            let wait = JOBS.wait(nth(attempt), seed);
            assert!(
                wait >= curve.mul_f64(0.8) && wait <= curve.mul_f64(1.2),
                "attempt {attempt}: {wait:?} against {curve:?}",
            );
        }
    }

    #[test]
    fn a_ceiling_far_above_the_first_wait_is_still_reached() {
        let backoff = Backoff::new(Duration::from_nanos(1), Duration::from_secs(60 * 60));
        let wait = backoff.wait(NonZeroU32::MAX, 7);
        assert!(
            wait >= Duration::from_secs(48 * 60) && wait <= Duration::from_secs(72 * 60),
            "{wait:?}"
        );
        let widest = Backoff::new(Duration::MAX, Duration::MAX);
        assert!(widest.wait(NonZeroU32::MAX, 7) >= Duration::MAX.mul_f64(0.8));
    }

    #[test]
    fn one_seed_and_attempt_always_wait_the_same() {
        let seed = Backoff::seed(b"one loop");
        assert_eq!(JOBS.wait(nth(3), seed), JOBS.wait(nth(3), seed));
        assert_eq!(Backoff::seed(b"one loop"), seed);
    }

    #[test]
    fn loops_failing_at_one_instant_do_not_all_wait_the_same() {
        let waits: BTreeSet<Duration> = (0u32..32)
            .map(|loop_| JOBS.wait(nth(1), Backoff::seed(&loop_.to_le_bytes())))
            .collect();
        assert!(waits.len() > 1, "{waits:?}");
    }

    #[test]
    fn a_first_wait_past_the_ceiling_is_held_to_it() {
        let backoff = Backoff::new(Duration::from_secs(10), Duration::from_secs(2));
        let wait = backoff.wait(nth(1), 7);
        assert!(
            wait >= Duration::from_millis(1_600) && wait <= Duration::from_millis(2_400),
            "{wait:?}"
        );
    }

    /// FNV-1a as its specification writes it, over `key`'s bytes then the
    /// attempt's little-endian ones: the queue's job retries, pinned since 7.0.
    #[test]
    fn the_jitter_is_fnv1a_of_the_key_then_the_attempt() {
        let key = [
            0x01, 0x89, 0x0a, 0x5d, 0xac, 0x96, 0x77, 0x4b, 0xbc, 0xce, 0xb3, 0x02, 0x09, 0x9a,
            0x80, 0x57,
        ];
        let seed = Backoff::seed(&key);
        let waits: Vec<u128> = [1, 2, 3, 4, 9, 10, 11]
            .into_iter()
            .map(|attempt| JOBS.wait(nth(attempt), seed).as_millis())
            .collect();
        assert_eq!(waits, [1129, 1938, 3236, 6816, 254_464, 250_200, 322_500]);
    }

    #[test]
    fn retries_climb_the_curve_and_start_over_once_a_call_answers() {
        let backoff = Backoff::new(Duration::from_millis(250), Duration::from_secs(5));
        let mut retries = Retries::new(backoff, 7);
        let climbed: Vec<Duration> = (0..3).map(|_| retries.next_wait()).collect();
        assert_eq!(
            climbed,
            [1, 2, 3].map(|attempt| backoff.wait(nth(attempt), 7)),
        );
        retries.reset();
        assert_eq!(retries.next_wait(), backoff.wait(nth(1), 7));
    }

    #[test]
    fn the_process_seed_is_drawn_once() {
        assert_eq!(process_seed(), process_seed());
    }
}

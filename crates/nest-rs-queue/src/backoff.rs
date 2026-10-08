//! How long a job waits between a failed attempt and the next one.
//!
//! Exponential — one second after the first failure, doubling after each, never
//! more than five minutes — and jittered to between 80% and 120% of that, so
//! the jobs a shared outage failed together do not all come back together.
//!
//! The jitter is a hash of the job's id and the attempt number, never a random
//! draw, so every wait is reproducible.

use std::time::Duration;

use crate::JobId;

/// The wait after a first failed attempt.
const FIRST: Duration = Duration::from_secs(1);

/// The longest wait before jitter, reached from the tenth attempt on.
const CEILING: Duration = Duration::from_secs(5 * 60);

/// The jitter's range, in thousandths of the unjittered wait: 800 to 1200.
const JITTER_LOW_PERMILLE: u64 = 800;
const JITTER_SPAN_PERMILLE: u64 = 400;

/// How long to wait after attempt number `attempt` (1-based) of job `id` failed
/// retryably, before the next attempt runs:
/// `min(5 min, 1 s · 2^(attempt − 1))`, scaled by a jitter in `[0.8, 1.2]`
/// derived from `(id, attempt)`.
pub(crate) fn retry_after(id: &JobId, attempt: u32) -> Duration {
    let first = FIRST.as_millis() as u64;
    let ceiling = CEILING.as_millis() as u64;
    // Past the ceiling long before the shift could overflow: 2^9 seconds is
    // already more than five minutes.
    let doublings = attempt.saturating_sub(1).min(16);
    let unjittered = first.saturating_mul(1 << doublings).min(ceiling);
    let permille = JITTER_LOW_PERMILLE + fnv1a(id, attempt) % (JITTER_SPAN_PERMILLE + 1);
    Duration::from_millis(unjittered * permille / 1000)
}

/// FNV-1a over the id's bytes and the attempt number's — a hash fixed by its
/// own specification, so the jitter does not move with the standard library's
/// `DefaultHasher`, whose algorithm is explicitly unspecified.
fn fnv1a(id: &JobId, attempt: u32) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    id.as_bytes()
        .iter()
        .chain(attempt.to_le_bytes().iter())
        .fold(OFFSET_BASIS, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(raw: &str) -> JobId {
        JobId::parse(raw).expect("a v7 id")
    }

    #[test]
    fn each_wait_doubles_within_its_jitter_up_to_the_ceiling() {
        let job = JobId::mint();
        for attempt in 1..=40u32 {
            let unjittered = FIRST
                .saturating_mul(1 << attempt.saturating_sub(1).min(16))
                .min(CEILING);
            let wait = retry_after(&job, attempt);
            assert!(
                wait >= unjittered.mul_f64(0.8) && wait <= unjittered.mul_f64(1.2),
                "attempt {attempt}: {wait:?} against {unjittered:?}",
            );
        }
        assert!(retry_after(&job, 1) <= Duration::from_millis(1200));
        assert!(retry_after(&job, u32::MAX) <= CEILING.mul_f64(1.2));
    }

    #[test]
    fn the_wait_is_a_function_of_the_job_and_the_attempt() {
        let job = JobId::mint();
        assert_eq!(retry_after(&job, 3), retry_after(&job, 3));
    }

    /// Pinned to literals computed outside this crate (FNV-1a as its
    /// specification writes it).
    #[test]
    fn the_jitter_is_pinned() {
        let job = id("01890a5d-ac96-774b-bcce-b302099a8057");
        let waits: Vec<u128> = [1, 2, 3, 4, 9, 10, 11]
            .into_iter()
            .map(|attempt| retry_after(&job, attempt).as_millis())
            .collect();
        assert_eq!(
            waits,
            [1129, 1938, 3236, 6816, 254_464, 250_200, 322_500],
            "attempts 10 and 11 wait on the five-minute ceiling, jittered",
        );
    }

    #[test]
    fn jobs_failing_at_one_instant_do_not_all_wait_the_same() {
        let waits: std::collections::BTreeSet<Duration> =
            (0..32).map(|_| retry_after(&JobId::mint(), 1)).collect();
        assert!(waits.len() > 1, "{waits:?}");
    }
}

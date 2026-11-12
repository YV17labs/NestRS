//! How long a job waits between a failed attempt and the next one.
//!
//! Exponential — one second after the first failure, doubling after each, never
//! more than five minutes — and jittered to between 80% and 120% of that, so
//! the jobs a shared outage failed together do not all come back together:
//! `nest_rs_core::Backoff`, the one policy every retry loop of the framework
//! takes.
//!
//! The jitter is drawn from the job's id and the attempt number, never a random
//! draw, so every wait is reproducible.
//!
//! A failure can ask for longer: a `Problem` carrying `with_retry_after`, an
//! upstream's `429` passed on, holds the job back at least that long, up to
//! [`ASKED_CEILING`], and up to a fifth longer, drawn the same way
//! (`Backoff::honouring`): jobs one answer refused at one instant do not all
//! come back at the instant it named, and none comes back before it.

use std::num::NonZeroU32;
use std::time::Duration;

use nest_rs_core::Backoff;

use crate::JobId;

/// The wait after a first failed attempt.
const FIRST: Duration = Duration::from_secs(1);

/// The longest wait before jitter, reached from the tenth attempt on.
const CEILING: Duration = Duration::from_secs(5 * 60);

/// A job's retries: one second, doubling to five minutes.
const RETRY: Backoff = Backoff::new(FIRST, CEILING);

/// The longest wait a failure can ask of its job, so a value no upstream meant
/// parks no job: an hour spans a rate limit's usual window, and the retry budget
/// bounds how many times it is waited.
const ASKED_CEILING: Duration = Duration::from_secs(60 * 60);

/// How long to wait after attempt number `attempt` (1-based) of job `id` failed
/// retryably, before the next attempt runs:
/// `min(5 min, 1 s · 2^(attempt − 1))`, scaled by a jitter in `[0.8, 1.2]`
/// drawn from `(id, attempt)`.
pub(crate) fn retry_after(id: &JobId, attempt: u32) -> Duration {
    let attempt = NonZeroU32::new(attempt).unwrap_or(NonZeroU32::MIN);
    RETRY.wait(attempt, Backoff::seed(id.as_bytes()))
}

/// How long to wait after attempt number `attempt` of job `id` failed retryably
/// asking `asked` seconds: its [`retry_after`], or what it asked when longer —
/// counted up to [`ASKED_CEILING`], then stretched by up to a fifth, drawn from
/// `(id, attempt)` as the backoff's jitter is.
pub(crate) fn honouring(id: &JobId, attempt: u32, asked: Option<u32>) -> Duration {
    let backoff = retry_after(id, attempt);
    asked.map_or(backoff, |seconds| {
        let asked = Duration::from_secs(seconds.into()).min(ASKED_CEILING);
        let attempt = NonZeroU32::new(attempt).unwrap_or(NonZeroU32::MIN);
        backoff.max(Backoff::honouring(
            asked,
            attempt,
            Backoff::seed(id.as_bytes()),
        ))
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

    #[test]
    fn what_a_failure_asks_lengthens_the_wait_and_never_shortens_it_or_passes_the_ceiling() {
        let job = JobId::mint();
        let attempt = 5;
        let backoff = retry_after(&job, attempt);
        assert!(backoff >= Duration::from_millis(12_800), "{backoff:?}");
        // Stretched by a fifth, 10 s reaches 12 s at most, under that floor.
        for asked in [None, Some(0), Some(10)] {
            assert_eq!(honouring(&job, attempt, asked), backoff, "{asked:?}");
        }
        for (asked, at_least) in [
            (30, Duration::from_secs(30)),
            (3600, ASKED_CEILING),
            (3601, ASKED_CEILING),
            (u32::MAX, ASKED_CEILING),
        ] {
            let wait = honouring(&job, attempt, Some(asked));
            assert!(
                wait >= at_least && wait <= at_least.mul_f64(1.2),
                "{asked}: {wait:?} against {at_least:?}"
            );
        }
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
    fn jobs_one_answer_asked_to_wait_do_not_all_come_back_at_its_instant() {
        let asked = Duration::from_secs(30);
        let waits: std::collections::BTreeSet<Duration> = (0..32)
            .map(|_| honouring(&JobId::mint(), 1, Some(30)))
            .collect();
        assert!(waits.len() > 1, "{waits:?}");
        for wait in &waits {
            assert!(
                *wait >= asked && *wait <= asked.mul_f64(1.2),
                "never before what was asked, at most a fifth after: {wait:?}"
            );
        }
    }

    #[test]
    fn jobs_failing_at_one_instant_do_not_all_wait_the_same() {
        let waits: std::collections::BTreeSet<Duration> =
            (0..32).map(|_| retry_after(&JobId::mint(), 1)).collect();
        assert!(waits.len() > 1, "{waits:?}");
    }
}

//! [`prove`] — the one loop a resource the framework opens is proved through at
//! boot — what one attempt met ([`Attempt`]), how a proof ended without the
//! resource ([`Unproved`]), and what it reports while it runs ([`Proving`]).

use std::future::Future;
use std::time::Duration;

use tokio::time::Instant;

use crate::backoff::{Backoff, Retries};

/// Why one attempt to open a resource failed.
#[derive(Debug)]
pub enum Attempt<E, R> {
    /// What every attempt would meet — refused credentials, a certificate the
    /// client does not accept — which ends the proof at once.
    Refused(R),
    /// What may clear, so the next attempt follows within the budget.
    Failed(E),
}

/// How a proof ended without the resource.
#[derive(Debug)]
pub enum Unproved<E, R> {
    /// An attempt met what every attempt would.
    Refused(R),
    /// The budget was spent on attempts that may have cleared: `last` is the
    /// last one's failure, `None` when the budget ran out during the first.
    Spent {
        /// How many attempts were made.
        attempts: u32,
        /// The last attempt's failure, if one ended before the budget did.
        last: Option<E>,
    },
}

/// What a proof reports while it runs, for the resource's crate to say on its
/// own target: a `tracing` target is a callsite constant, so the kernel cannot
/// log on a driver's behalf.
#[derive(Debug)]
#[non_exhaustive]
pub enum Proving<'a, E> {
    /// Attempt number `attempt` failed with `error`, which may clear: the next
    /// follows after a wait, if the budget leaves one.
    Retrying {
        /// The attempt that failed, from 1.
        attempt: u32,
        /// What it failed with.
        error: &'a E,
    },
    /// The budget ran out during attempt number `attempt`.
    TimedOut {
        /// The attempt the budget cut, from 1.
        attempt: u32,
    },
    /// Attempt number `attempts` opened and proved the resource.
    Proved {
        /// How many attempts it took, from 1.
        attempts: u32,
    },
}

/// Run `attempt` until it opens and proves the resource, meets a refusal, or
/// `budget` is spent between failures that may clear.
///
/// Each attempt is handed what is left of the budget and cut when it runs out.
/// Between two, the wait is `backoff`'s curve drawn from `seed` — a reconnect
/// loop's [`process_seed`](crate::process_seed), so replicas that lost a
/// resource together come back apart — clamped to the budget left. `observe`
/// hears every failure that may clear, a budget cut, and the proof.
///
/// ```
/// use std::time::Duration;
/// use nest_rs_core::{Attempt, Backoff, Proving, prove, process_seed};
///
/// # tokio::runtime::Builder::new_current_thread().enable_time().start_paused(true).build()?.block_on(async {
/// let (mut tries, mut retried) = (0, 0);
/// let opened = prove(
///     Duration::from_secs(5),
///     Backoff::new(Duration::from_millis(250), Duration::from_secs(2)),
///     process_seed(),
///     |proving| {
///         if let Proving::Retrying { .. } = proving {
///             retried += 1;
///         }
///     },
///     |_left| {
///         tries += 1;
///         let outcome = if tries < 3 {
///             Err(Attempt::<&str, &str>::Failed("connection refused"))
///         } else {
///             Ok("connected")
///         };
///         async move { outcome }
///     },
/// )
/// .await;
/// assert!(matches!(opened, Ok("connected")));
/// assert_eq!(retried, 2);
/// # });
/// # Ok::<(), std::io::Error>(())
/// ```
pub async fn prove<T, E, R, F, Fut>(
    budget: Duration,
    backoff: Backoff,
    seed: u64,
    mut observe: impl FnMut(Proving<'_, E>),
    mut attempt: F,
) -> Result<T, Unproved<E, R>>
where
    F: FnMut(Duration) -> Fut,
    Fut: Future<Output = Result<T, Attempt<E, R>>>,
{
    let deadline = Instant::now().checked_add(budget);
    let left = || {
        deadline.map_or(Duration::MAX, |at| {
            at.saturating_duration_since(Instant::now())
        })
    };
    let mut retries = Retries::new(backoff, seed);
    let mut attempts = 0u32;
    let mut last = None;
    loop {
        let remaining = left();
        if remaining.is_zero() {
            break;
        }
        attempts = attempts.saturating_add(1);
        match tokio::time::timeout(remaining, attempt(remaining)).await {
            Ok(Ok(opened)) => {
                observe(Proving::Proved { attempts });
                return Ok(opened);
            }
            Ok(Err(Attempt::Refused(refusal))) => return Err(Unproved::Refused(refusal)),
            Ok(Err(Attempt::Failed(error))) => {
                observe(Proving::Retrying {
                    attempt: attempts,
                    error: &error,
                });
                last = Some(error);
            }
            Err(_elapsed) => {
                observe(Proving::TimedOut { attempt: attempts });
                break;
            }
        }
        let remaining = left();
        if remaining.is_zero() {
            break;
        }
        tokio::time::sleep(retries.next_wait().min(remaining)).await;
    }
    Err(Unproved::Spent { attempts, last })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    const BUDGET: Duration = Duration::from_secs(10);
    const CURVE: Backoff = Backoff::new(Duration::from_millis(250), Duration::from_secs(2));

    /// What `observe` heard, owned.
    #[derive(Debug, PartialEq, Eq)]
    enum Heard {
        Retrying(u32, u32),
        TimedOut(u32),
        Proved(u32),
    }

    fn hear(heard: &RefCell<Vec<Heard>>) -> impl FnMut(Proving<'_, u32>) + '_ {
        |proving| {
            heard.borrow_mut().push(match proving {
                Proving::Retrying { attempt, error } => Heard::Retrying(attempt, *error),
                Proving::TimedOut { attempt } => Heard::TimedOut(attempt),
                Proving::Proved { attempts } => Heard::Proved(attempts),
            });
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_refusal_ends_the_proof_at_once() {
        let started = Instant::now();
        let heard = RefCell::new(Vec::new());
        let mut made = 0;
        let outcome = prove(BUDGET, CURVE, 1, hear(&heard), |_| {
            made += 1;
            async { Err::<(), _>(Attempt::<u32, &str>::Refused("wrong password")) }
        })
        .await;

        assert!(matches!(outcome, Err(Unproved::Refused("wrong password"))));
        assert_eq!(made, 1, "never retried");
        assert_eq!(started.elapsed(), Duration::ZERO, "and no wait spent");
        assert!(heard.borrow().is_empty(), "{:?}", heard.borrow());
    }

    #[tokio::test(start_paused = true)]
    async fn failures_are_retried_until_the_budget_and_the_last_is_kept() {
        let started = Instant::now();
        let heard = RefCell::new(Vec::new());
        let mut made = 0u32;
        let outcome = prove(BUDGET, CURVE, 1, hear(&heard), |_| {
            made += 1;
            let failed = made;
            async move { Err::<(), _>(Attempt::<u32, &str>::Failed(failed)) }
        })
        .await;

        let Err(Unproved::Spent { attempts, last }) = outcome else {
            panic!("a resource failing every attempt is never proved");
        };
        assert!(attempts > 1, "retried, made {attempts}");
        assert_eq!((attempts, last), (made, Some(made)), "the last kept");
        assert_eq!(started.elapsed(), BUDGET, "the whole budget, and no more");
        let expected: Vec<Heard> = (1..=made).map(|n| Heard::Retrying(n, n)).collect();
        assert_eq!(*heard.borrow(), expected);
    }

    #[tokio::test(start_paused = true)]
    async fn every_event_is_heard_in_order_up_to_the_proof() {
        let heard = RefCell::new(Vec::new());
        let mut made = 0u32;
        let outcome = prove(BUDGET, CURVE, 1, hear(&heard), |_| {
            made += 1;
            let now = made;
            async move {
                if now < 3 {
                    Err(Attempt::<u32, &str>::Failed(now))
                } else {
                    Ok(now)
                }
            }
        })
        .await;

        assert!(matches!(outcome, Ok(3)));
        assert_eq!(
            *heard.borrow(),
            [
                Heard::Retrying(1, 1),
                Heard::Retrying(2, 2),
                Heard::Proved(3)
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_wait_between_attempts_follows_the_curve_and_never_passes_the_budget() {
        let started = Instant::now();
        let mut at = Vec::new();
        let outcome = prove(
            Duration::from_secs(1),
            Backoff::new(Duration::from_secs(10), Duration::from_secs(60)),
            1,
            |_: Proving<'_, u32>| {},
            |left| {
                at.push((started.elapsed(), left));
                async { Err::<(), _>(Attempt::<u32, &str>::Failed(0)) }
            },
        )
        .await;

        assert!(matches!(
            outcome,
            Err(Unproved::Spent {
                attempts: 1,
                last: Some(0)
            })
        ));
        assert_eq!(at, [(Duration::ZERO, Duration::from_secs(1))]);
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(1),
            "a ten-second wait clamped to the second left"
        );

        let started = Instant::now();
        let mut waited = Vec::new();
        let spent = prove(
            BUDGET,
            CURVE,
            7,
            |_: Proving<'_, u32>| {},
            |left| {
                waited.push((started.elapsed(), left));
                async { Err::<(), _>(Attempt::<u32, &str>::Failed(0)) }
            },
        )
        .await;
        assert!(matches!(spent, Err(Unproved::Spent { .. })));
        let mut retries = Retries::new(CURVE, 7);
        for pair in waited.windows(2) {
            let ((first, left), (next, then)) = (pair[0], pair[1]);
            // tokio's timer counts whole milliseconds, rounding a wait up.
            let wait = retries.next_wait();
            assert!(
                next - first >= wait && next - first <= wait + Duration::from_millis(1),
                "{wait:?} in {waited:?}"
            );
            assert_eq!(
                left - then,
                next - first,
                "each attempt is handed what is left"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn an_attempt_the_budget_runs_out_on_is_cut_and_said() {
        let started = Instant::now();
        let heard = RefCell::new(Vec::new());
        let outcome = prove(BUDGET, CURVE, 1, hear(&heard), |_| {
            std::future::pending::<Result<(), Attempt<u32, &str>>>()
        })
        .await;

        assert!(matches!(
            outcome,
            Err(Unproved::Spent {
                attempts: 1,
                last: None
            })
        ));
        assert_eq!(started.elapsed(), BUDGET);
        assert_eq!(*heard.borrow(), [Heard::TimedOut(1)]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_refusal_after_failures_ends_the_proof_with_the_refusal() {
        let heard = RefCell::new(Vec::new());
        let mut made = 0u32;
        let outcome = prove(BUDGET, CURVE, 1, hear(&heard), |_| {
            made += 1;
            let now = made;
            async move {
                if now < 2 {
                    Err::<(), _>(Attempt::Failed(now))
                } else {
                    Err(Attempt::Refused("the server's certificate"))
                }
            }
        })
        .await;

        assert!(matches!(
            outcome,
            Err(Unproved::Refused("the server's certificate"))
        ));
        assert_eq!(*heard.borrow(), [Heard::Retrying(1, 1)]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_budget_no_clock_can_hold_is_never_spent() {
        let outcome = prove(
            Duration::MAX,
            CURVE,
            1,
            |_: Proving<'_, u32>| {},
            |_| async { Ok::<_, Attempt<u32, &str>>("opened") },
        )
        .await;
        assert!(matches!(outcome, Ok("opened")));
    }
}

use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use nest_rs_core::{Backoff, ToProblem};
use nest_rs_resilience::{CircuitBreaker, CircuitState, Policy, ResilienceError, Retry, code};

fn retry(attempts: u32) -> Retry {
    Retry::new(
        NonZeroU32::new(attempts).unwrap(),
        Backoff::new(Duration::from_millis(100), Duration::from_secs(1)),
    )
}

#[derive(Debug, PartialEq, Eq)]
struct Down;

#[tokio::test(start_paused = true)]
async fn an_idempotent_call_is_retried_until_it_answers() {
    let policy = Policy::builder("flaky").retry(retry(3)).build().unwrap();
    let tries = AtomicU32::new(0);
    let answer = policy
        .run_idempotent(|_| {
            let n = tries.fetch_add(1, Ordering::SeqCst);
            async move { if n < 2 { Err(Down) } else { Ok(n) } }
        })
        .await;
    assert_eq!(answer.unwrap(), 2);
    let stats = policy.stats();
    assert_eq!((stats.calls, stats.succeeded, stats.retried), (1, 1, 2));
}

#[tokio::test(start_paused = true)]
async fn a_call_not_declared_idempotent_is_made_once() {
    let policy = Policy::builder("payments").retry(retry(5)).build().unwrap();
    let tries = AtomicU32::new(0);
    let answer: Result<(), _> = policy
        .run(|_| {
            tries.fetch_add(1, Ordering::SeqCst);
            async { Err(Down) }
        })
        .await;
    assert_eq!(tries.load(Ordering::SeqCst), 1);
    match answer.unwrap_err() {
        ResilienceError::Failed {
            attempts, error, ..
        } => {
            assert_eq!((attempts, error), (1, Down));
        }
        other => panic!("expected the call's own error, got {other:?}"),
    }
    assert_eq!(policy.stats().failed, 1);
}

#[tokio::test(start_paused = true)]
async fn the_last_attempts_error_is_answered_once_retries_run_out() {
    let policy = Policy::builder("down").retry(retry(3)).build().unwrap();
    let answer: Result<(), _> = policy.run_idempotent(|_| async { Err(Down) }).await;
    let error = answer.unwrap_err();
    assert!(matches!(error, ResilienceError::Failed { attempts: 3, .. }));
    assert!(!error.is_rejected());
}

#[tokio::test(start_paused = true)]
async fn an_attempt_past_its_timeout_is_cancelled_and_answers_timeout() {
    let policy = Policy::builder("slow")
        .timeout(Duration::from_millis(500))
        .build()
        .unwrap();
    let seen = Arc::new(AtomicU32::new(0));
    let token_seen = Arc::clone(&seen);
    let answer: Result<(), ResilienceError<Down>> = policy
        .run(move |cancel| {
            let token_seen = Arc::clone(&token_seen);
            // The token a timed-out attempt receives is cancelled: work the
            // attempt spawned elsewhere stops with it.
            tokio::spawn(async move {
                cancel.cancelled().await;
                token_seen.fetch_add(1, Ordering::SeqCst);
            });
            async {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok(())
            }
        })
        .await;
    let error = answer.unwrap_err();
    assert!(matches!(
        error,
        ResilienceError::Timeout { after, attempts: 1, .. } if after == Duration::from_millis(500)
    ));
    let problem = error.problem().unwrap();
    assert_eq!((problem.status(), problem.code()), (504, code::TIMEOUT));
    tokio::task::yield_now().await;
    assert_eq!(seen.load(Ordering::SeqCst), 1);
    assert_eq!(policy.stats().timed_out, 1);
}

#[tokio::test(start_paused = true)]
async fn the_deadline_bounds_every_attempt_and_wait() {
    let policy = Policy::builder("bounded")
        .timeout(Duration::from_secs(1))
        .deadline(Duration::from_millis(1500))
        .retry(retry(10))
        .build()
        .unwrap();
    let started = tokio::time::Instant::now();
    let answer: Result<(), ResilienceError<Down>> = policy
        .run_idempotent(|_| async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(())
        })
        .await;
    assert!(matches!(
        answer.unwrap_err(),
        ResilienceError::Timeout { .. }
    ));
    assert!(started.elapsed() <= Duration::from_millis(1500));
}

#[tokio::test(start_paused = true)]
async fn the_breaker_opens_refuses_then_probes_and_closes() {
    let open_for = Duration::from_secs(5);
    let policy = Policy::builder("partner")
        .circuit_breaker(
            CircuitBreaker::new(NonZeroU32::new(4).unwrap(), 50, open_for).with_minimum_calls(2),
        )
        .build()
        .unwrap();
    for _ in 0..2 {
        policy
            .run(|_| async { Err::<(), _>(Down) })
            .await
            .unwrap_err();
    }
    assert_eq!(policy.circuit_state(), CircuitState::Open);

    let tries = AtomicU32::new(0);
    let refused: Result<(), _> = policy
        .run(|_| {
            tries.fetch_add(1, Ordering::SeqCst);
            async { Ok::<(), Down>(()) }
        })
        .await;
    assert_eq!(
        tries.load(Ordering::SeqCst),
        0,
        "an open breaker makes no call"
    );
    let refused = refused.unwrap_err();
    assert!(refused.is_rejected());
    let problem = refused.problem().unwrap();
    assert_eq!(
        (problem.status(), problem.code()),
        (503, code::CIRCUIT_OPEN)
    );
    assert_eq!(problem.retry_after(), Some(5));

    tokio::time::advance(open_for).await;
    assert_eq!(policy.circuit_state(), CircuitState::HalfOpen);
    policy.run(|_| async { Ok::<(), Down>(()) }).await.unwrap();
    assert_eq!(policy.circuit_state(), CircuitState::Closed);
    let stats = policy.stats();
    assert_eq!((stats.opened, stats.short_circuited), (1, 1));
}

#[tokio::test(start_paused = true)]
async fn an_error_that_is_not_a_failure_neither_retries_nor_trips() {
    #[derive(Debug, PartialEq, Eq)]
    enum Answer {
        NotFound,
        Down,
    }
    let policy = Policy::builder("catalog")
        .retry(retry(3))
        .circuit_breaker(CircuitBreaker::new(
            NonZeroU32::MIN,
            100,
            Duration::from_secs(5),
        ))
        .build()
        .unwrap();
    let tries = AtomicU32::new(0);
    let answer: Result<(), _> = policy
        .call()
        .idempotent()
        .failure_if(|error: &Answer| *error == Answer::Down)
        .run(|_| {
            tries.fetch_add(1, Ordering::SeqCst);
            async { Err(Answer::NotFound) }
        })
        .await;
    assert_eq!(answer.unwrap_err().into_inner(), Some(Answer::NotFound));
    assert_eq!(tries.load(Ordering::SeqCst), 1);
    assert_eq!(policy.circuit_state(), CircuitState::Closed);
}

#[tokio::test(start_paused = true)]
async fn a_fallback_answers_a_refused_call() {
    let policy = Policy::builder("prices")
        .circuit_breaker(CircuitBreaker::new(
            NonZeroU32::MIN,
            100,
            Duration::from_secs(5),
        ))
        .build()
        .unwrap();
    policy
        .run(|_| async { Err::<u32, _>(Down) })
        .await
        .unwrap_err();
    let price = policy
        .call()
        .run_or_else(
            |_| async { Ok::<u32, Down>(10) },
            |error| async move {
                assert!(error.is_rejected());
                Ok(7)
            },
        )
        .await;
    assert_eq!(price.unwrap(), 7);
}

#[tokio::test(start_paused = true)]
async fn clones_share_one_breaker() {
    let policy = Policy::builder("shared")
        .circuit_breaker(CircuitBreaker::new(
            NonZeroU32::MIN,
            100,
            Duration::from_secs(5),
        ))
        .build()
        .unwrap();
    let clone = policy.clone();
    clone
        .run(|_| async { Err::<(), _>(Down) })
        .await
        .unwrap_err();
    assert_eq!(policy.circuit_state(), CircuitState::Open);
}

#[test]
fn a_refusal_answers_through_to_problem_when_the_inner_error_does() {
    let error: ResilienceError<nest_rs_core::Problem> = ResilienceError::BulkheadFull {
        policy: Arc::from("p"),
    };
    let problem = error.to_problem().unwrap();
    assert_eq!(
        (problem.status(), problem.code()),
        (503, code::BULKHEAD_FULL)
    );
}

#[test]
fn a_policy_that_could_never_answer_is_refused() {
    assert!(Policy::builder("").build().is_err());
    assert!(
        Policy::builder("zero")
            .timeout(Duration::ZERO)
            .build()
            .is_err()
    );
}

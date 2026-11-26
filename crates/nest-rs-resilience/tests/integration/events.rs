//! Covers `src/events.rs` and the reads of `src/policy.rs` through the public
//! API: each event a real run tells, in order and with its fields, to every
//! subscription open as it happens; what a subscription that fell behind, or
//! whose policy is gone, is told; that nothing listening costs a failing call
//! no chain; and each read at each state of its mechanism. An event naming a
//! decorated method names it, and says its error, as the line of its fact does.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

use nest_rs_core::{error_message, injectable, module};
use nest_rs_resilience::{
    BreakerPhase, BreakerState, ResilienceConfig, ResilienceError, ResilienceEvent,
    ResilienceEvents, ResilienceModule, ResiliencePolicy, ResilienceRefusal, TARGET, resilient,
};
use nest_rs_testing::{CapturedEvent, HeadlessApp, LogCapture};
use tokio::time::Instant;

use crate::{quiet, told};

#[derive(Debug, thiserror::Error)]
#[error("the quotes API is down")]
struct Down;

type Answer<T = ()> = Result<T, ResilienceError<Down>>;

#[injectable]
struct QuotesService {
    #[inject]
    quotes: Arc<ResiliencePolicy>,
    attempts: AtomicU32,
}

impl QuotesService {
    /// Fails its first attempt, and answers the instant of the next.
    #[resilient(quotes, idempotent)]
    async fn quote(&self) -> Answer<Instant> {
        if self.attempts.fetch_add(1, Ordering::Relaxed) == 0 {
            return Err(Down);
        }
        Ok(Instant::now())
    }

    #[resilient(quotes)]
    async fn refresh(&self) -> Answer {
        Err(Down)
    }

    /// A job's progress, asked again while it is under a hundred.
    #[resilient(quotes, idempotent, retry_ok_if = |progress| *progress < 100)]
    async fn poll(&self) -> Answer<u8> {
        Ok(40)
    }

    /// Never answers, sent again as each attempt is given up.
    #[resilient(quotes, idempotent)]
    async fn unanswered(&self) -> Answer {
        silent().await
    }

    #[resilient(quotes, fallback = stale_quote)]
    async fn latest_quote(&self) -> Answer<u32> {
        Err(Down)
    }

    /// The quote last held, whatever `latest_quote`'s run ended on.
    async fn stale_quote(&self, _error: ResilienceError<Down>) -> Answer<u32> {
        Ok(7)
    }

    /// Fails on an error counting in `said` each time its chain is said.
    #[resilient(quotes, fallback = unwatched)]
    async fn watched(&self, said: &Arc<AtomicUsize>) -> Result<u32, ResilienceError<Watched>> {
        Err(Watched(Arc::clone(said)))
    }

    async fn unwatched(
        &self,
        _said: &Arc<AtomicUsize>,
        _error: ResilienceError<Watched>,
    ) -> Result<u32, ResilienceError<Watched>> {
        Ok(7)
    }
}

#[module(imports = [ResilienceModule], providers = [QuotesService])]
struct QuotesModule;

/// The service over the default instance's policy, pinned to `config`.
async fn quotes(
    config: ResilienceConfig,
) -> (HeadlessApp, Arc<QuotesService>, Arc<ResiliencePolicy>) {
    let app = HeadlessApp::builder()
        .module::<QuotesModule>()
        .provide(config)
        .build_headless()
        .await
        .expect("the module boots");
    let service = app
        .container()
        .get::<QuotesService>()
        .expect("the module provides it");
    let policy = crate::held::<nest_rs_core::Unnamed>(&app);
    (app, service, policy)
}

/// The method a retry's or a fallback's event names, and the error it says.
fn said(event: &ResilienceEvent) -> (Option<String>, Option<String>, Option<String>) {
    let (host, method, error) = match event {
        ResilienceEvent::Retry {
            host,
            method,
            error,
            ..
        } => (host, method, error.clone()),
        ResilienceEvent::Fallback {
            host,
            method,
            error,
            ..
        } => (host, method, Some(error.clone())),
        other => panic!("a retry or a fallback: {other:?}"),
    };
    (host.clone(), method.map(str::to_owned), error)
}

/// The method `line` names and the error it says, in its fields.
fn filed(line: &CapturedEvent) -> (Option<String>, Option<String>, Option<String>) {
    (
        line.field("host"),
        line.field("method"),
        line.field("error"),
    )
}

/// `run` polled once, and still waiting: on its call, or for a slot.
fn parked<'f, T>(run: impl Future<Output = T> + 'f) -> Pin<Box<dyn Future<Output = T> + 'f>> {
    let mut run: Pin<Box<dyn Future<Output = T> + 'f>> = Box::pin(run);
    assert!(
        run.as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending(),
        "the run waits"
    );
    run
}

/// A call that never answers.
fn silent() -> std::future::Pending<Result<(), Down>> {
    std::future::pending()
}

/// A retry is told as its line is filed: the attempt that failed, the wait the
/// run then waits, and the chain of what the attempt answered — naming the
/// decorated method that ran it, and no method for a call run by hand.
#[tokio::test(start_paused = true)]
async fn a_retry_is_told_with_the_attempt_that_failed_its_wait_and_its_errors_chain() {
    let (_app, service, policy) = quotes(ResilienceConfig {
        retries: Some(2),
        ..quiet()
    })
    .await;
    let mut events = policy.events();

    let started = Instant::now();
    let answered = service.quote().await.expect("its second attempt answers");
    let [
        ResilienceEvent::Retry {
            host,
            method,
            attempt: 1,
            wait,
            error,
            ..
        },
    ] = &told(&mut events)[..]
    else {
        panic!("one retry, of the first attempt");
    };
    assert_eq!(
        (host.as_deref(), *method, error.as_deref()),
        (
            Some("QuotesService"),
            Some("quote"),
            Some("the quotes API is down")
        )
    );
    let waited = answered - started;
    assert!(
        *wait <= waited && waited < *wait + Duration::from_millis(1),
        "the wait it says, {wait:?}, is the one waited, {waited:?}, to the timer's millisecond"
    );

    let failed = policy
        .run_idempotent(|_| async { Err::<(), _>(Down) })
        .await;
    assert!(matches!(failed, Err(ResilienceError::Failed(Down))));
    let said: Vec<_> = told(&mut events)
        .into_iter()
        .map(|event| match event {
            ResilienceEvent::Retry {
                host: None,
                method: None,
                attempt,
                error,
                ..
            } => (attempt, error),
            other => panic!("a retry of a call run by hand: {other:?}"),
        })
        .collect();
    let down = Some("the quotes API is down".to_owned());
    assert_eq!(
        said,
        [(1, down.clone()), (2, down)],
        "each attempt sent again, and none for the last, which ends the run"
    );
}

/// An `Ok` its method says to send again is a retry with no error to tell,
/// naming the method as its line does.
#[tokio::test(start_paused = true)]
async fn an_ok_sent_again_is_a_retry_telling_no_error() {
    let (_app, service, policy) = quotes(ResilienceConfig {
        retries: Some(1),
        ..quiet()
    })
    .await;
    let logs = LogCapture::install();
    let mut events = policy.events();

    let ran = service.poll().await;
    assert!(
        matches!(ran, Ok(40)),
        "the last answer, the retries spent: {ran:?}"
    );
    let told = told(&mut events);
    let [retry @ ResilienceEvent::Retry { attempt: 1, .. }] = &told[..] else {
        panic!("one retry, of the first attempt: {told:?}");
    };
    assert_eq!(
        said(retry),
        (
            Some("QuotesService".to_owned()),
            Some("poll".to_owned()),
            None
        )
    );
    assert_eq!(
        said(retry),
        filed(&logs.expect_one(TARGET, "attempt answered; retrying the call"))
    );
}

/// An attempt given up at its bound is told with that bound, before the retry
/// it is sent again by, whose error is the timeout's sentence; and a call its
/// unit had no time left to send, with no bound at all.
#[tokio::test(start_paused = true)]
async fn an_attempt_timed_out_is_told_with_its_bound_before_its_retry() {
    let (_app, policy) = crate::policy(ResilienceConfig {
        timeout: Duration::from_millis(100),
        retries: Some(1),
        ..quiet()
    })
    .await;
    let mut events = policy.events();

    let given_up = policy.run_idempotent(|_| silent()).await;
    assert!(matches!(
        given_up,
        Err(ResilienceError::Refused {
            refusal: ResilienceRefusal::TimedOut { .. },
            ..
        })
    ));
    let told_of_the_run = told(&mut events);
    let [
        ResilienceEvent::Timeout {
            host: None,
            method: None,
            timeout: first,
            ..
        },
        ResilienceEvent::Retry {
            attempt: 1, error, ..
        },
        ResilienceEvent::Timeout { timeout: last, .. },
    ] = &told_of_the_run[..]
    else {
        panic!("a timeout, its retry, the retry's timeout: {told_of_the_run:?}");
    };
    assert_eq!(
        (*first, *last, error.as_deref()),
        (
            Duration::from_millis(100),
            Duration::from_millis(100),
            Some("the call did not answer within its timeout")
        )
    );

    let out_of_time =
        nest_rs_core::__private::with_deadline(Some(Instant::now()), policy.run(|_| silent()))
            .await;
    assert!(matches!(
        out_of_time,
        Err(ResilienceError::Refused {
            refusal: ResilienceRefusal::TimedOut { .. },
            ..
        })
    ));
    let told = told(&mut events);
    assert!(
        matches!(&told[..], [ResilienceEvent::Timeout { timeout, .. }] if timeout.is_zero()),
        "{told:?}"
    );
}

/// The timeout of an attempt a decorated method sends again names the method,
/// as the retry it earns and the timeout its run ends on do.
#[tokio::test(start_paused = true)]
async fn an_attempt_of_a_decorated_method_timed_out_is_told_with_its_method_before_its_retry() {
    let (_app, service, policy) = quotes(ResilienceConfig {
        timeout: Duration::from_millis(100),
        retries: Some(1),
        ..quiet()
    })
    .await;
    let mut events = policy.events();

    let given_up = service.unanswered().await;
    assert!(
        matches!(
            given_up,
            Err(ResilienceError::Refused {
                refusal: ResilienceRefusal::TimedOut { .. },
                ..
            })
        ),
        "{given_up:?}"
    );
    let told = told(&mut events);
    let named: Vec<_> = told
        .iter()
        .map(|event| match event {
            ResilienceEvent::Timeout { host, method, .. }
            | ResilienceEvent::Retry { host, method, .. } => (host.as_deref(), *method),
            other => panic!("a timeout or a retry: {other:?}"),
        })
        .collect();
    assert!(
        matches!(
            &told[..],
            [
                ResilienceEvent::Timeout { .. },
                ResilienceEvent::Retry { attempt: 1, .. },
                ResilienceEvent::Timeout { .. },
            ]
        ),
        "a timeout, its retry, the retry's timeout: {told:?}"
    );
    assert_eq!(named, [(Some("QuotesService"), Some("unanswered")); 3]);
}

/// A timeout that leaves no run to answer counts in the breaker and is told
/// by no event of its policy: an attempt its caller drops past its bound
/// inside a unit's deadline, and one dropped, before its own bound, by the
/// timeout of the policy its caller runs under — whose own run tells its
/// timeout.
#[tokio::test(start_paused = true)]
async fn a_timeout_that_leaves_no_run_to_answer_is_told_by_no_event() {
    let (_app, policy) = crate::policy(ResilienceConfig {
        timeout: Duration::from_millis(100),
        retries: None,
        breaker_window: Some(4),
        breaker_failures: 4,
        ..ResilienceConfig::default()
    })
    .await;
    let mut events = policy.events();

    let deadline = Instant::now() + Duration::from_secs(10);
    nest_rs_core::__private::with_deadline(Some(deadline), async {
        let run = parked(policy.run(|_| silent()));
        tokio::time::advance(Duration::from_millis(200)).await;
        drop(run);
    })
    .await;
    assert_eq!(stats(&policy), (1, 1, 1.0), "dropped past its bound");
    assert_eq!(told(&mut events), []);

    let (_app, around) = crate::policy(ResilienceConfig {
        timeout: Duration::from_millis(50),
        ..quiet()
    })
    .await;
    let mut told_around = around.events();
    let cut = around.run(|_| policy.run(|_| silent())).await;
    assert!(
        matches!(cut, Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. })
            if timeout == Duration::from_millis(50)),
        "{cut:?}"
    );
    assert_eq!(stats(&policy), (2, 2, 1.0), "cut before its bound");
    assert_eq!(told(&mut events), []);
    let told_around = told(&mut told_around);
    assert!(
        matches!(told_around[..], [ResilienceEvent::Timeout { .. }]),
        "{told_around:?}"
    );
}

/// The phases a transition says it left and entered, and whether by hand.
fn transitions(events: &mut ResilienceEvents) -> Vec<(BreakerPhase, BreakerPhase, bool)> {
    told(events)
        .into_iter()
        .map(|event| match event {
            ResilienceEvent::Transition {
                from, to, manual, ..
            } => (from, to, manual),
            other => panic!("a transition: {other:?}"),
        })
        .collect()
}

/// Each transition is told as its `warn` is filed, in the order it took
/// effect, each leaving the phase the one before it entered: the breaker's
/// own, then a switch by hand, marked so, told even into the phase it found.
#[tokio::test(start_paused = true)]
async fn each_transition_of_the_breaker_is_told_with_the_phase_left_the_one_entered_and_whether_by_hand()
 {
    use BreakerPhase::{Closed, HalfOpen, Open};

    let (_app, policy) = crate::policy(ResilienceConfig {
        retries: None,
        breaker_window: Some(1),
        breaker_failures: 1,
        breaker_open: Duration::from_secs(30),
        ..ResilienceConfig::default()
    })
    .await;
    let mut events = policy.events();

    let failed = policy.run(|_| async { Err::<(), _>(Down) }).await;
    assert!(matches!(failed, Err(ResilienceError::Failed(Down))));
    assert_eq!(transitions(&mut events), [(Closed, Open, false)]);

    tokio::time::advance(Duration::from_secs(30)).await;
    assert_eq!(policy.breaker_state(), Some(BreakerState::HalfOpen));
    assert_eq!(
        transitions(&mut events),
        [],
        "the open time passing is no transition: none until a probe is sent"
    );
    let probed = policy.run(|_| async { Ok::<_, Down>(()) }).await;
    assert!(probed.is_ok());
    assert_eq!(
        transitions(&mut events),
        [(Open, HalfOpen, false), (HalfOpen, Closed, false)]
    );

    policy
        .open_breaker(Duration::from_secs(10))
        .expect("a breaker");
    policy
        .open_breaker(Duration::from_secs(10))
        .expect("a breaker");
    policy.close_breaker().expect("a breaker");
    assert_eq!(
        transitions(&mut events),
        [
            (Closed, Open, true),
            (Open, Open, true),
            (Open, Closed, true)
        ]
    );
}

/// What a refusal's event says of the method refused, and the refusal.
fn refusals(
    events: &mut ResilienceEvents,
) -> Vec<(Option<String>, Option<&'static str>, ResilienceRefusal)> {
    told(events)
        .into_iter()
        .filter_map(|event| match event {
            ResilienceEvent::Refusal {
                host,
                method,
                refusal,
                ..
            } => Some((host, method, refusal)),
            ResilienceEvent::Transition { .. } => None,
            other => panic!("a refusal: {other:?}"),
        })
        .collect()
}

/// A call refused unsent is told once, with the refusal its caller is
/// answered and the wait that one says: an open breaker's, a full bulkhead's
/// and a spent rate's.
#[tokio::test(start_paused = true)]
async fn a_call_refused_unsent_is_told_with_the_refusal_its_caller_is_answered() {
    let (_app, service, policy) = quotes(ResilienceConfig {
        retries: None,
        breaker_window: Some(1),
        breaker_failures: 1,
        breaker_open: Duration::from_secs(30),
        ..ResilienceConfig::default()
    })
    .await;
    let mut events = policy.events();
    assert!(matches!(
        service.refresh().await,
        Err(ResilienceError::Failed(Down))
    ));
    let Err(ResilienceError::Refused {
        refusal: ResilienceRefusal::Open { retry_after, .. },
        ..
    }) = service.refresh().await
    else {
        panic!("the breaker its failure opened refuses the next");
    };
    let told = refusals(&mut events);
    assert!(
        matches!(
            &told[..],
            [(Some(host), Some("refresh"), ResilienceRefusal::Open { retry_after: said, .. })]
                if host == "QuotesService" && *said == retry_after
        ),
        "{told:?}"
    );

    let (_app, policy) = crate::policy(ResilienceConfig {
        concurrency: Some(1),
        ..quiet()
    })
    .await;
    let mut events = policy.events();
    let holding = parked(policy.run(|_| silent()));
    let full = policy.run(|_| silent()).await;
    assert!(matches!(
        full,
        Err(ResilienceError::Refused {
            refusal: ResilienceRefusal::Full,
            ..
        })
    ));
    assert_eq!(
        refusals(&mut events),
        [(None, None, ResilienceRefusal::Full)]
    );
    drop(holding);

    let (_app, policy) = crate::policy(ResilienceConfig {
        rate_limit: Some(1),
        rate_window: Duration::from_secs(10),
        ..quiet()
    })
    .await;
    let mut events = policy.events();
    assert!(policy.run(|_| async { Ok::<_, Down>(()) }).await.is_ok());
    let Err(ResilienceError::Refused {
        refusal: ResilienceRefusal::Limited { retry_after, .. },
        ..
    }) = policy.run(|_| async { Ok::<_, Down>(()) }).await
    else {
        panic!("the one token is taken");
    };
    let told = refusals(&mut events);
    assert!(
        matches!(
            &told[..],
            [(None, None, ResilienceRefusal::Limited { retry_after: said, .. })]
                if *said == retry_after
        ),
        "{told:?}"
    );
}

/// A policy with a bulkhead refuses a call before it takes a slot, and tells
/// that refusal once, as it does one met at an attempt: a unit with no time
/// left, the attempt given up with no bound, and an open breaker, each naming
/// the method refused.
#[tokio::test(start_paused = true)]
async fn a_call_refused_before_it_takes_a_slot_of_the_bulkhead_is_told_once() {
    let (_app, service, policy) = quotes(ResilienceConfig {
        concurrency: Some(1),
        ..ResilienceConfig::default()
    })
    .await;
    let mut events = policy.events();

    let out_of_time =
        nest_rs_core::__private::with_deadline(Some(Instant::now()), service.refresh()).await;
    assert!(matches!(
        out_of_time,
        Err(ResilienceError::Refused {
            refusal: ResilienceRefusal::TimedOut { .. },
            ..
        })
    ));
    let told = told(&mut events);
    assert!(
        matches!(
            &told[..],
            [ResilienceEvent::Timeout { host: Some(host), method: Some("refresh"), timeout, .. }]
                if host == "QuotesService" && timeout.is_zero()
        ),
        "{told:?}"
    );

    policy
        .open_breaker(Duration::from_secs(30))
        .expect("a breaker");
    let Err(ResilienceError::Refused {
        refusal: ResilienceRefusal::Open { retry_after, .. },
        ..
    }) = service.refresh().await
    else {
        panic!("the breaker opened by hand refuses the call");
    };
    let told = refusals(&mut events);
    assert!(
        matches!(
            &told[..],
            [(Some(host), Some("refresh"), ResilienceRefusal::Open { retry_after: said, .. })]
                if host == "QuotesService" && *said == retry_after
        ),
        "{told:?}"
    );
}

/// A fallback answering is told with the chain of the error it answers for —
/// the call's own, or the policy's refusal, itself told first — naming the
/// method and saying the error as its line does.
#[tokio::test(start_paused = true)]
async fn a_fallback_answering_is_told_with_the_chain_of_the_error_it_answers_for() {
    const ANSWERS: &str = "the call ended on an error; its fallback answers";

    let (_app, service, policy) = quotes(ResilienceConfig {
        retries: None,
        breaker_window: Some(1),
        breaker_failures: 1,
        ..ResilienceConfig::default()
    })
    .await;
    let logs = LogCapture::install();
    let mut events = policy.events();

    let answered = service.latest_quote().await;
    assert!(matches!(answered, Ok(7)), "{answered:?}");
    let told_of_the_failure = told(&mut events);
    let [ResilienceEvent::Transition { .. }, fallback] = &told_of_the_failure[..] else {
        panic!("the breaker's opening, then the fallback: {told_of_the_failure:?}");
    };
    assert_eq!(
        said(fallback),
        (
            Some("QuotesService".to_owned()),
            Some("latest_quote".to_owned()),
            Some(error_message(&ResilienceError::Failed(Down)))
        )
    );
    assert_eq!(said(fallback), filed(&logs.expect_one(TARGET, ANSWERS)));

    let answered = service.latest_quote().await;
    assert!(matches!(answered, Ok(7)), "{answered:?}");
    let told_of_the_refusal = told(&mut events);
    let [
        ResilienceEvent::Refusal {
            refusal: ResilienceRefusal::Open { .. },
            ..
        },
        fallback,
    ] = &told_of_the_refusal[..]
    else {
        panic!("the open breaker's refusal, then the fallback: {told_of_the_refusal:?}");
    };
    let (host, method, error) = said(fallback);
    assert_eq!(
        (host.as_deref(), method.as_deref()),
        (Some("QuotesService"), Some("latest_quote"))
    );
    assert!(
        error
            .as_deref()
            .is_some_and(|error| error.starts_with("the circuit breaker is open")),
        "{error:?}"
    );
    let lines = logs.find(TARGET, ANSWERS);
    assert_eq!(lines.len(), 2, "a line a fallback");
    assert_eq!((host, method, error), filed(&lines[1]));
}

/// Every subscription open as an event is told answers it, and one opened
/// later answers nothing told before it.
#[tokio::test(start_paused = true)]
async fn every_subscription_is_told_each_event_from_its_opening_on() {
    use BreakerPhase::{Closed, Open};

    let (_app, policy) = crate::policy(ResilienceConfig::default()).await;
    let mut first = policy.events();
    policy
        .open_breaker(Duration::from_secs(10))
        .expect("a breaker");
    let mut late = policy.events();
    policy.close_breaker().expect("a breaker");

    assert_eq!(
        transitions(&mut first),
        [(Closed, Open, true), (Open, Closed, true)]
    );
    assert_eq!(
        transitions(&mut late),
        [(Open, Closed, true)],
        "opened once the breaker was"
    );
    drop(first);
    policy.close_breaker().expect("a breaker");
    assert_eq!(
        transitions(&mut late),
        [(Closed, Closed, true)],
        "the other closing takes nothing from it"
    );
}

/// A waker reading and switching the policy whose event woke it.
struct Reading {
    policy: Arc<ResiliencePolicy>,
    /// What each wake read of the breaker: its state, and its window's outcomes.
    read: Mutex<Vec<(Option<BreakerState>, Option<usize>)>>,
}

impl Wake for Reading {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        let read = (
            self.policy.breaker_state(),
            self.policy.breaker_stats().map(|stats| stats.outcomes),
        );
        let first = {
            let mut reads = self.read.lock().expect("no wake panics");
            reads.push(read);
            reads.len() == 1
        };
        if first {
            self.policy.close_breaker().expect("a breaker");
        }
    }
}

/// A subscription's reader is woken by the transition it waits for, on the
/// thread that made it and with no lock of the policy held: a waker reading
/// the breaker reads it, one closing it closes it, and that closing is told
/// after the opening it was made inside.
#[tokio::test(start_paused = true)]
async fn a_subscriptions_waker_reads_and_switches_the_breaker_whose_transition_woke_it() {
    use BreakerPhase::{Closed, Open};

    let (_app, policy) = crate::policy(ResilienceConfig::default()).await;
    let reading = Arc::new(Reading {
        policy: Arc::clone(&policy),
        read: Mutex::new(Vec::new()),
    });
    let (returned, done) = std::sync::mpsc::channel();
    let opening = {
        let (policy, reading) = (Arc::clone(&policy), Arc::clone(&reading));
        std::thread::spawn(move || {
            let mut events = policy.events();
            let waker = Waker::from(reading);
            let mut told = Vec::new();
            {
                let mut next = Box::pin(events.next());
                assert!(
                    next.as_mut()
                        .poll(&mut Context::from_waker(&waker))
                        .is_pending(),
                    "nothing told yet"
                );
                policy
                    .open_breaker(Duration::from_secs(60))
                    .expect("a breaker");
                let _ = returned.send(());
                // The closing's event wakes no reader: none waits since the opening's.
                let Poll::Ready(Some(first)) = next.as_mut().poll(&mut Context::from_waker(&waker))
                else {
                    panic!("the opening is told");
                };
                told.push(first);
            }
            told.extend(crate::told(&mut events));
            told
        })
    };
    if done.recv_timeout(Duration::from_secs(5)).is_err() {
        // Joined, the thread would hang the test past its verdict.
        std::mem::forget(opening);
        panic!("the reader's waker ran under a lock of the policy's");
    }
    let told: Vec<_> = opening
        .join()
        .expect("the opening's thread")
        .into_iter()
        .map(|event| match event {
            ResilienceEvent::Transition {
                from, to, manual, ..
            } => (from, to, manual),
            other => panic!("a transition: {other:?}"),
        })
        .collect();

    assert_eq!(
        told,
        [(Closed, Open, true), (Open, Closed, true)],
        "the opening, then the closing its reader made"
    );
    let read = reading.read.lock().expect("no wake panics").clone();
    assert!(
        matches!(read[..], [(Some(BreakerState::Open { .. }), Some(0))]),
        "woken once, by the opening, the breaker open then: {read:?}"
    );
    assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
}

/// A subscription left unread past the 256 events it holds is told how many
/// it missed, in the stream, before the oldest it kept and every one after.
#[tokio::test(start_paused = true)]
async fn a_subscription_that_fell_behind_is_told_how_many_events_it_missed() {
    const HELD: usize = 256;
    const MISSED: usize = 44;

    let (_app, policy) = crate::policy(ResilienceConfig::default()).await;
    let mut events = policy.events();
    policy
        .open_breaker(Duration::from_secs(10))
        .expect("a breaker");
    for _ in 1..HELD + MISSED {
        policy.close_breaker().expect("a breaker");
    }

    let mut told = told(&mut events).into_iter();
    assert!(
        matches!(told.next(), Some(ResilienceEvent::Lag { missed, .. }) if missed == MISSED as u64),
        "the opening and the first closings, dropped"
    );
    let kept: Vec<_> = told.collect();
    assert_eq!(kept.len(), HELD);
    assert!(
        kept.iter().all(|event| matches!(
            event,
            ResilienceEvent::Transition {
                from: BreakerPhase::Closed,
                to: BreakerPhase::Closed,
                manual: true,
                ..
            }
        )),
        "every closing after them"
    );
}

/// Its policy gone with its app, a subscription answers what it was told
/// before, then ends.
#[tokio::test(start_paused = true)]
async fn a_subscription_ends_once_its_policy_is_dropped() {
    let (app, policy) = crate::policy(ResilienceConfig::default()).await;
    let mut events = policy.events();
    policy.close_breaker().expect("a breaker");
    drop((app, policy));

    let ended = tokio::time::timeout(Duration::from_secs(1), async {
        (events.next().await, events.next().await)
    })
    .await
    .expect("the stream ends rather than waits");
    assert!(
        matches!(ended, (Some(ResilienceEvent::Transition { .. }), None)),
        "{ended:?}"
    );
}

/// An error whose `Display` counts each time it is asked.
#[derive(Debug)]
struct Watched(Arc<AtomicUsize>);

impl fmt::Display for Watched {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fetch_add(1, Ordering::Relaxed);
        f.write_str("the upstream refused")
    }
}

impl std::error::Error for Watched {}

/// A failure is put in words only when something tells it: with no line
/// filed and no subscription open, a failing call formats no chain, its
/// attempts' nor its fallback's; a retry's is said for its event alone, and a
/// fallback's once for its line and its event.
#[tokio::test(start_paused = true)]
async fn with_no_subscription_a_failing_call_formats_no_chain_for_an_event() {
    let (_app, service, policy) = quotes(ResilienceConfig {
        retries: Some(1),
        ..quiet()
    })
    .await;
    let said = Arc::new(AtomicUsize::new(0));
    let asked = || said.swap(0, Ordering::Relaxed);
    let failing = || {
        policy.run_idempotent(|_| {
            let said = Arc::clone(&said);
            async move { Err::<(), _>(Watched(said)) }
        })
    };

    assert!(failing().await.is_err());
    assert_eq!(asked(), 0, "nothing hears an attempt's chain");
    let mut events = policy.events();
    assert!(failing().await.is_err());
    assert_eq!(asked(), 1, "the retry's event, the one that hears it");
    assert!(matches!(
        &told(&mut events)[..],
        [ResilienceEvent::Retry { error: Some(error), .. }] if error == "the upstream refused"
    ));
    drop(events);

    let fallen = || service.watched(&said);
    assert!(matches!(fallen().await, Ok(7)));
    assert_eq!(asked(), 0, "nothing listens: no chain is said");
    let mut events = policy.events();
    assert!(matches!(fallen().await, Ok(7)));
    assert_eq!(asked(), 1, "the fallback's, for its event");
    let lines = LogCapture::install();
    assert!(matches!(fallen().await, Ok(7)));
    assert_eq!(
        asked(),
        1,
        "the fallback's chain said once for its line and its event"
    );
    assert_eq!(
        lines
            .find(TARGET, "the call ended on an error; its fallback answers")
            .len(),
        1
    );
    assert_eq!(told(&mut events).len(), 2, "a fallback each");
}

/// A subscription open costs a call answering at once nothing: no event is
/// told, so none is built.
#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "pins the allocations a run makes on Linux, where the dev container and CI run it"
)]
async fn a_call_answering_at_once_allocates_nothing_more_under_a_subscription() {
    let (_app, policy) = crate::policy(ResilienceConfig::default()).await;
    let mut events = policy.events();
    let token = crate::token_allocations();
    let boxed = usize::from(cfg!(debug_assertions));
    let (answered, made) = crate::allocations(|| policy.run(|_| async { Ok::<_, Down>(7) }));
    assert!(matches!(answered, Ok(7)));
    assert_eq!(made, token + boxed);
    assert_eq!(told(&mut events), []);
}

/// A mechanism that is off reads `None`, where one that is on and idle reads
/// its zero.
#[tokio::test(start_paused = true)]
async fn a_read_is_none_for_a_mechanism_that_is_off() {
    let (_app, off) = crate::policy(quiet()).await;
    assert_eq!(
        (
            off.breaker_state(),
            off.breaker_stats(),
            off.concurrency_running(),
            off.concurrency_waiting(),
            off.rate_available(),
        ),
        (None, None, None, None, None)
    );

    let (_app, on) = crate::policy(ResilienceConfig {
        concurrency: Some(4),
        rate_limit: Some(9),
        ..ResilienceConfig::default()
    })
    .await;
    let stats = on.breaker_stats().expect("a breaker");
    assert_eq!((stats.outcomes, stats.failures), (0, 0));
    assert_eq!(
        (
            on.concurrency_running(),
            on.concurrency_waiting(),
            on.rate_available()
        ),
        (Some(0), Some(0), Some(9))
    );
}

/// What the window counts, and its failures' share.
fn stats(policy: &ResiliencePolicy) -> (usize, usize, f64) {
    let stats = policy.breaker_stats().expect("a breaker");
    (stats.outcomes, stats.failures, stats.failure_rate())
}

/// The window is read as it counts while the breaker is closed, forgotten as
/// it opens, empty while it is open and half-open, and afresh once closed.
#[tokio::test(start_paused = true)]
async fn the_breakers_window_is_read_at_each_state() {
    let (_app, policy) = crate::policy(ResilienceConfig {
        retries: None,
        breaker_window: Some(4),
        breaker_failures: 2,
        breaker_open: Duration::from_secs(30),
        ..ResilienceConfig::default()
    })
    .await;
    let ok = || policy.run(|_| async { Ok::<_, Down>(()) });
    let fail = || policy.run(|_| async { Err::<(), _>(Down) });

    assert_eq!(
        stats(&policy),
        (0, 0, 0.0),
        "a rate of none over no outcome"
    );
    assert!(ok().await.is_ok() && fail().await.is_err());
    assert_eq!(stats(&policy), (2, 1, 0.5));
    assert!(ok().await.is_ok() && ok().await.is_ok() && ok().await.is_ok());
    assert_eq!(stats(&policy), (4, 1, 0.25), "the window's four latest");
    assert!(fail().await.is_err());
    assert_eq!(
        stats(&policy),
        (4, 1, 0.25),
        "the first failure left the window as this one came"
    );

    assert!(fail().await.is_err());
    assert!(matches!(
        policy.breaker_state(),
        Some(BreakerState::Open { .. })
    ));
    assert_eq!(stats(&policy), (0, 0, 0.0), "forgotten as it opened");
    tokio::time::advance(Duration::from_secs(30)).await;
    assert_eq!(policy.breaker_state(), Some(BreakerState::HalfOpen));
    assert_eq!(stats(&policy), (0, 0, 0.0));

    assert!(ok().await.is_ok(), "the probe");
    assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
    assert_eq!(stats(&policy), (0, 0, 0.0), "a probe counts in no window");
    assert!(fail().await.is_err());
    assert_eq!(stats(&policy), (1, 1, 1.0));
}

/// An outcome past its age counts no more in the read, though no outcome came
/// since to forget it.
#[tokio::test(start_paused = true)]
async fn an_outcome_past_its_age_is_read_no_more() {
    let (_app, policy) = crate::policy(ResilienceConfig {
        retries: None,
        breaker_window: Some(4),
        breaker_failures: 4,
        breaker_age: Some(Duration::from_secs(10)),
        ..ResilienceConfig::default()
    })
    .await;
    assert!(policy.run(|_| async { Err::<(), _>(Down) }).await.is_err());
    tokio::time::advance(Duration::from_secs(6)).await;
    assert!(policy.run(|_| async { Ok::<_, Down>(()) }).await.is_ok());
    assert_eq!(stats(&policy), (2, 1, 0.5));

    tokio::time::advance(Duration::from_secs(5)).await;
    assert_eq!(
        stats(&policy),
        (1, 0, 0.0),
        "the failure, eleven seconds old"
    );
    tokio::time::advance(Duration::from_secs(6)).await;
    assert_eq!(stats(&policy), (0, 0, 0.0));
}

/// The bulkhead reads the calls holding a slot and the ones waiting for one,
/// as each takes its slot, waits, and leaves, each call in one read at every
/// instant: a waiter the freed slot is handed to is read as waiting until its
/// own future takes it.
#[tokio::test(start_paused = true)]
async fn the_bulkhead_reads_the_calls_running_and_the_ones_waiting() {
    let (_app, policy) = crate::policy(ResilienceConfig {
        concurrency: Some(2),
        concurrency_wait: Some(Duration::from_secs(60)),
        ..quiet()
    })
    .await;
    let read = || {
        (
            policy.concurrency_running().expect("a bulkhead"),
            policy.concurrency_waiting().expect("a bulkhead"),
        )
    };

    let first = parked(policy.run(|_| silent()));
    assert_eq!(read(), (1, 0));
    let second = parked(policy.run(|_| silent()));
    let mut waiting = parked(policy.run(|_| async { Ok::<_, Down>(()) }));
    assert_eq!(read(), (2, 1), "both slots held, and one call waiting");

    drop(first);
    assert_eq!(
        read(),
        (1, 1),
        "two calls alive: the slot freed is the waiter's, which has yet to take it"
    );
    assert!(waiting.as_mut().await.is_ok(), "it took the slot freed");
    assert_eq!(read(), (1, 0), "it ran, and left");
    drop(second);
    assert_eq!(read(), (0, 0));
}

/// The rate reads the tokens free now: fewer by each attempt sent, more by
/// each one refilled, the bucket's limit at most.
#[tokio::test(start_paused = true)]
async fn the_rate_reads_the_tokens_free_after_sends_and_a_refill() {
    let (_app, policy) = crate::policy(ResilienceConfig {
        rate_limit: Some(3),
        rate_window: Duration::from_secs(3),
        ..quiet()
    })
    .await;
    let send = || policy.run(|_| async { Ok::<_, Down>(()) });

    assert_eq!(policy.rate_available(), Some(3));
    assert!(send().await.is_ok() && send().await.is_ok());
    assert_eq!(policy.rate_available(), Some(1));
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(policy.rate_available(), Some(2), "one token a second");
    assert!(send().await.is_ok() && send().await.is_ok());
    assert_eq!(policy.rate_available(), Some(0));
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(policy.rate_available(), Some(3), "the bucket holds no more");
}

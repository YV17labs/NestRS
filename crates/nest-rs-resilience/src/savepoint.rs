//! The savepoint an attempt runs in: a call declared idempotent, sent again
//! after an attempt failed, and a method declaring a fallback, answered by it,
//! must leave nothing a failed attempt wrote — for the next to repeat, or
//! beside the error the run ends on. Each of their attempts runs in a
//! savepoint of the transaction its unit of work holds, opened through the
//! data port and settled before the next attempt is let through and before the
//! run answers: kept when the run answers the attempt's `Ok`, rolled back
//! otherwise. The declaration alone decides, never the retries a deployment
//! sets: with none, a failed idempotent call still leaves no write. No unit of
//! work, or one holding no transaction — a pool, where each statement commits
//! on its own — and the attempt runs as it would with no policy.

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use nest_rs_database::__private::InstalledSavepoint;
use nest_rs_database::AtomicError;

use crate::policy::Retry;

/// One attempt's savepoint, of the transaction the ambient executor holds.
pub(crate) struct Savepoint(InstalledSavepoint);

impl Savepoint {
    /// A savepoint of the ambient executor's transaction, read where the
    /// attempt is sent, or `None` where there is none to open. Nothing is sent
    /// to the database until the attempt's first statement.
    pub(crate) fn open() -> Option<Self> {
        InstalledSavepoint::open().map(Self)
    }

    /// `call` polled with the savepoint as the ambient executor, so what it
    /// reads and writes through the data layer runs inside. Built at each poll
    /// of the attempt, it derives nothing: the savepoint was installed once, as
    /// it was opened.
    pub(crate) fn around<'a, F: Future>(&'a self, call: Pin<&'a mut F>) -> Around<'a, F> {
        Around {
            savepoint: &self.0,
            call,
        }
    }

    /// The savepoint settling, released when `keep` and rolled back otherwise,
    /// `ended` held until it has. The attempt's future is gone by then, with
    /// every handle it took on the savepoint.
    pub(crate) fn settle<T>(self, keep: bool, ended: Ended<T>) -> Box<Settling<T>> {
        Box::new(Settling {
            settle: self.0.settle(keep),
            ended: Some(ended),
        })
    }
}

/// What [`Savepoint::around`] answers: one poll of an attempt's call inside
/// its savepoint.
pub(crate) struct Around<'a, F> {
    savepoint: &'a InstalledSavepoint,
    call: Pin<&'a mut F>,
}

impl<F: Future> Future for Around<'_, F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let this = &mut *self;
        this.savepoint.enter(|| this.call.as_mut().poll(cx))
    }
}

/// What an attempt ended on, held while its savepoint settles.
pub(crate) enum Ended<T> {
    /// What the run answers.
    Answer(T),
    /// A failure the run goes on after: its call is sent again, as the retry
    /// decided then says.
    Again(Retry),
    /// A panic, which unwinds on once the savepoint is rolled back.
    Unwound(Box<dyn Any + Send>),
}

/// A savepoint settling before its run goes on, in a box of its own: a run
/// that opens none reserves no room for the answer one holds.
pub(crate) struct Settling<T> {
    settle: Pin<Box<dyn Future<Output = Result<(), AtomicError>> + Send>>,
    ended: Option<Ended<T>>,
}

impl<T> Settling<T> {
    /// What the attempt ended on, once its savepoint settled, and how it did.
    pub(crate) fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<(Ended<T>, Result<(), AtomicError>)> {
        let settled = ready!(self.settle.as_mut().poll(cx));
        let Some(ended) = self.ended.take() else {
            unreachable!("a savepoint is polled after it settled");
        };
        Poll::Ready((ended, settled))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use nest_rs_core::instance;
    use nest_rs_database::__private::with_request_executor;
    use nest_rs_database::{AtomicBlock, Executor, current_executor};

    use super::*;
    use crate::policy::{NOT_ATTEMPTED_AGAIN_UNSETTLED, RETRYING_FAILED};
    use crate::resilient::{judging_by_default, resilient_method};
    use crate::{ResilienceConfig, ResilienceError, ResiliencePolicy};

    #[instance]
    struct Ledger;

    #[derive(Debug, PartialEq, Eq, thiserror::Error)]
    #[error("the ledger refused the entry")]
    struct Refused;

    type Answer = Result<u32, ResilienceError<Refused>>;

    /// What a unit, its savepoints and a call saw, in order.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Saw {
        Opened,
        /// A call polled, inside a savepoint or on what it would run on with
        /// no policy.
        Called {
            inside: bool,
        },
        /// The future a call made, dropped.
        Spent,
        Kept,
        RolledBack,
        /// A savepoint dropped before it settled.
        Abandoned,
    }

    #[derive(Default)]
    struct Seen(Mutex<Vec<Saw>>);

    impl Seen {
        fn saw(&self, saw: Saw) {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(saw);
        }

        fn all(&self) -> Vec<Saw> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    /// How long a savepoint takes to settle: a round trip the run waits out.
    const SETTLES_IN: Duration = Duration::from_millis(60);

    /// How a savepoint of these tests settles.
    type Settles = fn() -> Result<(), AtomicError>;

    /// A unit of work holding a transaction: its driver offers a savepoint,
    /// settling as `settles` answers.
    struct Unit {
        seen: Arc<Seen>,
        settles: Settles,
    }

    impl Unit {
        fn settling(settles: Settles) -> (Arc<dyn Executor>, Arc<Seen>) {
            let seen = Arc::new(Seen::default());
            let unit = Arc::new(Self {
                seen: Arc::clone(&seen),
                settles,
            });
            (unit, seen)
        }
    }

    impl Executor for Unit {
        fn as_any(&self) -> &dyn Any {
            self
        }

        fn savepoint(&self) -> Option<Arc<dyn AtomicBlock>> {
            self.seen.saw(Saw::Opened);
            Some(Arc::new(Block {
                seen: Arc::clone(&self.seen),
                settles: self.settles,
                settled: AtomicBool::new(false),
            }))
        }
    }

    struct Block {
        seen: Arc<Seen>,
        settles: Settles,
        settled: AtomicBool,
    }

    impl Executor for Block {
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    impl AtomicBlock for Block {
        fn settle(
            self: Arc<Self>,
            ok: bool,
        ) -> Pin<Box<dyn Future<Output = Result<(), AtomicError>> + Send>> {
            self.settled.store(true, Ordering::SeqCst);
            Box::pin(async move {
                tokio::time::sleep(SETTLES_IN).await;
                self.seen.saw(if ok { Saw::Kept } else { Saw::RolledBack });
                (self.settles)()
            })
        }
    }

    impl Drop for Block {
        fn drop(&mut self) {
            if !self.settled.load(Ordering::SeqCst) {
                self.seen.saw(Saw::Abandoned);
            }
        }
    }

    /// A pool: a unit of work whose driver holds no transaction.
    struct Pool;

    impl Executor for Pool {
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    fn settles() -> Result<(), AtomicError> {
        Ok(())
    }

    fn refuses() -> Result<(), AtomicError> {
        Err(AtomicError::Database("ROLLBACK TO refused".into()))
    }

    /// The attempt answered `Ok` over a statement that failed inside it.
    fn poisoned() -> Result<(), AtomicError> {
        Err(AtomicError::Poisoned { retryable: false })
    }

    /// Says [`Saw::Spent`] when the future holding it is dropped.
    struct Spending(Arc<Seen>);

    impl Drop for Spending {
        fn drop(&mut self) {
            self.0.saw(Saw::Spent);
        }
    }

    /// One poll of a call, said with where it runs.
    fn called(seen: &Seen) {
        let inside = current_executor().is_some_and(|ambient| ambient.as_any().is::<Block>());
        seen.saw(Saw::Called { inside });
    }

    /// A call answering `answers` in turn, one per attempt, holding what says
    /// its future was dropped.
    fn answering<'a>(
        seen: &'a Arc<Seen>,
        answers: &'a [Result<u32, Refused>],
    ) -> impl FnMut(
        crate::CancellationToken,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<u32, Refused>> + Send + 'a>> {
        let attempts = AtomicU32::new(0);
        move |_| {
            let attempt = attempts.fetch_add(1, Ordering::Relaxed) as usize;
            Box::pin(async move {
                let _spending = Spending(Arc::clone(seen));
                called(seen);
                tokio::task::yield_now().await;
                match answers.get(attempt) {
                    Some(Ok(answer)) => Ok(*answer),
                    Some(Err(Refused)) | None => Err(Refused),
                }
            })
        }
    }

    /// Retries on, no breaker: the subject is the savepoint.
    fn retrying(retries: Option<usize>) -> ResiliencePolicy<Ledger> {
        ResiliencePolicy::new(&ResilienceConfig {
            timeout: Duration::from_secs(1),
            retries,
            backoff_first: Duration::from_millis(100),
            backoff_ceiling: Duration::from_millis(200),
            breaker_window: None,
            ..ResilienceConfig::default()
        })
        .expect("a valid policy")
    }

    const ROLLED_BACK_THEN_KEPT: [Saw; 8] = [
        Saw::Opened,
        Saw::Called { inside: true },
        Saw::Spent,
        Saw::RolledBack,
        Saw::Opened,
        Saw::Called { inside: true },
        Saw::Spent,
        Saw::Kept,
    ];

    #[tokio::test(start_paused = true)]
    async fn each_attempt_of_a_call_sent_again_runs_in_a_savepoint_settled_before_the_next() {
        let policy = retrying(Some(2));
        let (unit, seen) = Unit::settling(settles);

        let ran = with_request_executor(
            unit,
            policy.run_idempotent(answering(&seen, &[Err(Refused), Ok(7)])),
        )
        .await;

        assert!(matches!(ran, Ok(7)), "{ran:?}");
        assert_eq!(
            seen.all(),
            ROLLED_BACK_THEN_KEPT,
            "the failed attempt is rolled back, its future gone, before the next is made, and the \
             one answering kept before the run answers"
        );
    }

    /// The rollback runs inside the backoff, which counts from the attempt's
    /// answer: a run waits the longer of the two, never their sum.
    #[tokio::test(start_paused = true)]
    async fn a_rollback_is_waited_inside_the_backoff_never_after_it() {
        let policy = retrying(Some(1));
        let (unit, seen) = Unit::settling(settles);
        let started = tokio::time::Instant::now();
        let retried = Mutex::new(None);

        let ran = with_request_executor(
            unit,
            policy.run_idempotent(|_| async {
                called(&seen);
                let mut retried = retried
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if retried.is_none() && seen.all().contains(&Saw::RolledBack) {
                    *retried = Some(started.elapsed());
                    return Ok(7);
                }
                Err(Refused)
            }),
        )
        .await;

        assert!(matches!(ran, Ok(7)), "{ran:?}");
        let retried = retried
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .expect("the call was sent again once its savepoint was rolled back");
        assert!(
            SETTLES_IN < retried && retried <= Duration::from_millis(120),
            "the retry is sent at its backoff, 100 ms within a fifth, the {SETTLES_IN:?} of the \
             rollback inside it: {retried:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_run_whose_last_attempt_fails_rolls_it_back_and_answers_its_error() {
        let policy = retrying(Some(1));
        let (unit, seen) = Unit::settling(settles);

        let ran = with_request_executor(unit, policy.run_idempotent(answering(&seen, &[]))).await;

        assert!(
            matches!(ran, Err(ResilienceError::Failed(Refused))),
            "{ran:?}"
        );
        assert_eq!(
            seen.all(),
            [
                Saw::Opened,
                Saw::Called { inside: true },
                Saw::Spent,
                Saw::RolledBack,
                Saw::Opened,
                Saw::Called { inside: true },
                Saw::Spent,
                Saw::RolledBack,
            ]
        );
    }

    /// A call sent once opens nothing and reads nothing of its unit, whatever
    /// retries its policy has: its `Err` leaves what its body wrote.
    #[tokio::test(start_paused = true)]
    async fn a_call_sent_once_opens_no_savepoint() {
        for retries in [None, Some(2)] {
            let (unit, seen) = Unit::settling(settles);
            let call = answering(&seen, &[]);
            let once = with_request_executor(unit, retrying(retries).run(call)).await;
            assert!(
                matches!(once, Err(ResilienceError::Failed(Refused))),
                "{once:?}"
            );
            assert_eq!(
                seen.all(),
                [Saw::Called { inside: false }, Saw::Spent],
                "retries: {retries:?}"
            );
        }
    }

    /// The savepoint is the declaration's, never the retries' a deployment
    /// sets: with none, an idempotent call's one attempt is rolled back when
    /// it failed and kept when it answered, as its last is with retries on.
    #[tokio::test(start_paused = true)]
    async fn an_idempotent_call_whose_retries_are_off_runs_its_one_attempt_in_a_savepoint() {
        let policy = retrying(None);
        for (answers, settled) in [
            (&[Err(Refused)][..], Saw::RolledBack),
            (&[Ok(7)][..], Saw::Kept),
        ] {
            let (unit, seen) = Unit::settling(settles);
            let ran =
                with_request_executor(unit, policy.run_idempotent(answering(&seen, answers))).await;
            assert_eq!(ran.is_ok(), settled == Saw::Kept, "{ran:?}");
            assert_eq!(
                seen.all(),
                [
                    Saw::Opened,
                    Saw::Called { inside: true },
                    Saw::Spent,
                    settled
                ],
                "not attempted again, and settled before the run answers"
            );
        }
    }

    /// A transport's send opens none, whatever retries its policy has: what
    /// `ResilienceMiddleware` sends writes nothing of its caller's unit.
    #[cfg(feature = "reqwest")]
    #[tokio::test(start_paused = true)]
    async fn a_transports_attempt_opens_no_savepoint() {
        let policy = retrying(Some(2));
        let (unit, seen) = Unit::settling(settles);

        with_request_executor(unit, async {
            let run = policy
                .begin(None)
                .await
                .expect("the policy has no bulkhead");
            let (attempted, _outstanding) = run
                .attempt_once(
                    |_| async {
                        called(&seen);
                        Ok::<u32, Refused>(7)
                    },
                    |_| crate::policy::Verdict::Succeeded,
                )
                .await;
            assert!(matches!(attempted, Ok((Ok(7), _))), "{attempted:?}");
        })
        .await;

        assert_eq!(seen.all(), [Saw::Called { inside: false }]);
    }

    /// A run holds its answer while its savepoint settles, by value and never
    /// by reference: it stays `Send` for an answer that is `Send` and not
    /// `Sync`, a stream's.
    #[test]
    fn a_run_stays_send_for_an_answer_that_is_send_and_not_sync() {
        fn send<F: Send>(_run: &F) {}

        let policy = retrying(Some(2));
        let answer = |_| async { Ok::<_, Refused>(std::cell::Cell::new(7)) };
        send(&policy.run(answer));
        send(&policy.run_idempotent(answer));
    }

    fn host() -> &'static str {
        "ledger::LedgerService"
    }

    /// A method declaring a fallback goes on after its one attempt failed: that
    /// attempt is rolled back before the run answers what the fallback reads.
    #[tokio::test(start_paused = true)]
    async fn a_method_declaring_a_fallback_runs_its_one_attempt_in_a_savepoint() {
        let policy = retrying(None);
        let falling_back = resilient_method(host, "post", None, None).with_fallback();
        let plain = resilient_method(host, "post", None, None);

        for (answers, settled) in [
            (&[Err(Refused)][..], Saw::RolledBack),
            (&[Ok(7)][..], Saw::Kept),
        ] {
            let (unit, seen) = Unit::settling(settles);
            let call = answering(&seen, answers);
            let ran: Answer = with_request_executor(
                unit,
                policy.run_once::<Answer, _, _, _>(Some(falling_back), judging_by_default(), call),
            )
            .await;
            assert_eq!(ran.is_ok(), settled == Saw::Kept, "{ran:?}");
            assert_eq!(
                seen.all(),
                [
                    Saw::Opened,
                    Saw::Called { inside: true },
                    Saw::Spent,
                    settled
                ]
            );
        }

        let (unit, seen) = Unit::settling(settles);
        let call = answering(&seen, &[]);
        let ran: Answer = with_request_executor(
            unit,
            policy.run_once::<Answer, _, _, _>(Some(plain), judging_by_default(), call),
        )
        .await;
        assert!(ran.is_err(), "{ran:?}");
        assert_eq!(
            seen.all(),
            [Saw::Called { inside: false }, Saw::Spent],
            "a method declaring none, sent once, opens nothing"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_attempt_given_up_at_its_bound_is_rolled_back_before_its_retry_is_made() {
        let policy = retrying(Some(1));
        let (unit, seen) = Unit::settling(settles);
        let attempts = AtomicU32::new(0);

        let ran = with_request_executor(
            unit,
            policy.run_idempotent(|_| async {
                let _spending = Spending(Arc::clone(&seen));
                called(&seen);
                if attempts.fetch_add(1, Ordering::Relaxed) == 0 {
                    std::future::pending::<()>().await;
                }
                Ok::<_, Refused>(7)
            }),
        )
        .await;

        assert!(matches!(ran, Ok(7)), "{ran:?}");
        assert_eq!(seen.all(), ROLLED_BACK_THEN_KEPT);
    }

    #[tokio::test(start_paused = true)]
    async fn a_last_attempt_given_up_at_its_bound_is_rolled_back_before_the_run_answers() {
        let policy = retrying(Some(1));
        let (unit, seen) = Unit::settling(settles);

        let ran = with_request_executor(
            unit,
            policy.run_idempotent(|_| async {
                called(&seen);
                std::future::pending::<Result<u32, Refused>>().await
            }),
        )
        .await;

        assert!(
            matches!(ran, Err(ResilienceError::TimedOut { .. })),
            "{ran:?}"
        );
        assert_eq!(
            seen.all(),
            [
                Saw::Opened,
                Saw::Called { inside: true },
                Saw::RolledBack,
                Saw::Opened,
                Saw::Called { inside: true },
                Saw::RolledBack,
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_panicking_attempt_is_rolled_back_and_unwinds_on() {
        let policy = retrying(Some(2));
        let (unit, seen) = Unit::settling(settles);
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let unwound = nest_rs_core::panic::contain(with_request_executor(
            unit,
            policy.run_idempotent(|_| async {
                let _spending = Spending(Arc::clone(&seen));
                called(&seen);
                if seen.all().len() > 1 {
                    panic!("inside the attempt");
                }
                Ok::<u32, Refused>(7)
            }),
        ))
        .await;
        std::panic::set_hook(previous);

        let payload = unwound.expect_err("the panic unwinds past the run");
        assert_eq!(
            nest_rs_core::panic::panic_message(payload.as_ref()),
            "inside the attempt"
        );
        assert_eq!(
            seen.all(),
            [
                Saw::Opened,
                Saw::Called { inside: true },
                Saw::Spent,
                Saw::RolledBack
            ],
            "rolled back before the unwind goes on, and not attempted again"
        );
    }

    /// Nothing can be awaited as a run is dropped: its savepoint goes as the
    /// port says of a block dropped before it settled, never kept, once the
    /// attempt's future is gone.
    #[tokio::test(start_paused = true)]
    async fn a_run_dropped_mid_attempt_leaves_its_savepoint_unsettled_once_its_call_is_gone() {
        let policy = retrying(Some(2));
        let (unit, seen) = Unit::settling(settles);
        {
            let mut run = std::pin::pin!(with_request_executor(
                unit,
                policy.run_idempotent(|_| async {
                    let _spending = Spending(Arc::clone(&seen));
                    called(&seen);
                    std::future::pending::<Result<u32, Refused>>().await
                }),
            ));
            let polled = std::future::poll_fn(|cx| Poll::Ready(run.as_mut().poll(cx))).await;
            assert!(polled.is_pending());
        }
        assert_eq!(
            seen.all(),
            [
                Saw::Opened,
                Saw::Called { inside: true },
                Saw::Spent,
                Saw::Abandoned
            ]
        );
    }

    /// A savepoint the database could not settle ends the run whatever its
    /// attempt answered: an `Ok` whose writes are not kept is no answer — one
    /// answered over a statement that failed inside it least of all — and a
    /// failure whose writes may still stand is not sent again over them. The
    /// run's caller holds the cause, whose chain the policy's line tells as
    /// its `error`, once, announcing no retry it does not make.
    #[tokio::test(start_paused = true)]
    async fn a_savepoint_that_cannot_be_settled_ends_the_run_there_on_its_cause() {
        for (answers, attempted, settles, cause) in [
            (&[Ok(7)][..], Saw::Kept, refuses as Settles, refuses()),
            (&[][..], Saw::RolledBack, refuses, refuses()),
            (&[Ok(7)][..], Saw::Kept, poisoned, poisoned()),
        ] {
            let cause =
                nest_rs_core::error_message(&cause.expect_err("a savepoint left unsettled"));
            let lines = nest_rs_testing::LogCapture::install();
            let policy = retrying(Some(2));
            let (unit, seen) = Unit::settling(settles);

            let ran =
                with_request_executor(unit, policy.run_idempotent(answering(&seen, answers))).await;

            let unsettled = ran.expect_err("the run ends on its savepoint");
            assert!(
                matches!(unsettled, ResilienceError::Unsettled { .. }),
                "{unsettled:?}"
            );
            assert_eq!(
                nest_rs_core::error_message(&unsettled),
                format!("an attempt's savepoint could not be settled: {cause}")
            );
            assert_eq!(
                seen.all(),
                [
                    Saw::Opened,
                    Saw::Called { inside: true },
                    Saw::Spent,
                    attempted
                ],
                "not attempted again"
            );
            let line = lines.expect_one(crate::TARGET, NOT_ATTEMPTED_AGAIN_UNSETTLED);
            assert_eq!(line.level, "debug", "as every retry given up");
            assert_eq!(line.field("instance").as_deref(), Some("ledger"));
            assert_eq!(
                line.field("error").as_deref(),
                Some(cause.as_str()),
                "no driver owes a line saying why"
            );
            lines.expect_none(crate::TARGET, RETRYING_FAILED);
        }
    }

    /// The retry's line says a retry that is certain: filed once the failed
    /// attempt's savepoint was rolled back, never while that is awaited.
    #[tokio::test(start_paused = true)]
    async fn the_retrys_line_is_filed_once_its_attempts_savepoint_was_rolled_back() {
        let lines = nest_rs_testing::LogCapture::install();
        let policy = retrying(Some(2));
        let (unit, seen) = Unit::settling(settles);
        let mut run = std::pin::pin!(with_request_executor(
            unit,
            policy.run_idempotent(answering(&seen, &[Err(Refused), Ok(7)])),
        ));

        let rolling_back = tokio::time::timeout(SETTLES_IN / 2, run.as_mut()).await;
        assert!(rolling_back.is_err(), "the rollback is still awaited");
        assert_eq!(
            seen.all(),
            [Saw::Opened, Saw::Called { inside: true }, Saw::Spent]
        );
        lines.expect_none(crate::TARGET, RETRYING_FAILED);

        let ran = run.await;
        assert!(matches!(ran, Ok(7)), "{ran:?}");
        let line = lines.expect_one(crate::TARGET, RETRYING_FAILED);
        assert_eq!(line.field("attempt").as_deref(), Some("1"));
        assert_eq!(
            line.field("error").as_deref(),
            Some("the ledger refused the entry")
        );
    }

    /// The line names the decorated method the run came from and says the
    /// cause, and a panic still unwinds on over a savepoint left unsettled.
    #[tokio::test(start_paused = true)]
    async fn an_unsettled_savepoint_names_its_method_and_never_holds_a_panic_back() {
        let lines = nest_rs_testing::LogCapture::install();
        let policy = retrying(Some(2));
        let method = resilient_method(host, "post", None, None);
        let (unit, _seen) = Unit::settling(refuses);
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let unwound = nest_rs_core::panic::contain(with_request_executor(
            unit,
            policy.run_again::<Answer, _, _, _>(Some(method), judging_by_default(), |_| async {
                tokio::task::yield_now().await;
                panic!("inside the attempt")
            }),
        ))
        .await;
        std::panic::set_hook(previous);

        assert!(unwound.is_err(), "the panic unwinds past the run");
        let line = lines.expect_one(crate::TARGET, NOT_ATTEMPTED_AGAIN_UNSETTLED);
        assert_eq!(line.field("host").as_deref(), Some("LedgerService"));
        assert_eq!(line.field("method").as_deref(), Some("post"));
        assert_eq!(
            line.field("error"),
            refuses()
                .err()
                .map(|cause| nest_rs_core::error_message(&cause)),
            "the one place the cause is said: the panic's caller holds no error"
        );
    }

    /// With no unit of work, or one on a pool, each attempt runs as it would
    /// with no policy: on what is ambient where the run is polled.
    #[tokio::test(start_paused = true)]
    async fn outside_a_transaction_each_attempt_runs_as_it_did() {
        let policy = retrying(Some(2));
        let on_pool = |ambient: Option<Arc<dyn Executor>>| {
            ambient.is_some_and(|ambient| ambient.as_any().is::<Pool>())
        };

        let attempts = AtomicU32::new(0);
        let bare = policy
            .run_idempotent(|_| async {
                let ambient = current_executor();
                if attempts.fetch_add(1, Ordering::Relaxed) == 0 {
                    return Err(Refused);
                }
                Ok(ambient.is_none())
            })
            .await;
        assert!(
            matches!(bare, Ok(true)),
            "no executor is installed: {bare:?}"
        );

        let attempts = AtomicU32::new(0);
        let pooled = with_request_executor(
            Arc::new(Pool),
            policy.run_idempotent(|_| async {
                let ambient = current_executor();
                if attempts.fetch_add(1, Ordering::Relaxed) == 0 {
                    return Err(Refused);
                }
                Ok(on_pool(ambient))
            }),
        )
        .await;
        assert!(
            matches!(pooled, Ok(true)),
            "the pool stays ambient: {pooled:?}"
        );
    }
}

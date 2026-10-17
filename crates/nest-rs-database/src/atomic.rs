use std::any::Any;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use crate::error::AtomicError;
use crate::executor::{Executor, current_executor, with_executor};

/// Run `work` atomically: inside a savepoint of the ambient transaction, or in
/// a transaction of its own when the ambient executor holds none. `Ok` keeps
/// its writes and hands its after-commit work to the enclosing unit; `Err`, a
/// panic, or a statement that failed inside rolls the block back alone and
/// drops its after-commit work. Nothing is sent to the database until the
/// block's first statement, so a block that never queries costs no round trip.
///
/// ```
/// use nest_rs_database::{AtomicError, atomic};
///
/// #[derive(Debug)]
/// enum SignUpError {
///     Atomic(AtomicError),
/// }
///
/// impl From<AtomicError> for SignUpError {
///     fn from(err: AtomicError) -> Self {
///         Self::Atomic(err)
///     }
/// }
///
/// # #[nest_rs_core::main]
/// # async fn main() {
/// let outcome = atomic(async { Ok::<_, SignUpError>("written") }).await;
/// assert!(matches!(
///     outcome,
///     Err(SignUpError::Atomic(AtomicError::Unsupported)),
/// ));
/// # }
/// ```
///
/// The block reads the ambient executor where it is first polled, runs `work`
/// with the block installed in its place under the same scope, and leaves the
/// ability untouched. Outside any unit of work, or on a driver that offers no
/// atomic block, it answers [`AtomicError::Unsupported`] without polling
/// `work`.
///
/// A unit of work the framework opened waits for an open block: its own
/// statements queue until the block settles, since a statement issued beside
/// the block would land inside its savepoint and roll back with it. A
/// statement issued from inside a block on an enclosing unit's executor would
/// wait for itself, so the driver refuses it. A long block holds its unit for
/// its whole length. A transaction the caller opened and installed itself is
/// not held back: a statement the caller issues on it beside an open block
/// lands inside the block and rolls back with it, so await the block first.
///
/// A block dropped before it settles — cancelled by a timeout or a `select!` —
/// rolls back; inside a unit of work it leaves that unit unable to commit, so
/// nothing the unit wrote stands. A block of its own on a pool unit dropped
/// while it commits leaves the `COMMIT` to finish on its driver's task, and a
/// unit whose body failed without seeing its answer is in doubt
/// ([`Unhonoured::IN_DOUBT`](crate::Unhonoured::IN_DOUBT)).
pub fn atomic<'a, T, E, F>(work: F) -> impl Future<Output = Result<T, E>> + Send + 'a
where
    F: Future<Output = Result<T, E>> + Send + 'a,
    T: Send + 'a,
    E: From<AtomicError> + Send + 'a,
{
    // Not an `async fn`: its state would keep `work` beside the future built
    // from it, and nested blocks would weigh twice their size at every level.
    Atomic::Unopened(work)
}

/// The executor installed for an atomic block's duration, settled once by
/// [`atomic`](fn@atomic).
///
/// A driver returns one from [`Executor::atomic`]: its statements run inside
/// the block, and the work [`after_commit`](crate::after_commit) hands it is
/// held until the block settles.
pub trait AtomicBlock: Executor {
    /// Keep the block's writes and hand its held after-commit work to the
    /// enclosing unit when `ok`, or roll them back and drop that work. Called
    /// once, after the block's work ended, whatever way it ended.
    fn settle(
        self: Arc<Self>,
        ok: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), AtomicError>> + Send>>;
}

/// What [`atomic`](fn@atomic) returns.
enum Atomic<'a, F, T, E> {
    /// Not polled yet: the block is opened where it first runs.
    Unopened(F),
    /// The work runs with the block installed. `work` is declared first so a
    /// cancelled block drops it, with every handle it took on the block, before
    /// the block itself.
    Running {
        work: Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>,
        block: Arc<dyn AtomicBlock>,
    },
    /// The work ended; the block settles before its result is handed back.
    Settling {
        settle: Pin<Box<dyn Future<Output = Result<(), AtomicError>> + Send>>,
        ended: Ended<T, E>,
    },
    Done,
}

// `work` is never pinned in place: it moves into a box of its own on the first
// poll, so the future stays movable whatever `work` is.
impl<F, T, E> Unpin for Atomic<'_, F, T, E> {}

/// How the block's work ended.
enum Ended<T, E> {
    Returned(Result<T, E>),
    Panicked(Box<dyn Any + Send>),
}

impl<'a, F, T, E> Future for Atomic<'a, F, T, E>
where
    F: Future<Output = Result<T, E>> + Send + 'a,
    T: Send + 'a,
    E: From<AtomicError> + Send + 'a,
{
    type Output = Result<T, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        loop {
            match std::mem::replace(this, Atomic::Done) {
                Atomic::Unopened(work) => {
                    let Some(block) = current_executor().and_then(|executor| executor.atomic())
                    else {
                        return Poll::Ready(Err(AtomicError::Unsupported.into()));
                    };
                    let installed: Arc<dyn Executor> = block.clone();
                    *this = Atomic::Running {
                        work: Box::pin(with_executor(installed, work)),
                        block,
                    };
                }
                Atomic::Running { mut work, block } => {
                    // Caught to settle the block, then resumed as it was: a
                    // unit around this one still contains and reports it.
                    let ended = match catch_unwind(AssertUnwindSafe(|| work.as_mut().poll(cx))) {
                        Ok(Poll::Pending) => {
                            *this = Atomic::Running { work, block };
                            return Poll::Pending;
                        }
                        Ok(Poll::Ready(result)) => Ended::Returned(result),
                        Err(payload) => Ended::Panicked(payload),
                    };
                    drop(work);
                    let ok = matches!(ended, Ended::Returned(Ok(_)));
                    *this = Atomic::Settling {
                        settle: block.settle(ok),
                        ended,
                    };
                }
                Atomic::Settling { mut settle, ended } => {
                    let Poll::Ready(settled) = settle.as_mut().poll(cx) else {
                        *this = Atomic::Settling { settle, ended };
                        return Poll::Pending;
                    };
                    return match ended {
                        Ended::Panicked(payload) => resume_unwind(payload),
                        Ended::Returned(result) => Poll::Ready(match settled {
                            Ok(()) => result,
                            Err(err) => Err(err.into()),
                        }),
                    };
                }
                Atomic::Done => return Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::{ExecutorScope, current_executor_scope, with_job_executor, with_request_executor};

    #[derive(Debug)]
    enum Failure {
        Atomic(AtomicError),
        Work,
    }

    impl From<AtomicError> for Failure {
        fn from(err: AtomicError) -> Self {
            Self::Atomic(err)
        }
    }

    /// An executor whose driver offers no atomic block.
    struct Bare;

    impl Executor for Bare {
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    /// What a block was asked to do, in order.
    #[derive(Default)]
    struct Settlements(Mutex<Vec<bool>>);

    impl Settlements {
        fn seen(&self) -> Vec<bool> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    /// A unit whose driver opens a block answering `answer` when settled.
    struct Unit {
        settlements: Arc<Settlements>,
        answer: fn() -> Result<(), AtomicError>,
    }

    impl Unit {
        fn new(answer: fn() -> Result<(), AtomicError>) -> (Arc<Self>, Arc<Settlements>) {
            let settlements = Arc::new(Settlements::default());
            let unit = Arc::new(Self {
                settlements: Arc::clone(&settlements),
                answer,
            });
            (unit, settlements)
        }
    }

    impl Executor for Unit {
        fn as_any(&self) -> &dyn Any {
            self
        }

        fn atomic(&self) -> Option<Arc<dyn AtomicBlock>> {
            Some(Arc::new(Block {
                settlements: Arc::clone(&self.settlements),
                answer: self.answer,
            }))
        }
    }

    struct Block {
        settlements: Arc<Settlements>,
        answer: fn() -> Result<(), AtomicError>,
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
            self.settlements
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(ok);
            let answer = (self.answer)();
            Box::pin(async move { answer })
        }
    }

    fn settles() -> Result<(), AtomicError> {
        Ok(())
    }

    fn refuses() -> Result<(), AtomicError> {
        Err(AtomicError::Database("RELEASE refused".into()))
    }

    #[tokio::test]
    async fn outside_any_unit_the_work_never_runs() {
        let ran = AtomicBool::new(false);
        let outcome = atomic(async {
            ran.store(true, Ordering::SeqCst);
            Ok::<_, Failure>(())
        })
        .await;

        assert!(matches!(
            outcome,
            Err(Failure::Atomic(AtomicError::Unsupported))
        ));
        assert!(!ran.load(Ordering::SeqCst), "the work is never polled");
    }

    #[tokio::test]
    async fn a_driver_offering_no_block_never_runs_the_work() {
        let ran = AtomicBool::new(false);
        let outcome = with_request_executor(
            Arc::new(Bare),
            atomic(async {
                ran.store(true, Ordering::SeqCst);
                Ok::<_, Failure>(())
            }),
        )
        .await;

        assert!(matches!(
            outcome,
            Err(Failure::Atomic(AtomicError::Unsupported))
        ));
        assert!(!ran.load(Ordering::SeqCst), "the work is never polled");
    }

    #[tokio::test]
    async fn the_block_is_read_where_it_first_runs() {
        let (unit, settlements) = Unit::new(settles);
        let block = atomic(async { Ok::<_, Failure>(()) });

        let outcome = with_request_executor(unit, block).await;

        assert!(outcome.is_ok(), "{outcome:?}");
        assert_eq!(settlements.seen(), [true]);
    }

    #[tokio::test]
    async fn the_work_runs_on_the_block_under_the_units_scope() {
        let (unit, _) = Unit::new(settles);
        let seen = with_job_executor(
            unit,
            atomic(async {
                let on_block = current_executor().is_some_and(|e| e.as_any().is::<Block>());
                Ok::<_, Failure>((on_block, current_executor_scope()))
            }),
        )
        .await;

        assert!(
            matches!(seen, Ok((true, Some(ExecutorScope::Job)))),
            "{seen:?}"
        );
    }

    #[tokio::test]
    async fn the_unit_gets_its_executor_back_once_the_block_settled() {
        let (unit, _) = Unit::new(settles);
        with_request_executor(unit, async {
            let settled = atomic(async { Ok::<_, Failure>(()) }).await;
            assert!(settled.is_ok(), "{settled:?}");
            assert!(current_executor().is_some_and(|e| e.as_any().is::<Unit>()));
        })
        .await;
    }

    #[tokio::test]
    async fn an_ok_keeps_the_block_and_returns_the_value() {
        let (unit, settlements) = Unit::new(settles);
        let outcome = with_request_executor(unit, atomic(async { Ok::<_, Failure>(7) })).await;

        assert!(matches!(outcome, Ok(7)));
        assert_eq!(settlements.seen(), [true]);
    }

    #[tokio::test]
    async fn an_err_rolls_the_block_back_and_returns_the_error() {
        let (unit, settlements) = Unit::new(settles);
        let outcome =
            with_request_executor(unit, atomic(async { Err::<(), _>(Failure::Work) })).await;

        assert!(matches!(outcome, Err(Failure::Work)));
        assert_eq!(settlements.seen(), [false]);
    }

    #[tokio::test]
    async fn a_block_that_cannot_settle_answers_why_over_the_works_value() {
        let (unit, settlements) = Unit::new(refuses);
        let outcome = with_request_executor(unit, atomic(async { Ok::<_, Failure>(()) })).await;

        assert!(
            matches!(outcome, Err(Failure::Atomic(AtomicError::Database(_)))),
            "{outcome:?}"
        );
        assert_eq!(settlements.seen(), [true]);
    }

    #[tokio::test]
    async fn a_panic_rolls_the_block_back_and_unwinds_on() {
        let (unit, settlements) = Unit::new(settles);
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let unwound =
            nest_rs_core::panic::contain(with_request_executor(unit, atomic(explode()))).await;
        std::panic::set_hook(previous);

        let payload = unwound.expect_err("the panic unwinds past the block");
        assert_eq!(
            nest_rs_core::panic::panic_message(payload.as_ref()),
            "inside the block"
        );
        assert_eq!(settlements.seen(), [false]);
    }

    async fn explode() -> Result<(), Failure> {
        panic!("inside the block")
    }

    #[test]
    fn a_block_holds_the_unit_it_runs_once() {
        let heavy = || async {
            let held = [0u8; 32 * 1024];
            tokio::task::yield_now().await;
            std::hint::black_box(&held);
            Ok::<_, Failure>(())
        };
        let alone = std::mem::size_of_val(&heavy());
        let size = std::mem::size_of_val(&atomic(heavy()));
        assert!(
            size < alone + 1024,
            "atomic weighs {size} bytes around a {alone}-byte unit",
        );
    }
}

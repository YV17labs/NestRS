//! [`TickAnswer`] — what a scheduled method answers, read by its type, made
//! the outcome its tick runs to.

use std::pin::Pin;
use std::task::{Context, Poll};

/// What a `#[cron]`, `#[every]` or `#[after]` method answers: a
/// `Result<(), E>` whose error converts into `anyhow::Error`, or `!` for one
/// that never answers — whatever the type is called.
#[diagnostic::on_unimplemented(
    message = "a `#[cron]`, `#[every]` or `#[after]` method returns `Result<(), E>` whose `E` \
               converts into `anyhow::Error`, and this one returns `{Self}`",
    label = "not a `Result<(), E>` whose `E` converts into `anyhow::Error`",
    note = "an `Err` fails the occurrence, which the scheduler logs before the next one: `E` is \
            `anyhow::Error`, or a `std::error::Error + Send + Sync + 'static` such as a \
            `thiserror` type; a box, `Send + Sync` or not, is neither — make it a \
            `Box<dyn Error + Send + Sync>` and map it with `anyhow::Error::from_boxed`"
)]
pub trait TickAnswer {
    /// The answer as the tick's outcome, its error converted as `?` converts
    /// it, every link of its chain kept.
    fn into_outcome(self) -> anyhow::Result<()>;
}

#[diagnostic::do_not_recommend]
impl<E: Into<anyhow::Error>> TickAnswer for Result<(), E> {
    fn into_outcome(self) -> anyhow::Result<()> {
        self.map_err(Into::into)
    }
}

#[diagnostic::do_not_recommend]
impl TickAnswer for ! {
    fn into_outcome(self) -> anyhow::Result<()> {
        self
    }
}

/// A tick's run: its method's call awaited and its answer read as the scheduler
/// reads it — see [`TickAnswer`].
///
/// It takes the call's future and boxes the run itself, since an expansion
/// binding an answer of `!` trips lints a crate may forbid, and one awaiting a
/// future of this fn's own type checks its bound a second time.
pub fn answered<'a, F, A>(
    answer: F,
) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>
where
    F: Future<Output = A> + Send + 'a,
    A: TickAnswer,
{
    // Not an `async` block, whose state holds the future twice (rust-lang/rust#62958),
    // nor `FutureExt::map`, whose constructors each copy it onto a debug build's stack.
    Box::pin(Answered { answer })
}

pin_project_lite::pin_project! {
    /// What [`answered`] boxes: the call's future, polled in place, its answer
    /// read as it completes.
    struct Answered<F> {
        #[pin]
        answer: F,
    }
}

impl<F> Future for Answered<F>
where
    F: Future,
    F::Output: TickAnswer,
{
    type Output = anyhow::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().answer.poll(cx).map(TickAnswer::into_outcome)
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nest_rs_core::Container;

    use super::*;
    use crate::{ScheduledMethod, scheduled};

    #[derive(Debug, thiserror::Error)]
    #[error("the report store refused the connection")]
    struct StoreRefused;

    #[derive(Debug, thiserror::Error)]
    #[error("the nightly report failed")]
    struct ReportFailed(#[source] StoreRefused);

    #[tokio::test]
    async fn an_error_of_its_own_fails_the_tick_with_its_whole_chain() {
        let failed = answered(async { Err::<(), _>(ReportFailed(StoreRefused)) })
            .await
            .unwrap_err();
        assert!(
            failed.downcast_ref::<ReportFailed>().is_some(),
            "the error itself, not a message of it"
        );
        assert_eq!(
            format!("{failed:#}"),
            "the nightly report failed: the report store refused the connection"
        );
    }

    #[tokio::test]
    async fn an_anyhow_error_is_answered_as_it_was_written() {
        let written = anyhow::Error::new(StoreRefused).context("the nightly report failed");
        let failed = answered(async { Err::<(), _>(written) }).await.unwrap_err();
        assert_eq!(failed.to_string(), "the nightly report failed");
        assert!(
            failed.downcast_ref::<StoreRefused>().is_some(),
            "its chain is the one written, not wrapped again"
        );
        assert_eq!(failed.chain().count(), 2);
    }

    #[tokio::test]
    async fn an_ok_completes_the_tick() {
        assert!(answered(async { Ok::<(), StoreRefused>(()) }).await.is_ok());
    }

    /// The refusal's note names this conversion for the box anyhow's `From`
    /// does not take.
    #[tokio::test]
    async fn a_boxed_error_mapped_as_the_refusal_says_fails_the_tick_with_its_whole_chain() {
        let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(ReportFailed(StoreRefused));
        let failed = answered(async { Err::<(), _>(anyhow::Error::from_boxed(boxed)) })
            .await
            .unwrap_err();
        assert_eq!(
            format!("{failed:#}"),
            "the nightly report failed: the report store refused the connection"
        );
    }

    #[nest_rs_core::injectable]
    #[derive(Default)]
    struct ArchiveTasks;

    #[scheduled]
    impl ArchiveTasks {
        /// Holds a buffer across an `.await`, so a run keeping two copies of
        /// its call weighs twice what the published release's did.
        #[every("1h")]
        async fn archive(&self) -> anyhow::Result<()> {
            let buffer = [7_u8; 32 * 1024];
            tokio::task::yield_now().await;
            std::hint::black_box(&buffer);
            Ok(())
        }
    }

    #[test]
    fn a_ticks_run_is_boxed_at_the_size_of_its_calls_future() {
        let container = Container::builder().build().expect("the container builds");
        let tick = nest_rs_core::inventory::iter::<ScheduledMethod>()
            .find(|job| job.host_type_id() == TypeId::of::<ArchiveTasks>())
            .expect("the tick is submitted");

        let run = (tick.run())(&container);
        // The future the expansion builds from the call, which the published
        // release boxed as the run.
        let container = &container;
        let call = async move {
            let provider = Container::get::<ArchiveTasks>(container).expect("the host is provided");
            <ArchiveTasks>::archive(&provider).await
        };
        let (boxed, called) = (size_of_val(&*run), size_of_val(&call));
        assert!(
            boxed <= called + size_of::<usize>(),
            "in a {} build, `archive`'s run boxes {boxed} B for a call whose future is {called} B",
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
        );
    }
}

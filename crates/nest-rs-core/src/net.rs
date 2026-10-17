//! [`Net`] — what a port waits on a call before it gives up — the order the
//! boot holds between nets and the [`Budget`]s they reach, and [`within`], the
//! one wait that enforces them.

use std::any::{Any, TypeId};
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use crate::access::{ModuleDescriptor, injection_closure};
use crate::budget::Budget;
use crate::container::ContainerBuilder;
use crate::error::BudgetPastNetError;

/// What a port waits on a call before it gives up and answers in its own
/// terms: a guard denies, a push fails, a claim skips its occurrence.
///
/// A net bounds a backend that stopped bounding itself; a resource's own
/// [`Budget`] must fail first, with its cause, so the boot refuses a budget at
/// or past a net that reaches it ([`BudgetPastNetError`]). Declared as metadata
/// (`builder.provide_meta(Net::over::<R>(…))`).
#[derive(Debug)]
pub struct Net {
    port: &'static str,
    wait: Duration,
    reach: Reach,
}

/// Which budgets a net reaches.
#[derive(Clone, Copy, Debug)]
enum Reach {
    /// The one resource a binding handed the port.
    Over(TypeId),
    /// Whatever a provider's code reaches: what it injects, transitively, and
    /// every ambient resource.
    Around(TypeId),
}

impl Net {
    /// The net `port` waits `wait` under, over the resource `R` a binding hands
    /// it — the queue port's over the Redis connection its binding holds.
    ///
    /// `port` names it in a sentence, before "'s net" (`"the queue port"`).
    pub fn over<R: Any>(port: &'static str, wait: Duration) -> Self {
        Self {
            port,
            wait,
            reach: Reach::Over(TypeId::of::<R>()),
        }
    }

    /// The net `port` waits `wait` under around the provider `P`'s code — a
    /// guard around its strategy. It reaches every budget read off a provider
    /// `P` injects, through any depth of `#[inject]`, and every
    /// [ambient](Budget::ambient) one.
    pub fn around<P: Any>(port: &'static str, wait: Duration) -> Self {
        Self {
            port,
            wait,
            reach: Reach::Around(TypeId::of::<P>()),
        }
    }
}

/// Refuse the first budget at or past a net reaching it, nets and budgets in
/// declaration order, each budget read off the provider `builder` holds —
/// `only` those of the providers it names, or every one. `descriptors` are the
/// modules the app reaches, whose injections a net around a provider follows.
///
/// A resource declared twice — by the module opening it and by a binding over
/// it — is one budget, ambient if either declaration is.
pub(crate) fn check_budgets(
    builder: &ContainerBuilder,
    descriptors: &[&ModuleDescriptor],
    only: Option<&[TypeId]>,
) -> Result<(), BudgetPastNetError> {
    let nets: Vec<&Net> = builder.attached_meta::<Net>().collect();
    if nets.is_empty() {
        return Ok(());
    }
    let mut budgets: Vec<(&Budget, bool)> = Vec::new();
    for budget in builder.attached_meta::<Budget>() {
        match budgets.iter_mut().find(|(seen, _)| {
            seen.provider() == budget.provider() && seen.resource() == budget.resource()
        }) {
            Some((_, ambient)) => *ambient |= budget.is_ambient(),
            None => budgets.push((budget, budget.is_ambient())),
        }
    }
    budgets.retain(|(budget, _)| {
        builder.contains(budget.provider())
            && only.is_none_or(|providers| providers.contains(&budget.provider()))
    });
    if budgets.is_empty() {
        return Ok(());
    }
    let container = builder.snapshot();
    for net in nets {
        let injected = match net.reach {
            Reach::Over(_) => HashSet::new(),
            Reach::Around(provider) => injection_closure(descriptors, provider),
        };
        for (budget, ambient) in &budgets {
            let reached = match net.reach {
                Reach::Over(resource) => budget.provider() == resource,
                Reach::Around(_) => *ambient || injected.contains(&budget.provider()),
            };
            if !reached {
                continue;
            }
            let Some(wait) = budget.wait(&container) else {
                continue;
            };
            if wait >= net.wait {
                return Err(BudgetPastNetError {
                    resource: budget.resource(),
                    budget: wait,
                    port: net.port,
                    net: net.wait,
                    setting: budget.setting().to_owned(),
                });
            }
        }
    }
    Ok(())
}

/// Why a wait on a backend's call ended without its answer.
#[derive(Debug)]
#[non_exhaustive]
pub enum Unanswered {
    /// No answer within the wait.
    Elapsed(Duration),
    /// The call unwound. The payload is for the port to file once, through
    /// [`contained_panic!`](crate::contained_panic) on its own target; it never
    /// reaches an error a caller reads.
    Panicked(Box<dyn Any + Send>),
}

/// `call`'s answer within `wait`, or why there is none — the one wait a port
/// puts around a backend's call, whatever bounds it: a [`Net`], a budget, a
/// probe's ceiling.
///
/// Polled once bare first: an answer ready at once arms no timer and needs no
/// runtime clock. A panic while `call` is polled is contained and answered as
/// [`Unanswered::Panicked`], so the port fails closed exactly as it does past
/// the wait. Pass an `async` block, so a backend panicking before it hands back
/// its future is contained too.
///
/// ```
/// use std::time::Duration;
/// use nest_rs_core::{Unanswered, within};
///
/// # #[nest_rs_core::main]
/// # async fn main() {
/// let allowed = match within(Duration::from_secs(18), async { Ok::<_, ()>(true) }).await {
///     Ok(answered) => answered.unwrap_or(false),
///     Err(Unanswered::Panicked(payload)) => {
///         nest_rs_core::contained_panic!(
///             target: "my_store",
///             payload.as_ref(),
///             "store panicked; denying",
///         );
///         false
///     }
///     Err(_elapsed) => false,
/// };
/// assert!(allowed);
/// # }
/// ```
pub fn within<F: Future>(
    wait: Duration,
    call: F,
) -> impl Future<Output = Result<F::Output, Unanswered>> {
    // Not an `async fn`: its state would keep `call` beside the future built
    // from it.
    Within {
        call: crate::panic::contain(call),
        wait,
        timer: None,
    }
}

pin_project_lite::pin_project! {
    /// What [`within`] returns: its timer is armed at the call's first
    /// `Pending`.
    struct Within<C> {
        #[pin]
        call: C,
        wait: Duration,
        #[pin]
        timer: Option<tokio::time::Sleep>,
    }
}

impl<C, T> Future for Within<C>
where
    C: Future<Output = Result<T, Box<dyn Any + Send>>>,
{
    type Output = Result<T, Unanswered>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        let had_budget = tokio::task::coop::has_budget_remaining();
        if let Poll::Ready(answered) = this.call.poll(cx) {
            return Poll::Ready(answered.map_err(Unanswered::Panicked));
        }
        if this.timer.is_none() {
            this.timer.set(Some(tokio::time::sleep(*this.wait)));
        }
        let Some(timer) = this.timer.as_pin_mut() else {
            return Poll::Pending;
        };
        // As tokio's own `Timeout`: a call that spent the task's budget must not
        // keep its bound from firing.
        let elapsed = if had_budget && !tokio::task::coop::has_budget_remaining() {
            Pin::new(&mut tokio::task::coop::unconstrained(timer)).poll(cx)
        } else {
            timer.poll(cx)
        };
        elapsed.map(|()| Err(Unanswered::Elapsed(*this.wait)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A future whose every poll panics — a backend's hand-written future.
    struct Unwinding;

    impl Future for Unwinding {
        type Output = u8;

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<u8> {
            panic!("the backend unwound")
        }
    }

    #[test]
    fn an_answer_ready_at_once_needs_no_clock() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime with no time driver");
        let answered = runtime.block_on(within(Duration::from_secs(1), async { 7 }));
        assert!(matches!(answered, Ok(7)), "{answered:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_call_that_never_answers_is_given_up_at_the_wait() {
        let sent = tokio::time::Instant::now();
        let given_up = within(Duration::from_secs(5), std::future::pending::<()>()).await;
        assert!(
            matches!(given_up, Err(Unanswered::Elapsed(wait)) if wait == Duration::from_secs(5)),
            "{given_up:?}"
        );
        assert_eq!(sent.elapsed(), Duration::from_secs(5));
    }

    #[tokio::test(start_paused = true)]
    async fn a_call_answering_inside_the_wait_is_heard() {
        let answered = within(Duration::from_secs(5), async {
            tokio::time::sleep(Duration::from_millis(4_999)).await;
            "late but in time"
        })
        .await;
        assert!(matches!(answered, Ok("late but in time")), "{answered:?}");
    }

    /// One poll of `future`, answered whether or not it is ready.
    async fn poll_once<F: Future + Unpin>(future: &mut F) -> Poll<F::Output> {
        std::future::poll_fn(|cx| Poll::Ready(Pin::new(&mut *future).poll(cx))).await
    }

    #[tokio::test(start_paused = true)]
    async fn a_call_spending_the_tasks_budget_is_still_given_up_at_the_wait() {
        let sent = tokio::time::Instant::now();
        let mut net = std::pin::pin!(within(Duration::from_secs(5), async {
            loop {
                tokio::task::consume_budget().await;
            }
        }));
        // Polled by hand: a task that is never idle keeps a paused clock from
        // advancing on its own.
        assert!(
            poll_once(&mut net).await.is_pending(),
            "the call never answers"
        );
        tokio::time::advance(Duration::from_secs(5)).await;
        let given_up = poll_once(&mut net).await;
        assert!(
            matches!(given_up, Poll::Ready(Err(Unanswered::Elapsed(wait))) if wait == Duration::from_secs(5)),
            "the call spent the budget the timer needs to fire: {given_up:?}"
        );
        assert_eq!(sent.elapsed(), Duration::from_secs(5));
    }

    #[tokio::test]
    async fn a_future_that_unwinds_answers_panicked_with_its_payload() {
        let Err(Unanswered::Panicked(payload)) = within(Duration::from_secs(1), Unwinding).await
        else {
            panic!("the unwind is the answer");
        };
        assert_eq!(
            crate::panic_message(payload.as_ref()),
            "the backend unwound"
        );
    }

    #[tokio::test]
    async fn an_async_block_unwinding_at_its_first_statement_is_contained_too() {
        fn hand_back() -> std::future::Ready<u8> {
            panic!("panicked before handing its future back")
        }
        let Err(Unanswered::Panicked(payload)) =
            within(Duration::from_secs(1), async { hand_back().await }).await
        else {
            panic!("the unwind is the answer");
        };
        assert_eq!(
            crate::panic_message(payload.as_ref()),
            "panicked before handing its future back"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_call_unwinding_after_a_wait_is_contained_too() {
        let Err(Unanswered::Panicked(payload)) = within(Duration::from_secs(5), async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            panic!("unwound mid-call")
        })
        .await
        else {
            panic!("the unwind is the answer");
        };
        assert_eq!(crate::panic_message(payload.as_ref()), "unwound mid-call");
    }
}

//! [`ExceptionFilter`], the layer that answers a failed unit on any edge,
//! [`Caught`], its answer, [`Catch`], what it can catch, and the
//! [`ExceptionFilterSpec`] an app lists one with.

use std::any::{TypeId, type_name};
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;

use crate::layer::Layer;
use crate::layer_chain::__private::ResolvedLayer;
use crate::layer_chain::{GlobalSpecs, LayerSpec};
use crate::outcome::{Failure, Outcome, Settled};
use crate::problem::Problem;
use crate::unit_context::UnitContext;

/// Answers a unit that failed with its [`Exception`](Self::Exception), found
/// anywhere in the failure's source chain — or declines it. The unit stays
/// failed whatever the filter answers, so its data unit of work rolls back.
///
/// Filters run inside the guards: a denial reaches none. The method's filters
/// are tried first, then the host's, then the app's; the first that answers
/// wins.
///
/// ```
/// use nest_rs_core::problem::code;
/// use nest_rs_core::{Caught, ExceptionFilter, Layer, Problem, UnitContext, async_trait};
///
/// #[derive(Debug, thiserror::Error)]
/// #[error("the order is already paid")]
/// struct AlreadyPaid;
///
/// struct PaymentFilter;
/// impl Layer for PaymentFilter {}
///
/// #[async_trait]
/// impl ExceptionFilter for PaymentFilter {
///     type Exception = AlreadyPaid;
///
///     async fn catch(&self, _: &AlreadyPaid, _: &mut UnitContext<'_>) -> Caught {
///         Caught::Problem(Problem::new(409, code::CONFLICT).with_detail("already paid"))
///     }
/// }
/// ```
#[async_trait]
pub trait ExceptionFilter: Layer {
    /// An error type found anywhere in the failure's source chain, or
    /// `dyn Error + Send + Sync` to catch every failure.
    type Exception: Catch + ?Sized;

    /// Answer `exception`, or decline it for the next filter.
    async fn catch(&self, exception: &Self::Exception, cx: &mut UnitContext<'_>) -> Caught;
}

/// What an [`ExceptionFilter`] made of a failure.
#[derive(Debug)]
#[non_exhaustive]
pub enum Caught {
    /// Answer with this problem, in the edge's standard form.
    Problem(Problem),
    /// The filter wrote the reply through the edge's view.
    Answered(Settled),
    /// Not this filter's: the next one is tried.
    Declined,
}

/// What an [`ExceptionFilter`] can catch: any error type, found in the
/// failure's chain, or `dyn Error + Send + Sync`, which is every failure.
/// Sealed.
pub trait Catch: sealed::Sealed + Send + Sync + 'static {
    /// The `Self` `failure` carries, if any.
    #[doc(hidden)]
    fn __find(failure: &Failure) -> Option<&Self>;
}

impl<E: Error + Send + Sync + 'static> Catch for E {
    fn __find(failure: &Failure) -> Option<&Self> {
        failure.find::<E>()
    }
}

impl Catch for dyn Error + Send + Sync + 'static {
    fn __find(failure: &Failure) -> Option<&Self> {
        Some(failure.error())
    }
}

mod sealed {
    use std::error::Error;

    pub trait Sealed {}
    impl<E: Error + Send + Sync + 'static> Sealed for E {}
    impl Sealed for dyn Error + Send + Sync + 'static {}
}

/// One exception filter an app lists, resolved against the container when a
/// site composes its chains.
pub struct ExceptionFilterSpec(pub(crate) LayerSpec<dyn __private::ErasedExceptionFilter>);

impl fmt::Debug for ExceptionFilterSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ExceptionFilterSpec")
            .field(&self.0.name)
            .finish()
    }
}

/// The [`ExceptionFilterSpec`] of `F`, the provider of that type a module
/// imports.
pub fn exception_filter<F: ExceptionFilter>() -> ExceptionFilterSpec {
    ExceptionFilterSpec(LayerSpec::new(
        TypeId::of::<F>(),
        type_name::<F>(),
        |container| {
            container
                .get::<F>()
                .map(|filter| filter as Arc<dyn __private::ErasedExceptionFilter>)
        },
    ))
}

/// The exception filters an app lists for every unit.
pub(crate) struct GlobalExceptionFilters(
    pub(crate) Vec<LayerSpec<dyn __private::ErasedExceptionFilter>>,
);

impl GlobalSpecs for GlobalExceptionFilters {
    type Layer = dyn __private::ErasedExceptionFilter;

    fn specs(&self) -> &[LayerSpec<dyn __private::ErasedExceptionFilter>] {
        &self.0
    }
}

/// Try `filters` in order on `failure`; the first that answers wins, and the
/// unit stays failed either way.
pub(crate) async fn catch(
    cx: &mut UnitContext<'_>,
    filters: &[ResolvedLayer<dyn __private::ErasedExceptionFilter>],
    failure: Failure,
) -> Outcome {
    for filter in filters {
        let Some(caught) = filter.layer.try_catch(&failure, cx) else {
            continue;
        };
        match caught.await {
            Caught::Declined => {}
            Caught::Answered(reply) => return Err(failure.answered(reply)),
            Caught::Problem(problem) => return Err(failure.with_problem(problem)),
        }
    }
    Err(failure)
}

/// `exception_filter` is private: its tier-2 items live here, reached only
/// through the crate's `__private`.
pub(crate) mod __private {
    use super::{Catch, Caught, ExceptionFilter, Failure, Future, Layer, Pin, UnitContext};

    /// An [`ExceptionFilter`] behind its exception type, as a site holds it.
    pub trait ErasedExceptionFilter: Layer {
        /// The filter's answer to `failure`, or `None`, costing nothing, when
        /// `failure` carries no exception of its type.
        fn try_catch<'a>(
            &'a self,
            failure: &'a Failure,
            cx: &'a mut UnitContext<'_>,
        ) -> Option<Pin<Box<dyn Future<Output = Caught> + Send + 'a>>>;
    }

    impl<F: ExceptionFilter> ErasedExceptionFilter for F {
        fn try_catch<'a>(
            &'a self,
            failure: &'a Failure,
            cx: &'a mut UnitContext<'_>,
        ) -> Option<Pin<Box<dyn Future<Output = Caught> + Send + 'a>>> {
            let exception = F::Exception::__find(failure)?;
            Some(self.catch(exception, cx))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use super::*;
    use crate::layer::LayerSite;
    use crate::outcome::__private::{caught_problem, settled};
    use crate::problem::code;
    use crate::testing::{SEND, TrailView, record, run_counting};
    use crate::unit_context::new_unit_context;

    #[derive(Debug, thiserror::Error)]
    #[error("the ledger refused the write")]
    struct LedgerRefused;

    #[derive(Debug, thiserror::Error)]
    enum OrderError {
        #[error("the order could not be paid")]
        Payment(#[source] LedgerRefused),
    }

    /// Writes its name, then answers `answer`, for failures carrying `E`.
    struct Writing<E: ?Sized> {
        name: &'static str,
        answer: fn() -> Caught,
        catches: std::marker::PhantomData<fn() -> Box<E>>,
    }

    impl<E: ?Sized> Writing<E> {
        fn new(name: &'static str, answer: fn() -> Caught) -> Self {
            Self {
                name,
                answer,
                catches: std::marker::PhantomData,
            }
        }
    }

    impl<E: ?Sized + 'static> Layer for Writing<E> {}

    #[async_trait]
    impl<E: Catch + ?Sized> ExceptionFilter for Writing<E> {
        type Exception = E;

        async fn catch(&self, _: &E, cx: &mut UnitContext<'_>) -> Caught {
            record(cx, self.name);
            (self.answer)()
        }
    }

    fn declined() -> Caught {
        Caught::Declined
    }

    fn conflict() -> Caught {
        Caught::Problem(Problem::new(409, code::CONFLICT))
    }

    fn answered() -> Caught {
        Caught::Answered(settled())
    }

    fn resolved<F: ExceptionFilter>(
        filter: F,
    ) -> ResolvedLayer<dyn __private::ErasedExceptionFilter> {
        ResolvedLayer {
            type_id: TypeId::of::<F>(),
            name: type_name::<F>(),
            source: LayerSite::Method,
            layer: Arc::new(filter),
        }
    }

    async fn caught(
        filters: &[ResolvedLayer<dyn __private::ErasedExceptionFilter>],
        failure: Failure,
    ) -> (Failure, Vec<String>) {
        let mut view = TrailView::new();
        let mut cx = new_unit_context(&SEND, &mut view, None, None);
        let outcome = catch(&mut cx, filters, failure).await;
        let failure = outcome.expect_err("the unit stays failed whatever a filter answers");
        (failure, view.steps())
    }

    #[tokio::test]
    async fn a_declining_filter_passes_to_the_next() {
        let filters = [
            resolved(Writing::<dyn Error + Send + Sync>::new(
                "declines", declined,
            )),
            resolved(Writing::<LedgerRefused>::new("answers", conflict)),
        ];
        let (failure, steps) = caught(&filters, Failure::from(LedgerRefused)).await;
        assert_eq!(steps, ["declines", "answers"]);
        assert_eq!(
            caught_problem(&failure).map(Problem::code),
            Some(code::CONFLICT),
            "the edge renders the problem the filter answered with",
        );
        assert!(
            failure.find::<LedgerRefused>().is_some(),
            "the error is kept"
        );
    }

    #[tokio::test]
    async fn the_first_filter_that_answers_wins() {
        let filters = [
            resolved(Writing::<LedgerRefused>::new("first", answered)),
            resolved(Writing::<dyn Error + Send + Sync>::new("second", conflict)),
        ];
        let (failure, steps) = caught(&filters, Failure::from(LedgerRefused)).await;
        assert_eq!(steps, ["first"]);
        assert!(
            failure.is_answered(),
            "the filter wrote the reply through the view"
        );
        assert!(caught_problem(&failure).is_none());
    }

    #[tokio::test]
    async fn a_filter_catches_its_error_inside_a_thiserror_enums_source_chain() {
        let filters = [resolved(Writing::<LedgerRefused>::new("ledger", conflict))];
        let (failure, steps) =
            caught(&filters, Failure::from(OrderError::Payment(LedgerRefused))).await;
        assert_eq!(steps, ["ledger"]);
        assert!(caught_problem(&failure).is_some());
    }

    #[tokio::test]
    async fn a_filter_catches_its_error_inside_an_anyhow_error_made_a_failure() {
        let filters = [resolved(Writing::<LedgerRefused>::new("ledger", conflict))];
        let failure = Failure::new(anyhow::Error::new(LedgerRefused).context("paying order 42"));
        let (failure, steps) = caught(&filters, failure).await;
        assert_eq!(steps, ["ledger"]);
        assert!(caught_problem(&failure).is_some());
    }

    #[tokio::test]
    async fn a_failure_no_filter_catches_goes_on_as_it_came() {
        let filters = [resolved(Writing::<std::io::Error>::new("io", conflict))];
        let (failure, steps) = caught(&filters, Failure::from(LedgerRefused)).await;
        assert!(steps.is_empty());
        assert!(!failure.is_answered());
        assert!(caught_problem(&failure).is_none());
    }

    #[test]
    fn a_filter_that_does_not_match_costs_no_allocation() {
        let filters = [
            resolved(Writing::<std::io::Error>::new("io", conflict)),
            resolved(Writing::<std::fmt::Error>::new("fmt", conflict)),
        ];
        let failure = Failure::from(LedgerRefused);
        let mut view = TrailView::new();
        let mut cx = new_unit_context(&SEND, &mut view, None, None);
        let (outcome, allocations) = run_counting(catch(&mut cx, &filters, failure));
        assert!(outcome.is_err());
        assert_eq!(allocations, 0);
    }
}

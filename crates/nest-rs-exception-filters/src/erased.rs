//! Type-erased dispatch: the blanket impl exposes every [`ExceptionFilter`] as
//! `dyn ExceptionFilterErased` for the catch chains.

use std::any::{TypeId, type_name};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_core::Layer;
use nest_rs_http::__private::poem_bridge::downcast_from_poem;
use poem::{Error, Response};

use crate::ExceptionFilter;

/// Object-safe view of an [`ExceptionFilter`] — every catch chain holds
/// `Arc<dyn ExceptionFilterErased>` and tries each filter in order.
///
/// Each `try_*` method returns `Ok(value)` when the inner error matched
/// the filter's `Exception` and was handled, or `Err(value)` (the original
/// error, unchanged) when it did not — so the next filter can have a turn.
#[async_trait]
pub trait ExceptionFilterErased: Layer {
    /// `TypeId` of the concrete `Exception` this filter claims.
    fn exception_type_id(&self) -> TypeId;

    /// `type_name` of the concrete `Exception` this filter claims.
    fn exception_type_name(&self) -> &'static str;

    /// HTTP dispatch. Downcast `err` to `Exception`; if it matches, call
    /// [`ExceptionFilter::catch`] and return the response. Otherwise hand
    /// the error back unchanged.
    async fn try_catch(&self, err: Error) -> Result<Response, Error>;
}

#[async_trait]
impl<T> ExceptionFilterErased for T
where
    T: ExceptionFilter,
{
    fn exception_type_id(&self) -> TypeId {
        TypeId::of::<T::Exception>()
    }

    fn exception_type_name(&self) -> &'static str {
        type_name::<T::Exception>()
    }

    async fn try_catch(&self, err: Error) -> Result<Response, Error> {
        // A route's handler answers on the HTTP vocabulary, so its error reaches
        // this poem-typed chain carried: the downcast reaches inside the carrier.
        match downcast_from_poem::<T::Exception>(err) {
            Ok(exception) => Ok(self.catch(exception).await),
            Err(unchanged) => Err(unchanged),
        }
    }
}

// Manual forward, not `#[async_trait]`: the macro would box the already-boxed
// future a second time on every call through an `Arc<dyn ExceptionFilterErased>`.
impl<T: ExceptionFilterErased + ?Sized> ExceptionFilterErased for Arc<T> {
    fn exception_type_id(&self) -> TypeId {
        (**self).exception_type_id()
    }

    fn exception_type_name(&self) -> &'static str {
        (**self).exception_type_name()
    }

    fn try_catch<'s, 'fut>(
        &'s self,
        err: Error,
    ) -> Pin<Box<dyn Future<Output = Result<Response, Error>> + Send + 'fut>>
    where
        's: 'fut,
        Self: 'fut,
    {
        (**self).try_catch(err)
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::Layer;
    use poem::http::StatusCode;

    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("domain boom")]
    struct DomainError;

    #[derive(Debug, thiserror::Error)]
    #[error("other boom")]
    struct OtherError;

    struct DomainErrorFilter;

    impl Layer for DomainErrorFilter {}

    #[async_trait]
    impl crate::ExceptionFilter for DomainErrorFilter {
        type Exception = DomainError;

        async fn catch(&self, _err: DomainError) -> Response {
            Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body("caught")
        }
    }

    #[tokio::test]
    async fn a_matching_exception_is_caught_and_mapped() {
        let filter: &dyn ExceptionFilterErased = &DomainErrorFilter;
        let resp = filter
            .try_catch(Error::new(DomainError, StatusCode::INTERNAL_SERVER_ERROR))
            .await
            .expect("the typed exception matches");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_non_matching_exception_flows_through_unchanged() {
        let filter: &dyn ExceptionFilterErased = &DomainErrorFilter;
        let err = filter
            .try_catch(Error::new(OtherError, StatusCode::INTERNAL_SERVER_ERROR))
            .await
            .expect_err("a foreign exception is handed back");
        assert!(
            err.downcast_ref::<OtherError>().is_some(),
            "the original error must survive for the next filter",
        );
    }

    #[tokio::test]
    async fn a_handler_error_carried_across_the_bridge_is_caught_by_its_type() {
        let filter: &dyn ExceptionFilterErased = &DomainErrorFilter;
        let carried = nest_rs_http::__private::poem_bridge::error_to_poem(
            nest_rs_http::HttpError::new(DomainError, StatusCode::INTERNAL_SERVER_ERROR),
        );
        let resp = filter
            .try_catch(carried)
            .await
            .expect("the carried exception matches");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let other = nest_rs_http::__private::poem_bridge::error_to_poem(
            nest_rs_http::HttpError::new(OtherError, StatusCode::INTERNAL_SERVER_ERROR),
        );
        let err = filter
            .try_catch(other)
            .await
            .expect_err("a foreign carried exception is handed back");
        let back = nest_rs_http::__private::poem_bridge::error_from_poem(err);
        assert!(back.is::<OtherError>(), "the next filter still finds it");
    }

    #[test]
    fn the_erased_view_reports_the_claimed_exception_type() {
        let filter: &dyn ExceptionFilterErased = &DomainErrorFilter;
        assert_eq!(filter.exception_type_id(), TypeId::of::<DomainError>());
        assert!(filter.exception_type_name().contains("DomainError"));
    }
}

//! Type-erased dispatch — the runtime sees `dyn ExceptionFilterErased`, the
//! concrete typed exception lives in the impl. Users write [`ExceptionFilter`];
//! the blanket impl below exposes it as `dyn ExceptionFilterErased` for the
//! catch chain.

use std::any::{TypeId, type_name};
use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_core::Layer;
use poem::{Error, Response};

use crate::ExceptionFilter;

/// Object-safe view of an [`ExceptionFilter`] — the catch chain holds
/// `Arc<dyn ExceptionFilterErased>` and tries each in order.
///
/// [`Self::try_catch`] returns `Ok(response)` when the inner error matched
/// the filter's `Exception` and was handled, or `Err(err)` (the original
/// error, unchanged) when it did not — so the next filter can have a turn.
#[async_trait]
pub trait ExceptionFilterErased: Layer {
    /// `TypeId` of the concrete `Exception` this filter claims.
    fn exception_type_id(&self) -> TypeId;

    /// `type_name` of the concrete `Exception` this filter claims.
    fn exception_type_name(&self) -> &'static str;

    /// Try to catch `err`. Returns `Ok(response)` if the error downcast
    /// matched and the filter produced a response; returns `Err(err)`
    /// unchanged if it did not match, so the next filter can try.
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
        match err.downcast::<T::Exception>() {
            Ok(exception) => Ok(self.catch(exception).await),
            Err(unchanged) => Err(unchanged),
        }
    }
}

#[async_trait]
impl<T: ExceptionFilterErased + ?Sized> ExceptionFilterErased for Arc<T> {
    fn exception_type_id(&self) -> TypeId {
        (**self).exception_type_id()
    }

    fn exception_type_name(&self) -> &'static str {
        (**self).exception_type_name()
    }

    async fn try_catch(&self, err: Error) -> Result<Response, Error> {
        (**self).try_catch(err).await
    }
}

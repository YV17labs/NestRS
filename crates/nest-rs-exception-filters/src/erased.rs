//! Type-erased dispatch — the runtime sees `dyn ExceptionFilterErased`, the
//! concrete typed exception lives in the impl. Users write [`ExceptionFilter`];
//! the blanket impl below exposes it as `dyn ExceptionFilterErased` for the
//! catch chains on every transport.

use std::any::{TypeId, type_name};
use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_core::Layer;
use nest_rs_graphql::async_graphql::{Context as GraphqlContext, Error as GraphqlError};
use nest_rs_ws::WsClient;
use poem::{Error, Response};
use serde_json::Value as JsonValue;

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

    /// GraphQL dispatch. Inspect `err.source()` for `Exception`; if it
    /// matches, call [`ExceptionFilter::catch_graphql`] and return the
    /// reshaped [`GraphqlError`]. Otherwise hand the error back unchanged.
    async fn try_catch_graphql<'a>(
        &self,
        ctx: &GraphqlContext<'a>,
        err: GraphqlError,
    ) -> Result<GraphqlError, GraphqlError>;

    /// WS dispatch. Inspect `err.downcast_ref::<Exception>()`; if it
    /// matches, call [`ExceptionFilter::catch_ws`] and return the reply
    /// JSON. Otherwise hand the error back unchanged.
    async fn try_catch_ws(
        &self,
        client: &WsClient,
        event: &str,
        err: anyhow::Error,
    ) -> Result<JsonValue, anyhow::Error>;
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

    async fn try_catch_graphql<'a>(
        &self,
        ctx: &GraphqlContext<'a>,
        err: GraphqlError,
    ) -> Result<GraphqlError, GraphqlError> {
        // async_graphql::Error has both a `source` field
        // (`Option<Arc<dyn Any + Send + Sync>>`) and a `source<T>()`
        // method — Rust resolves `.source::<T>()` to the field with a
        // turbofish, not the method. Reach into the field directly to
        // avoid the ambiguity.
        let matched = err
            .source
            .as_ref()
            .and_then(|arc| arc.downcast_ref::<T::Exception>());
        match matched {
            Some(exception) => Ok(self.catch_graphql(ctx, exception).await),
            None => Err(err),
        }
    }

    async fn try_catch_ws(
        &self,
        client: &WsClient,
        event: &str,
        err: anyhow::Error,
    ) -> Result<JsonValue, anyhow::Error> {
        if let Some(exception) = err.downcast_ref::<T::Exception>() {
            Ok(self.catch_ws(client, event, exception).await)
        } else {
            Err(err)
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

    async fn try_catch_graphql<'a>(
        &self,
        ctx: &GraphqlContext<'a>,
        err: GraphqlError,
    ) -> Result<GraphqlError, GraphqlError> {
        (**self).try_catch_graphql(ctx, err).await
    }

    async fn try_catch_ws(
        &self,
        client: &WsClient,
        event: &str,
        err: anyhow::Error,
    ) -> Result<JsonValue, anyhow::Error> {
        (**self).try_catch_ws(client, event, err).await
    }
}

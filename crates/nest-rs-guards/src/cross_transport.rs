//! Cross-transport extensions for [`Interceptor`] and [`Filter`] — the
//! GraphQL / WS companions that pair with their HTTP counterparts in
//! `nest-rs-interceptors` / `nest-rs-filters`.
//!
//! These traits live here (rather than next to the base traits) because
//! `nest-rs-http` already depends on `nest-rs-interceptors` and
//! `nest-rs-filters` — putting graphql / ws methods on the base traits
//! would close a dependency cycle. `nest-rs-guards` is the home for
//! transport-spanning layer surface: it already depends on every
//! transport crate, and `Guard` already lives here.
//!
//! Each extension is its own sub-trait of the base. A type can opt into
//! some transports and not others — there is no forced blanket impl.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_filters::Filter;
use nest_rs_graphql::async_graphql::{
    Context as GraphqlContext, Error as GraphqlError, ServerResult, Value as GraphqlValue,
};
use nest_rs_interceptors::Interceptor;
use nest_rs_ws::WsClient;
use serde_json::Value as JsonValue;

/// Continuation passed to [`GraphqlInterceptor::wrap_graphql`]. `.await`
/// invokes the next interceptor in the chain (or the resolver itself when
/// this is the innermost wrap).
pub type GraphqlNext<'a> =
    Pin<Box<dyn Future<Output = ServerResult<GraphqlValue>> + Send + 'a>>;

/// Continuation passed to [`WsInterceptor::wrap_ws`]. `.await` invokes
/// the next interceptor in the chain (or the message handler itself when
/// this is the innermost wrap).
pub type WsNext<'a> =
    Pin<Box<dyn Future<Output = std::result::Result<Option<JsonValue>, String>> + Send + 'a>>;

/// GraphQL extension to [`Interceptor`]. Wraps each resolver call. Default
/// awaits `next` (no-op), so an HTTP-only interceptor can implement the
/// base [`Interceptor`] without also implementing this trait.
#[async_trait]
pub trait GraphqlInterceptor: Interceptor {
    async fn wrap_graphql<'a>(
        &self,
        _ctx: &GraphqlContext<'a>,
        next: GraphqlNext<'a>,
    ) -> ServerResult<GraphqlValue> {
        next.await
    }
}

#[async_trait]
impl<T: GraphqlInterceptor + ?Sized> GraphqlInterceptor for Arc<T> {
    async fn wrap_graphql<'a>(
        &self,
        ctx: &GraphqlContext<'a>,
        next: GraphqlNext<'a>,
    ) -> ServerResult<GraphqlValue> {
        (**self).wrap_graphql(ctx, next).await
    }
}

/// WS extension to [`Interceptor`]. Wraps each message handler call.
/// Default awaits `next` (no-op).
#[async_trait]
pub trait WsInterceptor: Interceptor {
    async fn wrap_ws<'a>(
        &self,
        _client: &WsClient,
        _event: &str,
        _data: &JsonValue,
        next: WsNext<'a>,
    ) -> std::result::Result<Option<JsonValue>, String> {
        next.await
    }
}

#[async_trait]
impl<T: WsInterceptor + ?Sized> WsInterceptor for Arc<T> {
    async fn wrap_ws<'a>(
        &self,
        client: &WsClient,
        event: &str,
        data: &JsonValue,
        next: WsNext<'a>,
    ) -> std::result::Result<Option<JsonValue>, String> {
        (**self).wrap_ws(client, event, data, next).await
    }
}

/// GraphQL extension to [`Filter`]. Called once per error the resolver
/// raises; the returned [`GraphqlError`] replaces the original. Default
/// returns `error` unchanged so an HTTP-only filter still satisfies the
/// trait without graphql-specific behavior.
#[async_trait]
pub trait GraphqlFilter: Filter {
    async fn filter_graphql<'a>(
        &self,
        _ctx: &GraphqlContext<'a>,
        error: GraphqlError,
    ) -> GraphqlError {
        error
    }
}

#[async_trait]
impl<T: GraphqlFilter + ?Sized> GraphqlFilter for Arc<T> {
    async fn filter_graphql<'a>(
        &self,
        ctx: &GraphqlContext<'a>,
        error: GraphqlError,
    ) -> GraphqlError {
        (**self).filter_graphql(ctx, error).await
    }
}

/// WS extension to [`Filter`]. Called once per error a message handler
/// raises; the returned message replaces the original error frame body.
/// Default returns `error` unchanged.
#[async_trait]
pub trait WsFilter: Filter {
    async fn filter_ws(
        &self,
        _client: &WsClient,
        _event: &str,
        error: String,
    ) -> String {
        error
    }
}

#[async_trait]
impl<T: WsFilter + ?Sized> WsFilter for Arc<T> {
    async fn filter_ws(&self, client: &WsClient, event: &str, error: String) -> String {
        (**self).filter_ws(client, event, error).await
    }
}

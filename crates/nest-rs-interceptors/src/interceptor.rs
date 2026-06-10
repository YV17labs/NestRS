//! The [`Interceptor`] trait — a [`Layer`] sub-trait whose impls wrap
//! handler execution across every transport (HTTP, GraphQL, WS).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_core::Layer;
use nest_rs_graphql::async_graphql::{
    Context as GraphqlContext, ServerResult, Value as GraphqlValue,
};
use nest_rs_ws::WsClient;
use poem::{Endpoint, IntoResponse, Request, Response, Result};
use serde_json::Value as JsonValue;

/// Wraps handler execution. An [`Interceptor`] sees the inputs before the
/// handler runs and the outputs after. `intercept(req, next)` is the HTTP
/// entry — the only one the framework wires today. A GraphQL `POST` and a WS
/// upgrade are HTTP requests, so a *global* interceptor covers them through
/// the transport-edge wrap; [`wrap_graphql`](Interceptor::wrap_graphql) /
/// [`wrap_ws`](Interceptor::wrap_ws) are reserved seams for per-resolver /
/// per-message wrapping and are **not invoked** yet.
///
/// `Interceptor` extends [`Layer`] so the same impl can be declared at any
/// scope (global / controller / method) and the Layer System dedups by
/// [`TypeId`](std::any::TypeId). Override only the method(s) where this
/// interceptor has work to do — the others inherit a pass-through default
/// (`next.run(...).await`).
///
/// Bind globally via [`use_interceptors_global`](crate::AppBuilderInterceptorsExt),
/// per-provider via `#[use_interceptors(...)]` on the
/// controller/resolver/gateway, or per-handler beside the verb /
/// `#[query]` / `#[subscribe_message]`.
#[async_trait]
pub trait Interceptor: Layer {
    /// HTTP entry. The per-route shaper calls this once for every HTTP
    /// route. Required (no default) so an `Interceptor` impl that
    /// genuinely targets HTTP cannot forget to wire it.
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response>;

    /// GraphQL per-resolver-call entry — a reserved seam, **not wired**
    /// today (no macro or dispatcher calls it). `next` resolves to the
    /// resolver's return value; the default just awaits it (no-op wrap).
    async fn wrap_graphql<'a>(
        &self,
        _ctx: &GraphqlContext<'a>,
        next: GraphqlNext<'a>,
    ) -> ServerResult<GraphqlValue> {
        next.await
    }

    /// WS per-message entry — a reserved seam, **not wired** today (no
    /// macro or dispatcher calls it). `next` resolves to the handler's reply
    /// (an optional JSON value); the default just awaits it (no-op wrap).
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
impl<T: Interceptor + ?Sized> Interceptor for Arc<T> {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
        (**self).intercept(req, next).await
    }

    async fn wrap_graphql<'a>(
        &self,
        ctx: &GraphqlContext<'a>,
        next: GraphqlNext<'a>,
    ) -> ServerResult<GraphqlValue> {
        (**self).wrap_graphql(ctx, next).await
    }

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

/// The continuation passed to an HTTP [`Interceptor::intercept`]. Call
/// [`Next::run`] to delegate to the inner endpoint (handler or next
/// interceptor).
pub struct Next<'a> {
    inner: &'a (dyn ErasedEndpoint + Send + Sync + 'a),
}

impl<'a> Next<'a> {
    pub fn new<E>(endpoint: &'a E) -> Self
    where
        E: Endpoint + Send + Sync,
        E::Output: IntoResponse,
    {
        Self { inner: endpoint }
    }

    pub async fn run(self, req: Request) -> Result<Response> {
        self.inner.call_boxed(req).await
    }
}

/// Continuation passed to [`Interceptor::wrap_graphql`]. `.await` invokes
/// the next interceptor in the chain (or the resolver itself when this is
/// the innermost wrap).
pub type GraphqlNext<'a> = Pin<Box<dyn Future<Output = ServerResult<GraphqlValue>> + Send + 'a>>;

/// Continuation passed to [`Interceptor::wrap_ws`]. `.await` invokes the
/// next interceptor in the chain (or the message handler itself when this
/// is the innermost wrap).
pub type WsNext<'a> =
    Pin<Box<dyn Future<Output = std::result::Result<Option<JsonValue>, String>> + Send + 'a>>;

/// Type-erased view of any `Endpoint<Output: IntoResponse>`. Lets [`Next`]
/// hold any inner endpoint without leaking the concrete `E` generic across
/// the [`Interceptor`] trait (which would force every impl to also be generic).
trait ErasedEndpoint {
    fn call_boxed<'a>(
        &'a self,
        req: Request,
    ) -> Pin<Box<dyn Future<Output = Result<Response>> + Send + 'a>>;
}

impl<E> ErasedEndpoint for E
where
    E: Endpoint + Send + Sync,
    E::Output: IntoResponse,
{
    fn call_boxed<'a>(
        &'a self,
        req: Request,
    ) -> Pin<Box<dyn Future<Output = Result<Response>> + Send + 'a>> {
        Box::pin(async move { self.call(req).await.map(IntoResponse::into_response) })
    }
}

pub struct InterceptorEndpoint<E, I> {
    inner: E,
    interceptor: I,
}

impl<E, I> InterceptorEndpoint<E, I> {
    pub fn new(inner: E, interceptor: I) -> Self {
        Self { inner, interceptor }
    }
}

impl<E, I> Endpoint for InterceptorEndpoint<E, I>
where
    E: Endpoint + Send + Sync,
    E::Output: IntoResponse,
    I: Interceptor,
{
    type Output = Response;

    async fn call(&self, req: Request) -> Result<Self::Output> {
        let next = Next::new(&self.inner);
        self.interceptor.intercept(req, next).await
    }
}

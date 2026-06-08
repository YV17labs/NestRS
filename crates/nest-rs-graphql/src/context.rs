//! Per-request context bridge: forward selected poem request values into the
//! async-graphql context. Needed because async-graphql-poem does not forward
//! poem request extensions, and an async-graphql `Extension` never sees the
//! poem request. [`ContextEndpoint`] folds every link-time-registered
//! [`GraphqlContextSeed`] over the parsed request before executing it.

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_graphql::{BatchRequest, Executor, Request as GqlRequest};
use async_graphql_poem::{GraphQLBatchRequest, GraphQLBatchResponse};
use nest_rs_core::{Container, ReachableProviders};
use poem::{Endpoint, FromRequest, IntoResponse, Request, Response, Result};

/// A per-request forwarder, submitted via `inventory`. `seed` reads from the
/// poem request (and the container) and attaches values to the GraphQL
/// request.
///
/// `owner_type_id == None` => framework-level seed, always fires.
/// `Some(id)` => fires only when the owner is in `ReachableProviders`, so
/// two GraphQL apps in one workspace can forward different principal types
/// without colliding.
pub struct GraphqlContextSeed {
    pub owner_type_id: fn() -> Option<TypeId>,
    pub seed: fn(&Request, &Container, GqlRequest) -> GqlRequest,
}

inventory::collect!(GraphqlContextSeed);

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Per-operation guard the GraphQL endpoint runs around every request — the
/// resolver-side analog of HTTP's `RouteResponseShaper`. `nestrs-graphql` only
/// defines this seam; `nest_rs_authz::graphql`'s `GraphqlAbilityBridge`
/// implements it to authenticate and install the caller's ambient `Ability`
/// for the operation's duration.
///
/// Bind with `providers = [MyBridge as dyn GraphqlOperationGuard]`; with none
/// registered the endpoint runs operations unguarded.
pub trait GraphqlOperationGuard: Send + Sync + 'static {
    /// Attach per-request state to the poem request before seeds forward it.
    /// Best-effort: an unauthenticated request passes through and the
    /// resolvers' own gate refuses it.
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, ()>;

    /// Wrap the operation's execution to install ambient state for its
    /// duration (e.g. the caller's `Ability`).
    fn around<'a>(
        &'a self,
        req: &'a Request,
        inner: BoxFuture<'a, Response>,
    ) -> BoxFuture<'a, Response>;
}

/// The `/graphql` endpoint. Mirrors `async_graphql_poem::GraphQL`'s GET / POST
/// / batch handling but folds every [`GraphqlContextSeed`] over the request first.
/// The upstream `accept: multipart/mixed` incremental-delivery path
/// (`@defer` / `@stream`) is not reproduced.
pub(crate) struct ContextEndpoint<E> {
    executor: E,
    container: Container,
    op_guard: Option<Arc<dyn GraphqlOperationGuard>>,
}

impl<E> ContextEndpoint<E> {
    pub(crate) fn new(executor: E, container: Container) -> Self {
        let op_guard = container.get_dyn::<dyn GraphqlOperationGuard>();
        Self {
            executor,
            container,
            op_guard,
        }
    }

    fn seed(&self, req: &Request, gql: GqlRequest) -> GqlRequest {
        // Module-gate the inventory: framework-level seeds always fire;
        // owner-keyed seeds fire only when the owner is reachable. A missing
        // gate (hand-rolled container in a test) skips owner-keyed seeds —
        // fail-closed.
        let reachable = self.container.get::<ReachableProviders>();
        inventory::iter::<GraphqlContextSeed>()
            .filter(|reg| match (reg.owner_type_id)() {
                None => true,
                Some(owner) => reachable.as_ref().is_some_and(|r| r.0.contains(&owner)),
            })
            .fold(gql, |gql, reg| (reg.seed)(req, &self.container, gql))
    }
}

impl<E: Executor> Endpoint for ContextEndpoint<E> {
    type Output = Response;

    async fn call(&self, req: Request) -> Result<Response> {
        let (mut req, mut body) = req.split();
        // Guard runs *before* parsing/seeding so attached state is on the
        // request when seeds forward it.
        if let Some(guard) = &self.op_guard {
            guard.before(&mut req).await;
        }
        let batch = GraphQLBatchRequest::from_request(&req, &mut body).await?.0;
        let batch = match batch {
            BatchRequest::Single(r) => BatchRequest::Single(self.seed(&req, r)),
            BatchRequest::Batch(rs) => {
                BatchRequest::Batch(rs.into_iter().map(|r| self.seed(&req, r)).collect())
            }
        };
        let response = match &self.op_guard {
            Some(guard) => {
                let inner: BoxFuture<Response> = Box::pin(async {
                    GraphQLBatchResponse(self.executor.execute_batch(batch).await).into_response()
                });
                guard.around(&req, inner).await
            }
            None => GraphQLBatchResponse(self.executor.execute_batch(batch).await).into_response(),
        };
        Ok(response)
    }
}

/// Forward a per-request value attached by the authentication guard into the
/// GraphQL context, so resolvers read it with `ctx.data::<T>()`.
///
/// ```ignore
/// nest_rs_graphql::forward_principal!(MyPrincipal, MyGraphqlAuthGuard);
/// ```
///
/// The second arg is the owner provider whose module gates the forward — pick
/// a provider declared by the module producing the principal (typically the
/// GraphQL auth guard). `T: Clone + Send + Sync + 'static`. Anonymous requests
/// pass through untouched.
#[macro_export]
macro_rules! forward_principal {
    ($ty:ty, $owner:ty) => {
        $crate::inventory::submit! {
            $crate::GraphqlContextSeed {
                owner_type_id: || ::core::option::Option::Some(::core::any::TypeId::of::<$owner>()),
                seed: |__req, _container, __gql| match __req.extensions().get::<$ty>() {
                    ::core::option::Option::Some(__v) => __gql.data(::core::clone::Clone::clone(__v)),
                    ::core::option::Option::None => __gql,
                },
            }
        }
    };
}

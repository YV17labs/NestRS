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
use poem::http::StatusCode;
use poem::{Endpoint, Error, FromRequest, IntoResponse, Request, Response, Result};

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
/// Bind with `providers = [MyBridge as dyn GraphqlOperationGuard]`. With none
/// registered the endpoint falls back to [`FallbackOperationGuard`] (the
/// global guard pool, seeded by `use_guards_global`) — `/graphql` is
/// `EdgePosture::Exempt` at the HTTP edge, so this in-band seam is the
/// *only* place guards run on GraphQL operations. A registered guard
/// **replaces** the fallback: it owns the chain (the canonical bridge runs
/// the same `AuthGuard` + `AuthzGuard` itself, so nothing runs twice).
pub trait GraphqlOperationGuard: Send + Sync + 'static {
    /// Attach per-request state to the poem request before seeds forward it.
    /// Return `Err(Response)` to reject the operation before parsing.
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, Result<(), Response>>;

    /// Wrap the operation's execution to install ambient state for its
    /// duration (e.g. the caller's `Ability`).
    fn around<'a>(
        &'a self,
        req: &'a Request,
        inner: BoxFuture<'a, Response>,
    ) -> BoxFuture<'a, Response>;
}

/// Factory slot for the fallback [`GraphqlOperationGuard`]. `nest-rs-guards`'
/// `use_guards_global` provides one (a fn pointer — the container does not
/// exist yet at builder time) that folds the global guard pool in-band;
/// [`ContextEndpoint`] invokes it at mount when no `dyn GraphqlOperationGuard`
/// is registered. This is what keeps `/graphql` fail-secure under
/// `EdgePosture::Exempt`: forgetting the authz bridge module does not leave
/// operations unguarded — the global pool still gates them.
pub struct FallbackOperationGuard(pub fn(&Container) -> Arc<dyn GraphqlOperationGuard>);

/// The `/graphql` endpoint. Mirrors `async_graphql_poem::GraphQL`'s GET / POST
/// / batch handling but folds every [`GraphqlContextSeed`] over the request first.
/// The upstream `accept: multipart/mixed` incremental-delivery path
/// (`@defer` / `@stream`) is not reproduced.
pub(crate) struct ContextEndpoint<E> {
    executor: E,
    container: Container,
    op_guard: Option<Arc<dyn GraphqlOperationGuard>>,
    max_batch_size: usize,
}

impl<E> ContextEndpoint<E> {
    pub(crate) fn new(executor: E, container: Container, max_batch_size: usize) -> Self {
        let op_guard = match container.get_dyn::<dyn GraphqlOperationGuard>() {
            Some(guard) => {
                tracing::debug!(
                    target: "nest_rs::graphql",
                    "operations gated by the registered GraphqlOperationGuard",
                );
                Some(guard)
            }
            None => match container.get::<FallbackOperationGuard>() {
                Some(factory) => {
                    tracing::debug!(
                        target: "nest_rs::graphql",
                        "operations gated by the global guard pool (fallback operation guard)",
                    );
                    Some((factory.0)(&container))
                }
                None => {
                    // No global guards, no bridge: the app has no authn
                    // posture, so an unguarded schema is its deliberate
                    // shape — but say so once at boot.
                    tracing::info!(
                        target: "nest_rs::graphql",
                        "no operation guard registered — GraphQL operations run unguarded",
                    );
                    None
                }
            },
        };
        Self {
            executor,
            container,
            op_guard,
            max_batch_size,
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
        if let Some(guard) = &self.op_guard
            && let Err(resp) = guard.before(&mut req).await
        {
            return Ok(resp);
        }
        let batch = GraphQLBatchRequest::from_request(&req, &mut body).await?.0;
        let batch = match batch {
            BatchRequest::Single(r) => BatchRequest::Single(self.seed(&req, r)),
            BatchRequest::Batch(rs) => {
                if rs.len() > self.max_batch_size {
                    return Err(Error::from_status(StatusCode::PAYLOAD_TOO_LARGE));
                }
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

//! Per-request context bridge: forward selected values from the *poem* request
//! into the *async-graphql* context, so a resolver reads per-request state an
//! HTTP guard attached. This is the seam GraphQL authorization needs — the
//! actor's `Ability`, built by an HTTP guard and stored on the request, must
//! reach the resolvers — and it serves any request-scoped value.
//!
//! It is needed because async-graphql-poem does not forward poem request
//! extensions into the graphql context, and an async-graphql `Extension`
//! (`prepare_request`) never sees the poem request. So the bridge lives at the
//! poem endpoint: [`ContextEndpoint`] folds every link-time-registered
//! [`ContextSeed`] over the parsed request before executing it.
//!
//! A resolver reads what a seed attached with a `ctx: &async_graphql::Context`
//! parameter (which `#[query]` / `#[mutation]` forward natively) and
//! `ctx.data_opt::<T>()` — no `#[resolver]` macro support is required.

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_graphql::{BatchRequest, Executor, Request as GqlRequest};
use async_graphql_poem::{GraphQLBatchRequest, GraphQLBatchResponse};
use nestrs_core::{Container, ReachableProviders};
use poem::{Endpoint, FromRequest, IntoResponse, Request, Response, Result};

/// One per-request forwarder, submitted via `inventory`. `seed` reads from the
/// poem request (and the container, for anything it must resolve) and attaches
/// values to the graphql request with [`Request::data`](GqlRequest::data),
/// returning the augmented request. `pub` so a downstream crate
/// (`nestrs_authz::graphql`) can submit one.
///
/// `owner_type_id` returns the `TypeId` of a provider whose module gates this
/// seed: when it returns `Some(id)`, [`ContextEndpoint`] only fires the seed if
/// the id is in `ReachableProviders`; when it returns `None`, the seed is
/// framework-level (e.g. forwarding the ambient `Ability`) and always fires.
/// Module-gating matches `#[resolver]` and `#[dataloader]` so two GraphQL apps
/// in one workspace can forward *different* principal types without colliding.
///
/// ```ignore
/// nestrs_graphql::inventory::submit! {
///     nestrs_graphql::ContextSeed {
///         owner_type_id: || Some(std::any::TypeId::of::<MyGraphqlAuthGuard>()),
///         seed: |req, _container, gql| match req.extensions().get::<Arc<Ability>>() {
///             Some(ability) => gql.data(ability.clone()),
///             None => gql,
///         },
///     }
/// }
/// ```
pub struct ContextSeed {
    pub owner_type_id: fn() -> Option<TypeId>,
    pub seed: fn(&Request, &Container, GqlRequest) -> GqlRequest,
}

inventory::collect!(ContextSeed);

/// A boxed `Send` future — the object-safe currency an [`OperationGuard`] passes
/// the operation's execution through.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A per-operation guard the GraphQL endpoint runs around every request — the
/// resolver-side analog of HTTP's `RouteResponseShaper`. The surface stays
/// authorization-agnostic: `nestrs-graphql` only defines this seam and resolves
/// an optional implementor from the container; `nestrs_authz::graphql`'s
/// `GraphqlAbilityBridge` implements it to authenticate the request and install
/// the caller's ambient `Ability` for the whole operation.
///
/// Bind an implementor with `providers = [MyBridge as dyn OperationGuard]`; the
/// endpoint resolves it via the container (`get_dyn`). With none registered the
/// endpoint runs operations unguarded — exactly the prior behaviour.
pub trait OperationGuard: Send + Sync + 'static {
    /// Run before the operation is parsed and seeded into the GraphQL context.
    /// Authenticate and attach per-request state (e.g. the caller's `Ability`)
    /// to the poem request, where a [`ContextSeed`] forwards it into the
    /// context. Best-effort: an unauthenticated request is left without that
    /// state, and the resolvers' own gate then refuses it.
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, ()>;

    /// Wrap the operation's execution — e.g. installing the caller's ability as
    /// ambient state for its duration so the data layer scopes every read.
    fn around<'a>(
        &'a self,
        req: &'a Request,
        inner: BoxFuture<'a, Response>,
    ) -> BoxFuture<'a, Response>;
}

/// The `/graphql` endpoint [`GraphqlModule`](crate::GraphqlModule) mounts. It
/// mirrors `async_graphql_poem::GraphQL`'s GET / POST / batch handling but folds
/// every [`ContextSeed`] over the request first, so per-request context reaches
/// resolvers. The upstream endpoint's experimental `accept: multipart/mixed`
/// incremental-delivery path (`@defer` / `@stream`) is not reproduced; ordinary
/// queries, mutations and batches behave identically.
pub(crate) struct ContextEndpoint<E> {
    executor: E,
    container: Container,
    /// The per-operation guard, resolved once from the container at mount. `None`
    /// when no app bound one — the endpoint then runs operations unguarded.
    op_guard: Option<Arc<dyn OperationGuard>>,
}

impl<E> ContextEndpoint<E> {
    pub(crate) fn new(executor: E, container: Container) -> Self {
        let op_guard = container.get_dyn::<dyn OperationGuard>();
        Self {
            executor,
            container,
            op_guard,
        }
    }

    fn seed(&self, req: &Request, gql: GqlRequest) -> GqlRequest {
        // Module-gate the inventory the same way the resolver registry and the
        // loader extension do. Framework-level seeds (`owner_type_id() == None`,
        // e.g. the ambient-`Ability` forwarder) always fire. Owner-keyed seeds
        // (`Some(id)`) fire only when the owner is in `ReachableProviders`.
        // The production boot always seeds the gate; a missing gate (a hand-
        // rolled container in a test) skips owner-keyed seeds — fail-closed.
        let reachable = self.container.get::<ReachableProviders>();
        inventory::iter::<ContextSeed>()
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
        // The guard runs *before* parsing/seeding so the state it attaches (the
        // caller's ability/actor) is on the request when the seeds forward it.
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
        // The guard wraps execution so it can install ambient state (the
        // ability) for the operation's whole duration. With no guard, run the
        // batch directly — no boxing, exactly the prior behaviour.
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

/// Forward a per-request value the authentication guard attached to the request —
/// typically the authenticated principal — into the GraphQL context, so resolvers
/// read it with `ctx.data::<T>()`. It registers the [`ContextSeed`] for you, so an
/// app writes one line instead of a hand-rolled `inventory::submit!` closure:
///
/// ```ignore
/// nestrs_graphql::forward_principal!(MyPrincipal, MyGraphqlAuthGuard);
/// ```
///
/// The second arg is the **owner provider** whose module gates the forward — pick
/// a provider declared by the module that produces the principal on the request
/// (typically the GraphQL auth guard). The seed only fires when that provider is
/// reachable, so a workspace shipping two GraphQL apps with different principal
/// types can keep both [`forward_principal!`] calls in the source tree without
/// one app silently seeing the other's principal.
///
/// `T` must be `Clone + Send + Sync + 'static`. A request that carries no such value
/// (anonymous) passes through untouched — the resolver's own `authorize` gate then
/// refuses it. The ambient `Ability` itself is already forwarded by
/// `nestrs_authz::graphql`; this is for the app's own principal type. Exported at the
/// crate root by `#[macro_export]`, so apps call `nestrs_graphql::forward_principal!`.
#[macro_export]
macro_rules! forward_principal {
    ($ty:ty, $owner:ty) => {
        $crate::inventory::submit! {
            $crate::ContextSeed {
                owner_type_id: || ::core::option::Option::Some(::core::any::TypeId::of::<$owner>()),
                seed: |__req, _container, __gql| match __req.extensions().get::<$ty>() {
                    ::core::option::Option::Some(__v) => __gql.data(::core::clone::Clone::clone(__v)),
                    ::core::option::Option::None => __gql,
                },
            }
        }
    };
}

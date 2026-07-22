//! Per-request context bridge: forward selected poem request values into the
//! async-graphql context. Needed because async-graphql-poem does not forward
//! poem request extensions, and an async-graphql `Extension` never sees the
//! poem request. [`ContextEndpoint`] folds every link-time-registered
//! [`GraphqlContextSeed`] over the parsed request before executing it.

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_graphql::parser::types::{DocumentOperations, OperationType};
use async_graphql::{BatchRequest, Executor, Request as GqlRequest};
use async_graphql_poem::{GraphQLBatchRequest, GraphQLBatchResponse};
use nest_rs_core::{Container, ReachableProviders, RequestScope};
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
    /// `None` for a framework-level seed (always fires); `Some(id)` gates the
    /// seed on its owner being reachable, so two apps forward different types
    /// without colliding.
    pub owner_type_id: fn() -> Option<TypeId>,
    /// Reads from the poem request and container and attaches values onto the
    /// outgoing GraphQL request.
    pub seed: fn(&Request, &Container, GqlRequest) -> GqlRequest,
}

inventory::collect!(GraphqlContextSeed);

// Framework-level seed (always fires): forward the per-request `RequestScope`
// installed by the HTTP `RequestScopeEndpoint` (outermost over the whole route
// tree, so a `/graphql` request already carries it) into the async-graphql
// context. Resolvers then reach request-scoped providers via
// [`crate::Scoped<T>`]. Absent (a hand-rolled executor in a test, or a non-HTTP
// mount) ⇒ the request is forwarded untouched.
//
// Caveat: this reaches resolver bodies only. A `#[dataloader]` batch runs
// off-task (its own spawned future) where this context does not propagate —
// batches re-establish ambient state through their own `GraphqlBatchContext`
// seam, not `Scoped<T>`.
inventory::submit! {
    GraphqlContextSeed {
        owner_type_id: || None,
        seed: |req, _container, gql| match req.extensions().get::<Arc<RequestScope>>() {
            Some(scope) => gql.data(Arc::clone(scope)),
            None => gql,
        },
    }
}

/// A boxed, `Send` future — the return type of an async method in a
/// dyn-compatible GraphQL trait.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Per-operation guard the GraphQL endpoint runs around every request — the
/// resolver-side analog of HTTP's `RouteResponseShaper`. `nest-rs-graphql` only
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
/// the same `AuthnGuard` + `AuthzGuard` itself, so nothing runs twice).
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
/// `ContextEndpoint` invokes it at mount when no `dyn GraphqlOperationGuard`
/// is registered. This is what keeps `/graphql` fail-secure under
/// `EdgePosture::Exempt`: forgetting the authz bridge module does not leave
/// operations unguarded — the global pool still gates them.
///
/// **Internal ABI** — a seeded fn-pointer wired by the framework crates
/// (lockstep with `nest-rs-graphql`); not a user-constructed type.
#[doc(hidden)]
pub struct FallbackOperationGuard(pub fn(&Container) -> Arc<dyn GraphqlOperationGuard>);

/// Bridge slot for global pipes on GraphQL operation **variables** — the
/// operation-level analog of HTTP's `transform_body`. `nest-rs-guards`'
/// `use_pipes_global` provides a fn pointer that folds every registered global
/// pipe's [`GlobalPipe::transform_graphql_variables`](nest_rs_pipes::GlobalPipe)
/// over an operation's variables; `ContextEndpoint` invokes it after parsing,
/// before execution. Defined here (the endpoint calls it) and provided by
/// guards (which owns the `PipeSpecs` registry) — the same seeded-fn-pointer
/// pattern as [`FallbackOperationGuard`], since guards depends on this crate,
/// not the reverse. A rejection becomes a GraphQL error response.
///
/// **Internal ABI** — a seeded fn-pointer wired by the framework crates
/// (lockstep with `nest-rs-graphql`); not a user-constructed type.
#[doc(hidden)]
pub struct GraphqlVariablePipe(
    pub fn(&Container, &mut serde_json::Value) -> Result<(), nest_rs_pipes::PipeError>,
);

/// The `/graphql` endpoint. Mirrors `async_graphql_poem::GraphQL`'s GET / POST
/// / batch handling but folds every [`GraphqlContextSeed`] over the request first.
/// The upstream `accept: multipart/mixed` incremental-delivery path
/// (`@defer` / `@stream`) is not reproduced.
/// The per-request step one [`GraphqlContextSeed`] contributes.
type ContextSeed = fn(&Request, &Container, GqlRequest) -> GqlRequest;

pub(crate) struct ContextEndpoint<E> {
    executor: E,
    container: Container,
    op_guard: Option<Arc<dyn GraphqlOperationGuard>>,
    max_batch_size: usize,
    /// The seeds that fire for this app, resolved **once** at mount. The
    /// module gate is an access-graph fact frozen at boot, so re-walking
    /// link-time inventory per request only re-answered it.
    seeds: Arc<[ContextSeed]>,
}

impl<E> ContextEndpoint<E> {
    pub(crate) fn new(executor: E, container: Container, max_batch_size: usize) -> Self {
        let op_guard = match container.get_dyn::<dyn GraphqlOperationGuard>() {
            Some(guard) => {
                tracing::debug!(
                    target: "nest_rs::graphql",
                    mode = "operation_guard",
                    "graphql operations gated",
                );
                Some(guard)
            }
            None => match container.get::<FallbackOperationGuard>() {
                Some(factory) => {
                    tracing::debug!(
                        target: "nest_rs::graphql",
                        mode = "global_guard_pool",
                        "graphql operations gated",
                    );
                    Some((factory.0)(&container))
                }
                None => {
                    // No global guards, no bridge: the app has no authn
                    // posture, so an unguarded schema is its deliberate
                    // shape — but say so once at boot.
                    tracing::warn!(
                        target: "nest_rs::graphql",
                        mode = "unguarded",
                        "no operation guard registered — graphql operations run unguarded",
                    );
                    None
                }
            },
        };
        // Module-gate the inventory once: framework-level seeds always fire;
        // owner-keyed seeds fire only when the owner is reachable. A missing
        // gate (hand-rolled container in a test) skips owner-keyed seeds —
        // fail-closed.
        let reachable = container.get::<ReachableProviders>();
        let seeds: Arc<[_]> = inventory::iter::<GraphqlContextSeed>()
            .filter(|reg| match (reg.owner_type_id)() {
                None => true,
                Some(owner) => reachable.as_ref().is_some_and(|r| r.0.contains(&owner)),
            })
            .map(|reg| reg.seed)
            .collect();
        Self {
            executor,
            container,
            op_guard,
            max_batch_size,
            seeds,
        }
    }

    /// Run the registered global pipes over each operation's variables when a
    /// [`GraphqlVariablePipe`] bridge is provided (`use_pipes_global`). No
    /// bridge ⇒ untouched. A pipe rejection returns a GraphQL error response.
    fn pipe_variables(
        &self,
        batch: BatchRequest,
    ) -> std::result::Result<BatchRequest, Box<Response>> {
        let Some(bridge) = self.container.get::<GraphqlVariablePipe>() else {
            return Ok(batch);
        };
        let apply = |mut r: GqlRequest| -> std::result::Result<GqlRequest, Box<Response>> {
            let mut value = serde_json::to_value(&r.variables).unwrap_or(serde_json::Value::Null);
            if let Err(err) = (bridge.0)(&self.container, &mut value) {
                return Err(variable_pipe_error_response(&err));
            }
            // A pipe may rewrite the variables into a shape that is no longer a
            // GraphQL variables object (a bare array, scalar, or `null`).
            // Deserialization back into `Variables` then fails — surface it as a
            // variable-pipe error naming the failure rather than silently running
            // the operation with no variables (`unwrap_or_default`).
            r.variables = match serde_json::from_value(value) {
                Ok(variables) => variables,
                Err(err) => {
                    return Err(variable_pipe_error_response(
                        &nest_rs_pipes::PipeError::new(format!(
                            "variable pipe produced an invalid variables object: {err}"
                        )),
                    ));
                }
            };
            Ok(r)
        };
        match batch {
            BatchRequest::Single(r) => Ok(BatchRequest::Single(apply(r)?)),
            BatchRequest::Batch(rs) => {
                let mut out = std::vec::Vec::with_capacity(rs.len());
                for r in rs {
                    out.push(apply(r)?);
                }
                Ok(BatchRequest::Batch(out))
            }
        }
    }

    fn seed(&self, req: &Request, gql: GqlRequest) -> GqlRequest {
        self.seeds
            .iter()
            .fold(gql, |gql, seed| seed(req, &self.container, gql))
    }
}

/// Whether **every** operation definition in **every** request of the batch is
/// a `query` — the only shape that provably cannot write, and so the only one
/// safe to run outside the request transaction (DATA-S5).
///
/// Conservative by construction: a `mutation` or `subscription` definition
/// anywhere in the batch answers `false` even when `operationName` selects a
/// query beside it, and so does any parse failure. Misreading a mutation as
/// read-only would run it with no atomicity or rollback, so every uncertain
/// case keeps the transaction.
///
/// Parsing here is not extra work: [`GqlRequest::parsed_query`] caches the
/// document on the request and async-graphql's executor reuses it.
fn is_read_only(batch: &mut BatchRequest) -> bool {
    let requests: &mut [GqlRequest] = match batch {
        BatchRequest::Single(request) => std::slice::from_mut(request),
        BatchRequest::Batch(requests) => requests.as_mut_slice(),
    };
    requests
        .iter_mut()
        .all(|request| match request.parsed_query() {
            Ok(document) => match &document.operations {
                DocumentOperations::Single(op) => op.node.ty == OperationType::Query,
                DocumentOperations::Multiple(ops) => {
                    ops.values().all(|op| op.node.ty == OperationType::Query)
                }
            },
            Err(_) => false,
        })
}

/// Run `fut` on a non-transactional handle when the ambient executor can hand
/// one out — see [`nest_rs_database::Executor::non_transactional`]. Nothing
/// installed (no ORM, or already on the pool) ⇒ the future runs untouched on
/// whatever the request boundary installed.
async fn without_transaction(fut: BoxFuture<'_, Response>) -> Response {
    match nest_rs_database::current_executor().and_then(|executor| executor.non_transactional()) {
        Some(executor) => nest_rs_database::with_request_executor(executor, fut).await,
        None => fut.await,
    }
}

/// Render a variable-pipe `PipeError` as a GraphQL error response — HTTP 200
/// with an `errors` array, the GraphQL wire convention (matching how a resolver
/// error surfaces), with any field-level `details` under `extensions`.
fn variable_pipe_error_response(err: &nest_rs_pipes::PipeError) -> Box<Response> {
    let mut error = serde_json::json!({ "message": err.message() });
    if let Some(details) = err.details() {
        error["extensions"] = serde_json::json!({ "details": details });
    }
    let body = serde_json::json!({ "data": serde_json::Value::Null, "errors": [error] });
    Box::new(
        Response::builder()
            .status(StatusCode::OK)
            .content_type("application/json")
            .body(serde_json::to_vec(&body).unwrap_or_default()),
    )
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
        // Enforce the batch-size cap FIRST — before the variable pipes fold over
        // every operation. Checking it only in the seed match below meant a
        // 10k-op batch paid the full pipe cost before the 413 (GQL-I6).
        if let BatchRequest::Batch(rs) = &batch
            && rs.len() > self.max_batch_size
        {
            return Err(Error::from_status(StatusCode::PAYLOAD_TOO_LARGE));
        }
        // Global variable pipes (operation-level; `transform_graphql_variables`).
        // A rejection short-circuits with a GraphQL error response.
        let batch = match self.pipe_variables(batch) {
            Ok(batch) => batch,
            Err(resp) => return Ok(*resp),
        };
        let mut batch = match batch {
            BatchRequest::Single(r) => BatchRequest::Single(self.seed(&req, r)),
            BatchRequest::Batch(rs) => {
                BatchRequest::Batch(rs.into_iter().map(|r| self.seed(&req, r)).collect())
            }
        };
        // Decided before execution and applied around the whole operation
        // (guard included) so nothing under it opens the request transaction.
        let read_only = is_read_only(&mut batch);
        let inner: BoxFuture<Response> = Box::pin(async {
            GraphQLBatchResponse(self.executor.execute_batch(batch).await).into_response()
        });
        let guarded = match &self.op_guard {
            Some(guard) => guard.around(&req, inner),
            None => inner,
        };
        let response = if read_only {
            without_transaction(guarded).await
        } else {
            guarded.await
        };
        Ok(response)
    }
}

/// Forward a per-request value attached by the authentication guard into the
/// GraphQL context, so resolvers read it with `ctx.data::<T>()`.
///
/// ```ignore
/// nest_rs_graphql::forward_principal!(MyPrincipal, MyGraphqlAuthnGuard);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn single(query: &str) -> BatchRequest {
        BatchRequest::Single(GqlRequest::new(query))
    }

    fn batch(queries: &[&str]) -> BatchRequest {
        BatchRequest::Batch(queries.iter().map(|q| GqlRequest::new(*q)).collect())
    }

    #[test]
    fn a_query_is_read_only() {
        assert!(is_read_only(&mut single("query { me { id } }")));
    }

    #[test]
    fn an_anonymous_shorthand_operation_is_read_only() {
        // The shorthand has no `query` keyword; the spec still makes it a query.
        assert!(is_read_only(&mut single("{ me { id } }")));
    }

    #[test]
    fn an_introspection_query_is_read_only() {
        assert!(is_read_only(&mut single("{ __schema { types { name } } }")));
    }

    #[test]
    fn a_mutation_is_not_read_only() {
        assert!(!is_read_only(&mut single("mutation { createUser { id } }")));
    }

    #[test]
    fn a_subscription_is_not_read_only() {
        assert!(!is_read_only(&mut single(
            "subscription { userAdded { id } }"
        )));
    }

    #[test]
    fn a_document_holding_a_mutation_beside_the_selected_query_is_not_read_only() {
        // `operationName` picks `Read`, but the document still defines a
        // mutation: we classify the whole document, never the selected
        // operation, so a selection bug upstream cannot strand a write outside
        // the transaction.
        let request =
            GqlRequest::new("query Read { me { id } } mutation Write { createUser { id } }")
                .operation_name("Read");
        assert!(!is_read_only(&mut BatchRequest::Single(request)));
    }

    #[test]
    fn a_batch_of_queries_is_read_only() {
        assert!(is_read_only(&mut batch(&["{ me { id } }", "query { a }"])));
    }

    #[test]
    fn a_batch_holding_one_mutation_is_not_read_only() {
        assert!(!is_read_only(&mut batch(&[
            "{ me { id } }",
            "mutation { bump }",
        ])));
    }

    #[test]
    fn an_unparsable_query_is_not_read_only() {
        // The executor will reject it; until then it stays transactional.
        assert!(!is_read_only(&mut single("{{{")));
    }

    #[test]
    fn a_field_named_like_a_mutation_stays_read_only() {
        // Pins the choice of parsing over text matching: a `mutation` substring
        // in a field name must not cost the optimization.
        assert!(is_read_only(&mut single("{ mutationLog { id } }")));
    }
}

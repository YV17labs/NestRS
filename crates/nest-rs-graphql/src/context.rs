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
use async_graphql::{BatchRequest, Data, Executor, Request as GqlRequest};
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
    /// `None` for a framework-level seed (always fires); `Some(id)` gates the
    /// seed on its owner being reachable, so two apps forward different types
    /// without colliding.
    pub owner_type_id: fn() -> Option<TypeId>,
    /// How far the forwarded value is allowed to travel — see [`SeedLifetime`].
    pub lifetime: SeedLifetime,
    /// Reads from the poem request and container and attaches values onto the
    /// outgoing GraphQL request.
    pub seed: fn(&Request, &Container, GqlRequest) -> GqlRequest,
}

/// How long a forwarded value stays valid, which on a graphql-ws socket is a
/// real question rather than a formality: the socket is opened by **one**
/// request and then serves operations for hours.
///
/// A value that is the caller's *identity* is the connection's — that is the
/// whole model of an authenticated socket, and it is why the lifetime ceiling
/// exists. A value that belongs to the *upgrade request* is not: forwarding it
/// would hand every operation on that socket the same per-request state, which
/// is a silent lie rather than a missing feature. So each seed says which it is,
/// and the socket carries only the first kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeedLifetime {
    /// The upgrade request's own state — forwarded on the POST path, and
    /// **dropped** on a socket. `Scoped<T>` then reports the scope as absent,
    /// which is the truth, instead of resolving a request-scoped provider built
    /// once at the upgrade and shared for the connection's life.
    Request,
    /// The caller's identity, established once at the upgrade and valid for as
    /// long as the connection is — a principal, an `Ability`. Forwarded on both
    /// paths.
    Connection,
}

inventory::collect!(GraphqlContextSeed);

// Framework-level seed (always fires): forward the per-request `RequestScope`
// installed by the HTTP transport edge (outermost over the whole route
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
        // The upgrade's scope is the upgrade's. A subscription reaching
        // `Scoped<T>` gets "not installed" rather than an instance built once,
        // hours ago, and shared by every operation on the socket since.
        lifetime: SeedLifetime::Request,
        seed: |_req, _container, gql| match nest_rs_http::current_request_scope() {
            Some(scope) => gql.data(scope),
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

    /// Wrap `inner` to install ambient state for its duration (e.g. the
    /// caller's `Ability`, which `Guard::check_graphql` and the ability-scoped
    /// data layer both read from the ambient slot).
    ///
    /// The future returns `()` rather than a `Response` because the two things
    /// that need scoping are not the same shape: one HTTP operation, and one
    /// graphql-ws **socket**, which lives across many operations and answers
    /// with frames rather than a response. One method for both is the point —
    /// a socket that installed different ambient state than the POST path would
    /// be the same endpoint enforcing two postures.
    fn around<'a>(&'a self, req: &'a Request, inner: BoxFuture<'a, ()>) -> BoxFuture<'a, ()>;
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

/// What both `/graphql` endpoints — the request/response one and the graphql-ws
/// one — need from the container, resolved **once** at mount: which guard gates
/// operations, and which context seeds fire for this app.
///
/// Shared rather than resolved twice, and that is the point: a socket and a POST
/// that disagreed about who is authenticated, or about which principal type is
/// forwarded, would be the same endpoint enforcing two postures.
pub(crate) struct OperationBridge {
    pub(crate) container: Container,
    pub(crate) op_guard: Option<Arc<dyn GraphqlOperationGuard>>,
    /// The seeds that fire for this app. The module gate is an access-graph
    /// fact frozen at boot, so re-walking link-time inventory per request only
    /// re-answered it.
    seeds: Arc<[ContextSeed]>,
    /// The subset a graphql-ws connection inherits from its upgrade — see
    /// [`SeedLifetime`]. Resolved at mount beside the full set rather than
    /// filtered per connection, for the same reason.
    connection_seeds: Arc<[ContextSeed]>,
}

pub(crate) struct ContextEndpoint<E> {
    executor: E,
    bridge: Arc<OperationBridge>,
    max_batch_size: usize,
}

impl OperationBridge {
    pub(crate) fn new(container: Container) -> Self {
        let op_guard = match container.get_dyn::<dyn GraphqlOperationGuard>() {
            Some(guard) => {
                tracing::debug!(
                    target: crate::TARGET,
                    mode = "operation_guard",
                    "graphql operations gated",
                );
                Some(guard)
            }
            None => match container.get::<FallbackOperationGuard>() {
                Some(factory) => {
                    tracing::debug!(
                        target: crate::TARGET,
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
                        target: crate::TARGET,
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
        let active: Vec<&GraphqlContextSeed> = inventory::iter::<GraphqlContextSeed>()
            .filter(|reg| match (reg.owner_type_id)() {
                None => true,
                Some(owner) => reachable.as_ref().is_some_and(|r| r.0.contains(&owner)),
            })
            .collect();
        let seeds: Arc<[_]> = active.iter().map(|reg| reg.seed).collect();
        let connection_seeds: Arc<[_]> = active
            .iter()
            .filter(|reg| reg.lifetime == SeedLifetime::Connection)
            .map(|reg| reg.seed)
            .collect();
        Self {
            connection_seeds,
            container,
            op_guard,
            seeds,
        }
    }

    fn seed(&self, req: &Request, gql: GqlRequest) -> GqlRequest {
        self.seeds
            .iter()
            .fold(gql, |gql, seed| seed(req, &self.container, gql))
    }

    /// The connection-level [`Data`] for a graphql-ws socket.
    ///
    /// The seeds are written against a `Request` because that is what an
    /// operation carries on the POST path; a socket has no per-operation HTTP
    /// request, only the upgrade. So they fold over a scratch request whose
    /// `data` is then taken — one implementation of "what this app forwards",
    /// rather than a parallel set of socket seeds that could forward a
    /// different principal than the POST endpoint does.
    ///
    /// Only [`SeedLifetime::Connection`] seeds fold: the identity established at
    /// the upgrade is the connection's, the upgrade's *request* state is not.
    pub(crate) fn connection_data(&self, req: &Request) -> Data {
        self.connection_seeds
            .iter()
            .fold(GqlRequest::new(""), |gql, seed| {
                seed(req, &self.container, gql)
            })
            .data
    }
}

impl<E> ContextEndpoint<E> {
    pub(crate) fn new(executor: E, bridge: Arc<OperationBridge>, max_batch_size: usize) -> Self {
        Self {
            executor,
            bridge,
            max_batch_size,
        }
    }

    /// Run the registered global pipes over each operation's variables when a
    /// [`GraphqlVariablePipe`] bridge is provided (`use_pipes_global`). No
    /// bridge ⇒ untouched. A pipe rejection returns a GraphQL error response.
    fn pipe_variables(
        &self,
        batch: BatchRequest,
    ) -> std::result::Result<BatchRequest, Box<Response>> {
        let container = &self.bridge.container;
        let Some(bridge) = container.get::<GraphqlVariablePipe>() else {
            return Ok(batch);
        };
        let apply = |mut r: GqlRequest| -> std::result::Result<GqlRequest, Box<Response>> {
            let mut value = serde_json::to_value(&r.variables).unwrap_or(serde_json::Value::Null);
            if let Err(err) = (bridge.0)(container, &mut value) {
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
async fn without_transaction(fut: BoxFuture<'_, ()>) {
    match nest_rs_database::current_executor().and_then(|executor| executor.non_transactional()) {
        Some(executor) => nest_rs_database::with_request_executor(executor, fut).await,
        None => fut.await,
    }
}

/// Render a variable-pipe `PipeError` as a GraphQL error response — HTTP 200
/// with an `errors` array, the GraphQL wire convention (matching how a resolver
/// error surfaces), with any field-level errors under
/// `extensions.errors` — the same member name every other transport uses.
fn variable_pipe_error_response(err: &nest_rs_pipes::PipeError) -> Box<Response> {
    let mut error = serde_json::json!({ "message": err.message() });
    if let Some(details) = err.details() {
        error["extensions"] = serde_json::json!({ crate::FIELD_ERRORS_EXTENSION: details });
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
        if let Some(guard) = &self.bridge.op_guard
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
            BatchRequest::Single(r) => BatchRequest::Single(self.bridge.seed(&req, r)),
            BatchRequest::Batch(rs) => {
                BatchRequest::Batch(rs.into_iter().map(|r| self.bridge.seed(&req, r)).collect())
            }
        };
        // Decided before execution and applied around the whole operation
        // (guard included) so nothing under it opens the request transaction.
        let read_only = is_read_only(&mut batch);
        // The response travels out through a local slot because the guard scopes
        // a `()` future — see [`GraphqlOperationGuard::around`] for why that is
        // the shape. A borrow, not a channel: the future ends at the `.await`
        // below, which is what lets the answer be read straight after.
        let mut answered: Option<Response> = None;
        let inner: BoxFuture<()> = Box::pin(async {
            answered = Some(
                GraphQLBatchResponse(self.executor.execute_batch(batch).await).into_response(),
            );
        });
        let guarded = match &self.bridge.op_guard {
            Some(guard) => guard.around(&req, inner),
            None => inner,
        };
        if read_only {
            without_transaction(guarded).await;
        } else {
            guarded.await;
        }
        Ok(answered.unwrap_or_else(|| {
            // Unreachable unless a panic unwound past the executor: report it
            // rather than serve an empty 200.
            tracing::error!(
                target: crate::TARGET,
                reason = "no_response",
                "the guarded operation produced no response",
            );
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .finish()
        }))
    }
}

/// Forward a per-request value attached by the authentication guard into the
/// GraphQL context, so resolvers read it with `ctx.data::<T>()`.
///
/// ```ignore
/// nest_rs_graphql::forward_principal!(MyPrincipal);
/// ```
///
/// `T: Clone + Send + Sync + 'static`. Anonymous requests pass through
/// untouched, which is also the whole gate: the forwarder copies a value only
/// if something already attached that value to the request, and the only thing
/// that does is the authn guard — already module-gated. A second gate below it
/// would be an owner provider the consumer declares, which is an empty marker
/// struct in every app, silent when forgotten, and *wider* than the condition
/// it replaces: a registered marker fires the forwarder whether or not anyone
/// authenticated. See the *Hard "no" list*'s module-gating entry, which records
/// why a request-scoped forwarder is not discovery.
#[macro_export]
macro_rules! forward_principal {
    ($ty:ty) => {
        $crate::inventory::submit! {
            $crate::GraphqlContextSeed {
                owner_type_id: || ::core::option::Option::None,
                // A principal is established once, at the request that carries
                // it — including the upgrade of a graphql-ws socket, whose
                // operations are that principal's for as long as it is open.
                lifetime: $crate::SeedLifetime::Connection,
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

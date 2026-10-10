//! Per-request context bridge: async-graphql-poem does not forward poem request
//! extensions, so [`ContextEndpoint`] folds every link-time-registered
//! [`GraphqlContextSeed`] over the parsed request before executing it.

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_graphql::parser::types::{DocumentOperations, OperationType};
use async_graphql::{BatchRequest, Data, Executor, Request as GqlRequest};
use async_graphql_poem::{GraphQLBatchRequest, GraphQLBatchResponse};
use nest_rs_core::{Container, ReachableProviders};
use poem::http::{StatusCode, header};
use poem::{Endpoint, Error, FromRequest, IntoResponse, Request, RequestBody, Response, Result};

/// A per-request forwarder, submitted via `inventory`, attaching values from
/// the poem request (and the container) to the GraphQL request.
pub struct GraphqlContextSeed {
    /// `None` for a framework-level seed (always fires); `Some(id)` gates the
    /// seed on its owner being reachable, so two apps forward different types
    /// without colliding.
    pub owner_type_id: fn() -> Option<TypeId>,
    /// How far the forwarded value may travel.
    pub lifetime: SeedLifetime,
    /// Attaches values onto the outgoing GraphQL request.
    pub seed: fn(&Request, &Container, GqlRequest) -> GqlRequest,
}

/// How long a forwarded value stays valid — on a graphql-ws socket, opened by
/// **one** request that then serves operations for hours, only the caller's
/// identity carries over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeedLifetime {
    /// The upgrade request's own state — forwarded on the POST path, and
    /// **dropped** on a socket, where `Scoped<T>` then reports the scope absent.
    Request,
    /// The caller's identity — a principal, an `Ability` — forwarded on both paths.
    Connection,
}

inventory::collect!(GraphqlContextSeed);

// Forwards the HTTP edge's `RequestScope` for `Scoped<T>`. It reaches resolver
// bodies only: a `#[dataloader]` batch runs off-task.
inventory::submit! {
    GraphqlContextSeed {
        owner_type_id: || None,
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

/// Per-operation guard the GraphQL endpoint runs around every request,
/// implemented by `nest_rs_authz::graphql`'s `GraphqlAbilityBridge`.
///
/// Bind with `providers = [MyBridge as dyn GraphqlOperationGuard]`. `/graphql`
/// is `EdgePosture::Exempt` at the HTTP edge, so this seam is the **only**
/// place guards run on GraphQL operations: with none registered the endpoint
/// falls back to the global guard pool, and a registered guard replaces it.
pub trait GraphqlOperationGuard: Send + Sync + 'static {
    /// Attach per-request state to the poem request before seeds forward it.
    /// Return `Err(Response)` to reject the operation before parsing.
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, Result<(), Response>>;

    /// Wrap `inner` to install ambient state (the caller's `Ability`) for its
    /// duration. It scopes both one HTTP operation and one graphql-ws socket,
    /// hence a `()` future rather than a `Response`.
    fn around<'a>(&'a self, req: &'a Request, inner: BoxFuture<'a, ()>) -> BoxFuture<'a, ()>;
}

/// Factory slot for the fallback [`GraphqlOperationGuard`], seeded by
/// `nest-rs-guards`' `use_guards_global` so a missing authz bridge still leaves
/// operations gated by the global pool.
pub struct FallbackOperationGuard(pub fn(&Container) -> Arc<dyn GraphqlOperationGuard>);

/// Bridge slot for global pipes on GraphQL operation **variables**, seeded by
/// `nest-rs-guards`' `use_pipes_global` with a fold of every
/// [`GlobalPipe::transform_graphql_variables`](nest_rs_pipes::GlobalPipe).
pub struct GraphqlVariablePipe(
    pub fn(&Container, &mut serde_json::Value) -> Result<(), nest_rs_pipes::PipeError>,
);

/// The per-request step one [`GraphqlContextSeed`] contributes.
type ContextSeed = fn(&Request, &Container, GqlRequest) -> GqlRequest;

/// What both `/graphql` endpoints — POST and graphql-ws — need from the
/// container, resolved **once** at mount so the two cannot enforce different
/// postures.
pub(crate) struct OperationBridge {
    pub(crate) container: Container,
    pub(crate) op_guard: Option<Arc<dyn GraphqlOperationGuard>>,
    /// The seeds that fire for this app, module-gated once at mount.
    seeds: Arc<[ContextSeed]>,
    /// The subset a graphql-ws connection inherits from its upgrade.
    connection_seeds: Arc<[ContextSeed]>,
}

/// The `/graphql` endpoint: `async_graphql_poem::GraphQL`'s GET / POST / batch
/// handling with every seed folded over the request first. The `multipart/mixed`
/// incremental-delivery path (`@defer` / `@stream`) is not reproduced.
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
                    tracing::warn!(
                        target: crate::TARGET,
                        mode = "unguarded",
                        "no operation guard registered — graphql operations run unguarded",
                    );
                    None
                }
            },
        };
        // Without `ReachableProviders`, owner-keyed seeds are skipped: fail-closed.
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

    /// The connection-level [`Data`] for a graphql-ws socket: the
    /// [`SeedLifetime::Connection`] seeds folded over a scratch request whose
    /// `data` is then taken, so the socket forwards what the POST path does.
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

    /// Run the global pipes over each operation's variables when a
    /// [`GraphqlVariablePipe`] bridge is provided; a rejection returns a GraphQL
    /// error response.
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
            // GraphQL variables object.
            r.variables = match serde_json::from_value(value) {
                Ok(variables) => variables,
                Err(err) => {
                    return Err(variable_pipe_error_response(
                        &nest_rs_pipes::PipeError::new(format!(
                            "variable pipe produced an invalid variables object: {}",
                            nest_rs_core::DecodeError::new(&err),
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
/// a `query`, the only shape safe to run outside the request transaction.
///
/// Conservative: a `mutation` or `subscription` definition anywhere answers
/// `false` even when `operationName` selects a query, and so does a parse
/// failure — a misread mutation would run with no atomicity. The parse is cached
/// for the executor ([`GqlRequest::parsed_query`]).
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
/// one out ([`nest_rs_database::Executor::non_transactional`]).
async fn without_transaction(fut: BoxFuture<'_, ()>) {
    match nest_rs_database::current_executor().and_then(|executor| executor.non_transactional()) {
        Some(executor) => nest_rs_database::with_request_executor(executor, fut).await,
        None => fut.await,
    }
}

/// The request's GraphQL batch, or the answer to a body that is not one.
///
/// JSON is decoded here rather than by async-graphql: `BatchRequest` is an
/// untagged enum whose serde error names neither variant, so a failed body is
/// re-read as the shape it was to say why.
#[expect(
    clippy::map_err_ignore,
    reason = "the refusal describes the body; the read and decode errors would quote it"
)]
async fn read_batch(req: &Request, body: &mut RequestBody) -> Result<BatchRequest, Response> {
    let multipart = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .trim_start()
                .get(..10)
                .is_some_and(|kind| kind.eq_ignore_ascii_case("multipart/"))
        });
    if multipart {
        return GraphQLBatchRequest::from_request(req, body)
            .await
            .map(|batch| batch.0)
            .map_err(|refusal| {
                let reason = match refusal.downcast_ref::<async_graphql::ParseRequestError>() {
                    Some(parse) => nest_rs_core::error_message(parse),
                    None => UNREADABLE.to_owned(),
                };
                refused(&reason, refusal.status())
            });
    }
    let bytes = match body.take() {
        Ok(taken) => taken
            .into_vec()
            .await
            .map_err(|_| refused(UNREADABLE, StatusCode::BAD_REQUEST))?,
        Err(_) => return Err(refused(UNREADABLE, StatusCode::BAD_REQUEST)),
    };
    serde_json::from_slice::<BatchRequest>(&bytes)
        .map_err(|_| refused(&what_it_is_not(&bytes), StatusCode::BAD_REQUEST))
}

/// What a body that failed to read is said as; the body cap answers `413` itself.
const UNREADABLE: &str = "the request body could not be read";

/// Why `bytes` is not a GraphQL request, as the shape it was decodes it.
fn what_it_is_not(bytes: &[u8]) -> String {
    let failed = match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(serde_json::Value::Array(_)) => serde_json::from_slice::<Vec<GqlRequest>>(bytes).err(),
        Ok(_) => serde_json::from_slice::<GqlRequest>(bytes).err(),
        Err(not_json) => Some(not_json),
    };
    match failed {
        Some(failed) => nest_rs_core::DecodeError::new(&failed).to_string(),
        None => "neither a GraphQL request nor a list of them".to_owned(),
    }
}

/// Answer a request that is not a GraphQL request: GraphQL-over-HTTP's 4xx with
/// an `errors` entry, and no `data` member since nothing was executed.
fn refused(reason: &str, status: StatusCode) -> Response {
    tracing::debug!(
        target: crate::TARGET,
        reason,
        "graphql request refused: the body is not a GraphQL request",
    );
    let body = serde_json::json!({
        "errors": [{ "message": format!("the body is not a GraphQL request: {reason}") }],
    });
    Response::builder()
        .status(status)
        .content_type("application/json")
        .body(serde_json::to_vec(&body).unwrap_or_default())
}

/// Render a variable-pipe `PipeError` as a GraphQL error response — HTTP 200
/// with an `errors` array, field-level errors under `extensions.errors`.
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
        if let Some(guard) = &self.bridge.op_guard
            && let Err(resp) = guard.before(&mut req).await
        {
            return Ok(resp);
        }
        let batch = match read_batch(&req, &mut body).await {
            Ok(batch) => batch,
            Err(refusal) => return Ok(refusal),
        };
        // Before the variable pipes fold over every operation.
        if let BatchRequest::Batch(rs) = &batch
            && rs.len() > self.max_batch_size
        {
            return Err(Error::from_status(StatusCode::PAYLOAD_TOO_LARGE));
        }
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
        let read_only = is_read_only(&mut batch);
        // A local slot, since the guard scopes a `()` future.
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
/// ```
/// use nest_rs_graphql::async_graphql::{Context, Result};
/// use nest_rs_graphql::{operations, resolver};
/// # use nest_rs_core::module;
/// # use nest_rs_graphql::GraphqlModule;
///
/// #[derive(Clone)]
/// struct MyPrincipal(String);
///
/// nest_rs_graphql::forward_principal!(MyPrincipal);
///
/// #[resolver]
/// struct MeResolver;
///
/// #[operations]
/// impl MeResolver {
///     #[query]
///     #[public]
///     async fn me(&self, ctx: &Context<'_>) -> Result<String> {
///         Ok(ctx.data::<MyPrincipal>()?.0.clone())
///     }
/// }
/// # #[module(imports = [GraphqlModule::for_root(None)], providers = [MeResolver])]
/// # struct AppModule;
/// #
/// # #[nest_rs_core::main]
/// # async fn main() -> nest_rs_core::anyhow::Result<()> {
/// # let app = nest_rs_testing::TestApp::for_module::<AppModule>().await?;
///
/// // Attached here the way the authentication guard attaches it.
/// let resp = app
///     .http()
///     .post("/graphql")
///     .data(MyPrincipal("ada".into()))
///     .body_json(&serde_json::json!({ "query": "{ me }" }))
///     .send()
///     .await;
/// resp.assert_json(serde_json::json!({ "data": { "me": "ada" } })).await;
/// # Ok(())
/// # }
/// ```
///
/// `T: Clone + Send + Sync + 'static`. Anonymous requests pass through
/// untouched: the forwarder copies only a value the module-gated authn guard
/// attached.
// Not module-gated itself: `.claude/decisions/forward-principal.md`.
#[macro_export]
macro_rules! forward_principal {
    ($ty:ty) => {
        $crate::__private::inventory::submit! {
            $crate::GraphqlContextSeed {
                owner_type_id: || ::core::option::Option::None,
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
        assert!(!is_read_only(&mut single("{{{")));
    }

    #[test]
    fn a_field_named_like_a_mutation_stays_read_only() {
        assert!(is_read_only(&mut single("{ mutationLog { id } }")));
    }
}

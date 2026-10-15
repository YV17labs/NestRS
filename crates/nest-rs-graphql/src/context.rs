//! Per-request context bridge: async-graphql does not see the HTTP request, so
//! [`ContextEndpoint`] folds every link-time-registered [`GraphqlContextSeed`]
//! over the parsed request before executing it.

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_graphql::parser::types::{DocumentOperations, OperationType};
use async_graphql::{BatchRequest, Data, Executor, Request as GqlRequest};
use async_graphql_poem::{GraphQLBatchRequest, GraphQLBatchResponse};
use nest_rs_core::__private::serde::Serialize;
use nest_rs_core::__private::serde::de::DeserializeOwned;
use nest_rs_core::__private::{HandlerDeclaration, MetaLevels, ResolvedLayer, declare_handler};
use nest_rs_core::{Container, Handler, Posture, ReachableProviders};
use nest_rs_guards::__private::{resolve_global_chain, split_at_covered};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::__private::poem_bridge::{
    request_from_poem, response_from_poem, response_to_poem,
};
use nest_rs_http::__private::{check_request_chain, denial_to_http_response, install_attached};
use nest_rs_http::{HeaderValue, Request, Response, StatusCode};
use nest_rs_pipes::{GlobalPipe, PipeError};
use poem::{Endpoint, Error, FromRequest, IntoResponse, RequestBody, Result};

use crate::module::GraphqlModule;

/// A per-request forwarder, submitted via `inventory`, attaching values from
/// the HTTP request (and the container) to the GraphQL request.
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

/// Per-operation guard the GraphQL endpoint runs at every request,
/// implemented by `nest_rs_authz::graphql`'s `GraphqlAbilityBridge`.
///
/// Bind with `providers = [MyBridge as dyn GraphqlOperationGuard]`. `/graphql`
/// gates in band, so the request's guards run here: the global guard pool
/// always runs, and a registered guard adds to it, never replaces it.
pub trait GraphqlOperationGuard: Send + Sync + 'static {
    /// Check the request before seeds forward it, keeping what the checks
    /// attach on it (`nest_rs_http`'s request guard stage). Return
    /// `Err(Response)` to reject the operation before parsing.
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, Result<(), Response>>;

    /// Wrap `inner` for its duration. It scopes both one HTTP operation and one
    /// graphql-ws socket, hence a `()` future rather than a `Response`. What
    /// `before` attached is already ambient there; the default passes through.
    fn around<'a>(&'a self, req: &'a Request, inner: BoxFuture<'a, ()>) -> BoxFuture<'a, ()> {
        let _ = req;
        inner
    }

    /// The guards [`before`](Self::before) runs itself, by type. The global pool
    /// leaves each of them to this guard, so a guard listed in both runs once.
    fn covers(&self) -> &[TypeId] {
        &[]
    }
}

/// The site every `/graphql` request runs as before its operations: an
/// endpoint whose operations each declare their own posture, so an
/// authentication guard admits an anonymous request there while still
/// verifying a presented credential.
///
/// Its address is the default mount, `/graphql`, wherever
/// [`GraphqlConfig::path`](crate::GraphqlConfig::path) moves it: an app serves
/// one schema, so the address names one endpoint either way.
pub static ENDPOINT: Handler = declare_handler(HandlerDeclaration {
    unit: nest_rs_http::unit::REQUEST,
    host: TypeId::of::<GraphqlModule>(),
    host_name: "GraphqlModule",
    method: "",
    address: "/graphql",
    posture: Posture::InBand,
    meta: MetaLevels::default,
});

/// The per-request step one [`GraphqlContextSeed`] contributes.
type ContextSeed = fn(&Request, &Container, GqlRequest) -> GqlRequest;

/// What both `/graphql` endpoints — POST and graphql-ws — need from the
/// container, resolved **once** at mount so the two cannot enforce different
/// postures. The global pipes are the exception: the graphql-ws engine takes
/// its operations without them.
pub(crate) struct OperationBridge {
    pub(crate) container: Container,
    /// The global pool's guards listed before the first one the bridge covers.
    head: Vec<ResolvedLayer<dyn Guard>>,
    /// The registered [`GraphqlOperationGuard`], run where the pool lists the
    /// first guard it covers, else after the pool.
    bridge: Option<Arc<dyn GraphqlOperationGuard>>,
    /// The rest of the pool, minus what the bridge covers.
    tail: Vec<ResolvedLayer<dyn Guard>>,
    /// The global pipes folded over each POST operation's variables.
    pipes: Arc<[Arc<dyn GlobalPipe>]>,
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
        let bridge = container.get_dyn::<dyn GraphqlOperationGuard>();
        let pool = resolve_global_chain(&container, "POST /graphql (operation)");
        match (pool.is_empty(), &bridge) {
            (false, bridge) => tracing::debug!(
                target: crate::TARGET,
                mode = "global_guard_pool",
                bridge = bridge.is_some(),
                "graphql operations gated",
            ),
            (true, Some(_)) => tracing::debug!(
                target: crate::TARGET,
                mode = "operation_guard",
                "graphql operations gated",
            ),
            (true, None) => tracing::warn!(
                target: crate::TARGET,
                mode = "unguarded",
                "no operation guard registered — graphql operations run unguarded",
            ),
        }
        let covers = bridge.as_deref().map_or(&[][..], |bridge| bridge.covers());
        let (head, tail) = split_at_covered(pool, covers);
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
        let pipes = nest_rs_pipes::__private::resolve_global_pipes(&container);
        Self {
            connection_seeds,
            container,
            head,
            bridge,
            tail,
            pipes,
            seeds,
        }
    }

    /// Run the request's guards — the pool, the bridge where it covers — on
    /// `req`, keeping what they attach on it; the refusal's answer otherwise.
    pub(crate) async fn before(&self, req: &mut Request) -> Result<(), Response> {
        // The pool's refusal keeps its status: a pooled throttler's `429` stays
        // a `429`.
        check_request_chain(&ENDPOINT, &self.head, req)
            .await
            .map_err(denied)?;
        if let Some(bridge) = &self.bridge {
            bridge.before(req).await?;
        }
        check_request_chain(&ENDPOINT, &self.tail, req)
            .await
            .map_err(denied)
    }

    /// `inner`, as the bridge wraps it.
    pub(crate) fn around<'a>(
        &'a self,
        req: &'a Request,
        inner: BoxFuture<'a, ()>,
    ) -> BoxFuture<'a, ()> {
        match &self.bridge {
            Some(bridge) => bridge.around(req, inner),
            None => inner,
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

/// A request guard's refusal, as the endpoint answers it.
fn denied(denial: Denial) -> Response {
    response_from_poem(denial_to_http_response(denial))
}

impl<E> ContextEndpoint<E> {
    pub(crate) fn new(executor: E, bridge: Arc<OperationBridge>, max_batch_size: usize) -> Self {
        Self {
            executor,
            bridge,
            max_batch_size,
        }
    }

    /// Run the global pipes over each operation's variables; a refusal returns
    /// a GraphQL error response.
    fn pipe_variables(
        &self,
        batch: BatchRequest,
    ) -> std::result::Result<BatchRequest, Box<Response>> {
        let pipes = &self.bridge.pipes;
        if pipes.is_empty() {
            return Ok(batch);
        }
        let apply = |mut r: GqlRequest| -> std::result::Result<GqlRequest, Box<Response>> {
            r.variables = fold_variables(&r.variables, pipes)?;
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

/// Fold `pipes` over `variables` through the JSON form a pipe reads; a pipe's
/// refusal, and variables that do not survive the round trip, answer as a
/// GraphQL error.
fn fold_variables<T: Serialize + DeserializeOwned>(
    variables: &T,
    pipes: &[Arc<dyn GlobalPipe>],
) -> std::result::Result<T, Box<Response>> {
    let mut value = serde_json::to_value(variables).map_err(|err| {
        let err = nest_rs_core::DecodeError::new(&err);
        tracing::error!(
            target: crate::TARGET,
            error = %err,
            "graphql variables could not be written as JSON for the global pipes",
        );
        variable_pipe_error_response(&PipeError::new(format!(
            "the operation variables could not be read as JSON: {err}",
        )))
    })?;
    for pipe in pipes {
        pipe.transform_graphql_variables(&mut value)
            .map_err(|err| variable_pipe_error_response(&err))?;
    }
    // A pipe may rewrite the variables into a shape that is no longer a
    // GraphQL variables object.
    serde_json::from_value(value).map_err(|err| {
        variable_pipe_error_response(&PipeError::new(format!(
            "variable pipe produced an invalid variables object: {}",
            nest_rs_core::DecodeError::new(&err),
        )))
    })
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
    let multipart = req.content_type().filter(|value| {
        value
            .trim_start()
            .get(..10)
            .is_some_and(|kind| kind.eq_ignore_ascii_case("multipart/"))
    });
    if let Some(content_type) = multipart {
        // The adapter's extractor reads the method, the query and the content type alone.
        let head = poem::Request::builder()
            .method(req.method().clone())
            .uri(req.uri().clone())
            .content_type(content_type)
            .finish();
        return GraphQLBatchRequest::from_request(&head, body)
            .await
            .map(|batch| batch.0)
            .map_err(|refusal| {
                let reason = match refusal.downcast_ref::<async_graphql::ParseRequestError>() {
                    Some(parse) => multipart_refusal(parse),
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

/// Why a multipart body is not a GraphQL request, read by variant: the
/// `operations` and `map` parts' decoders wrap serde's sentence, which quotes
/// the part; every other variant's own sentence is fixed.
fn multipart_refusal(parse: &async_graphql::ParseRequestError) -> String {
    use async_graphql::ParseRequestError as Refused;

    fn part(name: &str, source: &(dyn std::error::Error + Send + Sync + 'static)) -> String {
        match source.downcast_ref::<serde_json::Error>() {
            Some(error) => format!(
                "the `{name}` part: {}",
                nest_rs_core::DecodeError::new(error)
            ),
            None => format!("the `{name}` part does not decode"),
        }
    }

    match parse {
        Refused::InvalidRequest(source) => part("operations", source.as_ref()),
        Refused::InvalidFilesMap(source) => part("map", source.as_ref()),
        Refused::Io(_) => UNREADABLE.to_owned(),
        fixed => fixed.to_string(),
    }
}

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
        .content_type(HeaderValue::from_static("application/json"))
        .body(serde_json::to_vec(&body).unwrap_or_default())
}

/// Render a variable-pipe `PipeError` as a GraphQL error response — HTTP 200
/// with an `errors` array, field-level errors under `extensions.errors`.
fn variable_pipe_error_response(err: &PipeError) -> Box<Response> {
    let mut error = serde_json::json!({ "message": err.message() });
    if let Some(details) = err.details() {
        error["extensions"] = serde_json::json!({ crate::FIELD_ERRORS_EXTENSION: details });
    }
    let body = serde_json::json!({ "data": serde_json::Value::Null, "errors": [error] });
    Box::new(
        Response::builder()
            .status(StatusCode::OK)
            .content_type(HeaderValue::from_static("application/json"))
            .body(serde_json::to_vec(&body).unwrap_or_default()),
    )
}

impl<E: Executor> Endpoint for ContextEndpoint<E> {
    type Output = poem::Response;

    async fn call(&self, req: poem::Request) -> Result<poem::Response> {
        let (req, body) = req.split();
        let mut req = request_from_poem(req);
        if let Err(resp) = self.bridge.before(&mut req).await {
            return Ok(response_to_poem(resp));
        }
        // The rest of the request runs under what its guards attached: the
        // seeds forward it, the operations read it.
        install_attached(&req, self.answer(&req, body))
            .await
            .map(response_to_poem)
    }
}

impl<E: Executor> ContextEndpoint<E> {
    /// Read, pipe, seed and execute the request its guards admitted.
    async fn answer(&self, req: &Request, mut body: RequestBody) -> Result<Response> {
        let batch = match read_batch(req, &mut body).await {
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
            BatchRequest::Single(r) => BatchRequest::Single(self.bridge.seed(req, r)),
            BatchRequest::Batch(rs) => {
                BatchRequest::Batch(rs.into_iter().map(|r| self.bridge.seed(req, r)).collect())
            }
        };
        let read_only = is_read_only(&mut batch);
        // A local slot, since the guard scopes a `()` future.
        let mut answered: Option<Response> = None;
        let inner: BoxFuture<()> = Box::pin(async {
            answered = Some(response_from_poem(
                GraphQLBatchResponse(self.executor.execute_batch(batch).await).into_response(),
            ));
        });
        let guarded = self.bridge.around(req, inner);
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

/// Forward a value the request's guards attached — the authentication guard's
/// principal — into the GraphQL context, so resolvers read it with
/// `ctx.data::<T>()`.
///
/// ```
/// use nest_rs_core::{AmbientValue, Layer, UnitContext, injectable};
/// use nest_rs_graphql::async_graphql::{Context, Result};
/// use nest_rs_graphql::{operations, resolver};
/// use nest_rs_guards::{Denial, Guard, async_trait};
/// use nest_rs_http::HttpView;
/// # use nest_rs_core::module;
/// # use nest_rs_graphql::GraphqlModule;
/// # use nest_rs_guards::guard;
///
/// #[derive(Clone)]
/// struct MyPrincipal(String);
///
/// impl AmbientValue for MyPrincipal {}
///
/// nest_rs_graphql::forward_principal!(MyPrincipal);
///
/// /// Attaches the caller the way an authentication guard does.
/// #[injectable]
/// #[derive(Default)]
/// struct NameGuard;
///
/// impl Layer for NameGuard {}
///
/// #[async_trait]
/// impl Guard for NameGuard {
///     async fn check_http(&self, cx: &mut UnitContext<'_>) -> std::result::Result<(), Denial> {
///         let name = HttpView::of(cx)?.request().header("x-name").unwrap_or("anonymous");
///         let principal = MyPrincipal(name.to_owned());
///         cx.attach(principal);
///         Ok(())
///     }
/// }
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
/// # #[module(imports = [GraphqlModule::for_root(None)], providers = [NameGuard, MeResolver])]
/// # struct AppModule;
/// #
/// # #[nest_rs_core::main]
/// # async fn main() -> nest_rs_core::anyhow::Result<()> {
/// # let app = nest_rs_testing::TestApp::builder()
/// #     .module::<AppModule>()
/// #     .use_guards_global([guard::<NameGuard>()])
/// #     .build()
/// #     .await?;
///
/// let resp = app
///     .http()
///     .post("/graphql")
///     .header("x-name", "ada")
///     .body_json(&serde_json::json!({ "query": "{ me }" }))
///     .send()
///     .await;
/// resp.assert_json(serde_json::json!({ "data": { "me": "ada" } })).await;
/// # Ok(())
/// # }
/// ```
///
/// `T: AmbientValue + Clone`. Anonymous requests pass through untouched: the
/// forwarder copies only a value the module-gated authn guard attached.
// Not module-gated itself: `.claude/decisions/forward-principal.md`.
#[macro_export]
macro_rules! forward_principal {
    ($ty:ty) => {
        $crate::__private::inventory::submit! {
            $crate::GraphqlContextSeed {
                owner_type_id: || ::core::option::Option::None,
                lifetime: $crate::SeedLifetime::Connection,
                seed: |_req, _container, __gql| match $crate::__private::ambient::<$ty>() {
                    ::core::option::Option::Some(__v) => {
                        __gql.data(<$ty as ::core::clone::Clone>::clone(&__v))
                    }
                    ::core::option::Option::None => __gql,
                },
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use nest_rs_core::__private::serde;

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

    /// Variables whose JSON form cannot be written.
    struct Unwritable;

    impl serde::Serialize for Unwritable {
        fn serialize<S: serde::Serializer>(&self, _: S) -> std::result::Result<S::Ok, S::Error> {
            Err(<S::Error as serde::ser::Error>::custom(
                "sk_live_51HsecretTOKEN",
            ))
        }
    }

    impl<'de> serde::Deserialize<'de> for Unwritable {
        fn deserialize<D: serde::Deserializer<'de>>(_: D) -> std::result::Result<Self, D::Error> {
            Ok(Self)
        }
    }

    #[tokio::test]
    async fn variables_that_cannot_be_read_as_json_answer_the_pipe_error_not_null() {
        let Err(refusal) = fold_variables(&Unwritable, &[]) else {
            panic!("the pipes would run over `null` in place of the variables");
        };
        assert_eq!(refusal.status(), StatusCode::OK);
        let body = refusal
            .into_body()
            .into_bytes()
            .await
            .expect("the refusal's body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("a JSON body");
        assert_eq!(body["data"], serde_json::Value::Null, "{body}");
        let message = body["errors"][0]["message"].as_str().unwrap_or_default();
        assert!(
            message.starts_with("the operation variables could not be read as JSON"),
            "{body}",
        );
        assert!(!message.contains("sk_live"), "never the value: {body}");
    }
}

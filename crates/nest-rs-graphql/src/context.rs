//! Per-request context bridge: async-graphql does not see the HTTP request, so
//! [`ContextEndpoint`] folds every link-time-registered [`GraphqlContextSeed`]
//! over the parsed request before executing it.

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_graphql::http::{MultipartOptions, receive_batch_body};
use async_graphql::{
    BatchRequest, BatchResponse, Data, Executor, ParseRequestError, Request as GqlRequest,
};
use futures_util::TryStreamExt;
use nest_rs_core::__private::serde::Serialize;
use nest_rs_core::__private::serde::de::DeserializeOwned;
use nest_rs_core::problem::code;
use nest_rs_core::{
    Container, Discovery, Handler, Posture, ReachableProviders, UngatedDiscoveryError, UnitContext,
};
use nest_rs_guards::__private::global_guards_active;
use nest_rs_http::__private::{EndpointGuardsMeta, with_view};
use nest_rs_http::{
    Body, BodyError, Endpoint, HeaderValue, HttpError, Request, RequestDeadline, Response,
    ResponseError, StatusCode, current_body_limit, declare_endpoint, header,
};
use nest_rs_pipes::{ArgumentKind, GlobalPipe, PipeError};

use crate::module::GraphqlModule;
use crate::operation_units::OperationUnits;
use crate::view::GraphqlView;

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
    /// **dropped** on a socket.
    Request,
    /// The caller's identity — a principal, an `Ability` — forwarded on both paths.
    Connection,
}

inventory::collect!(GraphqlContextSeed);

/// A boxed, `Send` future: what the endpoint holds a socket's serving behind,
/// and what a DataLoader batch runs as.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The site every `/graphql` request runs as before its operations: an
/// endpoint whose operations each declare their own posture, so an
/// authentication guard admits an anonymous request there while still
/// verifying a presented credential.
///
/// Its address is the default mount, `/graphql`, wherever
/// [`GraphqlConfig::path`](crate::GraphqlConfig::path) moves it: an app serves
/// one schema, so the address names one endpoint either way.
pub static ENDPOINT: Handler =
    declare_endpoint::<GraphqlModule>("GraphqlModule", "/graphql", Posture::InBand);

/// The per-request step one [`GraphqlContextSeed`] contributes.
type ContextSeed = fn(&Request, &Container, GqlRequest) -> GqlRequest;

/// What both `/graphql` endpoints — POST and graphql-ws — need from the
/// container, resolved **once** at mount so the two cannot run different
/// global pipes, nor forward different seeds. Their request's guards run in
/// front of both, in the transport.
pub(crate) struct OperationBridge {
    pub(crate) container: Container,
    /// The global pipes folded over each operation's variables.
    pipes: Arc<[Arc<dyn GlobalPipe>]>,
    /// The seeds that fire for this app, module-gated once at mount.
    seeds: Arc<[ContextSeed]>,
    /// The subset a graphql-ws connection inherits from its upgrade.
    connection_seeds: Arc<[ContextSeed]>,
}

/// `POST` on the GraphQL path: a JSON request or batch, or a GraphQL multipart
/// request, with every seed folded over each operation first, each operation
/// run as its own unit. The `multipart/mixed` incremental-delivery answer
/// (`@defer` / `@stream`) is not offered.
pub(crate) struct ContextEndpoint<E> {
    units: OperationUnits<E>,
    bridge: Arc<OperationBridge>,
    max_batch_size: usize,
}

impl OperationBridge {
    /// What both endpoints read of the app `container` holds, or a refusal of
    /// a container no app sealed.
    pub(crate) fn new(container: Container) -> Result<Self, UngatedDiscoveryError> {
        let active = active_seeds(&container)?;
        let pool = global_guards_active(&container);
        let endpoint_guards = Discovery::new(&container)
            .meta::<EndpointGuardsMeta>()
            .iter()
            .any(|declared| declared.meta.endpoint() == TypeId::of::<GraphqlView>());
        if pool || endpoint_guards {
            tracing::debug!(
                target: crate::TARGET,
                pool,
                endpoint_guards,
                "graphql operations gated",
            );
        } else {
            tracing::warn!(
                target: crate::TARGET,
                mode = "unguarded",
                "no guard runs at the /graphql request — graphql operations run unguarded",
            );
        }
        let seeds: Arc<[_]> = active.iter().map(|reg| reg.seed).collect();
        let connection_seeds: Arc<[_]> = active
            .iter()
            .filter(|reg| reg.lifetime == SeedLifetime::Connection)
            .map(|reg| reg.seed)
            .collect();
        let pipes = nest_rs_pipes::__private::resolve_global_pipes(&container);
        Ok(Self {
            connection_seeds,
            container,
            pipes,
            seeds,
        })
    }

    /// The global pipes each operation's variables go through.
    pub(crate) fn pipes(&self) -> &Arc<[Arc<dyn GlobalPipe>]> {
        &self.pipes
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

/// What the context seed registry holds, as a refusal to read it ungated
/// names it.
const SEEDS: &str = "GraphQL context seed";

/// The seeds that fire for the app `container` holds: every framework-level
/// one, and those whose owner it reaches.
fn active_seeds(
    container: &Container,
) -> Result<Vec<&'static GraphqlContextSeed>, UngatedDiscoveryError> {
    let reachable = ReachableProviders::of(container, SEEDS)?;
    Ok(inventory::iter::<GraphqlContextSeed>()
        .filter(|reg| (reg.owner_type_id)().is_none_or(|owner| reachable.reaches(owner)))
        .collect())
}

/// `data` with what a graphql-ws connection on `req` inherits from its
/// upgrade: the app's connection seeds, folded under what is ambient where
/// this runs.
pub fn connection_data(
    container: &Container,
    req: &Request,
    data: Data,
) -> Result<Data, UngatedDiscoveryError> {
    let mut gql = GqlRequest::new("");
    gql.data = data;
    Ok(active_seeds(container)?
        .into_iter()
        .filter(|reg| reg.lifetime == SeedLifetime::Connection)
        .fold(gql, |gql, reg| (reg.seed)(req, container, gql))
        .data)
}

impl<E> ContextEndpoint<E> {
    pub(crate) fn new(
        units: OperationUnits<E>,
        bridge: Arc<OperationBridge>,
        max_batch_size: usize,
    ) -> Self {
        Self {
            units,
            bridge,
            max_batch_size,
        }
    }

    /// Hand the global pipes each operation's variables, as the `/graphql`
    /// request's unit; a refusal returns a GraphQL error response.
    fn pipe_variables(
        &self,
        req: &mut Request,
        batch: BatchRequest,
    ) -> std::result::Result<BatchRequest, Box<Response>> {
        let pipes = &self.bridge.pipes;
        if pipes.is_empty() {
            return Ok(batch);
        }
        with_view(&ENDPOINT, req, |cx| {
            let apply = |mut r: GqlRequest| -> std::result::Result<GqlRequest, Box<Response>> {
                r.variables = fold_variables(cx, &r.variables, pipes)
                    .map_err(|err| variable_pipe_error_response(&err))?;
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
        })
    }
}

/// Fold `pipes` over `variables` through the JSON form a pipe reads, as the
/// unit `cx` belongs to; a pipe's refusal, and variables that do not survive
/// the round trip, as the [`PipeError`] a GraphQL error answers.
pub(crate) fn fold_variables<T: Serialize + DeserializeOwned>(
    cx: &UnitContext<'_>,
    variables: &T,
    pipes: &[Arc<dyn GlobalPipe>],
) -> std::result::Result<T, PipeError> {
    let mut value = serde_json::to_value(variables).map_err(|err| {
        let err = nest_rs_core::DecodeError::new(&err);
        tracing::error!(
            target: crate::TARGET,
            error = %err,
            "graphql variables could not be written as JSON for the global pipes",
        );
        PipeError::new(format!(
            "the operation variables could not be read as JSON: {err}",
        ))
    })?;
    for pipe in pipes {
        pipe.transform(cx, ArgumentKind::Variables, &mut value)?;
    }
    // A pipe may rewrite the variables into a shape that is no longer a
    // GraphQL variables object.
    serde_json::from_value(value).map_err(|err| {
        PipeError::new(format!(
            "variable pipe produced an invalid variables object: {}",
            nest_rs_core::DecodeError::new(&err),
        ))
    })
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
async fn read_batch(req: &Request, body: Body) -> Result<BatchRequest, Response> {
    let multipart = req.content_type().filter(|value| {
        value
            .trim_start()
            .get(..10)
            .is_some_and(|kind| kind.eq_ignore_ascii_case("multipart/"))
    });
    if let Some(content_type) = multipart {
        // A file is never larger than the body the transport's cap admits.
        let options = MultipartOptions::default().max_file_size(current_body_limit());
        let body = body.into_stream().into_async_read();
        return receive_batch_body(Some(content_type), body, options)
            .await
            .map_err(|refusal| multipart_refused(&refusal));
    }
    let bytes = body.into_bytes().await.map_err(|_| refused(UNREADABLE))?;
    serde_json::from_slice::<BatchRequest>(&bytes).map_err(|_| refused(&what_it_is_not(&bytes)))
}

/// What a body that failed to read is said as; the body cap answers `413` itself.
const UNREADABLE: &str = "the request body could not be read";

/// The answer to a multipart body that is not a GraphQL request, read by
/// variant: a file past the cap as the cap answers; the `operations` and `map`
/// parts' decoders wrap serde's sentence, which quotes the part; every other
/// variant's own sentence is fixed.
fn multipart_refused(refusal: &ParseRequestError) -> Response {
    use ParseRequestError as Refused;

    fn part(name: &str, source: &(dyn std::error::Error + Send + Sync + 'static)) -> String {
        match source.downcast_ref::<serde_json::Error>() {
            Some(error) => format!(
                "the `{name}` part: {}",
                nest_rs_core::DecodeError::new(error)
            ),
            None => format!("the `{name}` part does not decode"),
        }
    }

    let reason = match refusal {
        Refused::PayloadTooLarge => {
            let limit = current_body_limit();
            return BodyError::TooLarge { limit }.as_response();
        }
        Refused::InvalidRequest(source) => part("operations", source.as_ref()),
        Refused::InvalidFilesMap(source) => part("map", source.as_ref()),
        Refused::Io(_) => UNREADABLE.to_owned(),
        fixed => fixed.to_string(),
    };
    refused(&reason)
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

/// Answer a request that is not a GraphQL request: GraphQL-over-HTTP's `400`
/// with an `errors` entry under its class, `INVALID_ARGUMENT`, and no `data`
/// member since nothing was executed.
fn refused(reason: &str) -> Response {
    tracing::debug!(
        target: crate::TARGET,
        reason,
        "graphql request refused: the body is not a GraphQL request",
    );
    let body = serde_json::json!({
        "errors": [{
            "message": format!("the body is not a GraphQL request: {reason}"),
            "extensions": { crate::CODE_EXTENSION: code::INVALID_ARGUMENT.as_str() },
        }],
    });
    Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .content_type(HeaderValue::from_static("application/json"))
        .body(serde_json::to_vec(&body).unwrap_or_default())
}

/// A variable-pipe `PipeError` as one GraphQL error, as a resolver's pipe
/// refusal reads ([`pipe_error`](crate::pipe_error)): its message, the
/// `INVALID_ARGUMENT` code under `extensions.code`, and field-level errors
/// under `extensions.errors`.
pub(crate) fn variable_pipe_error(err: &PipeError) -> serde_json::Value {
    let mut extensions = serde_json::Map::new();
    extensions.insert(
        crate::CODE_EXTENSION.to_owned(),
        code::INVALID_ARGUMENT.as_str().into(),
    );
    if let Some(details) = err.details() {
        extensions.insert(crate::FIELD_ERRORS_EXTENSION.to_owned(), details.clone());
    }
    serde_json::json!({ "message": err.message(), "extensions": extensions })
}

/// Render a variable-pipe `PipeError` as a GraphQL error response — HTTP 200
/// with an `errors` array holding [`variable_pipe_error`].
fn variable_pipe_error_response(err: &PipeError) -> Box<Response> {
    let body = serde_json::json!({
        "data": serde_json::Value::Null,
        "errors": [variable_pipe_error(err)],
    });
    Box::new(
        Response::builder()
            .status(StatusCode::OK)
            .content_type(HeaderValue::from_static("application/json"))
            .body(serde_json::to_vec(&body).unwrap_or_default()),
    )
}

impl<E: Executor> Endpoint for ContextEndpoint<E> {
    /// Read, pipe, seed and execute the request its guards admitted.
    async fn call(&self, mut req: Request) -> nest_rs_http::Result<Response> {
        let body = req.take_body();
        let batch = match read_batch(&req, body).await {
            Ok(batch) => batch,
            Err(refusal) => return Ok(refusal),
        };
        // Before the variable pipes fold over every operation.
        if let BatchRequest::Batch(rs) = &batch
            && rs.len() > self.max_batch_size
        {
            return Err(HttpError::from_status(StatusCode::PAYLOAD_TOO_LARGE));
        }
        let batch = match self.pipe_variables(&mut req, batch) {
            Ok(batch) => batch,
            Err(resp) => return Ok(*resp),
        };
        let batch = match batch {
            BatchRequest::Single(r) => BatchRequest::Single(self.bridge.seed(&req, r)),
            BatchRequest::Batch(rs) => {
                BatchRequest::Batch(rs.into_iter().map(|r| self.bridge.seed(&req, r)).collect())
            }
        };
        let executed = self
            .units
            .execute_batch_within(batch, req.extensions().get::<RequestDeadline>())
            .await;
        batch_response(&executed)
    }
}

/// `executed` as GraphQL over HTTP's JSON answer: `200` whatever `errors`
/// holds, the schema's `cache-control` hint when every operation succeeded,
/// then each header a resolver set, added beside the answer's own rather than
/// replacing them.
fn batch_response(executed: &BatchResponse) -> nest_rs_http::Result<Response> {
    let body = serde_json::to_vec(executed)
        .map_err(|err| HttpError::new(err, StatusCode::INTERNAL_SERVER_ERROR))?;
    let mut response = Response::builder()
        .content_type(HeaderValue::from_static("application/json; charset=utf-8"))
        .body(body);
    let headers = response.headers_mut();
    if executed.is_ok()
        && let Some(hint) = executed.cache_control().value()
        && let Ok(hint) = HeaderValue::try_from(hint)
    {
        headers.insert(header::CACHE_CONTROL, hint);
    }
    headers.extend(executed.http_headers_iter());
    Ok(response)
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

    #[test]
    fn seeds_folded_over_a_container_no_app_sealed_are_refused_naming_their_reader() {
        let by_hand = Container::builder().build().expect("the container builds");
        let upgrade = Request::builder().finish();

        let Err(ungated) = connection_data(&by_hand, &upgrade, Data::default()) else {
            panic!("an ungated discovery is refused, never served whole");
        };

        assert_eq!(ungated.reader, "GraphQL context seed");
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

    #[test]
    fn variables_that_cannot_be_read_as_json_answer_the_pipe_error_not_null() {
        let mut request = Request::builder().finish();
        let Err(refusal) = with_view(&ENDPOINT, &mut request, |cx| {
            fold_variables(cx, &Unwritable, &[])
        }) else {
            panic!("the pipes would run over `null` in place of the variables");
        };
        let message = refusal.message();
        assert!(
            message.starts_with("the operation variables could not be read as JSON"),
            "{message}",
        );
        assert!(!message.contains("sk_live"), "never the value: {message}");
        let error = variable_pipe_error(&refusal);
        assert_eq!(error["message"], message, "{error}");
    }

    /// A variable pipe's refusal names the class HTTP's `400` names, beside its
    /// field map when it has one, as a resolver's pipe refusal does.
    #[test]
    fn a_variable_pipe_s_refusal_names_the_invalid_argument_class() {
        let bare = variable_pipe_error(&PipeError::new("must be a valid UUID"));
        assert_eq!(
            bare,
            serde_json::json!({
                "message": "must be a valid UUID",
                "extensions": { "code": "INVALID_ARGUMENT" },
            }),
        );

        let details = serde_json::json!({ "name": [{ "code": "length" }] });
        let detailed = variable_pipe_error(&PipeError::with_details(
            "validation failed",
            details.clone(),
        ));
        assert_eq!(
            detailed,
            serde_json::json!({
                "message": "validation failed",
                "extensions": { "code": "INVALID_ARGUMENT", "errors": details },
            }),
        );
    }

    /// A file past the cap is answered as the cap answers a body past it; the
    /// transport's own read usually stops first, the body holding the file.
    #[test]
    fn a_file_past_the_cap_is_413_as_the_cap_answers() {
        let refused = multipart_refused(&ParseRequestError::PayloadTooLarge);

        assert_eq!(refused.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            refused.headers().get(header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("application/problem+json")),
        );
    }
}

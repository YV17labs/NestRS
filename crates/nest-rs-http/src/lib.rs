//! HTTP transport for nestrs — a [`nest_rs_core::Transport`] backed by poem.
//!
//! [`HttpTransport`] mounts every `#[routes]` controller, every self-mounting
//! endpoint another surface declares (a GraphQL schema, an MCP service — each
//! via [`HttpEndpointMeta`]), and any extra endpoint registered with
//! [`HttpTransport::mount`].
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod access_log;
mod allow;
mod boot_check;
pub mod challenge;
mod client_ip;
mod config;
mod context;
mod controller;
mod cors;
mod detached;
mod drain;
mod edge;
mod endpoint;
mod error;
mod header;
mod interceptor;
mod location;
mod matched;
mod metadata;
mod module;
mod multipart;
mod opaque;
mod pipe;
mod problem;
mod raw_body;
mod reflector;
mod response_body;
mod scope;
mod security_headers;
mod shaper;
mod sse;
pub mod target;
mod tls;
mod trace_context;
mod transport;
pub mod unit;
mod versioning;

pub use allow::{AllowedMethods, MethodTable};
pub use boot_check::{GlobalGuardsActive, HttpBootCheck};
pub use client_ip::{ClientIp, ClientOrigin};
pub use config::{
    HttpConfig, MAX_CONNECTION_CEILING, MAX_CONNECTION_FLOOR, SSE_KEEP_ALIVE_CEILING,
    SSE_KEEP_ALIVE_FLOOR,
};
pub use context::{Ctx, RejectedCredential};
pub use controller::{
    Controller, HttpControllerMeta, HttpRouteMeta, HttpVerb, NEXT_CURSOR_HEADER, RequestBodyMeta,
};
pub use cors::HttpCors;
pub use detached::DetachedWork;
pub use endpoint::{EdgePosture, HttpEndpointMeta};
pub use header::Header;
pub use location::{caller_path, set_created_location};
pub use matched::{Matched, matched};
pub use metadata::{HandlerMetadata, MappedError, Public};
pub use module::{HttpModule, HttpSetup};
pub use multipart::{PartExt, PartStream};
pub use nest_rs_core::input;
pub use nest_rs_core::{current_request_scope, with_request_scope};
pub use opaque::Opaque;
pub use pipe::{IntoInner, Piped, Valid};
pub use problem::{ProblemDetails, normalize_error_response};
pub use raw_body::{RawBody, current_body_limit};
pub use reflector::Reflector;
pub use response_body::OpenEndedBody;
pub use scope::Scoped;
pub use security_headers::HttpSecurityHeaders;
pub use shaper::{ResponseShaping, RouteFuture, RouteResponseShaper, ShapedEndpoint};
pub use sse::{SseEvent, SseSettings, SseStream};
pub use tls::HttpTls;
pub use trace_context::{
    TRACEPARENT_HEADER, TRACERESPONSE_HEADER, TRACESTATE_HEADER, UPSTREAM_REQUEST_ID_HEADER,
};
pub use transport::{
    HttpTransport, join_path, normalize_mount_path, version_path, versions_declare,
};
pub use versioning::{
    ApiVersioning, DEFAULT_VERSION_HEADER, MEDIA_TYPE_PARAM, VersionSelector, declared_versions,
};

// Not public API: sibling crates and macro output name them.
#[doc(hidden)]
pub use controller::{SchemaFn, schema_of};
#[doc(hidden)]
pub use endpoint::SelfMountGuardWrap;
#[doc(hidden)]
pub use interceptor::{HttpEndpointWrap, priority as endpoint_wrap_priority};
#[doc(hidden)]
pub use shaper::{CaptureFn, MaskProbe, ShaperProbe, UnshapedProbe, shaped};

pub use poem;
// The stream vocabulary an `#[sse]` route is built from.
pub use futures_util;
pub use schemars;
// `#[input]`'s derives resolve through here, so an app's manifest never names them.
#[doc(hidden)]
pub use serde;
#[doc(hidden)]
pub use validator;

pub use async_trait::async_trait;

/// `#[controller(path = "/users")]` — the HTTP host on a struct, paired with
/// [`#[routes]`](macro@routes) on its impl block.
///
/// ```
/// # use nest_rs_core::Container;
/// # use nest_rs_http::{controller, routes};
/// #[controller(path = "/users", version = "1")]
/// #[derive(Default)]
/// struct UsersController;
///
/// #[routes]
/// impl UsersController {
///     #[get("/")]
///     async fn list(&self) -> &'static str {
///         "[]"
///     }
/// }
///
/// assert_eq!(UsersController::PATH, "/users");
/// assert_eq!(UsersController::VERSIONS, ["1"]);
/// let _: fn(&Container) -> UsersController = UsersController::from_container;
/// ```
pub use nest_rs_http_macros::controller;

/// `#[routes]` — binds a [`#[controller]`](macro@controller)'s methods to HTTP
/// routes, one per verb attribute.
///
/// ```
/// # use nest_rs_core::{Discoverable, Discovery, module};
/// # use nest_rs_http::poem::web::Json;
/// # use nest_rs_http::{Controller, HttpControllerMeta, HttpVerb, controller, routes};
/// # use nest_rs_testing::TestApp;
/// #[controller(path = "/users")]
/// #[derive(Default)]
/// struct UsersController;
///
/// #[routes]
/// impl UsersController {
///     #[get("/")]
///     async fn list(&self) -> Json<Vec<String>> {
///         Json(vec!["ada".into()])
///     }
/// }
/// # #[module(providers = [UsersController])]
/// # struct UsersModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
///
/// fn implements<T: Controller + Discoverable>() {}
/// implements::<UsersController>();
///
/// # let app = TestApp::for_module::<UsersModule>().await?;
/// let controllers = Discovery::new(app.container()).meta::<HttpControllerMeta>();
/// let route = &controllers[0].meta.routes[0];
/// assert_eq!((route.verb, route.path, route.handler), (HttpVerb::Get, "/", "list"));
/// assert!(route.response.is_some());
///
/// app.http().get("/users").send().await.assert_json(["ada"]).await;
/// # Ok(())
/// # }
/// ```
pub use nest_rs_http_macros::routes;

/// `#[crud(...)]` — generates the standard REST operations (list, get,
/// create, update, delete) on a [`#[controller]`](macro@controller) impl block,
/// re-emitted under [`#[routes]`](macro@routes).
///
/// Each route delegates to the service's `nest_rs_seaorm::CrudService` and
/// declares `Authorize<Action, Entity>`.
pub use nest_rs_http_macros::crud;

/// `#[interceptor]` — mounts a struct implementing
/// `nest_rs_interceptors::Interceptor` around the whole HTTP endpoint, as a
/// transport-edge wrap rather than a provider.
///
/// ```
/// # use nest_rs_core::{Layer, module};
/// # use nest_rs_http::poem::http::HeaderValue;
/// # use nest_rs_http::poem::{Request, Response, Result};
/// # use nest_rs_http::{async_trait, interceptor};
/// # use nest_rs_interceptors::{Interceptor, Next};
/// # use nest_rs_testing::TestApp;
/// #[interceptor]
/// struct ServedBy;
///
/// impl Layer for ServedBy {}
///
/// #[async_trait]
/// impl Interceptor for ServedBy {
///     async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
///         let mut resp = next.run(req).await.unwrap_or_else(|err| err.into_response());
///         resp.headers_mut().insert("x-served-by", HeaderValue::from_static("api"));
///         Ok(resp)
///     }
/// }
///
/// #[module(providers = [ServedBy])]
/// struct AppModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// # let app = TestApp::for_module::<AppModule>().await?;
///
/// app.http().get("/nowhere").send().await.assert_header("x-served-by", "api");
/// assert!(app.container().get::<ServedBy>().is_none());
/// # Ok(())
/// # }
/// ```
pub use nest_rs_http_macros::interceptor;

pub use nest_rs_http_macros::http_code;
pub use nest_rs_http_macros::redirect;
pub use nest_rs_http_macros::response_header;

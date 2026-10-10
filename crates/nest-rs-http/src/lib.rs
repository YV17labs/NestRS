//! HTTP transport for nestrs — a [`nest_rs_core::Transport`] backed by poem.
//!
//! [`HttpTransport`] mounts every `#[routes]` controller, every self-mounting
//! endpoint another surface declares (a GraphQL schema, an MCP service — each
//! via [`HttpEndpointMeta`]), and any extra endpoint registered with
//! [`HttpTransport::mount`].
//!
//! Its vocabulary names no server library: the `http` crate's head types,
//! re-exported here as their one path, and nestrs's own [`Request`],
//! [`Response`], [`Body`], [`HttpError`] and [`Endpoint`].
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod access_log;
mod allow;
mod body;
mod boot_check;
pub mod challenge;
mod client_ip;
mod config;
mod context;
mod controller;
mod cors;
mod deprecation;
mod detached;
mod drain;
mod edge;
mod endpoint;
mod error;
mod fallback;
mod headers;
mod interceptor;
mod link;
mod location;
mod matched;
mod metadata;
mod module;
mod multipart;
mod opaque;
mod pipe;
mod poem_bridge;
mod problem;
mod raw_body;
mod reflector;
mod request;
mod response;
mod response_body;
mod scope;
mod security_headers;
mod shaper;
mod sse;
pub mod target;
#[cfg(test)]
mod testing;
mod tls;
mod trace_context;
mod transport;
pub mod unit;
mod versioning;

/// A cheaply cloned, shared slice of bytes.
///
/// ```
/// use nest_rs_http::Bytes;
///
/// assert_eq!(Bytes::from_static(b"abc").slice(1..), "bc");
/// ```
pub use bytes::Bytes;
/// Typed values riding with a request, a response or an error.
///
/// ```
/// use nest_rs_http::Extensions;
///
/// let mut extensions = Extensions::new();
/// extensions.insert(5_u8);
/// assert_eq!(extensions.get::<u8>(), Some(&5));
/// ```
pub use http::Extensions;
/// A multimap of header names to values.
///
/// ```
/// use nest_rs_http::{HeaderMap, HeaderValue, header};
///
/// let mut headers = HeaderMap::new();
/// headers.append(header::VARY, HeaderValue::from_static("origin"));
/// assert_eq!(headers[header::VARY], "origin");
/// ```
pub use http::HeaderMap;
/// A header name, lowercase.
///
/// ```
/// use nest_rs_http::HeaderName;
///
/// assert_eq!(HeaderName::from_static("x-request-id").as_str(), "x-request-id");
/// ```
pub use http::HeaderName;
/// A header value: bytes a header may carry, not necessarily text.
///
/// ```
/// use nest_rs_http::HeaderValue;
///
/// assert_eq!(HeaderValue::from_static("no-store"), "no-store");
/// assert!(HeaderValue::from_bytes(b"caf\xe9").is_ok());
/// ```
pub use http::HeaderValue;
/// A request method (RFC 9110 §9).
///
/// ```
/// use nest_rs_http::Method;
///
/// assert!(Method::GET.is_safe());
/// ```
pub use http::Method;
/// A response status (RFC 9110 §15).
///
/// ```
/// use nest_rs_http::StatusCode;
///
/// assert!(StatusCode::NOT_FOUND.is_client_error());
/// ```
pub use http::StatusCode;
/// A request target: path and query, or an absolute URI.
///
/// ```
/// use nest_rs_http::Uri;
///
/// assert_eq!(Uri::from_static("/users?page=2").query(), Some("page=2"));
/// ```
pub use http::Uri;
/// An HTTP protocol version.
///
/// ```
/// use nest_rs_http::Version;
///
/// assert_ne!(Version::HTTP_11, Version::HTTP_2);
/// ```
pub use http::Version;
/// The `http` crate's header module: every standard header name as a constant.
///
/// ```
/// use nest_rs_http::header;
///
/// assert_eq!(header::CONTENT_TYPE.as_str(), "content-type");
/// ```
pub use http::header;
/// A request's head as the `http` crate holds it, which
/// [`Request::head`] exposes.
///
/// ```
/// use nest_rs_http::{Method, Request, RequestParts};
///
/// let request = Request::builder().finish();
/// let head: &RequestParts = request.head();
/// assert_eq!(head.method, Method::GET);
/// ```
pub use http::request::Parts as RequestParts;
/// A response's head as the `http` crate holds it, which
/// [`Response::into_parts`] hands back.
///
/// ```
/// use nest_rs_http::{Response, ResponseParts, StatusCode};
///
/// let (head, _): (ResponseParts, _) = Response::builder().finish().into_parts();
/// assert_eq!(head.status, StatusCode::OK);
/// ```
pub use http::response::Parts as ResponseParts;
/// A URI scheme: what a request arrived over.
///
/// ```
/// use nest_rs_http::Scheme;
///
/// assert_eq!(Scheme::HTTPS.as_str(), "https");
/// ```
pub use http::uri::Scheme;

pub use allow::{AllowedMethods, MethodTable};
pub use body::Body;
pub use boot_check::{GlobalGuardsActive, HttpBootCheck};
pub use client_ip::{ClientIp, ClientOrigin};
pub use config::{
    HttpConfig, MAX_CONNECTION_CEILING, MAX_CONNECTION_FLOOR, SSE_KEEP_ALIVE_CEILING,
    SSE_KEEP_ALIVE_FLOOR,
};
pub use context::{Ctx, RejectedCredential};
pub use controller::{Controller, HttpControllerMeta, HttpRouteMeta, HttpVerb, RequestBodyMeta};
pub use cors::HttpCors;
pub use deprecation::DeprecationMeta;
#[doc(hidden)]
pub use deprecation::deprecated_route;
pub use detached::DetachedWork;
pub use endpoint::{BoxEndpoint, EdgePosture, Endpoint, HttpEndpointMeta, endpoint_fn};
pub use error::{BodyError, HttpError, ResponseError, Result};
pub use fallback::HttpFallbackMeta;
pub use headers::Header;
pub use link::set_next_link;
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
pub use request::{FromRequest, Request, RequestBody, RequestBuilder};
pub use response::{IntoResponse, Response, ResponseBuilder};
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
    HttpTransport, join_path, literal_mount_path, normalize_mount_path, version_path,
    versions_declare,
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
pub use endpoint::with_data;
#[doc(hidden)]
pub use interceptor::{HttpEndpointWrap, priority as endpoint_wrap_priority};
#[doc(hidden)]
pub use response::IntoResult;
#[doc(hidden)]
pub use shaper::{CaptureFn, MaskProbe, ShaperProbe, UnshapedProbe, shaped};

/// Converts between this crate's HTTP vocabulary and poem's, losing nothing
/// either way, while the transport moves onto the vocabulary. Called by this
/// framework's crates while poem is still on the public surface; not API, and
/// removed with it.
#[doc(hidden)]
pub mod __poem_bridge {
    pub use crate::poem_bridge::{
        body_from_poem, body_to_poem, error_from_poem, error_to_poem, from_poem, request_from_poem,
        request_from_poem_at, request_to_poem, response_from_poem, response_to_poem, to_poem,
    };
}

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

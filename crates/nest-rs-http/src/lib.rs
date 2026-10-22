//! HTTP transport for nestrs — a [`nest_rs_core::Transport`] routed by poem and
//! served by its own accept loop on hyper.
//!
//! [`HttpTransport`], built from the app's [`HttpConfig`] alone, assembles one
//! route table, [`HttpRoutes`], from every `#[routes]` controller and every
//! self-mounting endpoint another surface declares (a GraphQL schema, an MCP
//! service — each via [`HttpEndpointMeta`]).
//!
//! Its vocabulary names no server library: the `http` crate's head types,
//! re-exported here as their one path, and nestrs's own [`Request`],
//! [`Response`], [`Body`], [`HttpError`] and [`Endpoint`].
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod accept_encoding;
mod access_log;
mod allow;
mod body;
mod boot_check;
mod capped;
pub mod challenge;
mod client_ip;
mod config;
mod context;
mod controller;
mod cors;
mod deadline;
mod deprecation;
mod detached;
mod drain;
mod edge;
mod endpoint;
mod endpoint_guards;
mod error;
mod fallback;
mod form;
mod global_pipes;
mod guard_stage;
mod headers;
mod json;
mod link;
mod location;
mod matched;
mod module;
mod multipart;
mod opaque;
mod origin;
mod path;
mod phase;
mod pipe;
mod poem_adapter;
mod poem_bridge;
mod problem;
mod query;
mod raw_body;
mod request;
mod response;
mod response_body;
mod route_site;
mod route_template;
mod routes;
mod scope;
mod security_headers;
mod server;
mod shaper;
mod site;
mod sse;
mod stall;
pub mod target;
#[cfg(test)]
mod testing;
mod tls;
mod trace_context;
mod transport;
pub mod unit;
mod versioning;
mod view;
mod websocket;
mod window;

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

pub use body::Body;
pub use boot_check::HttpBootCheck;
pub use client_ip::{ClientIp, ClientOrigin};
pub use config::{
    HttpConfig, MAX_CONNECTION_CEILING, MAX_CONNECTION_FLOOR, SSE_KEEP_ALIVE_CEILING,
    SSE_KEEP_ALIVE_FLOOR, UNIT_TIMEOUT_CEILING, UNIT_TIMEOUT_FLOOR,
};
pub use context::{Ctx, RejectedCredential};
pub use controller::{HttpControllerMeta, HttpRouteMeta, RequestBodyMeta, SchemaFn};
pub use cors::HttpCors;
pub use deadline::RequestDeadline;
pub use deprecation::DeprecationMeta;
pub use detached::DetachedWork;
pub use endpoint::{BoxEndpoint, Endpoint, HttpEndpointMeta, declare_endpoint, endpoint_fn};
pub use endpoint_guards::{CarriedView, EndpointGuards, GuardList};
pub use error::{BodyError, HttpError, ResponseError, Result, RouteTemplateError};
pub use fallback::HttpFallbackMeta;
pub use form::Form;
pub use headers::Header;
pub use json::Json;
pub use link::set_next_link;
pub use location::{caller_path, set_created_location};
pub use module::{HttpModule, HttpSetup};
pub use multipart::{Field, Multipart, PartStream};
pub use nest_rs_core::input;
pub use nest_rs_core::{current_request_scope, with_request_scope};
pub use opaque::Opaque;
pub use path::Path;
pub use pipe::{IntoInner, Piped, Valid, pipe_error};
pub use problem::{ProblemDetails, normalize_error_response};
pub use query::Query;
pub use raw_body::{RawBody, current_body_limit};
pub use request::{FromRequest, Request, RequestBody, RequestBuilder};
pub use response::{IntoResponse, Response, ResponseBuilder};
pub use response_body::OpenEndedBody;
pub use route_template::RouteTemplate;
pub use routes::HttpRoutes;
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
pub use versioning::{ApiVersioning, DEFAULT_VERSION_HEADER, MEDIA_TYPE_PARAM, declared_versions};
pub use view::HttpView;

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::controller::{HttpRouteParts, http_controller_meta, http_route_meta, schema_of};
    pub use crate::deprecation::deprecated_route;
    pub use crate::endpoint::{OriginCheck, checks_origin, guarded_by};
    pub use crate::endpoint_guards::{EndpointGuardsMeta, EndpointPosture};
    pub use crate::error::{OriginRefused, OriginRefusedReason};
    pub use crate::guard_stage::{
        attached, attached_pending, behind_guards, check_request, denial_to_http_error,
        denial_to_http_response,
    };
    pub use crate::origin::{OriginPolicy, OriginScheme};
    pub use crate::problem::framework_sentence;
    pub use crate::response::IntoResult;
    pub use crate::route_site::{
        RouteGuards, RoutePipes, RouteWrapper, mount_controller, mount_route, route_guard_stage,
    };
    pub use crate::shaper::{CaptureFn, GateFn, MaskProbe, ShaperProbe, UnshapedProbe, shaped};
    pub use crate::view::with_view;
    pub use nest_rs_guards::__private::global_guards_active;
    pub use nest_rs_pipes::PipeSpec;

    pub use serde;
    pub use tracing;
    pub use validator;

    /// Whether a request asks for a WebSocket and, behind the `websocket`
    /// feature, the opening handshake on hyper's upgrade, the socket above it
    /// and the codes it closes with, for the edges that speak one.
    pub mod websocket {
        #[cfg(feature = "websocket")]
        pub use crate::error::HandshakeRefused;
        pub use crate::websocket::requested;
        #[cfg(feature = "websocket")]
        pub use crate::websocket::{
            CloseCode, Error, Message, Utf8Bytes, WebSocketLimits, WebSocketStream,
            WebSocketUpgrade, close_frame, read_failed,
        };
    }

    /// Converts between this crate's HTTP vocabulary and poem's, losing nothing
    /// either way, while the transport moves onto the vocabulary; removed once
    /// poem leaves the public surface.
    pub mod poem_bridge {
        pub use crate::poem_bridge::{
            body_from_poem, body_to_poem, downcast_from_poem, error_from_poem, error_to_poem,
            from_poem, request_from_poem, request_from_poem_at, request_to_poem,
            response_from_poem, response_to_poem, to_poem, wrap_poem,
        };
    }
}

pub use poem;
// The stream vocabulary an `#[sse]` route is built from.
pub use futures_util;
pub use schemars;

pub use async_trait::async_trait;

/// `#[controller(path = "/users")]` — the HTTP host on a struct, paired with
/// [`#[routes]`](macro@routes) on its impl block.
///
/// ```
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
/// ```
pub use nest_rs_http_macros::controller;

/// `#[routes]` — binds a [`#[controller]`](macro@controller)'s methods to HTTP
/// routes, one per verb attribute.
///
/// ```
/// # use nest_rs_core::{Discoverable, Discovery, module};
/// # use nest_rs_http::{HttpControllerMeta, Json, Method, controller, routes};
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
/// fn implements<T: Discoverable>() {}
/// implements::<UsersController>();
///
/// # let app = TestApp::for_module::<UsersModule>().await?;
/// let controllers = Discovery::new(app.container()).meta::<HttpControllerMeta>();
/// let route = &controllers[0].meta.routes()[0];
/// assert_eq!((route.method(), route.path(), route.handler()), (&Method::GET, "/", "list"));
/// assert!(route.response().is_some());
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
/// declares its posture with `#[authorize(Action, Entity)]`.
pub use nest_rs_http_macros::crud;

pub use nest_rs_http_macros::http_code;
pub use nest_rs_http_macros::redirect;
pub use nest_rs_http_macros::response_header;

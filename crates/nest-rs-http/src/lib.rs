//! HTTP transport for nestrs — a [`nest_rs_core::Transport`] backed by poem.
//!
//! [`HttpTransport`] mounts every `#[routes]` controller, every self-mounting
//! endpoint another surface declares (a GraphQL schema, an MCP service — each
//! via [`HttpEndpointMeta`]), and any extra endpoint registered with
//! [`HttpTransport::mount`].
#![warn(missing_docs)]

mod access_log;
mod allow;
mod boot_check;
pub mod challenge;
mod client_ip;
mod config;
mod context;
mod controller;
mod cors;
mod edge;
mod endpoint;
mod header;
mod interceptor;
mod location;
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
pub use config::HttpConfig;
pub use context::{Ctx, RejectedCredential};
pub use controller::{Controller, HttpControllerMeta, HttpRouteMeta, HttpVerb, RequestBodyMeta};
pub use cors::CorsConfig;
pub use endpoint::{EdgePosture, HttpEndpointMeta};
pub use header::Header;
pub use location::{caller_path, set_created_location};
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
pub use scope::Scoped;
pub use security_headers::SecurityHeadersConfig;
pub use shaper::{ResponseShaping, RouteFuture, RouteResponseShaper, ShapedEndpoint};
pub use sse::{SseEvent, SseSettings, SseStream};
pub use tls::TlsConfig;
pub use trace_context::{
    TRACEPARENT_HEADER, TRACERESPONSE_HEADER, TRACESTATE_HEADER, UPSTREAM_REQUEST_ID_HEADER,
};
pub use transport::{
    HttpTransport, join_path, normalize_mount_path, version_path, versions_declare,
};
pub use versioning::{
    ApiVersioning, DEFAULT_VERSION_HEADER, MEDIA_TYPE_PARAM, VersionSelector, declared_versions,
};

// Cross-crate wiring seams — `pub` by necessity (sibling framework crates and
// macro-emitted code name them) but not public API: `#[doc(hidden)]` so they do
// not render as documented surface and freeze at 1.0.
#[doc(hidden)]
pub use controller::{SchemaFn, schema_of};
#[doc(hidden)]
pub use endpoint::SelfMountGuardWrap;
#[doc(hidden)]
pub use interceptor::{HttpEndpointWrap, priority as endpoint_wrap_priority};
#[doc(hidden)]
pub use shaper::{CaptureFn, MaskProbe, ShaperProbe, UnshapedProbe, shaped};

pub use poem;
// The stream vocabulary an `#[sse]` route is built from — `stream::iter`,
// `StreamExt`, the channel adapters. Re-exported beside `poem` so a controller
// that streams declares the umbrella and nothing else.
pub use futures_util;
pub use schemars;
// `#[input]` carries the DTO derives so the developer does not; routing them
// through here is what keeps `serde` / `validator` / `schemars` out of their
// manifest. Plumbing, not curated surface.
#[doc(hidden)]
pub use serde;
#[doc(hidden)]
pub use validator;

pub use async_trait::async_trait;

pub use nest_rs_http_macros::{
    controller, crud, http_code, interceptor, redirect, response_header, routes,
};

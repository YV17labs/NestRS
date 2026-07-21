//! HTTP transport for nestrs — a [`nest_rs_core::Transport`] backed by poem.
//!
//! [`HttpTransport`] mounts every `#[routes]` controller, every self-mounting
//! endpoint another surface declares (a GraphQL schema, an MCP service — each
//! via [`HttpEndpointMeta`]), and any extra endpoint registered with
//! [`HttpTransport::mount`].
#![warn(missing_docs)]

mod boot_check;
mod client_ip;
mod config;
mod context;
mod controller;
mod cors;
mod endpoint;
mod interceptor;
mod module;
mod pipe;
mod problem;
mod raw_body;
mod reflector;
mod scope;
mod security_headers;
mod shaper;
mod tls;
mod transport;

pub use boot_check::{GlobalGuardsActive, HttpBootCheck};
pub use client_ip::ClientIp;
pub use config::HttpConfig;
pub use context::Ctx;
pub use controller::{Controller, HttpControllerMeta, HttpRouteMeta, HttpVerb};
pub use cors::CorsConfig;
pub use endpoint::{EdgePosture, HttpEndpointMeta};
pub use module::{HttpModule, HttpSetup};
pub use pipe::{IntoInner, Piped, Valid};
pub use problem::{ProblemDetails, normalize_error_response};
pub use raw_body::{RawBody, RawBodyLimit};
pub use reflector::Reflector;
pub use scope::{RequestScopeEndpoint, Scoped};
pub use security_headers::SecurityHeadersConfig;
pub use shaper::{RouteResponseShaper, ShapedEndpoint};
pub use tls::TlsConfig;
pub use transport::{HttpTransport, join_path, version_path};

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
pub use shaper::{MaskProbe, MaskProbedEndpoint, mask_probed, shaped};

pub use poem;
pub use schemars;

pub use async_trait::async_trait;

pub use nest_rs_http_macros::{
    controller, crud, http_code, input, interceptor, redirect, response_header, routes,
};

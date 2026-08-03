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
mod edge;
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
pub use client_ip::{ClientIp, ClientOrigin};
pub use config::HttpConfig;
pub use context::{Ctx, RejectedCredential};
pub use controller::{Controller, HttpControllerMeta, HttpRouteMeta, HttpVerb};
pub use cors::CorsConfig;
pub use endpoint::{EdgePosture, HttpEndpointMeta};
pub use module::{HttpModule, HttpSetup};
pub use nest_rs_core::input;
pub use nest_rs_core::{current_body_limit, current_request_scope, with_request_scope};
pub use pipe::{IntoInner, Piped, Valid};
pub use problem::{ProblemDetails, normalize_error_response};
pub use raw_body::RawBody;
pub use reflector::Reflector;
pub use scope::Scoped;
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

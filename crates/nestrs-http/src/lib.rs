//! HTTP transport for nestrs — a [`nestrs_core::Transport`] backed by poem.
//!
//! [`HttpTransport`] mounts every `#[routes]` controller, every self-mounting
//! endpoint another surface declares (a GraphQL schema, an MCP service — each
//! via [`HttpEndpointMeta`]), and any extra endpoint registered with
//! [`HttpTransport::mount`].

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
mod shaper;
mod tls;
mod transport;

pub use client_ip::ClientIp;
pub use config::HttpConfig;
pub use context::Ctx;
pub use controller::{
    Controller, HttpControllerMeta, HttpRouteMeta, HttpVerb, SchemaFn, schema_of,
};
pub use cors::CorsConfig;
pub use endpoint::HttpEndpointMeta;
pub use interceptor::HttpInterceptorMeta;
pub use module::{HttpModule, HttpSetup};
pub use pipe::{IntoInner, Piped, Valid};
pub use problem::ProblemDetails;
pub use raw_body::{RawBody, RawBodyLimit};
pub use reflector::Reflector;
pub use scope::{RequestScopeEndpoint, Scoped};
pub use shaper::{RouteResponseShaper, ShapedEndpoint, shaped};
pub use tls::TlsConfig;
pub use transport::{HttpTransport, join_path, version_path};

pub use poem;
pub use schemars;

pub use async_trait::async_trait;
pub use nestrs_middleware::{EndpointExt, Filter, Guard, Interceptor, Next, RequestSnapshot};

pub use nestrs_http_macros::{
    controller, crud, http_code, input, interceptor, redirect, response_header, routes,
};

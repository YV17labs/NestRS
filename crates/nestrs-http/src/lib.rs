//! HTTP transport for nestrs — a [`nestrs_core::Transport`] backed by poem.
//!
//! [`HttpTransport`] mounts every `#[routes]` controller, every self-mounting
//! endpoint another surface declares (a GraphQL schema, an MCP service — each
//! via [`HttpEndpointMeta`]), and any extra endpoint registered with
//! [`HttpTransport::mount`].

mod config;
mod context;
mod controller;
mod cors;
mod endpoint;
mod interceptor;
mod module;
mod pipe;
mod reflector;
mod scope;
mod shaper;
mod tls;
mod transport;

pub use config::HttpConfig;
pub use context::Ctx;
pub use cors::CorsConfig;
pub use controller::{
    schema_of, Controller, HttpControllerMeta, HttpRouteMeta, HttpVerb, SchemaFn,
};
pub use endpoint::HttpEndpointMeta;
pub use interceptor::HttpInterceptorMeta;
pub use module::{HttpModule, HttpSetup};
pub use pipe::{IntoInner, Piped, Valid};
pub use reflector::Reflector;
pub use scope::{RequestScopeEndpoint, Scoped};
pub use shaper::{shaped, RouteResponseShaper, ShapedEndpoint};
pub use tls::TlsConfig;
pub use transport::{join_path, version_path, HttpTransport};

pub use poem;
pub use schemars;

pub use async_trait::async_trait;
pub use nestrs_middleware::{EndpointExt, Filter, Guard, Interceptor, Next, RequestSnapshot};

pub use nestrs_http_macros::{controller, crud, interceptor, routes};

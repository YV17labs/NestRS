//! HTTP transport for nestrs.
//!
//! [`HttpTransport`] is a [`nestrs_core::Transport`] backed by `poem` that
//! mounts every controller declared in the module tree (each emits an
//! [`HttpControllerMeta`] via the `#[routes]` macro), plus every
//! self-mounting endpoint declared by another surface (a GraphQL schema, an
//! MCP service — each emits an [`HttpEndpointMeta`]), plus any extra
//! endpoints attached imperatively via [`HttpTransport::mount`]. The
//! framework owns the route assembly and the server lifecycle so apps don't
//! have to.
//!
//! Apps interact with this crate through three entry points:
//! - [`HttpTransport`] — the transport itself (`new`, `bind`, `interceptor`,
//!   `mount`, …).
//! - [`Controller`] — implemented automatically by the `#[routes]` macro;
//!   you rarely impl it by hand.
//! - [`HttpControllerMeta`] / [`HttpEndpointMeta`] / [`HttpInterceptorMeta`] —
//!   the discovery metadata that the `#[routes]`, `#[graphql]` /
//!   `#[mcp]`, and `#[interceptor]` macros attach to a type; the
//!   transport reads them at boot via [`nestrs_core::DiscoveryService`].
//! - [`Valid`] / [`Piped`] — the poem adapter that applies a `nestrs_pipes`
//!   pipe to a handler parameter between extraction and the handler (validate
//!   or transform); the pipes themselves live in `nestrs-pipes`.

mod context;
mod controller;
mod endpoint;
mod interceptor;
mod pipe;
mod reflector;
mod scope;
mod shaper;
mod tls;
mod transport;

pub use context::Ctx;
pub use controller::{
    schema_of, Controller, HttpControllerMeta, HttpRouteMeta, HttpVerb, SchemaFn,
};
pub use endpoint::HttpEndpointMeta;
pub use interceptor::HttpInterceptorMeta;
pub use pipe::{IntoInner, Piped, Valid};
pub use reflector::Reflector;
pub use scope::{RequestScopeEndpoint, Scoped};
pub use shaper::{shaped, RouteResponseShaper, ShapedEndpoint};
pub use tls::TlsConfig;
pub use transport::{join_path, version_path, HttpTransport};

pub use poem;

// Re-exported so `#[routes]`-generated schema-capture code names `schemars`
// through the framework (`::nestrs_http::schemars::…`), and so apps can derive
// `nestrs_http::schemars::JsonSchema` on their DTOs without a direct dependency.
pub use schemars;

// `#[routes]`-generated code names `::nestrs_http::EndpointExt` to wrap a
// `#[use_guards]` / `#[use_filters]` / `#[use_interceptors]` handler, and a
// guard, filter, or interceptor is written `#[nestrs_http::async_trait] impl
// nestrs_http::Guard` (resp. `Filter` / `Interceptor`) — so all are surfaced
// here. A bindable interceptor is a plain `#[injectable] + impl Interceptor`;
// `#[interceptor]` (below) stays the global auto-discovered form.
pub use async_trait::async_trait;
pub use nestrs_middleware::{EndpointExt, Filter, Guard, Interceptor, Next, RequestSnapshot};

/// HTTP decorators (`#[controller]`, `#[routes]`, `#[crud]`, the verb
/// attributes, `#[interceptor]`), defined in `nestrs-http-macros` and surfaced
/// here so apps write `nestrs_http::controller` etc.
pub use nestrs_http_macros::{controller, crud, interceptor, routes};

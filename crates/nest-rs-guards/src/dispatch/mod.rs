//! Runtime dispatch of the Layer System chain — the helpers and types
//! the three shaper macros emit at the start of every handler.
//!
//! ## HTTP — per-route shaper wrapped via [`RouteShaper`]
//!
//! Each route gets its own [`RouteShaper`] instance, baked at mount time
//! with the per-route guard / pipe specs the `#[routes]` macro collected
//! from `#[use_guards]` / `#[use_pipes]` / `#[force_guards]`. Wrapped as
//! the outermost handler layer so the guard pool runs before the handler.
//! The response-side pools (exception-filters / filters / interceptors)
//! wrap inside it via `route_layers`.
//!
//! Note: `#[public]` is NOT a framework-level skip — the macro attaches
//! a [`Public`](nest_rs_core::Public) marker via the same metadata
//! mechanism as `#[meta(...)]`, and individual guards decide whether to
//! honor it.
//!
//! ## GraphQL — inline chain call, composed once per site
//!
//! The `#[resolver]` macro emits a call to `run_layered_graphql_chain` at
//! the start of every handler method, beside a `static` [`GraphqlChainCell`]
//! that memoizes the composed chain per container — GraphQL has no mount seam
//! to bake a shaper into, so the site is its own. WS has no inline runner: the
//! `#[messages]` macro composes its per-event guard table at gateway
//! mount, wrapping each guard via `GuardAsWsMessageCheck`.

// The GraphQL in-band chain runner lives here; only compiled when that
// transport is served.
#[cfg(feature = "graphql")]
mod chain;
mod denial_convert;
// The two in-band fallback operation guards, one per `Exempt`-edge transport,
// over the pool they share.
#[cfg(any(feature = "graphql", feature = "mcp"))]
mod global_pool;
#[cfg(feature = "graphql")]
mod graphql_operation_guard;
#[cfg(feature = "mcp")]
mod mcp_operation_guard;
mod route_layers;
mod route_shaper;
mod scoped_spec;
mod validate;

#[cfg(feature = "graphql")]
pub use chain::{GraphqlChainCell, GraphqlChainSources, run_layered_graphql_chain};
#[cfg(feature = "graphql")]
pub use denial_convert::denial_to_graphql_error;
pub use denial_convert::denial_to_http_response;
pub(crate) use denial_convert::deny_http;
#[cfg(feature = "graphql")]
pub use graphql_operation_guard::GlobalPoolOperationGuard;
#[cfg(feature = "mcp")]
pub use mcp_operation_guard::GlobalPoolMcpGuard;
pub use route_layers::{wrap_route_exception_filters, wrap_route_filters, wrap_route_interceptors};
pub use route_shaper::RouteShaper;
pub use scoped_spec::{
    ScopedExceptionFilterSpec, ScopedFilterSpec, ScopedGuardSpec, ScopedInterceptorSpec,
    ScopedLayerSpec, ScopedPipeSpec,
};
pub use validate::{boot_validate_guards, validate_guard_chain};

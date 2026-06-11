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
//! wrap inside it via [`route_layers`].
//!
//! Note: `#[public]` is NOT a framework-level skip — the macro attaches
//! a [`Public`](nest_rs_core::Public) marker via the same metadata
//! mechanism as `#[meta(...)]`, and individual guards decide whether to
//! honor it.
//!
//! ## GraphQL / WS — inline chain calls
//!
//! The `#[resolver]` and `#[messages]` macros emit a call to
//! [`run_layered_graphql_chain`] / [`run_layered_ws_chain`] at the start
//! of every handler method.

mod chain;
mod denial_convert;
#[cfg(feature = "graphql")]
mod operation_guard;
mod route_layers;
mod route_shaper;
mod scoped_spec;

pub use chain::{run_layered_ws_chain};
#[cfg(feature = "graphql")]
pub use chain::run_layered_graphql_chain;
pub use denial_convert::denial_to_http_response;
#[cfg(feature = "graphql")]
pub use denial_convert::denial_to_graphql_error;
#[cfg(feature = "graphql")]
pub use operation_guard::GlobalPoolOperationGuard;
pub use route_layers::{wrap_route_exception_filters, wrap_route_filters, wrap_route_interceptors};
pub use route_shaper::RouteShaper;
pub use scoped_spec::{
    ScopedExceptionFilterSpec, ScopedFilterSpec, ScopedGuardSpec, ScopedInterceptorSpec,
    ScopedLayerSpec, ScopedPipeSpec,
};

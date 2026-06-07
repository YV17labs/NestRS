//! Runtime dispatch of the Layer System chain — the helpers and types
//! the three shaper macros emit at the start of every handler.
//!
//! ## HTTP — per-route shaper wrapped via [`RouteShaper`]
//!
//! Each route gets its own [`RouteShaper`] instance, baked at mount time
//! with the per-route guard / pipe / exception-filter specs the
//! `#[routes]` macro collected from `#[use_guards]` / `#[use_pipes]` /
//! `#[use_exception_filters]` / `#[force_guards]`. Wrapped as the
//! outermost handler layer so the global chain runs before the handler.
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
mod route_shaper;
mod scoped_spec;

pub use chain::{run_layered_graphql_chain, run_layered_ws_chain};
pub use denial_convert::{denial_to_graphql_error, denial_to_http_response};
pub use route_shaper::RouteShaper;
pub use scoped_spec::{
    ScopedExceptionFilterSpec, ScopedGuardSpec, ScopedLayerSpec, ScopedPipeSpec,
};

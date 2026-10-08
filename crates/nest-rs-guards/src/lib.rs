//! # nest-rs-guards
//!
//! Transport-spanning guards for nestrs — **one trait, four transports**
//! (HTTP, GraphQL, WS, MCP). Declared once with
//! `App::builder().use_guards_global(...)`, every handler on every
//! transport runs through the chain.
//!
//! Plug-in point for the Layer System: every guard is a [`Layer`](nest_rs_core::Layer), so the
//! `#[routes]` / `#[operations]` / `#[messages]` shapers dedup by `TypeId` when
//! the same guard is declared at multiple sites (global + controller +
//! method) — the broadest [`LayerSite`](nest_rs_core::LayerSite) wins and the
//! rest log a `warn`. The framework runs guards in **declaration order**;
//! [`Layer::priority`](nest_rs_core::Layer::priority) is an opt-in tiebreaker.
//!
//! `#[public]` is not a framework-level skip: the macro attaches a
//! [`Public`](nest_rs_http::Public) marker via the same metadata channel
//! as `#[meta(...)]`, and each guard decides whether to honor it. An
//! `AbilityGuard` may still run on a public route to apply visitor rules;
//! an `AuthnGuard` may skip rejection when no token is present.
//!
//! ## Defining a guard
//!
//! Override only the `check_*` method(s) where this guard has work to do —
//! the rest inherit `Ok(())` defaults — then **attest each edge you
//! overrode** with its marker ([`HttpGuard`], [`GraphqlGuard`], [`WsGuard`],
//! [`McpGuard`]). The impl-half decorators bind against the marker, so a guard
//! declared where it has no `check_*` is a compile error at the
//! `#[use_guards]` line instead of a chain entry that passes everything.
//! `Layer` provides `priority()` / `name()` defaults; override `priority()`
//! only when this guard must beat declaration order.
//!
//! ```
//! use nest_rs_guards::prelude::*;
//! # mod audit { pub const TARGET: &str = "features::audit"; }
//!
//! #[injectable]
//! #[derive(Default)]
//! pub struct AuditGuard;
//!
//! impl Layer for AuditGuard {}
//!
//! #[async_trait]
//! impl Guard for AuditGuard {
//!     async fn check_http(&self, req: &mut HttpRequest) -> Result<(), Denial> {
//!         tracing::info!(target: audit::TARGET, method = %req.method(), path = %req.uri(), "request seen");
//!         Ok(())
//!     }
//! }
//!
//! impl HttpGuard for AuditGuard {}
//! # fn main() {}
//! ```
//!
//! ## Registering globally
//!
//! Register with `App::builder().use_guards_global([...])`
//! ([`AppBuilderGuardsExt`]); the example on [`guard`](fn@guard) runs it.
//!
//! Declaration order is the runtime order. If you list `AuthzGuard` before
//! `AuthnGuard` the authorization check runs against an empty principal — a
//! name-based heuristic logs a `warn` at boot.
//!
//! The pool holds `Arc<dyn Guard>` and runs every `check_*` a pooled guard
//! overrides, marker or not; decorator sites bind on the markers, so declare
//! them anyway.
//!
//! ## Marking a handler `#[public]`
//!
//! ```
//! # use nest_rs_http::{controller, routes};
//! # #[controller(path = "/")]
//! # struct HealthController;
//! # #[routes]
//! # impl HealthController {
//! #[get("/health/live")]
//! #[public]
//! async fn live(&self) -> &'static str { "ok" }
//! # }
//! # fn main() {}
//! ```
//!
//! The macro attaches a [`Public`](nest_rs_http::Public) marker to the
//! route. Guards that want to honor it read it via the transport's
//! reflector and adjust their policy.
//!
//! ## Architecture
//!
//! `#[routes]` bakes a [`RouteShaper`] per route at mount; `#[operations]`
//! emits a `run_layered_graphql_chain` call at the start of every resolver
//! method; `#[messages]` composes its per-event guard table at gateway
//! mount (wrapping each guard via `GuardAsWsMessageCheck`) — the per-route
//! entry is what gives TypeId-level dedup against the global chain. Guards
//! have **no** transport-edge band: the pool executes in the shaper
//! (post-routing, so it reads `#[public]`), at a `Guarded` self-mount's
//! edge (`SelfMountGuardWrap`), or in-band on `/graphql` (the
//! `GlobalPoolOperationGuard`
//! fallback when no bridge is registered).
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod builder;
mod denial;
pub mod dispatch;
mod endpoint;
mod guard;
pub mod prelude;
mod registry;
mod scope;

pub use builder::{AppBuilderGuardsExt, AppBuilderPipesExt};
pub use denial::Denial;
pub use endpoint::{GuardEndpoint, GuardExt};
pub use guard::{Guard, GuardPhase, HttpGuard, PrincipalClaim};
// Capability markers the impl-half decorators bind on. `HttpGuard` takes no
// `cfg`: HTTP is the substrate the other three edges mount on.
#[cfg(feature = "graphql")]
pub use guard::GraphqlGuard;
#[cfg(feature = "mcp")]
pub use guard::McpGuard;
#[cfg(feature = "ws")]
pub use guard::WsGuard;
pub use scope::{GrantedScopes, NoBearerChallenge, RequiredScopes};
// The bridge `#[messages]` wraps per-event guards in.
#[cfg(feature = "ws")]
pub use guard::GuardAsWsMessageCheck;
// Re-exported for macro-emitted code.
pub use nest_rs_core::layer_chain;
pub use registry::{GuardSpec, GuardSpecs, PipeSpec, PipeSpecs, guard, pipe};
// Re-exported so a `Guard` impl needs no `async-trait` dependency of its own.
pub use async_trait::async_trait;

// Re-exported for macro-emitted code.
#[cfg(feature = "ws")]
pub use dispatch::denial_to_ws_error;
#[cfg(feature = "graphql")]
pub use dispatch::{GraphqlSite, denial_to_graphql_error, run_layered_graphql_chain};
pub use dispatch::{RouteShaper, denial_to_http_error, denial_to_http_response};
#[cfg(any(feature = "graphql", feature = "mcp"))]
pub use dispatch::{SiteChainCell, SiteChainSources};
#[cfg(feature = "mcp")]
pub use dispatch::{denial_to_mcp_error, run_layered_mcp_chain};

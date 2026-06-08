//! # nest-rs-guards
//!
//! Transport-spanning guards for nestrs — **one trait, three transports**
//! (HTTP, GraphQL, WS). Declared once with
//! `App::builder().use_guards_global(...)`, every handler on every
//! transport runs through the chain.
//!
//! Plug-in point for the Layer System: every guard is a [`Layer`], so the
//! `#[routes]` / `#[resolver]` / `#[messages]` shapers dedup by `TypeId` when
//! the same guard is declared at multiple sites (global + controller +
//! method) — the broadest [`LayerSite`](nest_rs_core::LayerSite) wins and the
//! rest log a `warn`. The framework runs guards in **declaration order**;
//! [`Layer::priority`] is an opt-in tiebreaker.
//!
//! `#[public]` is not a framework-level skip: the macro attaches a
//! [`Public`](nest_rs_core::Public) marker via the same metadata channel
//! as `#[meta(...)]`, and each guard decides whether to honor it. An
//! `AbilityGuard` may still run on a public route to apply visitor rules;
//! an `AuthGuard` may skip rejection when no token is present.
//!
//! ## Defining a guard
//!
//! Override only the `check_*` method(s) where this guard has work to do —
//! the rest inherit `Ok(())` defaults. `Layer` provides `priority()` /
//! `name()` defaults; override `priority()` only when this guard must beat
//! declaration order.
//!
//! ```rust,ignore
//! use nest_rs_guards::prelude::*;
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
//!         tracing::info!(target: "audit", method = %req.method(), path = %req.uri(), "request");
//!         Ok(())
//!     }
//! }
//! ```
//!
//! ## Registering globally
//!
//! ```rust,ignore
//! use nest_rs::App;
//! use nest_rs_guards::{AppBuilderGuardsExt, guard};
//!
//! App::builder()
//!     .use_guards_global([guard::<AuthGuard>(), guard::<AuthzGuard>()])
//!     .module::<AppModule>()
//!     .build().await?
//!     .run().await
//! ```
//!
//! Declaration order is the runtime order. If you list `AuthzGuard` before
//! `AuthGuard` the authorization check runs against an empty principal — a
//! name-based heuristic logs a `warn` at boot.
//!
//! ## Marking a handler `#[public]`
//!
//! ```rust,ignore
//! #[get("/health/live")]
//! #[public]
//! async fn live() -> &'static str { "ok" }
//! ```
//!
//! The macro attaches a [`Public`](nest_rs_core::Public) marker to the
//! route. Guards that want to honor it read it via the transport's
//! reflector and adjust their policy.
//!
//! ## Architecture
//!
//! Each shaper macro (`#[routes]`, `#[resolver]`, `#[messages]`) emits a
//! call to one of [`RouteShaper`] / [`run_layered_graphql_chain`]
//! / [`run_layered_ws_chain`] at the start of every handler. There is no
//! global interceptor — the per-route entry is the whole point so we get
//! TypeId-level dedup against the global chain.
//!
//! **Larger than its siblings on purpose.** Where `nest-rs-interceptors` /
//! `nest-rs-filters` / `nest-rs-exception-filters` each carry only their own
//! trait + a builder + a registry, this crate also owns the cross-transport
//! [`dispatch`] machinery (the [`RouteShaper`] entry, the layer-chain helpers,
//! the graphql/ws chain runners) that the other three trio members consume.
//! Splitting it would mean duplicating the chain across crates or routing
//! through a fifth — both worse than the asymmetry.

mod builder;
mod denial;
mod endpoint;
mod guard;
pub mod dispatch;
pub mod layer_chain;
pub mod prelude;
mod registry;

pub use builder::{AppBuilderGuardsExt, AppBuilderPipesExt};
// Cross-transport interceptor / filter / exception-filter trait methods
// (`wrap_graphql`, `wrap_ws`, `filter_graphql`, `filter_ws`,
// `catch_graphql`, `catch_ws`) and the matching continuation types
// (`GraphqlNext`, `WsNext`) now live directly on the base traits in
// `nest-rs-interceptors` / `nest-rs-filters` / `nest-rs-exception-filters`.
// Re-exported here for the historical import path used by the macros.
pub use nest_rs_interceptors::{GraphqlNext, WsNext};
pub use denial::Denial;
pub use endpoint::{GuardEndpoint, GuardExt};
pub use guard::{Guard, GuardAsWsMessageCheck};
pub use layer_chain::{LayerSource, ResolvedLayer};
pub use registry::{GuardSpec, GuardSpecs, PipeSpec, PipeSpecs, guard, pipe};

// Re-export dispatch helpers for macro-emitted code.
pub use dispatch::{
    RouteShaper, denial_to_graphql_error, denial_to_http_response,
    run_layered_graphql_chain, run_layered_ws_chain,
};

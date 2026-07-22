//! # nest-rs-guards
//!
//! Transport-spanning guards for nestrs — **one trait, three transports**
//! (HTTP, GraphQL, WS). Declared once with
//! `App::builder().use_guards_global(...)`, every handler on every
//! transport runs through the chain.
//!
//! Plug-in point for the Layer System: every guard is a [`Layer`](nest_rs_core::Layer), so the
//! `#[routes]` / `#[resolver]` / `#[messages]` shapers dedup by `TypeId` when
//! the same guard is declared at multiple sites (global + controller +
//! method) — the broadest [`LayerSite`] wins and the
//! rest log a `warn`. The framework runs guards in **declaration order**;
//! [`Layer::priority`](nest_rs_core::Layer::priority) is an opt-in tiebreaker.
//!
//! `#[public]` is not a framework-level skip: the macro attaches a
//! [`Public`](nest_rs_core::Public) marker via the same metadata channel
//! as `#[meta(...)]`, and each guard decides whether to honor it. An
//! `AbilityGuard` may still run on a public route to apply visitor rules;
//! an `AuthnGuard` may skip rejection when no token is present.
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
//!         tracing::info!(target: "api::authn", method = %req.method(), path = %req.uri(), "request seen");
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
//!     .use_guards_global([guard::<AuthnGuard>(), guard::<AuthzGuard>()])
//!     .module::<AppModule>()
//!     .build().await?
//!     .run().await
//! ```
//!
//! Declaration order is the runtime order. If you list `AuthzGuard` before
//! `AuthnGuard` the authorization check runs against an empty principal — a
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
//! `#[routes]` bakes a [`RouteShaper`] per route at mount; `#[resolver]`
//! emits a `run_layered_graphql_chain` call at the start of every resolver
//! method; `#[messages]` composes its per-event guard table at gateway
//! mount (wrapping each guard via `GuardAsWsMessageCheck`) — the per-route
//! entry is what gives TypeId-level dedup against the global chain. Guards
//! have **no** transport-edge band: the pool executes in the shaper
//! (post-routing, so it reads `#[public]`), at a `Guarded` self-mount's
//! edge (`SelfMountGuardWrap`), or in-band on `/graphql` (the
//! `GlobalPoolOperationGuard`
//! fallback when no bridge is registered).
//!
//! **Larger than its siblings on purpose.** Where `nest-rs-interceptors` /
//! `nest-rs-filters` / `nest-rs-exception-filters` each carry only their own
//! trait + a builder + a registry, this crate also owns the cross-transport
//! [`dispatch`] machinery (the [`RouteShaper`] entry, the layer-chain helpers,
//! the graphql chain runner and the WS message bridge) that the other three
//! trio members consume.
//! Splitting it would mean duplicating the chain across crates or routing
//! through a fifth — both worse than the asymmetry.
//!
//! **HTTP-coupled by design.** [`Guard`] requires `check_http` and this
//! crate depends on the HTTP stack, so a worker-only binary links HTTP even
//! when it never serves a request. This is the deliberate 1.x shape: one
//! trait, one chain, zero duplicated dispatch. The cost is binary size
//! only — there is no runtime, security or correctness effect, and
//! `check_graphql` / `check_ws_message` are already feature-gated. Moving
//! `check_http` onto an `HttpGuard` extension trait touches every guard impl
//! and the HTTP dispatch, so a transport-neutral guard core is a planned
//! major-version evolution (see ROADMAP, "Transport-neutral guard core").
#![warn(missing_docs)]

mod builder;
mod denial;
pub mod dispatch;
mod endpoint;
mod guard;
pub mod prelude;
mod registry;

pub use builder::{AppBuilderGuardsExt, AppBuilderPipesExt};
pub use denial::Denial;
pub use endpoint::{GuardEndpoint, GuardExt};
pub use guard::{Guard, GuardPhase, PrincipalClaim};
// The WS bridge the `#[messages]` macro wraps per-event guards in — only
// exists (and is only needed) when the `ws` feature is on.
#[cfg(feature = "ws")]
pub use guard::GuardAsWsMessageCheck;
// The dedup logic itself lives in `nest_rs_core::layer_chain` — the single
// home every execution site (route shaper, transport pool folds, graphql/ws
// in-band chains) composes through. Re-exported for macro-emitted code.
pub use nest_rs_core::layer_chain;
pub use registry::{GuardSpec, GuardSpecs, PipeSpec, PipeSpecs, guard, pipe};

// Re-export dispatch helpers for macro-emitted code.
#[cfg(feature = "graphql")]
pub use dispatch::{
    GraphqlChainCell, GraphqlChainSources, denial_to_graphql_error, run_layered_graphql_chain,
};
pub use dispatch::{RouteShaper, denial_to_http_response};

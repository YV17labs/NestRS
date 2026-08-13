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
//! the rest inherit `Ok(())` defaults — then **attest each edge you
//! overrode** with its marker ([`HttpGuard`], [`GraphqlGuard`], [`WsGuard`],
//! [`McpGuard`]). The impl-half decorators bind against the marker, so a guard
//! declared where it has no `check_*` is a compile error at the
//! `#[use_guards]` line instead of a chain entry that passes everything.
//! `Layer` provides `priority()` / `name()` defaults; override `priority()`
//! only when this guard must beat declaration order.
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
//!
//! impl HttpGuard for AuditGuard {}
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
//! **HTTP-coupled by design, and it costs nothing.** [`Guard`] carries
//! `check_http` unconditionally and this crate depends on `nest-rs-http`
//! unconditionally — no `cfg`, no optional flag. That is the deliberate shape:
//! one trait, one `dyn Guard`, zero duplicated dispatch. It is also why gating
//! the method would save nothing — every build that links this crate links the
//! HTTP stack whatever the consumer asked for, so there are no bytes for a
//! `cfg` to save. A headless worker links neither.
//!
//! Moving `check_http` onto an extension trait would therefore buy nothing and
//! cost a second erasure: every execution site holds `Arc<dyn Guard>`, so a
//! guard serving HTTP *and* GraphQL would need two container registrations. The
//! attestation the extension trait was wanted for is what [`HttpGuard`]
//! provides, at no runtime cost.
#![warn(missing_docs)]

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
// Capability markers: which transports a guard actually checks. The impl-half
// decorators emit a bound against these, so a guard bound where it has no
// `check_*` is a compile error rather than a chain entry that passes everything.
// `HttpGuard` is not behind a `cfg`: its three siblings gate a `check_*` that
// only exists when that edge is compiled in, and HTTP is the substrate the other
// three mount on.
#[cfg(feature = "graphql")]
pub use guard::GraphqlGuard;
#[cfg(feature = "mcp")]
pub use guard::McpGuard;
#[cfg(feature = "ws")]
pub use guard::WsGuard;
pub use scope::{GrantedScopes, RequiredScopes};
// The WS bridge the `#[messages]` macro wraps per-event guards in — only
// exists (and is only needed) when the `ws` feature is on.
#[cfg(feature = "ws")]
pub use guard::GuardAsWsMessageCheck;
// The dedup logic itself lives in `nest_rs_core::layer_chain` — the single
// home every execution site (route shaper, transport pool folds, graphql/ws
// in-band chains) composes through. Re-exported for macro-emitted code.
pub use nest_rs_core::layer_chain;
pub use registry::{GuardSpec, GuardSpecs, PipeSpec, PipeSpecs, guard, pipe};
// Re-exported so a crate writing an `Guard` impl needs no direct
// `async-trait` dependency of its own. `nest-rs-http`, `nest-rs-queue` and
// `nest-rs-ws` already do this; the layer crates did not, so the one import a
// reader needed most was the one no page could name — and the miss cascades
// (without the attribute, every trait method reports a lifetime mismatch, so
// the real cause is buried under four unrelated errors).
pub use async_trait::async_trait;

// Re-export dispatch helpers for macro-emitted code. The chain cell and its
// sources are shared by the two in-band transports — one memo, one composition,
// so a fix to either cannot land on only one of them.
#[cfg(feature = "ws")]
pub use dispatch::denial_to_ws_error;
pub use dispatch::{RouteShaper, denial_to_http_error, denial_to_http_response};
#[cfg(any(feature = "graphql", feature = "mcp"))]
pub use dispatch::{SiteChainCell, SiteChainSources};
#[cfg(feature = "graphql")]
pub use dispatch::{denial_to_graphql_error, run_layered_graphql_chain};
#[cfg(feature = "mcp")]
pub use dispatch::{denial_to_mcp_error, run_layered_mcp_chain};

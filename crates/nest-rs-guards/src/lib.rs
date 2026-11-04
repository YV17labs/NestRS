//! # nest-rs-guards
//!
//! One guard contract for every edge. A [`Guard`] writes its decision once, in
//! [`check`](Guard::check): it runs on a unit an in-band edge carries — a
//! GraphQL field, a WebSocket message, an MCP operation — and on an HTTP
//! request too — a route, a WebSocket upgrade, the request of an in-band
//! endpoint — through [`check_http`](Guard::check_http), which a guard
//! overrides only where the request differs from the unit. Declared once, by
//! the module the app boots ([`GlobalGuards`]), the pool runs at every site of
//! every edge, and the edge files every denial once, on the guard's
//! [`target`](Guard::target), with the [`reason`](Denial::reason) it names.
//!
//! The contract names a unit's context, never an edge's types: this crate
//! depends on `nest-rs-core` alone, and every edge depends on it and runs the
//! guards itself.
//!
//! Every guard is a [`Layer`](nest_rs_core::Layer), so a guard declared at
//! several sites (global, host, method) runs once, deduplicated by `TypeId` —
//! the broadest [`LayerSite`](nest_rs_core::LayerSite) wins, and a redundant
//! declaration is logged at `debug`. Sites run from the broadest in, each in
//! **declaration order**; [`Layer::priority`](nest_rs_core::Layer::priority) is
//! an opt-in tiebreaker within a site, never across.
//!
//! `#[public]` is not a framework-level skip: a site's
//! [`Posture`](nest_rs_core::Posture) is read through
//! [`UnitContext::reflector`](nest_rs_core::UnitContext::reflector), and each
//! guard decides what it means — an authentication guard admits an anonymous
//! caller there, an ability guard builds the visitor's rules.
//!
//! ## Defining a guard
//!
//! Write the decision in `check` — required, so no guard admits a unit by
//! default — then **declare each edge it checks** with [`GuardFor`]. A guard
//! bound on routes reads the HTTP request there, as `AuditGuard` below does;
//! one in the global pool deciding on the request writes that decision in
//! `check_http` instead (see *Registering globally*). The decorators bind a
//! guard on the edges it declares, so a guard bound where its decision reads
//! another edge's view is a compile error at the `#[use_guards]` line instead
//! of a chain entry that refuses everything. `#[derive(Layer)]` writes the
//! [`Layer`](nest_rs_core::Layer) impl every guard extends.
//!
//! ```
//! use nest_rs_core::{Layer, UnitContext, async_trait, injectable};
//! use nest_rs_guards::{Denial, Guard, GuardFor};
//! use nest_rs_http::HttpView;
//! # mod audit { pub const TARGET: &str = "features::audit"; }
//!
//! #[injectable]
//! #[derive(Layer)]
//! pub struct AuditGuard;
//!
//! #[async_trait]
//! impl Guard for AuditGuard {
//!     async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
//!         let request = HttpView::of(cx)?.request();
//!         tracing::info!(target: audit::TARGET, method = %request.method(), "request seen");
//!         Ok(())
//!     }
//! }
//!
//! impl GuardFor<HttpView> for AuditGuard {}
//! # fn main() {}
//! ```
//!
//! A value a guard makes known for the rest of the unit — a principal, a
//! tenant — is attached with [`UnitContext::attach`](nest_rs_core::UnitContext::attach):
//! its type implements [`AmbientValue`](nest_rs_core::AmbientValue), and it is
//! ambient once the whole stage passed, never on a refused unit.
//!
//! ## Registering globally
//!
//! List [`GlobalGuards`] in the providers of the module the app boots — the
//! binary's `main` and its test suites boot the same module, so they run the
//! same pool; its example runs it.
//!
//! Declaration order is the runtime order. List `AuthzGuard` before
//! `AuthnGuard` — globally, on a controller or gateway, or beside one route —
//! and the boot fails naming both, instead of every request answering `500`.
//!
//! The pool holds `Arc<dyn Guard>` and runs whichever entry a site calls,
//! declared or not: `check_http` at every route and request stage — the
//! `/graphql` and `/mcp` requests, a WebSocket upgrade — and `check` at every
//! GraphQL field, WebSocket message and MCP operation, which has no HTTP view.
//! So a pooled guard deciding on the HTTP request writes it in `check_http`
//! and admits in `check`, with `Ok(())`, the units the request it checked
//! carries; one deciding on a unit admits the request in `check_http`. The
//! decorators bind on [`GuardFor`], so declare the edges anyway.
//!
//! ## Where guards run
//!
//! The HTTP edge runs [`check_http`](Guard::check_http) after routing, so a
//! guard reads the route's `#[public]`; a self-mount runs it at its request.
//! The in-band edges run [`check`](Guard::check) per operation, from a chain
//! each site composes once.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod chain;
mod denial;
mod guard;
mod line;
mod reason;
mod registry;
mod scope;
mod stage;
mod validate;

pub use denial::Denial;
pub use guard::{Guard, GuardFor, GuardPhase, PrincipalClaim};
pub use registry::{GlobalGuardList, GlobalGuards};
pub use scope::{GrantedScopes, RequiredScopes, Scope};

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::chain::{GlobalBucket, SiteChainSources, compose_site_chain, split_at_covered};
    pub use crate::denial::{ANONYMOUS_CALLER, UNAUTHENTICATED, denial_of};
    pub use crate::reason::{client_message, denial_code, denial_reason};
    pub use crate::registry::{
        GuardSpec, global_guards_active, guard, resolve_global_chain, resolve_global_guards,
    };
    pub use crate::scope::is_scope_token;
    pub use crate::stage::{RefusalMark, Refused, check_guards};
    pub use crate::validate::{
        boot_validate_guards, host_chain, identifying_guard, validate_carried_chain,
        validate_guard_chain, validate_route_chain,
    };

    pub use nest_rs_core::__private::{ResolvedLayer, compose_chain};
}

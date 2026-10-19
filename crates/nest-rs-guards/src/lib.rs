//! # nest-rs-guards
//!
//! One guard contract for every edge. A [`Guard`] checks an HTTP request
//! through [`check_http`](Guard::check_http) — a route, a WebSocket upgrade, the
//! request of an in-band endpoint — and a unit an in-band edge carries through
//! [`check`](Guard::check): a GraphQL field, a WebSocket message, an MCP
//! operation. Declared once with `App::builder().use_guards_global(...)`, the
//! pool runs at every site of every edge.
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
//! Override the entry where the guard has work — both admit by default — then
//! **declare each edge it checks** with [`GuardFor`]. The decorators bind a
//! guard on the edges it declares, so a guard declared where its check is the
//! default is a compile error at the `#[use_guards]` line instead of a chain
//! entry that passes everything.
//!
//! ```
//! use nest_rs_guards::prelude::*;
//! use nest_rs_http::HttpView;
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
//!     async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
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
//! Register with `App::builder().use_guards_global([...])`
//! ([`AppBuilderGuardsExt`]); the example on [`guard`](fn@guard) runs it.
//!
//! Declaration order is the runtime order. List `AuthzGuard` before
//! `AuthnGuard` — globally, on a controller or gateway, or beside one route —
//! and the boot fails naming both, instead of every request answering `500`.
//!
//! The pool holds `Arc<dyn Guard>` and runs whichever entry a site calls,
//! declared or not; the decorators bind on [`GuardFor`], so declare the edges
//! anyway.
//!
//! ## Where guards run
//!
//! The HTTP edge runs [`check_http`](Guard::check_http) after routing, so a
//! guard reads the route's `#[public]`; a self-mount runs it at its request.
//! The in-band edges run [`check`](Guard::check) per operation, from a chain
//! each site composes once.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod builder;
mod chain;
mod denial;
mod guard;
pub mod prelude;
mod reason;
mod registry;
mod scope;
mod stage;
#[cfg(test)]
mod testing;
mod validate;

pub use builder::AppBuilderGuardsExt;
pub use denial::Denial;
pub use guard::{Guard, GuardFor, GuardPhase, PrincipalClaim};
pub use registry::{GuardSpec, GuardSpecs, guard};
pub use scope::{GrantedScopes, RequiredScopes};
// Re-exported so a `Guard` impl needs no `async-trait` dependency of its own.
pub use async_trait::async_trait;

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::chain::{
        Chain, GlobalBucket, SiteChainCell, SiteChainSources, compose_site_chain, split_at_covered,
    };
    pub use crate::denial::{denial_of, failure_chain};
    pub use crate::reason::{client_message, denial_code, structured_reason};
    pub use crate::registry::{global_guards_active, resolve_global_chain, resolve_global_guards};
    pub use crate::stage::{RefusalMark, Refused, run_stage};
    pub use crate::validate::{boot_validate_guards, validate_guard_chain, validate_route_chain};

    pub use nest_rs_core::__private::{ResolvedLayer, compose_chain};
}

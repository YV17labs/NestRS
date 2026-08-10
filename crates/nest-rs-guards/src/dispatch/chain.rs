//! The per-site guard chain: what an in-band transport composes once and then
//! runs on every operation.
//!
//! HTTP bakes its chain into a [`RouteShaper`](super::RouteShaper) at mount and
//! WS into an `EventLayerTable` at gateway mount. GraphQL and MCP have no such
//! seam — the schema is built by `nest-rs-graphql` and the MCP host by rmcp,
//! neither of which can see [`Guard`] — so each *site* memoizes its own chain in
//! a [`SiteChainCell`] the decorator emits as a `static` beside the call.
//! Composition (container lookups, dedup, sort) therefore happens once per site,
//! not once per operation; the steady-state cost is one atomic load and one
//! `Arc` clone.
//!
//! The cell is keyed by [`ContainerId`] rather than blindly memoizing: a test
//! process serves several apps, and one app's guard chain must never gate
//! another's operations. Ids are never recycled, so a stale hit is impossible.
//!
//! Both transports share the cell and the sources; only *which* `check_*` runs
//! and how a [`Denial`](crate::Denial) renders differ, and those live in
//! `graphql_chain.rs` and `mcp_chain.rs` beside their transports.

use std::any::TypeId;
use std::sync::{Arc, Mutex, OnceLock};

use nest_rs_core::layer_chain::{LayerSite, ResolvedLayer, compose_chain, dedup_bucket};
use nest_rs_core::{Container, ContainerId};

use crate::Guard;
use crate::dispatch::route_shaper::log_effective_chain;
use crate::dispatch::scoped_spec::{ScopedGuardSpec, resolve_global_guards, resolve_specs};

/// The scope-tagged guard declarations of one operation site, as the decorator
/// knows them. Read **once per site** — on the cache miss that composes the
/// chain — so building the `Vec`s costs nothing per operation.
///
/// Macro-emitted, not public API.
#[doc(hidden)]
pub struct SiteChainSources {
    /// `#[use_guards(...)]` on the provider — the resolver struct, the MCP host.
    pub provider: Vec<ScopedGuardSpec>,
    /// `#[use_guards(...)]` beside the operation.
    pub method: Vec<ScopedGuardSpec>,
    /// `#[force_guards(...)]` — replay these even when a broader scope has them.
    pub force: Vec<TypeId>,
}

/// One site's composed guard chain, memoized per [`ContainerId`].
///
/// A decorator emits one as a `static` per guarded operation and hands it to
/// its transport's runner.
///
/// Macro-emitted, not public API.
#[doc(hidden)]
#[derive(Default)]
pub struct SiteChainCell {
    /// The serving app's chain — the only entry a real process ever fills.
    primary: OnceLock<Cached>,
    /// Further apps sharing the process (integration tests build several).
    /// Allocated only when a second container reaches this site.
    extra: OnceLock<Mutex<Vec<Cached>>>,
}

struct Cached {
    container: ContainerId,
    chain: Arc<[ResolvedLayer<dyn Guard>]>,
}

impl SiteChainCell {
    /// An empty cell — `const` so the macro can put one in a `static`.
    pub const fn new() -> Self {
        Self {
            primary: OnceLock::new(),
            extra: OnceLock::new(),
        }
    }

    /// This site's chain for `container`, composing it on first sight.
    ///
    /// `globals` says where this transport's pool executes; `already_ran` is
    /// what the transport's edge reports it has already executed for the
    /// request. See [`GlobalScope`] and [`compose`].
    pub(crate) fn chain(
        &self,
        container: &Container,
        route_label: &str,
        globals: GlobalScope,
        already_ran: &[TypeId],
        sources: &(dyn Fn() -> SiteChainSources + Sync),
    ) -> Arc<[ResolvedLayer<dyn Guard>]> {
        let id = container.id();
        let primary = self.primary.get_or_init(|| Cached {
            container: id,
            chain: compose(container, route_label, globals, already_ran, sources()),
        });
        if primary.container == id {
            return Arc::clone(&primary.chain);
        }

        // Another app in the same process. A poisoned lock must not deny
        // service — the vector holds only memoized values, so recovering it is
        // safe (worst case a chain composes twice).
        let mut slots = self
            .extra
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(hit) = slots.iter().find(|c| c.container == id) {
            return Arc::clone(&hit.chain);
        }
        let chain = compose(container, route_label, globals, already_ran, sources());
        slots.push(Cached {
            container: id,
            chain: Arc::clone(&chain),
        });
        chain
    }
}

/// Where this transport's app-wide guard pool executes.
///
/// The two in-band transports answer differently, and the difference is
/// structural rather than stylistic.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum GlobalScope {
    /// At this site. `/graphql`'s operation guard is the app's own bridge, so
    /// the resolver site is where a pooled guard reaches a `#[query]`.
    #[cfg_attr(
        not(feature = "graphql"),
        expect(
            dead_code,
            reason = "GraphQL is the only site that runs the pool in-band"
        )
    )]
    AtThisSite,
    /// At the transport's **edge**, before any operation dispatches — `/mcp`,
    /// whose `McpOperationGuard` gates the HTTP request itself. The pool is
    /// therefore not part of what an operation's own chain runs; whether a
    /// *particular* pooled guard already ran is answered by `already_ran`, not
    /// assumed from the pool's membership.
    AtTheEdge,
}

/// Resolve, dedup and order this site's chain.
///
/// `already_ran` is dropped from **every** bucket before `compose_chain` sees
/// them, and the order matters: filtering afterwards would let the dedup collapse
/// a scoped declaration onto a global entry that is then removed, so a guard the
/// developer explicitly wrote beside an operation would silently never run. That
/// is a fail-open, and dropping first is what makes it unrepresentable.
fn compose(
    container: &Container,
    route_label: &str,
    globals: GlobalScope,
    already_ran: &[TypeId],
    sources: SiteChainSources,
) -> Arc<[ResolvedLayer<dyn Guard>]> {
    let ran = |entry: &ResolvedLayer<dyn Guard>| already_ran.contains(&entry.type_id);
    let global = match globals {
        GlobalScope::AtThisSite => dedup_bucket(resolve_global_guards(container)),
        GlobalScope::AtTheEdge => Vec::new(),
    };
    let mut global = global;
    global.retain(|entry| !ran(entry));
    let mut provider = resolve_specs(container, &sources.provider, LayerSite::Controller);
    provider.retain(|entry| !ran(entry));
    let mut method = resolve_specs(container, &sources.method, LayerSite::Method);
    method.retain(|entry| !ran(entry));

    let chain = compose_chain::<dyn Guard>(global, provider, method, &sources.force, route_label);
    log_effective_chain(route_label, "guards", &chain);
    chain.into()
}

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

/// Whether a site composes the app-wide guard pool into its chain.
///
/// Every site does, bar one — see [`compose`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum GlobalBucket {
    /// Fold the pool in, deduped against the narrower scopes.
    Fold,
    /// Leave it out: a site in front of this one already ran it.
    Skip,
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

    /// This site's chain for `container`, composing it on first sight. See
    /// [`compose`].
    ///
    /// `global` is a **function of the container**, not a value, so the memo's
    /// key covers everything the composition reads. A site whose bucket varied
    /// by anything else would otherwise get whichever chain was composed first,
    /// silently — and in the fail-open direction.
    pub(crate) fn chain(
        &self,
        container: &Container,
        route_label: &str,
        sources: &(dyn Fn() -> SiteChainSources + Sync),
        global: fn(&Container) -> GlobalBucket,
    ) -> Arc<[ResolvedLayer<dyn Guard>]> {
        let id = container.id();
        let primary = self.primary.get_or_init(|| Cached {
            container: id,
            chain: compose(container, route_label, sources(), global(container)),
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
        let chain = compose(container, route_label, sources(), global(container));
        slots.push(Cached {
            container: id,
            chain: Arc::clone(&chain),
        });
        chain
    }
}

/// Resolve, dedup and order this site's chain.
///
/// **The app-wide pool is part of it, on both transports.** This site runs the
/// pool's [`check_graphql`](Guard::check_graphql) / `check_mcp` against the
/// operation. What an `Exempt` *edge* runs is `check_http` against the request —
/// and only when no bridge is registered, since a registered one replaces the
/// fallback that folds the pool there. Different methods asking different
/// questions, so one is never a reason to skip the other; skipping this one is a
/// fail-open, because a guard written for an operation would then never be
/// consulted at the only site that could consult it.
///
/// **One site subtracts it, and only because another site already ran it.**
/// `#[entity]` operations are reached exclusively through `_entities`, in front
/// of which `nest_rs_graphql`'s federation gate runs the pool once per field —
/// so folding it here again would re-run every pooled guard once per
/// representation. That is [`GlobalBucket::Skip`]; every other site is
/// [`Fold`](GlobalBucket::Fold), and nothing else may subtract a bucket without
/// naming where it ran instead.
///
/// **Subtracting is done by composing and then dropping, never by composing
/// without.** The pool is what `compose_chain`'s `TypeId` dedup collapses a
/// narrower declaration *against* — broadest scope wins — so leaving it out
/// makes a guard declared both globally and on the resolver survive as the
/// resolver's own copy, and it then runs once per representation *on top of* the
/// gate's one run. That is the shape `demo`'s resolvers have. Composing first
/// and dropping the entries whose surviving `source` is the global bucket keeps
/// the dedup and removes exactly what already ran.
fn compose(
    container: &Container,
    route_label: &str,
    sources: SiteChainSources,
    bucket: GlobalBucket,
) -> Arc<[ResolvedLayer<dyn Guard>]> {
    let global = dedup_bucket(resolve_global_guards(container));
    let provider = resolve_specs(container, &sources.provider, LayerSite::Controller);
    let method = resolve_specs(container, &sources.method, LayerSite::Method);

    let mut chain =
        compose_chain::<dyn Guard>(global, provider, method, &sources.force, route_label);
    if bucket == GlobalBucket::Skip {
        chain.retain(|entry| entry.source != LayerSite::Global);
    }
    log_effective_chain(route_label, "guards", &chain);
    chain.into()
}

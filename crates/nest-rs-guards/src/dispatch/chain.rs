//! The per-site guard chain: what an in-band transport composes once and then
//! runs on every operation.
//!
//! GraphQL and MCP have no mount seam — neither the schema nor the MCP host can
//! see [`Guard`] — so each site memoizes its chain in a [`SiteChainCell`] the
//! decorator emits as a `static`: composed once per site, one atomic load per
//! operation.
//!
//! The cell is keyed by [`ContainerId`]: a test process serves several apps, and
//! one app's guard chain must never gate another's operations.

use std::any::TypeId;
use std::sync::{Arc, Mutex, OnceLock};

use nest_rs_core::layer_chain::{LayerSite, ResolvedLayer, compose_chain, dedup_bucket};
use nest_rs_core::{Container, ContainerId};

use crate::Guard;
use crate::dispatch::route_shaper::log_effective_chain;
use crate::dispatch::scoped_spec::{ScopedGuardSpec, resolve_global_guards, resolve_specs};

/// The scope-tagged guard declarations of one operation site, as the decorator
/// knows them. Read once per site, on the cache miss that composes the chain.
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
    /// `global` is a function of the container, so the memo's key covers
    /// everything the composition reads.
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
/// The app-wide pool is folded in on both transports: this site runs its
/// [`check_graphql`](Guard::check_graphql) / `check_mcp`, which an `Exempt`
/// edge's `check_http` never stands in for.
///
/// Only `#[entity]` operations skip it ([`GlobalBucket::Skip`]): the federation
/// gate in front of `_entities` already ran it. Skipping composes and then drops
/// the global entries, never composes without them — the pool is what
/// `compose_chain`'s dedup collapses a narrower declaration against.
fn compose(
    container: &Container,
    route_label: &str,
    sources: SiteChainSources,
    bucket: GlobalBucket,
) -> Arc<[ResolvedLayer<dyn Guard>]> {
    let global = dedup_bucket(resolve_global_guards(container));
    let provider = resolve_specs(container, &sources.provider, LayerSite::Host);
    let method = resolve_specs(container, &sources.method, LayerSite::Method);

    let mut chain =
        compose_chain::<dyn Guard>(global, provider, method, &sources.force, route_label);
    if bucket == GlobalBucket::Skip {
        chain.retain(|entry| entry.source != LayerSite::Global);
    }
    log_effective_chain(route_label, "guards", &chain);
    chain.into()
}

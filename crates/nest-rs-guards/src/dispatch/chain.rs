//! The per-site guard chain: what an in-band transport composes once and then
//! runs on every operation.
//!
//! GraphQL and MCP have no mount seam — neither the schema nor the MCP host can
//! see [`Guard`] — so each site memoizes its chain in a [`SiteChainCell`] the
//! decorator emits as a `static`: composed once per site, then one atomic load
//! and one `Weak` upgrade per operation.
//!
//! The cell is keyed by [`ContainerId`]: a test process serves several apps, and
//! one app's guard chain must never gate another's operations. The `static`
//! keeps only `Weak` handles; the strong chain lives in the container's
//! `SiteChains`, so the guards it holds go with their app.

use std::any::TypeId;
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};

use nest_rs_core::__private::{ResolvedLayer, compose_chain, dedup_bucket, site_chains};
use nest_rs_core::layer_chain::LayerSite;
use nest_rs_core::{Container, ContainerId, UnresolvedLayerError};

use crate::dispatch::route_shaper::log_effective_chain;
use crate::dispatch::scoped_spec::{
    ScopedGuardSpec, report_unresolved, resolve_global_guards, resolve_scoped,
};
use crate::{Denial, Guard};

/// The scope-tagged guard declarations of one operation site, as the decorator
/// knows them. Read once per site, on the cache miss that composes the chain.
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

/// One composed chain, as shared by every operation of its site.
type Chain = Arc<[ResolvedLayer<dyn Guard>]>;

/// One site's composed guard chain, memoized per [`ContainerId`].
///
/// A decorator emits one as a `static` per guarded operation and hands it to
/// its transport's runner.
#[derive(Default)]
pub struct SiteChainCell {
    /// The serving app's chain — the only entry a real process ever fills.
    primary: OnceLock<Cached>,
    /// Further apps sharing the process (integration tests build several).
    /// Allocated only when a second container reaches this site.
    extra: OnceLock<Mutex<Vec<Cached>>>,
}

/// A chain the cell can reach while its container lives, and never keeps alive.
struct Cached {
    container: ContainerId,
    chain: Weak<[ResolvedLayer<dyn Guard>]>,
}

impl Cached {
    /// The chain, when it is `id`'s and that container still holds it.
    fn live_for(&self, id: ContainerId) -> Option<Chain> {
        if self.container == id {
            self.chain.upgrade()
        } else {
            None
        }
    }
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
    /// everything the composition reads. A site whose chain does not compose
    /// is refused with an opaque [`Denial::internal`], its `error` filed, and
    /// nothing is memoized: the unit never runs without a layer it declares.
    pub(crate) fn chain(
        &self,
        container: &Container,
        route_label: &str,
        sources: &(dyn Fn() -> SiteChainSources + Sync),
        global: fn(&Container) -> GlobalBucket,
    ) -> Result<Chain, Denial> {
        if let Some(chain) = self.primary.get().and_then(|p| p.live_for(container.id())) {
            return Ok(chain);
        }
        self.compose_for(container, route_label, sources, global)
            .map_err(|unresolved| {
                report_unresolved(&unresolved);
                Denial::internal("a layer the operation declares did not resolve")
            })
    }

    /// The miss: another app in the process, or this site's first operation.
    /// An upgrade that fails here belongs to a container that is gone, which no
    /// unit of a live app reaches.
    #[cold]
    fn compose_for(
        &self,
        container: &Container,
        route_label: &str,
        sources: &(dyn Fn() -> SiteChainSources + Sync),
        global: fn(&Container) -> GlobalBucket,
    ) -> Result<Chain, UnresolvedLayerError> {
        let id = container.id();
        // A poisoned lock must not deny service — the vector holds only weak
        // handles, so recovering it is safe (worst case a chain composes twice).
        let mut extra = self
            .extra
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // Another operation of this site may have filled a slot meanwhile.
        if let Some(chain) = self
            .primary
            .get()
            .and_then(|p| p.live_for(id))
            .or_else(|| extra.iter().find_map(|cached| cached.live_for(id)))
        {
            return Ok(chain);
        }
        let chain = compose(container, route_label, sources(), global(container))?;
        site_chains(container).hold(Arc::clone(&chain));
        let cached = Cached {
            container: id,
            chain: Arc::downgrade(&chain),
        };
        if let Err(cached) = self.primary.set(cached) {
            extra.retain(|held| held.chain.strong_count() > 0);
            extra.push(cached);
        }
        Ok(chain)
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
) -> Result<Chain, UnresolvedLayerError> {
    let global = dedup_bucket(resolve_global_guards(container));
    let provider = resolve_scoped(container, &sources.provider, LayerSite::Host, route_label)?;
    let method = resolve_scoped(container, &sources.method, LayerSite::Method, route_label)?;

    let mut chain =
        compose_chain::<dyn Guard>(global, provider, method, &sources.force, route_label);
    if bucket == GlobalBucket::Skip {
        chain.retain(|entry| entry.source != LayerSite::Global);
    }
    log_effective_chain(route_label, "guards", &chain);
    Ok(chain.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declares_nothing() -> SiteChainSources {
        SiteChainSources {
            provider: Vec::new(),
            method: Vec::new(),
            force: Vec::new(),
        }
    }

    fn compose_on(cell: &SiteChainCell, container: &Container) -> Chain {
        cell.chain(container, "Query.orders", &declares_nothing, |_| {
            GlobalBucket::Fold
        })
        .unwrap_or_else(|denial| panic!("an empty chain composes: {denial:?}"))
    }

    #[test]
    fn ten_containers_composing_one_site_leave_no_dead_entry_after_the_eleventh() {
        let cell = SiteChainCell::new();
        for _ in 0..10 {
            let container = Container::builder().build();
            compose_on(&cell, &container);
        }
        let eleventh = Container::builder().build();
        let chain = compose_on(&cell, &eleventh);

        let primary = cell
            .primary
            .get()
            .unwrap_or_else(|| panic!("the first container took the primary slot"));
        assert_eq!(
            primary.chain.strong_count(),
            0,
            "the first container's chain went with it: the cell holds no strong handle",
        );
        let extra = cell
            .extra
            .get()
            .unwrap_or_else(|| panic!("later containers reached the site"))
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let live: Vec<ContainerId> = extra.iter().map(|cached| cached.container).collect();
        assert_eq!(
            live,
            [eleventh.id()],
            "the dead entries were pruned on insert"
        );
        assert!(extra[0].chain.strong_count() > 0);
        drop(extra);

        let again = compose_on(&cell, &eleventh);
        assert!(
            Arc::ptr_eq(&chain, &again),
            "a live container's site is composed once"
        );
    }
}

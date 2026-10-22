//! The per-site guard chain: what an in-band edge composes once per site, at
//! the wiring step, and then runs on every operation.

use std::any::TypeId;

use nest_rs_core::__private::{
    ResolvedLayer, compose_chain, dedup_bucket, log_effective_chain, resolve_scoped,
};
use nest_rs_core::layer_chain::LayerSite;
use nest_rs_core::{Container, UnresolvedLayerError};

use crate::Guard;
use crate::registry::{GuardSpec, resolve_global_guards};

/// The scope-tagged guard declarations of one operation site, as the decorator
/// knows them.
pub struct SiteChainSources {
    /// `#[use_guards(...)]` on the provider — the resolver struct, the MCP host.
    pub provider: Vec<GuardSpec>,
    /// `#[use_guards(...)]` beside the operation.
    pub method: Vec<GuardSpec>,
    /// `#[force_guards(...)]` — replay these even when a broader scope has them.
    pub force: Vec<TypeId>,
}

/// Whether a site composes the app-wide guard pool into its chain.
///
/// Every site does, bar the ones a site in front of them already ran it for —
/// see [`compose_site_chain`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GlobalBucket {
    /// Fold the pool in, deduped against the narrower scopes.
    Fold,
    /// Leave it out: a site in front of this one already ran it.
    Skip,
}

/// Resolve, dedup and order one site's chain — an in-band operation's or a
/// WebSocket message's.
///
/// The app-wide pool is folded in: this site runs each guard's
/// [`check`](Guard::check), which the endpoint request's `check_http` never
/// stands in for. Skipping it ([`GlobalBucket::Skip`]) composes and then drops
/// the global entries, never composes without them — the pool is what
/// `compose_chain`'s dedup collapses a narrower declaration against.
pub fn compose_site_chain(
    container: &Container,
    route_label: &str,
    sources: SiteChainSources,
    bucket: GlobalBucket,
) -> Result<Vec<ResolvedLayer<dyn Guard>>, UnresolvedLayerError> {
    let global = dedup_bucket(resolve_global_guards(container));
    let provider = resolve_scoped(container, &sources.provider, LayerSite::Host, route_label)?;
    let method = resolve_scoped(container, &sources.method, LayerSite::Method, route_label)?;

    let mut chain =
        compose_chain::<dyn Guard>(global, provider, method, &sources.force, route_label);
    if bucket == GlobalBucket::Skip {
        chain.retain(|entry| entry.source != LayerSite::Global);
    }
    log_effective_chain(route_label, "guards", &chain);
    Ok(chain)
}

/// Guards in the order they run.
type Guards = Vec<ResolvedLayer<dyn Guard>>;

/// Split the global pool at the first guard `covers` names, an endpoint's own
/// guards running every one of them: the guards listed before that one, then
/// the rest, every covered guard left to the endpoint's. Run head, the
/// endpoint's guards, tail, and they sit where the pool placed the
/// authentication they carry, ahead of a pooled guard reading the principal;
/// with nothing covered they run after the whole pool.
pub fn split_at_covered(pool: Guards, covers: &[TypeId]) -> (Guards, Guards) {
    let covered = |entry: &ResolvedLayer<dyn Guard>| covers.contains(&entry.type_id);
    let mut head = pool;
    let at = head.iter().position(covered).unwrap_or(head.len());
    let mut tail = head.split_off(at);
    tail.retain(|entry| !covered(entry));
    (head, tail)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nest_rs_core::Layer;

    use super::*;

    struct First;
    struct Covered;
    struct Middle;
    struct AlsoCovered;
    struct Last;

    macro_rules! admit_all {
        ($($guard:ident),*) => {$(
            impl Layer for $guard {}
            impl Guard for $guard {}
        )*};
    }

    admit_all!(First, Covered, Middle, AlsoCovered, Last);

    fn entry<G: Guard>(guard: G) -> ResolvedLayer<dyn Guard> {
        ResolvedLayer {
            type_id: TypeId::of::<G>(),
            name: std::any::type_name::<G>(),
            source: LayerSite::Global,
            layer: Arc::new(guard),
        }
    }

    fn pool() -> Vec<ResolvedLayer<dyn Guard>> {
        vec![
            entry(First),
            entry(Covered),
            entry(Middle),
            entry(AlsoCovered),
            entry(Last),
        ]
    }

    fn ids(chain: &[ResolvedLayer<dyn Guard>]) -> Vec<TypeId> {
        chain.iter().map(|entry| entry.type_id).collect()
    }

    #[test]
    fn endpoint_guards_sit_at_the_first_guard_they_name_and_run_every_named_one() {
        let covers = [TypeId::of::<Covered>(), TypeId::of::<AlsoCovered>()];
        let (head, tail) = split_at_covered(pool(), &covers);
        assert_eq!(ids(&head), [TypeId::of::<First>()]);
        assert_eq!(ids(&tail), [TypeId::of::<Middle>(), TypeId::of::<Last>()]);
    }

    #[test]
    fn endpoint_guards_naming_nothing_in_the_pool_run_after_it() {
        let (head, tail) = split_at_covered(pool(), &[TypeId::of::<u8>()]);
        assert_eq!(ids(&head).len(), 5);
        assert!(ids(&tail).is_empty());
    }

    #[test]
    fn endpoint_guards_naming_the_whole_pool_run_alone() {
        let covers = [
            TypeId::of::<First>(),
            TypeId::of::<Covered>(),
            TypeId::of::<Middle>(),
            TypeId::of::<AlsoCovered>(),
            TypeId::of::<Last>(),
        ];
        let (head, tail) = split_at_covered(pool(), &covers);
        assert!(ids(&head).is_empty());
        assert!(ids(&tail).is_empty());
    }
}

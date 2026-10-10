//! Layer chain composition — the dedup-by-`TypeId` logic shared by every
//! execution site of the Layer System.
//!
//! Each resolved entry is tagged with its [`LayerSite`]; the chain keeps the
//! broadest site for any duplicated [`TypeId`] and runs entries in declaration
//! order, with [`Layer::priority`](crate::Layer::priority) as the tiebreaker
//! within a site.

use std::any::{Any, TypeId};
use std::sync::Arc;

use crate::container::Container;
pub use crate::layer::LayerSite;

/// A global-layer registration: the `TypeId` a chain dedups on, the type's
/// name for diagnostics, and a `resolve` fn that recovers the `Arc<L>` from the
/// live container.
pub struct LayerSpec<L: ?Sized> {
    /// `TypeId` of the layer type — the dedup key across scopes.
    pub type_id: TypeId,
    /// The layer type's name, for boot logs and fail-secure diagnostics.
    pub name: &'static str,
    resolve: fn(&Container) -> Option<Arc<L>>,
}

impl<L: ?Sized> LayerSpec<L> {
    /// Build a spec from its `type_id`, `name` and `resolve` fn.
    pub fn new(
        type_id: TypeId,
        name: &'static str,
        resolve: fn(&Container) -> Option<Arc<L>>,
    ) -> Self {
        Self {
            type_id,
            name,
            resolve,
        }
    }

    /// Resolve the layer instance from the live container, or `None` if its
    /// provider was never registered (a fail-secure boot check flags this).
    pub fn resolve(&self, container: &Container) -> Option<Arc<L>> {
        (self.resolve)(container)
    }
}

/// The container-registered global registry of one Layer-System family
/// (`GuardSpecs`, `FilterSpecs`, …), read when a chain is composed.
pub trait GlobalSpecs: Any + Send + Sync {
    /// The erased layer trait this family's specs resolve to.
    type Layer: ?Sized;

    /// The registered specs, in declaration order.
    fn specs(&self) -> &[LayerSpec<Self::Layer>];
}

/// `layer_chain` is public: its tier-2 items live here, reached only
/// through the crate's `__private`.
pub(crate) mod __private {
    use std::any::TypeId;
    use std::sync::Arc;

    use super::{GlobalSpecs, LayerSpec};
    use crate::container::Container;
    use crate::layer::{Layer, LayerSite};

    /// Resolve a family's global registry into [`LayerSite::Global`] entries.
    ///
    /// A spec whose provider is not registered is skipped —
    /// [`check_specs_resolvable`] fails the boot on it; an unregistered family
    /// yields an empty chain.
    pub fn resolve_global_layers<S: GlobalSpecs>(
        container: &Container,
    ) -> Vec<ResolvedLayer<S::Layer>> {
        let Some(registry) = container.get::<S>() else {
            return Vec::new();
        };
        registry
            .specs()
            .iter()
            .filter_map(|spec| {
                spec.resolve(container).map(|layer| ResolvedLayer {
                    type_id: spec.type_id,
                    name: spec.name,
                    source: LayerSite::Global,
                    layer,
                })
            })
            .collect()
    }

    /// Fail-secure boot check: name the specs whose provider is not resolvable
    /// from `container`. `kind` is the family noun (`"guard"`, `"filter"`, …) and
    /// `consequence` the tail saying what a silent drop would cost.
    pub fn check_specs_resolvable<L: ?Sized>(
        specs: &[LayerSpec<L>],
        container: &Container,
        kind: &str,
        consequence: &str,
    ) -> Result<(), String> {
        let missing: Vec<&str> = specs
            .iter()
            .filter(|s| s.resolve(container).is_none())
            .map(|s| s.name)
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "global {kind}(s) not resolvable from the container: {} — import the \
                 module that provides them; {consequence}",
                missing.join(", "),
            ))
        }
    }

    /// A layer that survived dedup, paired with its origin site and its name.
    pub struct ResolvedLayer<L: ?Sized> {
        /// The layer type's identity — the key dedup collapsed duplicates on.
        pub type_id: TypeId,
        /// The layer type's name, as logged when the shaper mounts it.
        pub name: &'static str,
        /// The site the surviving instance came from (global, host, method).
        pub source: LayerSite,
        /// The resolved layer instance to run.
        pub layer: Arc<L>,
    }

    impl<L: ?Sized> Clone for ResolvedLayer<L> {
        fn clone(&self) -> Self {
            Self {
                type_id: self.type_id,
                name: self.name,
                source: self.source,
                layer: Arc::clone(&self.layer),
            }
        }
    }

    /// Compose a deduplicated chain from global + per-route entries.
    ///
    /// 1. Dedup by `TypeId` — the broadest site wins.
    /// 2. A `TypeId` listed in `force` survives even when declared more broadly.
    /// 3. Stable sort by [`Layer::priority`]; declaration order breaks ties.
    ///
    /// `chain` names the dispatch site (a route, a WS message, `transport`), not
    /// only a route.
    pub fn compose_chain<L>(
        global: Vec<ResolvedLayer<L>>,
        host: Vec<ResolvedLayer<L>>,
        method: Vec<ResolvedLayer<L>>,
        force: &[TypeId],
        chain: &str,
    ) -> Vec<ResolvedLayer<L>>
    where
        L: Layer + ?Sized,
    {
        let mut entries: Vec<ResolvedLayer<L>> = Vec::new();
        let mut seen: Vec<(TypeId, LayerSite)> = Vec::new();

        for source in [LayerSite::Global, LayerSite::Host, LayerSite::Method] {
            let bucket = match source {
                LayerSite::Global => &global,
                LayerSite::Host => &host,
                _ => &method,
            };
            for entry in bucket {
                let forced = force.contains(&entry.type_id);
                if let Some((_, existing)) = seen.iter().find(|(tid, _)| *tid == entry.type_id) {
                    if !forced {
                        report_redundant_site(entry.type_id, *existing, entry.source, entry.name);
                        continue;
                    }
                    tracing::info!(
                        target: crate::target::LAYERS,
                        // Shortened as in `report_redundant_site`, so one grep finds both lines.
                        layer = crate::type_name::shorten(entry.name),
                        site = entry.source.label(),
                        chain,
                        "layer forced to re-run despite being declared at a broader site",
                    );
                }
                seen.push((entry.type_id, entry.source));
                entries.push(entry.clone());
            }
        }

        entries.sort_by_key(|e| e.layer.priority());

        entries
    }

    /// Report a redundant multi-site declaration once per process, at `debug`:
    /// `compose_chain` runs per route, and the layer still runs exactly once.
    fn report_redundant_site(
        type_id: TypeId,
        existing: LayerSite,
        skipped: LayerSite,
        name: &'static str,
    ) {
        use std::collections::HashSet;
        use std::sync::{LazyLock, Mutex};

        static SEEN: LazyLock<Mutex<HashSet<(TypeId, LayerSite, LayerSite)>>> =
            LazyLock::new(|| Mutex::new(HashSet::new()));

        // On a poisoned lock, fall back to emitting — a duplicate diagnostic line
        // is harmless; a swallowed one is not.
        let first_time = SEEN
            .lock()
            .map(|mut seen| seen.insert((type_id, existing, skipped)))
            .unwrap_or(true);
        if first_time {
            tracing::debug!(
                target: crate::target::LAYERS,
                layer = crate::type_name::shorten(name),
                kept = existing.label(),
                skipped = skipped.label(),
                hint = "broadest site wins; use `#[force_*]` to re-run",
                "redundant layer declaration deduped",
            );
        }
    }

    /// Drop intra-bucket duplicates by `TypeId`, keeping the first declaration,
    /// silently — the site executing the global sub-chain already reported them.
    pub fn dedup_bucket<L: ?Sized>(bucket: Vec<ResolvedLayer<L>>) -> Vec<ResolvedLayer<L>> {
        let mut seen: Vec<TypeId> = Vec::new();
        bucket
            .into_iter()
            .filter(|entry| {
                if seen.contains(&entry.type_id) {
                    return false;
                }
                seen.push(entry.type_id);
                true
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::__private::{ResolvedLayer, compose_chain, dedup_bucket};
    use super::*;
    use crate::layer::Layer;

    struct Authn;
    impl Layer for Authn {}
    struct Authz;
    impl Layer for Authz {}
    struct Audit;
    impl Layer for Audit {}

    fn entry<L: Layer + 'static>(layer: L, source: LayerSite) -> ResolvedLayer<dyn Layer> {
        ResolvedLayer {
            type_id: TypeId::of::<L>(),
            name: std::any::type_name::<L>(),
            source,
            layer: Arc::new(layer) as Arc<dyn Layer>,
        }
    }

    #[test]
    fn dedup_keeps_global_drops_method_for_same_typeid() {
        let chain = compose_chain::<dyn Layer>(
            vec![entry(Authn, LayerSite::Global)],
            vec![],
            vec![entry(Authn, LayerSite::Method)],
            &[],
            "GET /test",
        );
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].source, LayerSite::Global);
    }

    #[test]
    fn declaration_order_survives_when_priorities_tie() {
        let chain = compose_chain::<dyn Layer>(
            vec![
                entry(Authn, LayerSite::Global),
                entry(Authz, LayerSite::Global),
                entry(Audit, LayerSite::Global),
            ],
            vec![],
            vec![],
            &[],
            "x",
        );
        let names: Vec<_> = chain.iter().map(|e| e.name).collect();
        assert_eq!(
            names,
            vec![
                std::any::type_name::<Authn>(),
                std::any::type_name::<Authz>(),
                std::any::type_name::<Audit>(),
            ],
        );
    }

    #[test]
    fn force_replays_layer_despite_global_declaration() {
        let force = vec![TypeId::of::<Authn>()];
        let chain = compose_chain::<dyn Layer>(
            vec![entry(Authn, LayerSite::Global)],
            vec![],
            vec![entry(Authn, LayerSite::Method)],
            &force,
            "x",
        );
        assert_eq!(chain.len(), 2);
    }

    #[test]
    fn dedup_bucket_keeps_first_declaration_silently() {
        let bucket = dedup_bucket::<dyn Layer>(vec![
            entry(Authn, LayerSite::Global),
            entry(Authz, LayerSite::Global),
            entry(Authn, LayerSite::Global),
        ]);
        assert_eq!(bucket.len(), 2);
        assert_eq!(bucket[0].name, std::any::type_name::<Authn>());
        assert_eq!(bucket[1].name, std::any::type_name::<Authz>());
    }
}

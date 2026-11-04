//! Layer chain composition — the dedup-by-`TypeId` logic shared by every
//! execution site of the Layer System.
//!
//! Each resolved entry is tagged with its [`LayerSite`](crate::LayerSite); the
//! chain keeps the broadest site for any duplicated [`TypeId`] and runs the sites
//! from the broadest in, each ordered by
//! [`Layer::priority`](crate::Layer::priority), declaration order breaking a tie.

use std::any::TypeId;
use std::sync::Arc;

use crate::container::Container;

/// One layer a site or the app-wide pool declares: the `TypeId` a chain dedups
/// on, the type's name for diagnostics, and a `resolve` fn that recovers the
/// `Arc<L>` from the live container.
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

/// Implement one family's app-wide pool declaration: its sealed list trait on
/// every tuple of one to eight layers, each spec built by `$spec`, and the
/// declaration's `Discoverable`, whose `register` attaches the pool — and the
/// layers the access graph holds the declaring module to, as it holds a site's
/// host to the layers the site binds — so the four families cannot drift
/// apart.
#[doc(hidden)]
#[macro_export]
macro_rules! __global_layer_pool {
    (
        pool: $pool:ident,
        list: $list:ident,
        sealed: $sealed:path,
        layer: $layer:ty,
        each: $bound:path => $spec:ident $(,)?
    ) => {
        $crate::__global_layer_pool!(
            @lists $list, $sealed, $layer, $bound, $spec; A, B, C, D, E, F, G, H
        );

        impl<L: $list> $crate::Discoverable for $pool<L> {
            fn register(builder: $crate::ContainerBuilder) -> $crate::ContainerBuilder {
                $crate::__private::declare_global::<$layer, Self>(builder, L::__specs())
            }
        }
    };
    (@lists $list:ident, $sealed:path, $layer:ty, $bound:path, $spec:ident;) => {};
    (
        @lists $list:ident, $sealed:path, $layer:ty, $bound:path, $spec:ident;
        $head:ident $(, $tail:ident)*
    ) => {
        impl<$head: $bound $(, $tail: $bound)*> $sealed for ($head, $($tail,)*) {}

        impl<$head: $bound $(, $tail: $bound)*> $list for ($head, $($tail,)*) {
            fn __specs() -> ::std::vec::Vec<$crate::LayerSpec<$layer>> {
                ::std::vec![$spec::<$head>() $(, $spec::<$tail>())*]
            }
        }

        $crate::__global_layer_pool!(@lists $list, $sealed, $layer, $bound, $spec; $($tail),*);
    };
}

/// `layer_chain` is private: its tier-2 items live here, reached only
/// through the crate's `__private`.
pub(crate) mod __private {
    use std::any::{Any, TypeId};
    use std::sync::{Arc, Mutex, PoisonError};

    use super::LayerSpec;
    use crate::access::{Composition, ModuleDescriptor};
    use crate::container::{Container, ContainerBuilder};
    use crate::discovery::Discovery;
    use crate::error::UnresolvedLayerError;
    use crate::layer::{Layer, LayerSite};

    /// What one layer family says of the app-wide pool its root module
    /// declares — implemented on the family's trait object (`dyn Guard`,
    /// `dyn GlobalPipe`, `dyn Interceptor`, the erased exception filter).
    pub trait GlobalLayer: 'static {
        /// The family's layer as a boot error names it: `"guard"`.
        const KIND: &'static str;
        /// The pool as the wiring step names it, by its declaration's type:
        /// `"GlobalGuards"`.
        const POOL: &'static str;

        /// The family's own check of the pool the app declared, once the
        /// container is sealed: a guard pool's phase order.
        fn check(container: &Container) -> Result<(), String> {
            let _ = container;
            Ok(())
        }
    }

    /// One app-wide pool, as the declaration listing it attached it.
    pub(crate) struct GlobalPool<L: ?Sized> {
        declared_by: &'static str,
        /// The module whose providers registered the declaration.
        listed_by: Option<&'static str>,
        specs: Vec<LayerSpec<L>>,
    }

    /// The layers one declaration lists, which the access graph reads as the
    /// layers a site binds: reached by the declaring module, and singletons,
    /// since the pool resolves them once, at mount.
    pub(crate) struct PooledLayers {
        pub(crate) declaration: TypeId,
        pub(crate) layers: Vec<(TypeId, &'static str)>,
    }

    /// The kernel's check of one family's pool, run at the wiring step before
    /// every module's wiring.
    pub(crate) struct GlobalPoolCheck {
        pub(crate) pool: &'static str,
        pub(crate) check: fn(&Container) -> anyhow::Result<()>,
    }

    /// Attach the app-wide pool of `L` that the declaration `D` lists — what a
    /// `Global*` provider's `register` does — and its check at the wiring step.
    pub fn declare_global<L: GlobalLayer + ?Sized, D: 'static>(
        builder: ContainerBuilder,
        specs: Vec<LayerSpec<L>>,
    ) -> ContainerBuilder {
        let pooled = PooledLayers {
            declaration: TypeId::of::<D>(),
            layers: specs.iter().map(|spec| (spec.type_id, spec.name)).collect(),
        };
        let listed_by = builder.registering_module();
        builder
            .attach_meta::<D, GlobalPool<L>>(GlobalPool {
                declared_by: std::any::type_name::<D>(),
                listed_by,
                specs,
            })
            .provide_meta(pooled)
            .provide_meta(GlobalPoolCheck {
                pool: L::POOL,
                check: check_pool::<L>,
            })
    }

    /// The app-wide pool of `L` resolved into [`LayerSite::Global`] entries, in
    /// declaration order; empty when the app declares none.
    ///
    /// The access graph refuses a declaration whose module does not reach each
    /// of its layers' providers, or listing one that is not a singleton, so
    /// every spec resolves on a booted container.
    pub fn resolve_global_layers<L: GlobalLayer + ?Sized>(
        container: &Container,
    ) -> Vec<ResolvedLayer<L>> {
        Discovery::new(container)
            .meta::<GlobalPool<L>>()
            .iter()
            .flat_map(|found| found.meta.specs.iter())
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

    /// Whether the app declares an app-wide pool of `L` listing any layer.
    pub fn global_pool_declared<L: GlobalLayer + ?Sized>(container: &Container) -> bool {
        Discovery::new(container)
            .meta::<GlobalPool<L>>()
            .iter()
            .any(|found| !found.meta.specs.is_empty())
    }

    /// The wiring step's check of `L`'s pool: each declaration listed by a
    /// module the app boots, then declared once, then the family's own check.
    fn check_pool<L: GlobalLayer + ?Sized>(container: &Container) -> anyhow::Result<()> {
        let declared = Discovery::new(container).meta::<GlobalPool<L>>();
        if let Some(composition) = container.get::<Composition>() {
            for found in &declared {
                let Some(provider) = found.provider_type_id else {
                    continue;
                };
                let declaration = crate::type_name::shorten(found.meta.declared_by);
                match listing(&composition, provider) {
                    Listing::ByARoot => {}
                    Listing::BelowTheRoot(module) => anyhow::bail!(
                        "`{declaration}` is listed by `{module}`, a module the app imports: the \
                         app-wide {kind} pool is declared by the module the app boots — list it \
                         in {booted}'s providers",
                        module = crate::type_name::shorten(module),
                        kind = L::KIND,
                        booted = booted(&composition),
                    ),
                    Listing::Unlisted => anyhow::bail!(
                        "`{declaration}` is registered by no module's providers: the app-wide \
                         {kind} pool is declared by the module the app boots — list it in \
                         {booted}'s providers",
                        kind = L::KIND,
                        booted = booted(&composition),
                    ),
                }
            }
        }
        if let [first, second, ..] = declared.as_slice() {
            anyhow::bail!(
                "the app-wide {kind} pool is declared twice, by `{first}` in {first_module} and \
                 `{second}` in {second_module}: an app declares it once, in the module it boots",
                kind = L::KIND,
                first = crate::type_name::shorten(first.meta.declared_by),
                first_module = listed_by(&first.meta),
                second = crate::type_name::shorten(second.meta.declared_by),
                second_module = listed_by(&second.meta),
            );
        }
        L::check(container).map_err(anyhow::Error::msg)
    }

    /// The module the app boots, as a remedy names it: `` `ApiModule` ``, or
    /// each of several roots.
    fn booted(composition: &Composition) -> String {
        let roots: Vec<String> = inventory::iter::<ModuleDescriptor>()
            .filter(|descriptor| composition.is_root((descriptor.module)()))
            .map(|descriptor| format!("`{}`", crate::type_name::shorten(descriptor.name)))
            .collect();
        match roots.as_slice() {
            [] => "that module".to_owned(),
            [root] => root.clone(),
            [init @ .., last] => format!("{} or {last}", init.join(", ")),
        }
    }

    /// The module that registered `pool`, as a refusal names it.
    fn listed_by<L: ?Sized>(pool: &GlobalPool<L>) -> String {
        pool.listed_by.map_or_else(
            || "no module".to_owned(),
            |module| format!("`{}`", crate::type_name::shorten(module)),
        )
    }

    /// Which modules of the app list a provider.
    enum Listing {
        /// A module the app boots, and no other.
        ByARoot,
        /// This module, which the app imports without booting it.
        BelowTheRoot(&'static str),
        /// No module the app reaches: a `register` written by hand installed it.
        Unlisted,
    }

    /// Which modules the app reaches list the provider `declaration`.
    fn listing(composition: &Composition, declaration: TypeId) -> Listing {
        let mut by_a_root = false;
        for descriptor in inventory::iter::<ModuleDescriptor>() {
            let module = (descriptor.module)();
            let lists = descriptor
                .providers
                .iter()
                .any(|provider| (provider.provider)() == declaration);
            if !lists || !composition.reaches(module) {
                continue;
            }
            if !composition.is_root(module) {
                return Listing::BelowTheRoot(descriptor.name);
            }
            by_a_root = true;
        }
        if by_a_root {
            Listing::ByARoot
        } else {
            Listing::Unlisted
        }
    }

    /// The chains a container's sites composed, held for as long as the
    /// container lives: a per-site cache keeps only a `Weak` handle to what this
    /// holds, so no `static` keeps a provider alive past its app.
    #[derive(Default)]
    pub struct SiteChains {
        held: Mutex<Vec<Box<dyn Any + Send + Sync>>>,
    }

    impl SiteChains {
        /// Hold `chain` until the container that owns this drops.
        pub fn hold(&self, chain: impl Any + Send + Sync) {
            self.held
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Box::new(chain));
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
    /// 3. Each site's entries are sorted stably by [`Layer::priority`], so
    ///    declaration order breaks ties; the sites then run Global, Host,
    ///    Method — a priority orders within its site, never across.
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
            let start = entries.len();
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
            // Stable, so declaration order breaks a tie.
            entries[start..].sort_by_key(|e| e.layer.priority());
        }

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
                hint = "broadest site wins; `#[force_guards]` re-runs a guard",
                "redundant layer declaration deduped",
            );
        }
    }

    /// Resolve one site's scoped declarations into entries from `source`, so
    /// dedup against the global chain finds the same key. The first spec no
    /// imported module provides refuses the site, named as its boot log names
    /// it: a site never runs without a layer it declares.
    pub fn resolve_scoped<'s, L: ?Sized + 's>(
        container: &Container,
        specs: impl IntoIterator<Item = &'s LayerSpec<L>>,
        source: LayerSite,
        site: &str,
    ) -> Result<Vec<ResolvedLayer<L>>, UnresolvedLayerError> {
        specs
            .into_iter()
            .map(|spec| {
                let layer = spec
                    .resolve(container)
                    .ok_or_else(|| UnresolvedLayerError {
                        layer: spec.name,
                        site: site.to_owned(),
                    })?;
                Ok(ResolvedLayer {
                    type_id: spec.type_id,
                    name: spec.name,
                    source,
                    layer,
                })
            })
            .collect()
    }

    /// The one `error` a site files when a layer it declares does not resolve —
    /// for the operator; the client reads an opaque refusal.
    pub fn report_unresolved(unresolved: &UnresolvedLayerError) {
        tracing::error!(
            target: crate::target::LAYERS,
            layer = unresolved.layer,
            site = unresolved.site.as_str(),
            "a layer the site declares is provided by no imported module",
        );
    }

    /// Refuse a site composed at mount whose declared layer does not resolve:
    /// its `error`, and the refusal filed on `container` for the boot to fail on.
    pub fn refuse_unresolved(container: &Container, unresolved: UnresolvedLayerError) {
        report_unresolved(&unresolved);
        crate::container::__private::refuse_site(container, unresolved.into());
    }

    /// Trace the chain one site composed for a family (`kind`), in the order it
    /// runs; nothing for an empty one.
    pub fn log_effective_chain<L: Layer + ?Sized>(
        site: &str,
        kind: &str,
        chain: &[ResolvedLayer<L>],
    ) {
        if chain.is_empty() {
            return;
        }
        let entries: Vec<String> = chain
            .iter()
            .map(|e| format!("{} ({})", e.name, e.source.label()))
            .collect();
        tracing::trace!(
            target: crate::target::LAYERS,
            route = site,
            kind,
            chain = entries.join(", ").as_str(),
            "effective layer chain",
        );
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
    use crate::layer::{Layer, LayerSite};

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

    /// A layer whose priority the test chooses, one type per entry.
    struct Ranked<const N: u8>(i8);
    impl<const N: u8> Layer for Ranked<N> {
        fn priority(&self) -> i8 {
            self.0
        }
    }

    #[test]
    fn a_method_priority_never_jumps_the_global_bucket() {
        let chain = compose_chain::<dyn Layer>(
            vec![
                entry(Ranked::<1>(0), LayerSite::Global),
                entry(Ranked::<2>(5), LayerSite::Global),
            ],
            vec![entry(Ranked::<3>(0), LayerSite::Host)],
            vec![entry(Ranked::<4>(-10), LayerSite::Method)],
            &[],
            "GET /orders",
        );
        let sites: Vec<LayerSite> = chain.iter().map(|e| e.source).collect();
        assert_eq!(
            sites,
            [
                LayerSite::Global,
                LayerSite::Global,
                LayerSite::Host,
                LayerSite::Method
            ],
            "a priority orders within its site, never ahead of a broader one",
        );
    }

    #[test]
    fn priority_orders_inside_one_bucket() {
        let chain = compose_chain::<dyn Layer>(
            vec![],
            vec![],
            vec![
                entry(Ranked::<1>(3), LayerSite::Method),
                entry(Ranked::<2>(-1), LayerSite::Method),
                entry(Ranked::<3>(3), LayerSite::Method),
                entry(Ranked::<4>(0), LayerSite::Method),
            ],
            &[],
            "GET /orders",
        );
        let names: Vec<_> = chain.iter().map(|e| e.name).collect();
        assert_eq!(
            names,
            [
                std::any::type_name::<Ranked<2>>(),
                std::any::type_name::<Ranked<4>>(),
                std::any::type_name::<Ranked<1>>(),
                std::any::type_name::<Ranked<3>>(),
            ],
            "lower runs first; declaration order breaks a tie",
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

    /// Declared twice by the one test below alone: the line is filed once per
    /// process for each layer type.
    struct Redeclared;
    impl Layer for Redeclared {}

    #[test]
    fn a_redundant_declaration_names_the_one_decorator_that_re_runs_a_layer() {
        let logs = nest_rs_testing::LogCapture::install();

        let chain = compose_chain::<dyn Layer>(
            vec![entry(Redeclared, LayerSite::Global)],
            vec![entry(Redeclared, LayerSite::Host)],
            vec![],
            &[],
            "GET /posts",
        );

        assert_eq!(chain.len(), 1, "the broadest site's declaration runs alone");
        let deduped: Vec<_> = logs
            .events()
            .into_iter()
            .filter(|event| event.message == "redundant layer declaration deduped")
            .collect();
        assert_eq!(deduped.len(), 1, "one line: {deduped:#?}");
        assert_eq!(deduped[0].target, crate::target::LAYERS);
        assert_eq!(
            deduped[0].field("hint").as_deref(),
            Some("broadest site wins; `#[force_guards]` re-runs a guard"),
        );
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

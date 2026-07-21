//! The IoC [`Container`] and its [`ContainerBuilder`] — the single flat
//! registry keyed by [`ProviderKey`] that every provider resolves through, plus
//! the request-scoped and transient factory machinery layered on top of it.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;

use crate::RequestScope;
use crate::cycle_guard::{BuildStack, Cycle, CycleGuard};
use crate::module::DynamicModule;

type AnyArc = Arc<dyn Any + Send + Sync>;

/// The identity a singleton provider registers under: a `TypeId` optionally
/// disambiguated by a `name`. `name: None` is the default, bare registration —
/// every non-keyed `provide*`/`get*` path keys with `None`, so existing code
/// behaves exactly as before. `name: Some(_)` is a **keyed** provider
/// ([`ContainerBuilder::provide_keyed`]), letting several instances of the same
/// concrete type coexist in the flat container (two `OAuth2Client`s, one per
/// upstream provider).
///
/// Keying is singleton-only: request-scoped and transient factories, and the
/// metadata index, stay bare `TypeId`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ProviderKey {
    /// The provided type's identity.
    pub type_id: TypeId,
    /// The key when this is a keyed provider; `None` for the bare, default
    /// identity — two entries of the same type differ only by this.
    pub name: Option<&'static str>,
}

impl ProviderKey {
    /// The bare (unkeyed) identity for a concrete type — the default path.
    pub fn typed<T: Any>() -> Self {
        Self {
            type_id: TypeId::of::<T>(),
            name: None,
        }
    }

    /// A keyed identity for a concrete type.
    pub fn named<T: Any>(name: &'static str) -> Self {
        Self {
            type_id: TypeId::of::<T>(),
            name: Some(name),
        }
    }

    /// The bare identity for a raw `TypeId` — used internally where the type is
    /// only available as a `TypeId` (trait-object bindings keyed by
    /// `TypeId::of::<Arc<dyn Trait>>()`, the register-phase `contains` check).
    pub fn of(type_id: TypeId) -> Self {
        Self {
            type_id,
            name: None,
        }
    }
}

/// A keyed `#[inject(key = "…")]` dependency reported by
/// [`Discoverable::injected_keyed`](crate::Discoverable::injected_keyed): the
/// container [`ProviderKey`] used to match it against the global keyed set,
/// plus the injected type's name so a boot failure can name both the type and
/// the key.
///
/// **Internal ABI** — constructed by `#[injectable]`'s codegen, lockstep with
/// `nest-rs-core`; do not hand-construct.
#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct KeyedDependency {
    /// The keyed container identity this dependency must match.
    pub key: ProviderKey,
    /// The injected type's name, so a boot failure can name it alongside the key.
    pub type_name: &'static str,
}

/// Builds a fresh instance of a request-scoped provider, invoked once per
/// request by a [`RequestScope`]. The factory receives the
/// scope (not the bare root [`Container`]) so a request-scoped provider may
/// depend on **another** request-scoped provider and share its per-request
/// instance; singleton deps still resolve because the scope forwards them to
/// the root.
pub(crate) type ScopedFactory = Arc<dyn Fn(&RequestScope) -> AnyArc + Send + Sync>;

/// Builds a fresh instance of a transient provider on every resolution.
/// Emitted by `#[injectable(scope = transient)]`. Like [`ScopedFactory`] it
/// receives the [`RequestScope`] (not the bare root [`Container`]) so a
/// transient's `#[inject]` deps resolve through the request boundary: a
/// request-scoped dep resolves to the request's shared instance, singletons
/// forward to the root, and another transient rebuilds. Resolved outside a
/// request (`Container::get`), the scope is a throwaway wrapping the root.
pub(crate) type TransientFactory = Arc<dyn Fn(&RequestScope) -> AnyArc + Send + Sync>;

thread_local! {
    /// Re-entrancy guard for transient resolution: a transient provider that
    /// (transitively) injects itself would loop forever. The shared
    /// [`CycleGuard`] catches the cycle on the second entry for the same
    /// `TypeId` and renders the chain for a clear panic. Kept a transient-only
    /// stack so a legitimate concurrent resolution of the same transient on
    /// another thread is never mistaken for a cycle.
    static TRANSIENT_BUILDING: BuildStack = const { RefCell::new(Vec::new()) };
}

/// A registration applied once a factory has produced its value, so factory
/// outputs flow through the same path — and the same duplicate detection — as
/// any other provider.
pub(crate) type Registrar = Box<dyn FnOnce(ContainerBuilder) -> ContainerBuilder + Send>;
type FactoryFuture = Pin<Box<dyn Future<Output = Result<Registrar>> + Send>>;
pub(crate) type BoxedFactory = Box<dyn FnOnce(Container) -> FactoryFuture + Send>;

#[derive(Clone)]
pub(crate) struct MetaEntry {
    pub(crate) provider_type_id: Option<TypeId>,
    pub(crate) meta: AnyArc,
}

/// The single flat provider registry. Cheap to clone — every field is an
/// [`Arc`], so a clone shares the same immutable maps. Built once by
/// [`ContainerBuilder::build`] and thereafter read-only.
#[derive(Clone, Default)]
pub struct Container {
    providers: Arc<HashMap<ProviderKey, AnyArc>>,
    metadata: Arc<HashMap<TypeId, Vec<MetaEntry>>>,
    scoped: Arc<HashMap<TypeId, ScopedFactory>>,
    transient: Arc<HashMap<TypeId, TransientFactory>>,
}

impl Container {
    /// Start an empty [`ContainerBuilder`] to register providers into.
    pub fn builder() -> ContainerBuilder {
        ContainerBuilder::default()
    }

    /// Resolve a provider by type. Returns `None` if no provider was registered.
    ///
    /// This is the `ModuleRef.get()` analog and bypasses the build-time access
    /// contract (see [`crate::access`]) — prefer declarative `#[inject]`.
    ///
    /// A transient provider (`#[injectable(scope = transient)]`) is rebuilt on
    /// every call. A transient that (transitively) depends on itself panics
    /// with a clear cycle diagnostic — break it with `Arc<dyn Trait>` or
    /// pick a different scope.
    pub fn get<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        let id = TypeId::of::<T>();
        if let Some(factory) = self.transient.get(&id) {
            // No live request here (the escape-hatch path): wrap the root in a
            // throwaway scope so the transient's factory can still resolve a
            // request-scoped `#[inject]` dep (built fresh, request-of-one)
            // instead of panicking on a missing provider.
            let scope = RequestScope::new(self.clone());
            let any = build_transient(id, std::any::type_name::<T>(), factory, &scope);
            return any.downcast::<T>().ok();
        }
        self.providers
            .get(&ProviderKey::of(id))
            .and_then(|any| any.clone().downcast::<T>().ok())
    }

    /// Resolve a **keyed** singleton registered via
    /// [`ContainerBuilder::provide_keyed`]. `None` if no provider was registered
    /// under `(T, name)`. Keyed providers are singletons only — there is no
    /// keyed transient or request-scoped resolution.
    pub fn get_keyed<T: Any + Send + Sync>(&self, name: &'static str) -> Option<Arc<T>> {
        self.providers
            .get(&ProviderKey::named::<T>(name))
            .and_then(|any| any.clone().downcast::<T>().ok())
    }

    /// Resolve a trait-object provider registered via
    /// [`ContainerBuilder::provide_dyn`]. Same unchecked-escape-hatch caveat as
    /// [`get`](Self::get).
    pub fn get_dyn<T: ?Sized + Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.providers
            .get(&ProviderKey::of(TypeId::of::<Arc<T>>()))
            .and_then(|any| any.clone().downcast::<Arc<T>>().ok())
            .map(|outer| (*outer).clone())
    }

    pub(crate) fn metadata_entries(&self, key: TypeId) -> Option<&Vec<MetaEntry>> {
        self.metadata.get(&key)
    }

    pub(crate) fn scoped_factory(&self, id: TypeId) -> Option<ScopedFactory> {
        self.scoped.get(&id).cloned()
    }

    pub(crate) fn transient_factory(&self, id: TypeId) -> Option<TransientFactory> {
        self.transient.get(&id).cloned()
    }
}

/// Resolve a transient provider with re-entrancy detection. A transient that
/// (transitively) injects itself would recurse forever; we catch the second
/// entry for the same `TypeId` and panic with a chain naming every type on
/// the cycle.
///
/// The push/pop pairing is panic-safe via [`CycleGuard`]: a factory that
/// panics still pops the stack as the guard unwinds.
///
/// The factory resolves its `#[inject]` deps through `scope`
/// ([`RequestScope`]): a request-scoped dep resolves to the request's shared
/// instance, singletons forward to the root, another transient rebuilds.
pub(crate) fn build_transient(
    id: TypeId,
    type_name: &'static str,
    factory: &TransientFactory,
    scope: &RequestScope,
) -> AnyArc {
    let _guard = CycleGuard::push(&TRANSIENT_BUILDING, id, type_name).unwrap_or_else(|Cycle { chain }| {
        panic!(
            "transient provider cycle: {chain} — break the cycle by injecting `Arc<dyn Trait>` or picking a different scope"
        )
    });
    factory(scope)
    // `_guard` drops here — pops the stack even if `factory` panics and the
    // value path above is skipped.
}

/// Mutable staging area for the container: providers, metadata and scoped
/// factories accumulate here across the build phases, then [`build`](Self::build)
/// freezes them into an immutable [`Container`].
#[derive(Default)]
pub struct ContainerBuilder {
    providers: HashMap<ProviderKey, AnyArc>,
    metadata: HashMap<TypeId, Vec<MetaEntry>>,
    /// Idempotency for the register phase — a diamond import registers once.
    registered_modules: HashSet<TypeId>,
    /// Idempotency for the collect phase.
    collected_modules: HashSet<TypeId>,
    /// Builder-only: drained by [`AppBuilder::build`](crate::AppBuilder::build),
    /// never copied into the [`Container`] or a [`snapshot`](Self::snapshot).
    /// The `TypeId` lets the build skip a factory whose output a seed already
    /// supplies (a test injecting a pre-built resource in place of a `for_root`).
    factories: Vec<(TypeId, BoxedFactory)>,
    scoped: HashMap<TypeId, ScopedFactory>,
    transient: HashMap<TypeId, TransientFactory>,
    /// Concrete/keyed registrations that replaced an earlier one — a wiring
    /// mistake the boot rejects (see [`duplicate_providers`](Self::duplicate_providers)).
    duplicates: Vec<DuplicateProvider>,
    /// Dynamic imports (`Foo::for_root(opts)`) already constructed by the
    /// collect phase, keyed by import site so the register phase consumes the
    /// same value instead of re-evaluating the expression. Builder-only —
    /// never copied into a [`Container`] or a [`snapshot`](Self::snapshot).
    dynamic_registrars: HashMap<DynamicImportSite, Registrar>,
}

/// Identifies one `#[module(imports = [...])]` entry: the importing module's
/// type plus its position in the list. Stable across the collect and register
/// phases because both walk the same import list in order.
type DynamicImportSite = (TypeId, usize);

/// A bare concrete-type provider registered more than once. Surfaced by the
/// boot as a fatal wiring error rather than a silent last-write-wins. (Keyed
/// providers keep the documented last-write-wins and are not collected here.)
#[derive(Clone, Copy)]
pub struct DuplicateProvider {
    /// Type name of the doubly-registered provider.
    pub type_name: &'static str,
}

impl ContainerBuilder {
    /// Register a value, wrapped in `Arc` internally.
    pub fn provide<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.record_if_replacing(&ProviderKey::typed::<T>(), std::any::type_name::<T>());
        self.warn_if_cross_kind_singleton(TypeId::of::<T>(), std::any::type_name::<T>());
        self.providers
            .insert(ProviderKey::typed::<T>(), Arc::new(value));
        self
    }

    /// Register an already-shared `Arc<T>`.
    pub fn provide_arc<T: Any + Send + Sync>(mut self, value: Arc<T>) -> Self {
        self.record_if_replacing(&ProviderKey::typed::<T>(), std::any::type_name::<T>());
        self.warn_if_cross_kind_singleton(TypeId::of::<T>(), std::any::type_name::<T>());
        self.providers.insert(ProviderKey::typed::<T>(), value);
        self
    }

    /// Register a **keyed** singleton — a second instance of `T` under a
    /// distinct `name`, resolvable with [`Container::get_keyed`] or an
    /// `#[inject(key = "…")]` field. Lets several instances of one concrete
    /// type coexist in the flat container without a newtype wrapper.
    ///
    /// Keyed providers are singletons only. Registering twice under the same
    /// `(T, name)` warns and last-write-wins, mirroring [`provide`](Self::provide).
    pub fn provide_keyed<T: Any + Send + Sync>(mut self, name: &'static str, value: T) -> Self {
        self.record_if_replacing(&ProviderKey::named::<T>(name), std::any::type_name::<T>());
        self.providers
            .insert(ProviderKey::named::<T>(name), Arc::new(value));
        self
    }

    /// [`provide_keyed`](Self::provide_keyed) for an already-shared `Arc<T>`.
    pub fn provide_keyed_arc<T: Any + Send + Sync>(
        mut self,
        name: &'static str,
        value: Arc<T>,
    ) -> Self {
        self.record_if_replacing(&ProviderKey::named::<T>(name), std::any::type_name::<T>());
        self.providers.insert(ProviderKey::named::<T>(name), value);
        self
    }

    /// Replace a concrete provider without the override warning — the
    /// intentional swap path used by
    /// [`AppBuilder::override_value`](crate::AppBuilder::override_value).
    pub(crate) fn replace<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.providers
            .insert(ProviderKey::typed::<T>(), Arc::new(value));
        self
    }

    /// Replace a concrete provider with a pre-shared `Arc<T>` without the
    /// override warning — the intentional swap path used by
    /// [`AppBuilder::override_provider`](crate::AppBuilder::override_provider).
    pub(crate) fn replace_arc<T: Any + Send + Sync>(mut self, value: Arc<T>) -> Self {
        self.providers.insert(ProviderKey::typed::<T>(), value);
        self
    }

    /// A **bare** concrete-type registration that replaces an earlier one — two
    /// modules (or a seed and a module) registering the same type by mistake.
    /// [`App`](crate::App) reads [`duplicate_providers`](Self::duplicate_providers)
    /// after the register phase and **fails the boot**, uniform with every other
    /// wiring error. Trait-object bindings ([`provide_dyn`](Self::provide_dyn))
    /// are exempt: last-binding-wins is their documented override mechanism, and
    /// the test override path ([`replace`](Self::replace)) does not route here.
    /// **Keyed** providers ([`provide_keyed`](Self::provide_keyed)) keep the
    /// documented last-write-wins: re-registering `(T, name)` is a supported
    /// keyed override, so it warns rather than failing the boot.
    fn record_if_replacing(&mut self, key: &ProviderKey, type_name: &'static str) {
        if !self.providers.contains_key(key) {
            return;
        }
        match key.name {
            None => self.duplicates.push(DuplicateProvider { type_name }),
            Some(name) => tracing::warn!(
                target: "nest_rs::container",
                provider = type_name,
                key = name,
                "keyed provider override",
            ),
        }
    }

    /// Concrete/keyed providers registered more than once — collected during the
    /// register phase so the boot fails naming them (a duplicate silently
    /// last-write-wins would otherwise be a wiring bug that only surfaces as
    /// wrong behaviour). Empty in the common case.
    pub(crate) fn duplicate_providers(&self) -> &[DuplicateProvider] {
        &self.duplicates
    }

    /// Warn when a singleton registration shadows an existing transient
    /// factory of the same `TypeId`. `Container::get` checks `transient`
    /// before `providers`, so the singleton would be unreachable — the most
    /// likely cause is two modules registering the same type with different
    /// scopes by mistake.
    fn warn_if_cross_kind_singleton(&self, id: TypeId, type_name: &'static str) {
        if self.transient.contains_key(&id) {
            tracing::warn!(
                target: "nest_rs::container",
                provider = type_name,
                existing_kind = "transient",
                new_kind = "singleton",
                "provider scope conflict",
            );
        }
    }

    /// Warn when a transient registration shadows an existing singleton of
    /// the same `TypeId`. Resolution now silently returns the transient
    /// build, leaving the singleton state unreachable.
    fn warn_if_cross_kind_transient(&self, id: TypeId, type_name: &'static str) {
        if self.providers.contains_key(&ProviderKey::of(id)) {
            tracing::warn!(
                target: "nest_rs::container",
                provider = type_name,
                existing_kind = "singleton",
                new_kind = "transient",
                "provider scope conflict",
            );
        }
    }

    /// Register a trait-object provider. Stored as `Arc<Arc<T>>` so the outer
    /// `Arc` is sized and retrievable via the trait's `TypeId`.
    pub fn provide_dyn<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.providers
            .insert(ProviderKey::of(TypeId::of::<Arc<T>>()), Arc::new(value));
        self
    }

    /// Attach metadata of type `M` to the provider type `P`, discovered via
    /// [`crate::DiscoveryService::meta`].
    pub fn attach_meta<P: 'static, M: Any + Send + Sync>(mut self, meta: M) -> Self {
        self.metadata
            .entry(TypeId::of::<M>())
            .or_default()
            .push(MetaEntry {
                provider_type_id: Some(TypeId::of::<P>()),
                meta: Arc::new(meta),
            });
        self
    }

    /// Attach metadata not bound to a specific provider — e.g. a module-level
    /// descriptor a scanner aggregates globally.
    pub fn provide_meta<M: Any + Send + Sync>(mut self, meta: M) -> Self {
        self.metadata
            .entry(TypeId::of::<M>())
            .or_default()
            .push(MetaEntry {
                provider_type_id: None,
                meta: Arc::new(meta),
            });
        self
    }

    /// Whether a provider for `id` has already been registered. Lets `#[module]`
    /// register providers in any order by checking dependencies against this.
    pub fn contains(&self, id: TypeId) -> bool {
        self.providers.contains_key(&ProviderKey::of(id))
    }

    /// Record that a module of type `id` is being registered. Returns `true`
    /// the first time, `false` thereafter — a module imported via several
    /// paths registers exactly once.
    pub fn mark_registered(&mut self, id: TypeId) -> bool {
        self.registered_modules.insert(id)
    }

    /// Collect-phase counterpart of [`mark_registered`](Self::mark_registered).
    pub fn mark_collected(&mut self, id: TypeId) -> bool {
        self.collected_modules.insert(id)
    }

    /// Collect phase for one dynamic import: run its
    /// [`DynamicModule::collect`], then park the value so
    /// [`register_dynamic_import`](Self::register_dynamic_import) can consume
    /// it at the same site. The import expression is therefore evaluated
    /// **once** for both phases.
    ///
    /// **Internal ABI** — emitted by `#[module]`, lockstep with
    /// `nest-rs-core-macros`; do not call by hand.
    #[doc(hidden)]
    pub fn collect_dynamic_import<D>(mut self, module: TypeId, index: usize, value: D) -> Self
    where
        D: DynamicModule + Send + 'static,
    {
        self = value.collect(self);
        self.dynamic_registrars.insert(
            (module, index),
            Box::new(move |builder| value.register(builder)),
        );
        self
    }

    /// Register phase for one dynamic import: consume the value the collect
    /// phase parked at this site, or build one from `fallback` when there was
    /// no collect phase (the synchronous [`App::new`](crate::App::new) path).
    /// Either way the import expression runs exactly once.
    ///
    /// **Internal ABI** — emitted by `#[module]`, lockstep with
    /// `nest-rs-core-macros`; do not call by hand.
    #[doc(hidden)]
    pub fn register_dynamic_import<D, F>(
        mut self,
        module: TypeId,
        index: usize,
        fallback: F,
    ) -> Self
    where
        D: DynamicModule + Send + 'static,
        F: FnOnce() -> D,
    {
        match self.dynamic_registrars.remove(&(module, index)) {
            Some(registrar) => registrar(self),
            None => fallback().register(self),
        }
    }

    /// Queue an async factory whose awaited output is stored as a provider
    /// (injectable as `Arc<T>`). Drained by
    /// [`AppBuilder::build`](crate::AppBuilder::build) before providers are
    /// built.
    pub fn provide_factory<T, F, Fut>(mut self, factory: F) -> Self
    where
        T: Any + Send + Sync,
        F: FnOnce(Container) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        let boxed: BoxedFactory = Box::new(move |container| {
            Box::pin(async move {
                let value = factory(container).await?;
                let registrar: Registrar = Box::new(move |builder| builder.provide(value));
                Ok(registrar)
            })
        });
        self.factories.push((TypeId::of::<T>(), boxed));
        self
    }

    /// Register a request-scoped provider: `factory` builds a fresh `T` for
    /// each request, cached by a [`RequestScope`].
    ///
    /// Emitted by `#[injectable(scope = request)]`. The factory resolves
    /// dependencies through the [`RequestScope`], so a
    /// request-scoped provider may depend on singletons **and** on other
    /// request-scoped providers (which it shares with the rest of the request).
    /// The reverse — a singleton depending on a request-scoped provider — stays
    /// structurally impossible (singletons are built before any request exists).
    pub fn provide_scoped<T, F>(mut self, factory: F) -> Self
    where
        T: Any + Send + Sync,
        F: Fn(&RequestScope) -> T + Send + Sync + 'static,
    {
        let id = TypeId::of::<T>();
        if self.scoped.contains_key(&id) {
            tracing::warn!(
                target: "nest_rs::container",
                provider = std::any::type_name::<T>(),
                kind = "request_scoped",
                "provider override",
            );
        }
        self.scoped.insert(
            id,
            Arc::new(move |scope| Arc::new(factory(scope)) as AnyArc),
        );
        self
    }

    /// Register a transient provider: `factory` builds a fresh `T` every time
    /// `Container::get::<T>()` (or a [`RequestScope`])
    /// resolves it. There is no caching — same scope, multiple resolutions,
    /// different instances.
    ///
    /// Emitted by `#[injectable(scope = transient)]`. The factory receives the
    /// [`RequestScope`], so a transient may depend on singletons **and** on
    /// request-scoped providers (sharing the request's instance); a transient
    /// depending (transitively) on itself panics at resolution with a cycle
    /// diagnostic.
    pub fn provide_transient<T, F>(mut self, factory: F) -> Self
    where
        T: Any + Send + Sync,
        F: Fn(&RequestScope) -> T + Send + Sync + 'static,
    {
        let id = TypeId::of::<T>();
        if self.transient.contains_key(&id) {
            tracing::warn!(
                target: "nest_rs::container",
                provider = std::any::type_name::<T>(),
                kind = "transient",
                "provider override",
            );
        }
        self.warn_if_cross_kind_transient(id, std::any::type_name::<T>());
        self.transient.insert(
            id,
            Arc::new(move |scope| Arc::new(factory(scope)) as AnyArc),
        );
        self
    }

    pub(crate) fn take_factories(&mut self) -> Vec<(TypeId, BoxedFactory)> {
        std::mem::take(&mut self.factories)
    }

    /// Provider keys registered so far. Snapshotted by `AppBuilder::build`
    /// after the factory phase to form the **global** set (seeds + factory
    /// outputs) for the access-graph check.
    pub(crate) fn provider_ids(&self) -> HashSet<TypeId> {
        self.providers
            .keys()
            .filter(|k| k.name.is_none())
            .map(|k| k.type_id)
            .collect()
    }

    /// Every unkeyed `TypeId` resolvable from this builder — singleton values
    /// **plus** request-scoped and transient factories. Used by the access-graph
    /// missing-dependency check to tolerate a dependency provided imperatively
    /// (a hand-written `impl Module`) or by a lazy factory: those never appear in
    /// the declarative `providers = [...]` graph, so the check must consult the
    /// actual registered set before declaring a dependency unmet.
    pub(crate) fn registered_ids(&self) -> HashSet<TypeId> {
        self.providers
            .keys()
            .filter(|k| k.name.is_none())
            .map(|k| k.type_id)
            .chain(self.scoped.keys().copied())
            .chain(self.transient.keys().copied())
            .collect()
    }

    /// Keyed provider identities registered so far. Snapshotted alongside
    /// [`provider_ids`](Self::provider_ids) after the factory phase to form the
    /// **global keyed** set for the access-graph keyed check — a keyed
    /// dependency is legal when a seed or factory output supplies it.
    pub(crate) fn keyed_provider_keys(&self) -> HashSet<ProviderKey> {
        self.providers
            .keys()
            .filter(|k| k.name.is_some())
            .copied()
            .collect()
    }

    /// Freeze the accumulated registrations into the immutable [`Container`].
    pub fn build(self) -> Container {
        Container {
            providers: Arc::new(self.providers),
            metadata: Arc::new(self.metadata),
            scoped: Arc::new(self.scoped),
            transient: Arc::new(self.transient),
        }
    }

    /// Snapshot the providers registered so far. Used by `#[module]` to let a
    /// provider being built resolve its dependencies while the builder is
    /// still under construction.
    ///
    /// **Known ceiling:** this deep-clones four maps per provider
    /// registration, making boot O(n²) in provider count. Fine at current
    /// scales (hundreds of providers boot in milliseconds); if provider
    /// counts grow into the thousands, switch these maps to persistent
    /// (structurally shared) maps rather than optimizing call sites.
    pub fn snapshot(&self) -> Container {
        Container {
            providers: Arc::new(self.providers.clone()),
            metadata: Arc::new(self.metadata.clone()),
            scoped: Arc::new(self.scoped.clone()),
            transient: Arc::new(self.transient.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Greeter(&'static str);
    struct Counter(u32);

    #[test]
    fn resolves_a_provided_value() {
        let container = Container::builder().provide(Greeter("hi")).build();
        let resolved: Arc<Greeter> = container.get().expect("greeter is registered");
        assert_eq!(resolved.0, "hi");
    }

    #[test]
    fn resolves_multiple_distinct_types() {
        let container = Container::builder()
            .provide(Greeter("hi"))
            .provide(Counter(42))
            .build();
        assert_eq!(container.get::<Greeter>().unwrap().0, "hi");
        assert_eq!(container.get::<Counter>().unwrap().0, 42);
    }

    #[test]
    fn missing_type_returns_none() {
        let container = Container::builder().build();
        assert!(container.get::<Greeter>().is_none());
    }

    #[test]
    fn provide_override_keeps_the_last_value() {
        // Overriding logs a warn, but last-write-wins matches `provide_dyn`.
        let container = Container::builder()
            .provide(Counter(1))
            .provide(Counter(2))
            .build();
        assert_eq!(container.get::<Counter>().unwrap().0, 2);
    }

    #[test]
    fn provide_arc_preserves_the_same_instance() {
        let shared = Arc::new(Counter(7));
        let container = Container::builder().provide_arc(shared.clone()).build();
        let resolved: Arc<Counter> = container.get().unwrap();
        assert!(Arc::ptr_eq(&shared, &resolved));
    }

    #[test]
    fn keyed_providers_of_the_same_type_coexist() {
        // Two instances of one concrete type, disambiguated by key, live in the
        // flat container at once — the whole point of `provide_keyed`.
        let container = Container::builder()
            .provide_keyed("github", Greeter("gh"))
            .provide_keyed("google", Greeter("goog"))
            .build();
        assert_eq!(container.get_keyed::<Greeter>("github").unwrap().0, "gh");
        assert_eq!(container.get_keyed::<Greeter>("google").unwrap().0, "goog");
    }

    #[test]
    fn keyed_and_bare_of_the_same_type_are_distinct_slots() {
        // A bare `provide` and a keyed `provide_keyed` of the same type do not
        // collide: `None` and `Some(name)` are separate `ProviderKey`s.
        let container = Container::builder()
            .provide(Greeter("bare"))
            .provide_keyed("github", Greeter("gh"))
            .build();
        assert_eq!(container.get::<Greeter>().unwrap().0, "bare");
        assert_eq!(container.get_keyed::<Greeter>("github").unwrap().0, "gh");
        // A bare `get` never sees a keyed slot, and vice versa.
        assert!(container.get_keyed::<Greeter>("google").is_none());
    }

    #[test]
    fn missing_keyed_provider_returns_none() {
        let container = Container::builder()
            .provide_keyed("github", Greeter("gh"))
            .build();
        assert!(container.get_keyed::<Greeter>("google").is_none());
        assert!(container.get_keyed::<Counter>("github").is_none());
    }

    #[test]
    fn keyed_provider_arc_preserves_the_same_instance() {
        let shared = Arc::new(Counter(7));
        let container = Container::builder()
            .provide_keyed_arc("primary", shared.clone())
            .build();
        let resolved: Arc<Counter> = container.get_keyed("primary").unwrap();
        assert!(Arc::ptr_eq(&shared, &resolved));
    }

    #[test]
    fn keyed_override_keeps_the_last_value_and_warns() {
        let logs = capture_warns(|| {
            let container = Container::builder()
                .provide_keyed("github", Counter(1))
                .provide_keyed("github", Counter(2))
                .build();
            assert_eq!(container.get_keyed::<Counter>("github").unwrap().0, 2);
        });
        assert!(
            logs.contains("provider override"),
            "a same-(type, key) re-registration must warn: {logs}",
        );
    }

    #[test]
    fn keyed_collision_is_scoped_to_the_same_key() {
        // Distinct keys of the same type never warn — they are different slots.
        let logs = capture_warns(|| {
            let _ = Container::builder()
                .provide_keyed("github", Counter(1))
                .provide_keyed("google", Counter(2))
                .build();
        });
        assert!(
            !logs.contains("provider override"),
            "distinct keys must not be treated as an override: {logs}",
        );
    }

    #[test]
    fn container_is_cheap_to_clone() {
        let container = Container::builder().provide(Greeter("hi")).build();
        let cloned = container.clone();
        assert_eq!(cloned.get::<Greeter>().unwrap().0, "hi");
    }

    trait Hello: Send + Sync {
        fn say(&self) -> &'static str;
    }
    struct Polite;
    impl Hello for Polite {
        fn say(&self) -> &'static str {
            "hello"
        }
    }
    struct Curt;
    impl Hello for Curt {
        fn say(&self) -> &'static str {
            "hi"
        }
    }

    #[test]
    fn provide_dyn_then_get_dyn_returns_the_impl() {
        let polite: Arc<dyn Hello + Send + Sync> = Arc::new(Polite);
        let container = Container::builder().provide_dyn(polite).build();

        let resolved: Arc<dyn Hello + Send + Sync> =
            container.get_dyn().expect("dyn Hello provider");
        assert_eq!(resolved.say(), "hello");
    }

    #[test]
    fn provide_dyn_last_binding_wins() {
        let polite: Arc<dyn Hello + Send + Sync> = Arc::new(Polite);
        let curt: Arc<dyn Hello + Send + Sync> = Arc::new(Curt);
        let container = Container::builder()
            .provide_dyn(polite)
            .provide_dyn(curt)
            .build();

        let resolved: Arc<dyn Hello + Send + Sync> = container.get_dyn().unwrap();
        assert_eq!(resolved.say(), "hi");
    }

    #[derive(Debug, PartialEq)]
    struct Marker(&'static str);

    struct Host;

    #[test]
    fn attach_meta_preserves_insertion_order() {
        let container = Container::builder()
            .attach_meta::<Host, _>(Marker("first"))
            .attach_meta::<Host, _>(Marker("second"))
            .attach_meta::<Host, _>(Marker("third"))
            .build();
        let entries = container
            .metadata_entries(TypeId::of::<Marker>())
            .expect("Marker metadata present");
        assert_eq!(entries.len(), 3);
        let values: Vec<&str> = entries
            .iter()
            .map(|e| e.meta.clone().downcast::<Marker>().unwrap().0)
            .collect();
        assert_eq!(values, ["first", "second", "third"]);
    }

    #[test]
    fn attach_meta_records_provider_type_id() {
        let container = Container::builder()
            .attach_meta::<Host, _>(Marker("hi"))
            .build();
        let entries = container.metadata_entries(TypeId::of::<Marker>()).unwrap();
        assert_eq!(entries[0].provider_type_id, Some(TypeId::of::<Host>()));
    }

    #[test]
    fn provide_meta_has_no_host() {
        let container = Container::builder().provide_meta(Marker("free")).build();
        let entries = container.metadata_entries(TypeId::of::<Marker>()).unwrap();
        assert_eq!(entries[0].provider_type_id, None);
    }

    #[test]
    fn metadata_returns_none_when_absent() {
        let container = Container::builder().build();
        assert!(container.metadata_entries(TypeId::of::<Marker>()).is_none());
    }

    #[test]
    fn mark_registered_is_true_once_then_false() {
        let mut builder = Container::builder();
        assert!(builder.mark_registered(TypeId::of::<Host>()));
        assert!(!builder.mark_registered(TypeId::of::<Host>()));
        assert!(builder.mark_registered(TypeId::of::<Marker>()));
    }

    #[test]
    fn transient_factory_rebuilds_on_every_resolution() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let calls = Arc::new(AtomicU32::new(0));
        let calls_factory = calls.clone();
        let container = Container::builder()
            .provide_transient(move |_| Counter(calls_factory.fetch_add(1, Ordering::SeqCst)))
            .build();

        let first: Arc<Counter> = container.get().expect("first build");
        let second: Arc<Counter> = container.get().expect("second build");
        assert_eq!(first.0, 0);
        assert_eq!(second.0, 1);
        // Two builds means two distinct allocations.
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn transient_factory_reads_singleton_deps() {
        // A transient pulling a singleton: the singleton stays shared, the
        // transient stays fresh.
        let container = Container::builder()
            .provide(Greeter("hello"))
            .provide_transient(|c| {
                let g: Arc<Greeter> = c.get().expect("singleton resolves");
                Counter(g.0.len() as u32)
            })
            .build();

        let a: Arc<Counter> = container.get().unwrap();
        let b: Arc<Counter> = container.get().unwrap();
        assert_eq!(a.0, 5);
        assert_eq!(b.0, 5);
        assert!(!Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn transient_depending_on_request_scoped_resolves_off_the_bare_container() {
        // B-CORE: resolving a transient whose `#[inject]` dep is request-scoped
        // through the bare `Container::get` escape hatch (no live request) must
        // NOT panic on a missing provider. A throwaway scope builds the
        // request-scoped dep fresh, so the transient resolves.
        struct Dep(u32);
        struct Trans(u32);
        let container = Container::builder()
            .provide_scoped::<Dep, _>(|_| Dep(9))
            .provide_transient::<Trans, _>(|scope| {
                Trans(
                    scope
                        .get::<Dep>()
                        .expect("scoped dep builds request-of-one")
                        .0,
                )
            })
            .build();

        let resolved: Arc<Trans> = container
            .get()
            .expect("the transient resolves off the bare container without panicking");
        assert_eq!(resolved.0, 9);
    }

    #[test]
    #[should_panic(expected = "transient provider cycle")]
    fn transient_self_dependency_panics_with_cycle_diagnostic() {
        let container = Container::builder()
            .provide_transient(|c| {
                // Resolving the same transient inside its factory loops; the
                // re-entrancy guard catches the second entry and panics.
                let _self: Arc<Counter> = c.get().expect("re-entrant resolution");
                Counter(0)
            })
            .build();
        let _ = container.get::<Counter>();
    }

    // A two-step cycle (A → B → A) must produce a diagnostic that names BOTH
    // types in order. A bug that printed only the type currently being built
    // (here: A) would be indistinguishable from a self-cycle and useless for
    // diagnosing which intermediate provider closes the loop.
    #[test]
    fn transient_transitive_cycle_diagnostic_lists_full_chain() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let container = Container::builder()
                // A's factory resolves B; B's factory resolves A → second
                // re-entry of A is caught.
                .provide_transient(|c| {
                    let _b: Arc<Counter> = c.get().expect("B resolves");
                    Greeter("A")
                })
                .provide_transient(|c| {
                    let _a: Arc<Greeter> = c.get().expect("A resolves");
                    Counter(0)
                })
                .build();
            let _: Option<Arc<Greeter>> = container.get();
        }));

        let payload = result.expect_err("the cycle must panic");
        let msg = payload
            .downcast_ref::<String>()
            .map(|s| s.as_str())
            .or_else(|| payload.downcast_ref::<&'static str>().copied())
            .unwrap_or("<non-string panic>");
        assert!(
            msg.contains("transient provider cycle"),
            "missing prefix: {msg}",
        );
        assert!(
            msg.contains("Greeter"),
            "diagnostic must name A (Greeter): {msg}",
        );
        assert!(
            msg.contains("Counter"),
            "diagnostic must name B (Counter): {msg}",
        );
        // Order matters — the chain reads from where the cycle starts to the
        // offending re-entry. Resolution begins at A (Greeter), reaches B
        // (Counter), then loops back to A.
        let greeter_at = msg.find("Greeter").unwrap();
        let counter_at = msg.find("Counter").unwrap();
        assert!(greeter_at < counter_at, "chain must read A then B: {msg}",);
    }

    #[test]
    fn transient_override_replaces_earlier_factory() {
        let container = Container::builder()
            .provide_transient(|_| Counter(1))
            .provide_transient(|_| Counter(2))
            .build();
        // Second registration wins (logged at `warn`).
        let resolved: Arc<Counter> = container.get().unwrap();
        assert_eq!(resolved.0, 2);
    }

    /// Capture `tracing` events emitted on the calling thread while `f`
    /// runs. Returns the rendered event lines so a test can assert
    /// substrings without depending on the global subscriber.
    fn capture_warns<F: FnOnce()>(f: F) -> String {
        use std::io::Write;
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::fmt::MakeWriter;

        #[derive(Clone, Default)]
        struct Buf(Arc<Mutex<Vec<u8>>>);

        impl Write for Buf {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        impl<'a> MakeWriter<'a> for Buf {
            type Writer = Buf;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }

        let buf = Buf::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buf.clone())
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        let bytes = buf.0.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap_or_default()
    }

    #[test]
    fn singleton_then_transient_same_typeid_warns_cross_kind() {
        // Y1: registering a singleton then a transient of the same TypeId
        // leaves both registered; `Container::get` returns the transient
        // and the singleton state is unreachable. The builder must warn
        // at registration time so the conflict is visible.
        let logs = capture_warns(|| {
            let _ = Container::builder()
                .provide(Counter(1))
                .provide_transient(|_| Counter(2))
                .build();
        });
        assert!(
            logs.contains("provider scope conflict"),
            "expected cross-kind warn, got: {logs}",
        );
        assert!(
            logs.contains("existing_kind") && logs.contains("singleton"),
            "warn must name the existing singleton: {logs}",
        );
        assert!(
            logs.contains("new_kind") && logs.contains("transient"),
            "warn must name the incoming transient: {logs}",
        );
    }

    #[test]
    fn transient_then_singleton_same_typeid_warns_cross_kind() {
        // Symmetric direction: a transient registered first, then a
        // singleton — the singleton would be unreachable through `get`.
        let logs = capture_warns(|| {
            let _ = Container::builder()
                .provide_transient(|_| Counter(1))
                .provide(Counter(2))
                .build();
        });
        assert!(
            logs.contains("provider scope conflict"),
            "expected cross-kind warn, got: {logs}",
        );
        assert!(
            logs.contains("existing_kind") && logs.contains("transient"),
            "warn must name the existing transient: {logs}",
        );
        assert!(
            logs.contains("new_kind") && logs.contains("singleton"),
            "warn must name the incoming singleton: {logs}",
        );
    }

    #[test]
    fn transient_panic_clears_reentrancy_stack() {
        // A factory that panics must still pop its entry from the
        // thread-local stack — otherwise the next legitimate resolution on
        // this thread would either report a spurious cycle (re-entering the
        // same type) or silently leak into an unrelated transient build.
        //
        // The observable contract is: after a factory panic, the thread is
        // usable. We don't peek at the thread-local depth — the assertions
        // here (re-resolve the same panicking transient without "cycle",
        // resolve a different transient cleanly) prove cleanup.
        let container = Container::builder()
            .provide_transient(|_| -> Counter { panic!("boom from factory") })
            .provide_transient(|_| Greeter("recovered"))
            .build();

        // First resolution: the factory panics. Catch the unwind so the
        // RAII drop guard pops the entry as the stack unwinds.
        let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _: Option<Arc<Counter>> = container.get();
        }));
        let payload = first.expect_err("factory panic should propagate");
        let msg = payload
            .downcast_ref::<&'static str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(|s| s.as_str()))
            .unwrap_or("<non-string panic>");
        assert!(
            msg.contains("boom from factory"),
            "first call surfaces the factory panic, not a spurious cycle: {msg}",
        );

        // Re-resolve the SAME panicking transient. If the prior entry leaked
        // on the stack, `TransientGuard::push` would see itself already
        // present and panic with "transient provider cycle" — a spurious
        // diagnostic that hides the real factory bug. The second call must
        // still panic with the factory's own message instead.
        let second = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _: Option<Arc<Counter>> = container.get();
        }));
        let payload = second.expect_err("factory still panics on the second call");
        let msg = payload
            .downcast_ref::<&'static str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(|s| s.as_str()))
            .unwrap_or("<non-string panic>");
        assert!(
            !msg.contains("transient provider cycle"),
            "a popped stack must not surface as a spurious cycle: {msg}",
        );
        assert!(
            msg.contains("boom from factory"),
            "the second call must surface the same factory panic: {msg}",
        );

        // A *different* transient on the same thread must resolve cleanly —
        // proves the thread-local is not poisoned by the prior panics.
        let resolved: Arc<Greeter> = container
            .get()
            .expect("different transient resolves after a sibling factory panicked");
        assert_eq!(resolved.0, "recovered");
    }
}

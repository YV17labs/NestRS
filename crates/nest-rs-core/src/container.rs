//! The IoC [`Container`] and its [`ContainerBuilder`] — the single flat
//! registry keyed by [`ProviderKey`] that every provider resolves through, plus
//! the request-scoped and transient factory machinery layered on top of it.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::RequestScope;
use crate::cycle_guard::{BuildStack, Cycle, CycleGuard};
use crate::layer_chain::__private::SiteChains;
use crate::module::{Collecting, Module, Registering};

type AnyArc = Arc<dyn Any + Send + Sync>;

/// The identity a singleton provider registers under: a `TypeId` optionally
/// disambiguated by a `name`. `name: Some(_)` is a **keyed** provider
/// ([`ContainerBuilder::provide_keyed`]), letting several instances of one
/// concrete type coexist. Keying is singleton-only.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ProviderKey {
    /// The provided type's identity.
    pub type_id: TypeId,
    /// The key when this is a keyed provider; `None` for the bare identity.
    pub name: Option<&'static str>,
}

impl ProviderKey {
    /// The bare (unkeyed) identity for a concrete type.
    pub(crate) fn typed<T: Any>() -> Self {
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

    /// The bare identity for a raw `TypeId`.
    pub(crate) fn of(type_id: TypeId) -> Self {
        Self {
            type_id,
            name: None,
        }
    }
}

/// A keyed `#[inject(key = "…")]` dependency reported by
/// [`Discoverable::injected_keyed`](crate::Discoverable::injected_keyed).
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

/// Builds a fresh instance of a request-scoped provider, once per request,
/// from the [`RequestScope`] so its request-scoped deps share the request's
/// instance.
pub(crate) type ScopedFactory = Arc<dyn Fn(&RequestScope) -> AnyArc + Send + Sync>;

/// Builds a fresh instance of a transient provider on every resolution, from
/// the [`RequestScope`]; outside a request, a throwaway scope over the root.
pub(crate) type TransientFactory = Arc<dyn Fn(&RequestScope) -> AnyArc + Send + Sync>;

thread_local! {
    /// Re-entrancy guard for transient resolution ([`CycleGuard`]).
    static TRANSIENT_BUILDING: BuildStack = const { RefCell::new(Vec::new()) };
}

/// A registration applied once a factory has produced its value, so factory
/// outputs flow through the same path — and the same duplicate detection — as
/// any other provider.
pub(crate) type Registrar = Box<dyn FnOnce(ContainerBuilder) -> ContainerBuilder + Send>;
type FactoryFuture = Pin<Box<dyn Future<Output = Result<Registrar>> + Send>>;
pub(crate) type BoxedFactory = Box<dyn FnOnce(Container) -> FactoryFuture + Send>;

/// One entry of the async factory queue, drained by
/// [`AppBuilder::build`](crate::AppBuilder::build).
///
/// `after` names the factory outputs this one reads from its snapshot; the
/// drain runs it once each is present or provided by nothing queued. `provides`
/// is every key it registers — the concrete `T` and each `Arc<dyn D>` bound off
/// it — since a dependent may wait on either.
pub(crate) struct QueuedFactory {
    pub(crate) name: &'static str,
    pub(crate) provides: Vec<TypeId>,
    pub(crate) after: Vec<TypeId>,
    /// Whether an import site chose `T`, so a value already present is not
    /// the one it declared.
    pub(crate) declared: bool,
    /// The trait objects bound off `T` once it is present, whoever built it:
    /// this entry's factory, a seed, or the declaration that superseded it.
    pub(crate) derives: Vec<Registrar>,
    pub(crate) factory: BoxedFactory,
}

impl QueuedFactory {
    /// The concrete `T` this entry is keyed by — the first of
    /// [`provides`](Self::provides).
    pub(crate) fn id(&self) -> TypeId {
        self.provides[0]
    }
}

/// One import whose phase is running: the module whose `imports = [..]` names
/// it — `None` for a root the app was built from — and the import as written.
#[derive(Clone, Copy)]
struct ImportSite {
    /// The module and the import's position in its list.
    importer: Option<(&'static str, usize)>,
    import: &'static str,
}

impl std::fmt::Display for ImportSite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.importer {
            Some((importer, at)) => {
                write!(f, "`{}` at `imports[{at}]` of `{importer}`", self.import)
            }
            None => write!(f, "`{}`, a root the app is built from", self.import),
        }
    }
}

/// How one [`ContainerBuilder::queue`] call differs from the plain form.
#[derive(Default)]
struct QueueSpec {
    /// Set when the call is a **declaration** — an import site chose this value
    /// — carrying the sentence `ContestedDeclarationError` appends.
    remedy: Option<&'static str>,
    /// The trait object bound off `T`, for a dyn binding.
    binds: Option<DynBinding>,
    /// The factory outputs this one reads from its snapshot.
    after: Vec<TypeId>,
}

/// The `Arc<dyn D>` a dyn binding registers beside its `T`: a name a dependent
/// may wait on, and a declaration — one implementation holds a trait object.
struct DynBinding {
    id: TypeId,
    name: &'static str,
    /// Binds it off the `T` present, whoever built it.
    derive: Registrar,
}

impl DynBinding {
    fn of<T, D>(bind: fn(T) -> Arc<D>) -> Self
    where
        T: Any + Clone + Send + Sync,
        D: ?Sized + Send + Sync + 'static,
    {
        Self {
            id: TypeId::of::<Arc<D>>(),
            name: std::any::type_name::<Arc<D>>(),
            derive: Box::new(move |builder| match builder.get::<T>() {
                Some(present) => install_dyn_only(builder, (*present).clone(), bind),
                None => builder,
            }),
        }
    }
}

/// One type an import site chose a value for.
struct Declaration {
    /// The import that declared it, as a boot error names it.
    site: String,
    /// The concrete type whose factory makes the value: a binding queued twice
    /// by one module is one declaration, not two.
    binder: TypeId,
    /// The sentence a contest appends; `None` for a dyn binding's, whose seam
    /// takes none.
    remedy: Option<&'static str>,
}

/// Appended to a contest between two dyn bindings, neither of which carries a
/// port's own sentence.
const DYN_BINDING_REMEDY: &str =
    "A trait object has one implementation: import one module binding it, not both.";

#[derive(Clone)]
pub(crate) struct MetaEntry {
    pub(crate) provider_type_id: Option<TypeId>,
    pub(crate) meta: AnyArc,
}

/// Identity of one built [`Container`], handed out in construction order.
///
/// Clones share their id; every distinct construction gets a fresh one. Ids are
/// never recycled, so a cache may key on one without a pointer's address-reuse
/// hazard.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ContainerId(u64);

/// The single flat provider registry. Cheap to clone — every field is an
/// [`Arc`], so a clone shares the same immutable maps. Built once by
/// [`ContainerBuilder::build`] and thereafter read-only.
#[derive(Clone)]
pub struct Container {
    id: ContainerId,
    providers: Arc<HashMap<ProviderKey, AnyArc>>,
    metadata: Arc<HashMap<TypeId, Vec<MetaEntry>>>,
    scoped: Arc<HashMap<TypeId, ScopedFactory>>,
    transient: Arc<HashMap<TypeId, TransientFactory>>,
    /// The first refusal of a site a transport composed against this
    /// container — a mount has no `Result` to return it through.
    refusal: Arc<Mutex<Option<anyhow::Error>>>,
    /// What this container's sites composed, dropped with it.
    site_chains: Arc<SiteChains>,
}

impl Default for Container {
    fn default() -> Self {
        Self {
            id: next_container_id(),
            providers: Arc::default(),
            metadata: Arc::default(),
            scoped: Arc::default(),
            transient: Arc::default(),
            refusal: Arc::default(),
            site_chains: Arc::default(),
        }
    }
}

fn next_container_id() -> ContainerId {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    ContainerId(NEXT.fetch_add(1, Ordering::Relaxed))
}

impl Container {
    /// This registry's identity — see [`ContainerId`].
    pub fn id(&self) -> ContainerId {
        self.id
    }

    /// Whether any provider is request-scoped or transient — i.e. whether a
    /// [`RequestScope`] over this container has anything to build or cache; a
    /// transport may skip minting one per request when it is `false`.
    pub fn has_dynamic_scopes(&self) -> bool {
        !self.scoped.is_empty() || !self.transient.is_empty()
    }

    /// Start an empty [`ContainerBuilder`] to register providers into.
    pub fn builder() -> ContainerBuilder {
        ContainerBuilder::default()
    }

    /// Resolve a provider by type. Returns `None` if no provider was registered.
    ///
    /// Bypasses the build-time access contract ([`crate::access`]) — prefer
    /// declarative `#[inject]`. A transient provider is rebuilt on every call.
    ///
    /// # Panics
    ///
    /// When a transient provider (transitively) depends on itself.
    pub fn get<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        let id = TypeId::of::<T>();
        if let Some(factory) = self.transient.get(&id) {
            // A throwaway scope, so a request-scoped dep still resolves.
            let scope = RequestScope::new(self.clone());
            let any = build_transient(id, std::any::type_name::<T>(), factory, &scope);
            return any.downcast::<T>().ok();
        }
        self.providers
            .get(&ProviderKey::of(id))
            .and_then(|any| any.clone().downcast::<T>().ok())
    }

    /// Resolve a **keyed** singleton registered via
    /// [`ContainerBuilder::provide_keyed`]; `None` if nothing is registered
    /// under `(T, name)`.
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

    /// Whether a singleton is registered under the type `id` itself.
    pub(crate) fn holds(&self, id: TypeId) -> bool {
        self.providers.contains_key(&ProviderKey::of(id))
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

/// Resolve a transient provider, panicking on a cycle with the chain it
/// closes ([`CycleGuard`]).
pub(crate) fn build_transient(
    id: TypeId,
    type_name: &'static str,
    factory: &TransientFactory,
    scope: &RequestScope,
) -> AnyArc {
    #[expect(clippy::panic, reason = "a provider cycle is a wiring defect, and building a transient has no Result to report it through")]
    let _guard = CycleGuard::push(&TRANSIENT_BUILDING, id, type_name).unwrap_or_else(|Cycle { chain }| {
        panic!(
            "transient provider cycle: {chain} — break the cycle by injecting `Arc<dyn Trait>` or picking a different scope"
        )
    });
    factory(scope)
}

/// The trait object a dyn binding makes of `value`. One already bound — a
/// seed — wins over it, as a seed of `T` wins over the factory itself.
fn install_dyn_only<T, D>(
    builder: ContainerBuilder,
    value: T,
    bind: fn(T) -> Arc<D>,
) -> ContainerBuilder
where
    T: Any,
    D: ?Sized + Send + Sync + 'static,
{
    if builder.contains(TypeId::of::<Arc<D>>()) {
        builder
    } else {
        builder.provide_dyn(bind(value))
    }
}

/// Mutable staging area for the container: providers, metadata and scoped
/// factories accumulate here across the build phases, then [`build`](Self::build)
/// freezes them into an immutable [`Container`].
#[derive(Default)]
pub struct ContainerBuilder {
    providers: HashMap<ProviderKey, AnyArc>,
    metadata: HashMap<TypeId, Vec<MetaEntry>>,
    /// The boot phase [`import`](Self::import) runs a module's.
    phase: Phase,
    /// Idempotency for the register phase — a diamond import registers once.
    registered_modules: HashSet<TypeId>,
    /// Idempotency for the collect phase.
    collected_modules: HashSet<TypeId>,
    /// Every module collected, in order, for the boot to name one no register
    /// phase reached.
    collected_order: Vec<(TypeId, &'static str)>,
    /// Builder-only: drained by [`AppBuilder::build`](crate::AppBuilder::build),
    /// never copied into the [`Container`] or a [`snapshot`](Self::snapshot).
    factories: Vec<QueuedFactory>,
    /// Types queued by [`provide_declared_factory`](Self::provide_declared_factory)
    /// and the trait objects a dyn factory binds, each with the import that
    /// declared it: a declaration supersedes a queued default, and a second
    /// declaration names the first.
    declared_factories: HashMap<TypeId, Declaration>,
    /// Two declarations for one type
    /// ([`ContestedDeclarationError`](crate::ContestedDeclarationError)).
    contested_factories: Vec<crate::ContestedDeclarationError>,
    /// The first error a `register` refused the boot with — see
    /// [`refuse`](Self::refuse).
    refusal: Option<anyhow::Error>,
    /// The imports whose phase is running, innermost last — what a declaration
    /// made now is named by.
    import_sites: Vec<ImportSite>,
    scoped: HashMap<TypeId, ScopedFactory>,
    transient: HashMap<TypeId, TransientFactory>,
    /// Unkeyed registrations that replaced an earlier one
    /// ([`duplicate_providers`](Self::duplicate_providers)).
    duplicates: Vec<DuplicateProvider>,
    /// Dynamic imports (`Foo::for_root(opts)`) the collect phase constructed,
    /// keyed by import site so the register phase consumes the same value.
    dynamic_registrars: HashMap<DynamicImportSite, Registrar>,
}

/// The boot phase a builder is in, which decides what
/// [`ContainerBuilder::import`] runs of a module. A builder made outside a boot
/// registers.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Modules queue the async factories they declare.
    Collect,
    /// Modules build their providers, every factory's output present.
    #[default]
    Register,
}

/// One `#[module(imports = [...])]` entry: the importing module's type plus its
/// position in the list.
type DynamicImportSite = (TypeId, usize);

/// An unkeyed provider registered more than once; a keyed one warns and the
/// last write wins.
#[derive(Clone, Copy)]
pub(crate) struct DuplicateProvider {
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
    /// [`AppBuilder::override_arc`](crate::AppBuilder::override_arc).
    pub(crate) fn replace_arc<T: Any + Send + Sync>(mut self, value: Arc<T>) -> Self {
        self.providers.insert(ProviderKey::typed::<T>(), value);
        self
    }

    /// A **bare** registration — a concrete type or a trait object — that
    /// replaces an earlier one: two modules (or a seed and a module) registering
    /// the same type by mistake. [`App`](crate::App) reads
    /// [`duplicate_providers`](Self::duplicate_providers) after the register
    /// phase and **fails the boot**, uniform with every other wiring error. The
    /// test override path ([`replace`](Self::replace),
    /// [`replace_dyn`](Self::replace_dyn)) does not route here.
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
                target: crate::target::CONTAINER,
                provider = type_name,
                key = name,
                "keyed provider override",
            ),
        }
    }

    /// Unkeyed providers registered more than once, for the boot to refuse.
    pub(crate) fn duplicate_providers(&self) -> &[DuplicateProvider] {
        &self.duplicates
    }

    /// Warn when a singleton registration shadows an existing transient factory
    /// of the same `TypeId`: `Container::get` checks `transient` first, so the
    /// singleton would be unreachable.
    fn warn_if_cross_kind_singleton(&self, id: TypeId, type_name: &'static str) {
        if self.transient.contains_key(&id) {
            tracing::warn!(
                target: crate::target::CONTAINER,
                provider = type_name,
                existing_kind = "transient",
                new_kind = "singleton",
                "provider scope conflict",
            );
        }
    }

    /// Warn when a transient registration shadows an existing singleton of the
    /// same `TypeId`, leaving the singleton unreachable.
    fn warn_if_cross_kind_transient(&self, id: TypeId, type_name: &'static str) {
        if self.providers.contains_key(&ProviderKey::of(id)) {
            tracing::warn!(
                target: crate::target::CONTAINER,
                provider = type_name,
                existing_kind = "singleton",
                new_kind = "transient",
                "provider scope conflict",
            );
        }
    }

    /// Register a trait-object provider, stored as `Arc<Arc<T>>` so the outer
    /// `Arc` is sized.
    ///
    /// A second binding of `T` is a duplicate the boot refuses
    /// ([`DuplicateProviderError`](crate::DuplicateProviderError)); a test swaps
    /// one with [`AppBuilder::override_dyn`](crate::AppBuilder::override_dyn).
    pub fn provide_dyn<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        let key = ProviderKey::of(TypeId::of::<Arc<T>>());
        self.record_if_replacing(&key, std::any::type_name::<Arc<T>>());
        self.providers.insert(key, Arc::new(value));
        self
    }

    /// Replace a trait-object provider without the duplicate check.
    pub(crate) fn replace_dyn<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.providers
            .insert(ProviderKey::of(TypeId::of::<Arc<T>>()), Arc::new(value));
        self
    }

    /// Attach metadata of type `M` to the provider type `P`, discovered via
    /// [`crate::Discovery::meta`].
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

    /// Metadata of type `M` attached **so far**, in attach order — the mid-build
    /// read of what [`Discovery::meta`](crate::Discovery::meta) exposes once the
    /// container is frozen.
    pub fn attached_meta<M: Any + Send + Sync>(&self) -> impl Iterator<Item = &M> {
        self.metadata
            .get(&TypeId::of::<M>())
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.meta.downcast_ref::<M>())
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

    /// Fill the registry `name` from the assembled container once, through
    /// `wire`, after the seal and before any transport is built or any lifecycle
    /// hook runs — so a hook already reads it full.
    ///
    /// `name` is the registry as a boot failure names it
    /// (`"nest_rs::events::listeners"`); the first `wire` that fails ends the
    /// boot with [`WiringFailedError`](crate::WiringFailedError). Wirings run in
    /// registration order, and `wire` is synchronous, so none waits on I/O.
    ///
    /// ```
    /// use nest_rs_core::{App, ContainerBuilder, Module, Registering};
    ///
    /// struct RecipesModule;
    ///
    /// impl Module for RecipesModule {
    ///     fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
    ///         builder.provide_wiring("acme::recipes::catalog", |_container| Ok(()))
    ///     }
    /// }
    ///
    /// assert!(App::new::<RecipesModule>().is_ok());
    /// ```
    pub fn provide_wiring(
        self,
        name: &'static str,
        wire: fn(&Container) -> anyhow::Result<()>,
    ) -> Self {
        self.provide_meta(crate::wiring::WiringContribution::new(name, wire))
    }

    /// Refuse the boot from a `register`, which has no `Result` to return: the
    /// boot fails with `error` once the register phase ends. The first refusal
    /// is the one reported.
    pub fn refuse(mut self, error: impl Into<anyhow::Error>) -> Self {
        if self.refusal.is_none() {
            self.refusal = Some(error.into());
        }
        self
    }

    /// The refusal [`refuse`](Self::refuse) filed, taken by the boot as the
    /// register phase ends.
    pub(crate) fn take_refusal(&mut self) -> Option<anyhow::Error> {
        self.refusal.take()
    }

    /// Whether a provider for `id` has already been registered.
    pub fn contains(&self, id: TypeId) -> bool {
        self.providers.contains_key(&ProviderKey::of(id))
    }

    /// The singleton registered so far under `T`, without snapshotting.
    pub(crate) fn get<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.providers
            .get(&ProviderKey::typed::<T>())
            .and_then(|any| any.clone().downcast::<T>().ok())
    }

    /// Import the module `M`: in the collect phase its
    /// [`collect`](Module::collect), in the register phase its
    /// [`register`](Module::register) — each once per app, `collect` first. A
    /// module first imported in the register phase is collected too late for a
    /// factory ([`LateFactoryError`](crate::LateFactoryError)), so a
    /// hand-written module imports in both of its phases.
    #[must_use]
    pub fn import<M: Module>(mut self) -> Self {
        let id = TypeId::of::<M>();
        // Marked before the phase runs, so an import cycle ends.
        if self.collected_modules.insert(id) {
            self.collected_order.push((id, std::any::type_name::<M>()));
            let phase = std::mem::replace(&mut self.phase, Phase::Collect);
            self = M::collect(self, Collecting::new());
            self.phase = phase;
        }
        if self.phase == Phase::Register && self.registered_modules.insert(id) {
            self = M::register(self, Registering::new());
        }
        self
    }

    /// Enter the boot phase `phase`, which [`import`](Self::import) runs.
    pub(crate) fn enter_phase(mut self, phase: Phase) -> Self {
        self.phase = phase;
        self
    }

    /// Queue an async factory whose awaited output is stored as a provider
    /// (injectable as `Arc<T>`), drained before providers are built.
    pub fn provide_factory<T, F, Fut>(self, factory: F) -> Self
    where
        T: Any + Send + Sync,
        F: FnOnce(Container) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.queue(QueueSpec::default(), factory, |builder, value| {
            builder.provide(value)
        })
    }

    /// Queue an async factory whose awaited output is bound **twice**: as the
    /// concrete `T` and as the `Arc<D>` trait object `bind` derives from it.
    ///
    /// `bind` receives a clone, so `T`'s `Clone` must share the underlying
    /// resource (a pooled or multiplexed handle), not duplicate it.
    ///
    /// `T` is a default; the `Arc<D>` is a **declaration**: another type's
    /// binding of it fails the boot
    /// ([`ContestedDeclarationError`](crate::ContestedDeclarationError)), and a
    /// port's default of it is superseded. A seeded `Arc<D>` keeps its binding.
    pub fn provide_factory_dyn<T, D, F, Fut>(self, factory: F, bind: fn(T) -> Arc<D>) -> Self
    where
        T: Any + Clone + Send + Sync,
        D: ?Sized + Send + Sync + 'static,
        F: FnOnce(Container) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.queue(
            QueueSpec {
                binds: Some(DynBinding::of(bind)),
                ..QueueSpec::default()
            },
            factory,
            |builder, value| builder.provide(value),
        )
    }

    /// Queue a factory that **supersedes** an ordinary one for the same type,
    /// wherever the two happen to fall in import order.
    ///
    /// The plain [`provide_factory`](Self::provide_factory) queue is
    /// first-queued-wins, right for a default several modules declare but wrong
    /// for a value an import site *chose* (`StorageModule::for_root(cfg)`), which
    /// takes the slot instead. Two declarations for one type fail the build
    /// ([`ContestedDeclarationError`](crate::ContestedDeclarationError)), which
    /// appends `remedy`.
    pub fn provide_declared_factory<T, F, Fut>(self, remedy: &'static str, factory: F) -> Self
    where
        T: Any + Send + Sync,
        F: FnOnce(Container) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.queue(
            QueueSpec {
                remedy: Some(remedy),
                ..QueueSpec::default()
            },
            factory,
            |builder, value| builder.provide(value),
        )
    }

    /// [`provide_factory`](Self::provide_factory) for a factory that reads
    /// another factory's output — `After` — from its snapshot. The drain runs it
    /// once `After` is present, wherever the two fall in `imports = [..]`.
    pub fn provide_factory_after<T, After, F, Fut>(self, factory: F) -> Self
    where
        T: Any + Send + Sync,
        After: Any,
        F: FnOnce(Container) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.queue(
            QueueSpec {
                after: vec![TypeId::of::<After>()],
                ..QueueSpec::default()
            },
            factory,
            |builder, value| builder.provide(value),
        )
    }

    /// [`provide_declared_factory`](Self::provide_declared_factory) for a
    /// factory that reads another factory's output — `After` — from its
    /// snapshot. See [`provide_factory_after`](Self::provide_factory_after).
    pub fn provide_declared_factory_after<T, After, F, Fut>(
        self,
        remedy: &'static str,
        factory: F,
    ) -> Self
    where
        T: Any + Send + Sync,
        After: Any,
        F: FnOnce(Container) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.queue(
            QueueSpec {
                remedy: Some(remedy),
                after: vec![TypeId::of::<After>()],
                ..QueueSpec::default()
            },
            factory,
            |builder, value| builder.provide(value),
        )
    }

    /// [`provide_declared_factory_after`](Self::provide_declared_factory_after)
    /// for a factory that reads two other factories' outputs, `A` and `B`.
    pub fn provide_declared_factory_after_both<T, A, B, F, Fut>(
        self,
        remedy: &'static str,
        factory: F,
    ) -> Self
    where
        T: Any + Send + Sync,
        A: Any,
        B: Any,
        F: FnOnce(Container) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.queue(
            QueueSpec {
                remedy: Some(remedy),
                after: vec![TypeId::of::<A>(), TypeId::of::<B>()],
                ..QueueSpec::default()
            },
            factory,
            |builder, value| builder.provide(value),
        )
    }

    /// [`provide_factory_dyn`](Self::provide_factory_dyn) for a factory that
    /// reads another factory's output — `After` — from its snapshot. See
    /// [`provide_declared_factory_after`](Self::provide_declared_factory_after).
    pub fn provide_factory_dyn_after<T, D, After, F, Fut>(
        self,
        factory: F,
        bind: fn(T) -> Arc<D>,
    ) -> Self
    where
        T: Any + Clone + Send + Sync,
        D: ?Sized + Send + Sync + 'static,
        After: Any,
        F: FnOnce(Container) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.queue(
            QueueSpec {
                binds: Some(DynBinding::of(bind)),
                after: vec![TypeId::of::<After>()],
                ..QueueSpec::default()
            },
            factory,
            |builder, value| builder.provide(value),
        )
    }

    /// The factory-queue protocol every public form shares; the forms differ
    /// only by [`QueueSpec`].
    fn queue<T, F, Fut, I>(mut self, spec: QueueSpec, factory: F, install: I) -> Self
    where
        T: Any + Send + Sync,
        F: FnOnce(Container) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
        I: FnOnce(ContainerBuilder, T) -> ContainerBuilder + Send + 'static,
    {
        let QueueSpec {
            remedy,
            mut binds,
            after,
        } = spec;
        let id = TypeId::of::<T>();
        let name = std::any::type_name::<T>();
        let mut declares = Vec::new();
        if let Some(remedy) = remedy {
            declares.push((id, name, Some(remedy)));
        }
        if let Some(binding) = &binds {
            declares.push((binding.id, binding.name, None));
        }
        for &(key, key_name, remedy) in &declares {
            let site = self.declaring_site();
            match self.declared_factories.get(&key) {
                // The same module's binding, queued by a second importer.
                Some(first) if remedy.is_none() && first.binder == id => {}
                Some(first) => {
                    let remedy = remedy.or(first.remedy).unwrap_or(DYN_BINDING_REMEDY);
                    self.contested_factories
                        .push(crate::ContestedDeclarationError {
                            type_name: key_name,
                            first: first.site.clone(),
                            second: site,
                            remedy,
                        });
                    return self;
                }
                None => {
                    self.declared_factories.insert(
                        key,
                        Declaration {
                            site,
                            binder: id,
                            remedy,
                        },
                    );
                }
            }
        }
        // A default never displaces a declaration, whichever arrives first; what
        // it binds off `T` binds off the declared one.
        if remedy.is_none() && self.declared_factories.contains_key(&id) {
            let Some(binding) = binds else {
                return self;
            };
            if let Some(declarer) = self.factories.iter_mut().find(|q| q.provides.contains(&id)) {
                declarer.provides.push(binding.id);
                declarer.derives.push(binding.derive);
                // The trait object is declared now, so its port's default
                // leaves the queue, as it does beside the binding's own entry.
                self.factories.retain(|q| q.id() != binding.id);
                return self;
            }
            // Past the factory phase nothing builds `T` again: queued, the
            // binding is refused as late rather than lost.
            binds = Some(binding);
        }
        let boxed: BoxedFactory = Box::new(move |container| {
            Box::pin(async move {
                let value = factory(container).await?;
                let registrar: Registrar = Box::new(move |builder| install(builder, value));
                Ok(registrar)
            })
        });
        let mut queued = QueuedFactory {
            name,
            provides: vec![id],
            after,
            declared: remedy.is_some(),
            derives: Vec::new(),
            factory: boxed,
        };
        if let Some(binding) = binds {
            queued.provides.push(binding.id);
            queued.derives.push(binding.derive);
        }
        // A declaration takes the displaced default's slot: a factory queued
        // after that default reads its output, and would otherwise run first.
        let displaced = |q: &QueuedFactory| declares.iter().any(|&(key, ..)| q.id() == key);
        let slot = self.factories.iter().position(displaced);
        let mut kept = Vec::with_capacity(self.factories.len());
        for q in std::mem::take(&mut self.factories) {
            if displaced(&q) {
                queued.provides.extend(q.provides.into_iter().skip(1));
                queued.derives.extend(q.derives);
            } else {
                kept.push(q);
            }
        }
        self.factories = kept;
        match slot {
            Some(slot) => self.factories.insert(slot, queued),
            None => self.factories.push(queued),
        }
        self
    }

    /// Types declared by more than one import site, each naming both sites.
    /// Checked by `AppBuilder::build` before any factory runs.
    pub(crate) fn contested_factories(&self) -> &[crate::ContestedDeclarationError] {
        &self.contested_factories
    }

    /// The innermost import whose phase is running, as a declaration names its
    /// site.
    fn declaring_site(&self) -> String {
        match self.import_sites.last() {
            Some(site) => site.to_string(),
            None => "a call outside any module's imports".to_owned(),
        }
    }

    /// Enter the phase of the root module `root` the app is built from.
    pub(crate) fn enter_root(mut self, root: &'static str) -> Self {
        self.import_sites.push(ImportSite {
            importer: None,
            import: root,
        });
        self
    }

    /// Leave the import (or root) entered last.
    pub(crate) fn leave_import(mut self) -> Self {
        self.import_sites.pop();
        self
    }

    /// Register a request-scoped provider: `factory` builds a fresh `T` for
    /// each request, cached by a [`RequestScope`] — emitted by
    /// `#[injectable(scope = request)]`.
    pub fn provide_scoped<T, F>(mut self, factory: F) -> Self
    where
        T: Any + Send + Sync,
        F: Fn(&RequestScope) -> T + Send + Sync + 'static,
    {
        let id = TypeId::of::<T>();
        if self.scoped.contains_key(&id) {
            tracing::warn!(
                target: crate::target::CONTAINER,
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
    /// `Container::get::<T>()` (or a [`RequestScope`]) resolves it — emitted by
    /// `#[injectable(scope = transient)]`.
    pub fn provide_transient<T, F>(mut self, factory: F) -> Self
    where
        T: Any + Send + Sync,
        F: Fn(&RequestScope) -> T + Send + Sync + 'static,
    {
        let id = TypeId::of::<T>();
        if self.transient.contains_key(&id) {
            tracing::warn!(
                target: crate::target::CONTAINER,
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

    pub(crate) fn take_factories(&mut self) -> Vec<QueuedFactory> {
        std::mem::take(&mut self.factories)
    }

    /// Types a module queued an async factory for.
    pub(crate) fn queued_factory_names(&self) -> Vec<&'static str> {
        self.factories.iter().map(|queued| queued.name).collect()
    }

    /// The first type a factory still queued would provide and nothing does,
    /// read as the register phase ends. A declaration always counts: the value
    /// present is not the one it chose.
    pub(crate) fn late_factory_name(&self) -> Option<&'static str> {
        self.factories
            .iter()
            .find(|queued| {
                queued.declared || queued.provides.iter().any(|key| !self.contains(*key))
            })
            .map(|queued| queued.name)
    }

    /// The first module collected that no register phase reached — imported in
    /// a collect alone — read as the register phase ends.
    pub(crate) fn unregistered_module(&self) -> Option<&'static str> {
        self.collected_order
            .iter()
            .find(|(id, _)| !self.registered_modules.contains(id))
            .map(|&(_, name)| name)
    }

    /// Unkeyed provider keys registered so far — after the factory phase, the
    /// access graph's **global** set.
    pub(crate) fn provider_ids(&self) -> HashSet<TypeId> {
        self.providers
            .keys()
            .filter(|k| k.name.is_none())
            .map(|k| k.type_id)
            .collect()
    }

    /// Every unkeyed `TypeId` resolvable from this builder — singleton values
    /// **plus** request-scoped and transient factories.
    pub(crate) fn registered_ids(&self) -> HashSet<TypeId> {
        self.providers
            .keys()
            .filter(|k| k.name.is_none())
            .map(|k| k.type_id)
            .chain(self.scoped.keys().copied())
            .chain(self.transient.keys().copied())
            .collect()
    }

    /// Every unkeyed `TypeId` registered as a **request-scoped or transient**
    /// factory, which a singleton may not inject.
    pub(crate) fn scoped_or_transient_ids(&self) -> HashSet<TypeId> {
        self.scoped
            .keys()
            .copied()
            .chain(self.transient.keys().copied())
            .collect()
    }

    /// Keyed provider identities registered so far — after the factory phase,
    /// the access graph's **global keyed** set.
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
            id: next_container_id(),
            providers: Arc::new(self.providers),
            metadata: Arc::new(self.metadata),
            scoped: Arc::new(self.scoped),
            transient: Arc::new(self.transient),
            refusal: Arc::default(),
            site_chains: Arc::default(),
        }
    }

    /// Snapshot the providers registered so far, for a provider being built to
    /// resolve its dependencies.
    ///
    /// Deep-clones four maps per call, so boot is O(n²) in provider count.
    pub fn snapshot(&self) -> Container {
        Container {
            id: next_container_id(),
            providers: Arc::new(self.providers.clone()),
            metadata: Arc::new(self.metadata.clone()),
            scoped: Arc::new(self.scoped.clone()),
            transient: Arc::new(self.transient.clone()),
            refusal: Arc::default(),
            site_chains: Arc::default(),
        }
    }
}

/// `container` is public: its tier-2 items live here, reached only
/// through the crate's `__private`.
pub(crate) mod __private {
    use std::any::TypeId;
    use std::sync::PoisonError;

    use super::{Container, ContainerBuilder, ImportSite};
    use crate::layer_chain::__private::SiteChains;
    use crate::module::{Collecting, DynamicModule, Registering};

    /// Refuse the boot from a site a transport composes at `configure` — a route,
    /// a gateway — against the built `container`: a mount has no `Result` to
    /// return, so the site files the refusal, mounts something that denies, and
    /// the boot fails on [`take_site_refusal`] once its transports are
    /// configured. The first refusal is the one kept.
    pub fn refuse_site(container: &Container, error: anyhow::Error) {
        let mut slot = container
            .refusal
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if slot.is_none() {
            *slot = Some(error);
        }
    }

    /// The chains `container`'s sites composed, which it holds for its life —
    /// see [`SiteChains`].
    pub fn site_chains(container: &Container) -> &SiteChains {
        &container.site_chains
    }

    /// The refusal a site filed against `container` ([`refuse_site`]), taken by
    /// whatever configured its transports, to end the boot with.
    pub fn take_site_refusal(container: &Container) -> Option<anyhow::Error> {
        container
            .refusal
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// Collect phase for one dynamic import: run its
    /// [`DynamicModule::collect`], then park the value for
    /// [`register_dynamic_import`].
    pub fn collect_dynamic_import<D>(
        mut builder: ContainerBuilder,
        module: TypeId,
        index: usize,
        value: D,
    ) -> ContainerBuilder
    where
        D: DynamicModule + Send + 'static,
    {
        builder = value.collect(builder, Collecting::new());
        builder.dynamic_registrars.insert(
            (module, index),
            Box::new(move |builder| value.register(builder, Registering::new())),
        );
        builder
    }

    /// Register phase for one dynamic import: consume the value the collect
    /// phase parked at this site; a site with none is refused naming it.
    pub fn register_dynamic_import(
        mut builder: ContainerBuilder,
        module: TypeId,
        index: usize,
    ) -> ContainerBuilder {
        match builder.dynamic_registrars.remove(&(module, index)) {
            Some(registrar) => registrar(builder),
            None => {
                let site = builder.declaring_site();
                builder.refuse(crate::error::UncollectedImportError { site })
            }
        }
    }

    /// Enter the phase of the import `import` — written as it stands at `at` in
    /// `importer`'s `imports = [..]` — so a declaration it makes can be named.
    /// `#[module]` emits it around each import.
    pub fn enter_import(
        mut builder: ContainerBuilder,
        importer: &'static str,
        at: usize,
        import: &'static str,
    ) -> ContainerBuilder {
        builder.import_sites.push(ImportSite {
            importer: Some((importer, at)),
            import,
        });
        builder
    }

    /// Leave the import [`enter_import`] entered last.
    pub fn leave_import(builder: ContainerBuilder) -> ContainerBuilder {
        builder.leave_import()
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
        // The boot, not the builder, refuses the duplicate.
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
    fn a_second_dyn_binding_is_a_duplicate() {
        let polite: Arc<dyn Hello + Send + Sync> = Arc::new(Polite);
        let curt: Arc<dyn Hello + Send + Sync> = Arc::new(Curt);
        let builder = Container::builder().provide_dyn(polite).provide_dyn(curt);
        let names: Vec<&str> = builder
            .duplicate_providers()
            .iter()
            .map(|duplicate| duplicate.type_name)
            .collect();
        assert_eq!(
            names,
            [std::any::type_name::<Arc<dyn Hello + Send + Sync>>()]
        );
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

    /// A module imported twice in each phase runs each once, `collect` first,
    /// and one first imported in the register phase is collected there.
    #[test]
    fn an_import_runs_a_modules_collect_then_its_register_once_each() {
        static RAN: std::sync::Mutex<Vec<&str>> = std::sync::Mutex::new(Vec::new());
        struct Counted;
        impl Module for Counted {
            fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
                RAN.lock().expect("ran").push("collect");
                builder
            }
            fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
                RAN.lock().expect("ran").push("register");
                builder
            }
        }
        let builder = Container::builder()
            .enter_phase(Phase::Collect)
            .import::<Counted>()
            .import::<Counted>();
        assert_eq!(*RAN.lock().expect("ran"), ["collect"]);
        let _registered = builder
            .enter_phase(Phase::Register)
            .import::<Counted>()
            .import::<Counted>();
        assert_eq!(*RAN.lock().expect("ran"), ["collect", "register"]);

        RAN.lock().expect("ran").clear();
        let _registered = Container::builder().import::<Counted>();
        assert_eq!(*RAN.lock().expect("ran"), ["collect", "register"]);
    }

    #[test]
    fn a_dynamic_import_site_with_no_parked_value_is_refused_naming_it() {
        let builder = __private::enter_import(
            Container::builder(),
            "AppModule",
            0,
            "SomeModule::for_root(..)",
        );
        let mut builder = __private::register_dynamic_import(builder, TypeId::of::<Host>(), 0);
        let refused = builder.take_refusal().expect("a refusal");
        assert!(
            refused
                .to_string()
                .contains("`SomeModule::for_root(..)` at `imports[0]` of `AppModule`"),
            "{refused}"
        );
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
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn transient_factory_reads_singleton_deps() {
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
                let _self: Arc<Counter> = c.get().expect("re-entrant resolution");
                Counter(0)
            })
            .build();
        let _ = container.get::<Counter>();
    }

    #[test]
    fn transient_transitive_cycle_diagnostic_lists_full_chain() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let container = Container::builder()
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
        let resolved: Arc<Counter> = container.get().unwrap();
        assert_eq!(resolved.0, 2);
    }

    /// The `tracing` lines emitted on the calling thread while `f` runs.
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
        let container = Container::builder()
            .provide_transient(|_| -> Counter { panic!("boom from factory") })
            .provide_transient(|_| Greeter("recovered"))
            .build();

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

        let resolved: Arc<Greeter> = container
            .get()
            .expect("different transient resolves after a sibling factory panicked");
        assert_eq!(resolved.0, "recovered");
    }
}

//! The IoC [`Container`] and its [`ContainerBuilder`] — the single flat
//! registry keyed by `TypeId` that every provider resolves through, plus
//! the request-scoped and transient factory machinery layered on top of it.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::RequestScope;
use crate::access::{
    ModuleDescriptor, reachable_descriptors, scoped_cycle, singleton_cycle, unbuilt_cause,
};
use crate::close::InstallRecord;
use crate::cycle_guard::{BuildStack, Cycle, CycleGuard};
use crate::error::FactoryKind;
use crate::factory_inputs::FactoryInputs;
use crate::layer_chain::__private::SiteChains;
use crate::module::{Collecting, Module, Registering};
use crate::net::check_budgets;

type AnyArc = Arc<dyn Any + Send + Sync>;

/// How long one instance of a provider lives — the values of
/// `#[injectable(scope = …)]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProviderScope {
    /// One instance for the app's lifetime, built at boot.
    Singleton,
    /// One instance per request, built from the request's scope.
    Request,
    /// A fresh instance on every resolution.
    Transient,
}

impl ProviderScope {
    /// How a registration under this scope reads in a sentence: "first as a
    /// singleton, then request-scoped".
    pub(crate) fn as_registered(self) -> &'static str {
        match self {
            Self::Singleton => "as a singleton",
            Self::Request => "request-scoped",
            Self::Transient => "transient",
        }
    }

    /// How a provider of this scope reads before a noun: "a request-scoped
    /// provider cycle".
    pub(crate) fn as_adjective(self) -> &'static str {
        match self {
            Self::Singleton => "singleton",
            Self::Request => "request-scoped",
            Self::Transient => "transient",
        }
    }
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
    /// Its place among every factory the builder queued, so the entries one
    /// `collect` queued are told from the rest wherever a declaration's slot
    /// put them in the queue.
    pub(crate) seq: u64,
    /// The import whose phase queued it — `None` at the app root — which its
    /// output's registration is named by.
    pub(crate) queued_by: Option<ImportSite>,
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
pub(crate) struct ImportSite {
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

/// Where a registration the builder holds came from, as a duplicate names it.
#[derive(Clone, Copy)]
pub(crate) enum Origin {
    /// Registered before any root is entered: a value `App::builder()` seeded.
    Seed,
    /// Registered on a container built by hand outside any import a
    /// `#[module]` entered: a call on the builder, or a module it imported.
    Hand,
    /// Registered while this import's phase ran.
    Import(ImportSite),
    /// A factory's output, queued by this import — `None` at the app root.
    Factory(Option<ImportSite>),
    /// A test's override, installed before any module collects.
    Override,
}

impl std::fmt::Display for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Seed => f.write_str("a seed (`App::builder().provide…`)"),
            Self::Hand => {
                f.write_str("a call on a container built by hand (`Container::builder()…`)")
            }
            Self::Import(site) => site.fmt(f),
            Self::Factory(Some(site)) => write!(f, "a factory queued by {site}"),
            Self::Factory(None) => {
                f.write_str("a factory queued by `App::builder().provide_factory`")
            }
            Self::Override => f.write_str("a test override (`App::builder().override_…`)"),
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
    providers: Arc<HashMap<TypeId, AnyArc>>,
    metadata: Arc<HashMap<TypeId, Vec<MetaEntry>>>,
    scoped: Arc<HashMap<TypeId, ScopedFactory>>,
    transient: Arc<HashMap<TypeId, TransientFactory>>,
    /// One `Arc`, not one per field: a request scope clones the container.
    sites: Arc<ComposedSites>,
}

/// What the sites a transport composes against a container leave on it.
#[derive(Default)]
struct ComposedSites {
    /// The first refusal of a site — a mount has no `Result` to return it
    /// through.
    refusal: Mutex<Option<anyhow::Error>>,
    /// What the sites composed, dropped with the container.
    chains: SiteChains,
}

impl Default for Container {
    fn default() -> Self {
        Self {
            id: next_container_id(),
            providers: Arc::default(),
            metadata: Arc::default(),
            scoped: Arc::default(),
            transient: Arc::default(),
            sites: Arc::default(),
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
    ///
    /// It drains no factory, so a module whose `collect` queues one is refused
    /// there as [`ContainerBuilder::import`] says, and
    /// [`ContainerBuilder::build`] returns the refusal;
    /// [`App::builder`](crate::App::builder) is the boot that drains them.
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
            .get(&id)
            .and_then(|any| any.clone().downcast::<T>().ok())
    }

    /// Resolve a trait-object provider registered via
    /// [`ContainerBuilder::provide_dyn`]. Same unchecked-escape-hatch caveat as
    /// [`get`](Self::get).
    pub fn get_dyn<T: ?Sized + Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.providers
            .get(&TypeId::of::<Arc<T>>())
            .and_then(|any| any.clone().downcast::<Arc<T>>().ok())
            .map(|outer| (*outer).clone())
    }

    /// Whether a singleton is registered under the type `id` itself.
    pub(crate) fn holds(&self, id: TypeId) -> bool {
        self.providers.contains_key(&id)
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
    #[expect(
        clippy::panic,
        reason = "a provider cycle is a wiring defect, and building a transient has no Result to report it through"
    )]
    let _guard =
        CycleGuard::push(&TRANSIENT_BUILDING, id, type_name).unwrap_or_else(|Cycle { chain }| {
            panic!(
                "{}",
                crate::error::cycle_panic_message(ProviderScope::Transient, &chain)
            )
        });
    factory(scope)
}

/// The trait object a dyn binding makes of `value`. One already bound — a
/// seed, a test's override — wins over it, as a seed of `T` wins over the
/// factory itself.
fn install_dyn_only<T, D>(
    builder: ContainerBuilder,
    value: T,
    bind: fn(T) -> Arc<D>,
) -> ContainerBuilder
where
    T: Any,
    D: ?Sized + Send + Sync + 'static,
{
    let key = TypeId::of::<Arc<D>>();
    if builder.contains(key) {
        builder.stand_down(key)
    } else {
        builder.provide_dyn(bind(value))
    }
}

/// Mutable staging area for the container: providers, metadata and scoped
/// factories accumulate here across the build phases, then [`build`](Self::build)
/// freezes them into an immutable [`Container`].
#[derive(Default)]
pub struct ContainerBuilder {
    providers: HashMap<TypeId, AnyArc>,
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
    /// The [`seq`](QueuedFactory::seq) the next factory queued takes.
    next_factory_seq: u64,
    /// Types queued by [`provide_declared_factory`](Self::provide_declared_factory)
    /// and the trait objects a dyn factory binds, each with the import that
    /// declared it: a declaration supersedes a queued default, and a second
    /// declaration names the first.
    declared_factories: HashMap<TypeId, Declaration>,
    /// Two declarations for one type, a transport type attached twice among
    /// them ([`ContestedDeclarationError`](crate::ContestedDeclarationError)).
    contested: Vec<crate::ContestedDeclarationError>,
    /// Each transport type attached, with the import that attached it: an app
    /// runs one transport of each type.
    transports: HashMap<TypeId, String>,
    /// The first refusal filed on the builder: a `register`'s own
    /// ([`refuse`](Self::refuse)), or the kernel's for a module it left out.
    refusal: Option<crate::ContainerError>,
    /// Whether a boot drains this builder's factories and reads its refusals:
    /// `false` for a container built by hand.
    booting: bool,
    /// The imports whose phase is running, innermost last — what a declaration
    /// made now is named by.
    import_sites: Vec<ImportSite>,
    scoped: HashMap<TypeId, ScopedFactory>,
    transient: HashMap<TypeId, TransientFactory>,
    /// The first registration of each type, whatever its scope, so a second
    /// names both.
    origins: HashMap<TypeId, (Origin, ProviderScope)>,
    /// Set while the factory drain installs a factory's output
    /// ([`install`](Self::install)): the registrations it makes are that
    /// factory's.
    installing: Option<Origin>,
    /// Second registrations of one type, refused once the register phase
    /// ends.
    duplicates: Vec<DuplicateProvider>,
    /// Dynamic imports (`Foo::for_root(opts)`) the collect phase constructed,
    /// keyed by import site so the register phase consumes the same value.
    dynamic_registrars: HashMap<DynamicImportSite, Registrar>,
    /// The providers a `#[module]`'s register step could not build, root
    /// causes first, for the seal to refuse.
    stalls: Vec<Stall>,
    /// The providers a `#[module]`'s register step built while an optional
    /// dependency was absent, for the seal to refuse when it arrived later.
    absent_optionals: Vec<AbsentOptional>,
    /// The keys a test overrode, in the order the test gave them, each claimed
    /// once whatever would have registered it stood down.
    overrides: Vec<Override>,
    /// The providers a `#[module]` never built because a test overrode one of
    /// their keys, each with the keys the override left absent.
    overridden_providers: Vec<OverriddenProvider>,
    /// The checks a seed of a factory's output passes before anything reads
    /// it, by the output's type ([`__private::check_seed`]).
    seed_checks: HashMap<TypeId, SeedCheck>,
    /// Every type in the order its first registration installed it, the
    /// reverse of which the closes run in.
    installed: Vec<TypeId>,
}

/// A provider a `#[module]`'s register step built without some of its
/// optional dependencies.
pub(crate) struct AbsentOptional {
    /// The module listing it, by type and by name.
    pub(crate) module: (TypeId, &'static str),
    /// The provider, by its own type and as its module lists it.
    pub(crate) provider: (TypeId, &'static str),
    /// The optional dependencies absent when it was built, each with its
    /// label.
    pub(crate) absent: Vec<(TypeId, &'static str)>,
}

/// A provider a `#[module]`'s register step could not build: what it waited
/// on was absent when its module's providers were built.
pub(crate) struct Stall {
    /// The module listing it, by type and by name.
    pub(crate) module: (TypeId, &'static str),
    /// The provider, as its module lists it.
    pub(crate) provider: &'static str,
    /// Every key it would have registered.
    pub(crate) provides: Vec<TypeId>,
    /// What it waited on, each with its label.
    pub(crate) missing: Vec<(TypeId, &'static str)>,
}

/// One key a test overrode: the double sits under it from before any module
/// collects, and whatever would have registered it claims it instead.
pub(crate) struct Override {
    /// The overridden key.
    pub(crate) key: TypeId,
    /// The key's type, as a boot error names it.
    pub(crate) type_name: &'static str,
    /// Whether a seed, a factory or a provider would have registered it.
    pub(crate) claimed: bool,
}

/// A provider a `#[module]` lists that a test override stands in for: it is
/// never built, so its own dependencies go unchecked, and the keys it would
/// have registered beside the overridden one are absent.
pub(crate) struct OverriddenProvider {
    /// The provider's own type, and its name as its module lists it.
    pub(crate) provider: (TypeId, &'static str),
    /// The scope it would have been built under, which its consumers are
    /// still held to.
    pub(crate) scope: ProviderScope,
    /// The key the test overrode it through, by type name.
    pub(crate) through: &'static str,
    /// Its other keys, absent in this app.
    pub(crate) absent: Vec<TypeId>,
}

/// What a seed of a factory's output passes before anything reads it, run
/// against the container the drain has built so far.
pub(crate) type SeedCheck = fn(&Container) -> anyhow::Result<()>;

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

/// A second registration of one type, whatever the two scopes.
#[derive(Clone, Copy)]
pub(crate) struct DuplicateProvider {
    /// Type name of the doubly-registered provider.
    pub type_name: &'static str,
    first: (Origin, ProviderScope),
    second: (Origin, ProviderScope),
}

impl DuplicateProvider {
    /// The boot error naming both registrations.
    pub(crate) fn error(&self) -> crate::DuplicateProviderError {
        let ((first, first_scope), (second, second_scope)) = (self.first, self.second);
        crate::DuplicateProviderError {
            type_name: self.type_name,
            first: first.to_string(),
            second: second.to_string(),
            scopes: (first_scope != second_scope).then_some((first_scope, second_scope)),
        }
    }
}

impl ContainerBuilder {
    /// A builder a boot drains and reads — [`App::new`](crate::App::new)'s and
    /// [`AppBuilder`](crate::AppBuilder)'s.
    pub(crate) fn booting() -> Self {
        Self {
            booting: true,
            ..Self::default()
        }
    }

    /// Register a value, wrapped in `Arc` internally.
    ///
    /// A type has one provider whatever its scope: a second registration of
    /// `T` is a duplicate the boot refuses
    /// ([`DuplicateProviderError`](crate::DuplicateProviderError)), naming
    /// both.
    pub fn provide<T: Any + Send + Sync>(self, value: T) -> Self {
        self.provide_arc(Arc::new(value))
    }

    /// Register an already-shared `Arc<T>`.
    pub fn provide_arc<T: Any + Send + Sync>(mut self, value: Arc<T>) -> Self {
        self.record::<T>(ProviderScope::Singleton);
        if self.claim(TypeId::of::<T>()).is_some() {
            return self;
        }
        self.providers.insert(TypeId::of::<T>(), value);
        self
    }

    /// Install a test's double under the key `K` before any module collects:
    /// what would register `K` later claims it instead. A seed already there
    /// is replaced, and counts as the claim.
    pub(crate) fn install_override<K: ?Sized + 'static>(mut self, value: AnyArc) -> Self {
        let (key, type_name) = (TypeId::of::<K>(), std::any::type_name::<K>());
        let claimed = self.providers.insert(key, value).is_some();
        if let Entry::Vacant(slot) = self.origins.entry(key) {
            slot.insert((Origin::Override, ProviderScope::Singleton));
            self.installed.push(key);
        }
        match self.overrides.iter_mut().find(|o| o.key == key) {
            Some(first) => first.claimed |= claimed,
            None => self.overrides.push(Override {
                key,
                type_name,
                claimed,
            }),
        }
        self
    }

    /// The overridden type's name when a test overrode `key`, marking the
    /// override claimed: what was about to register it stands down.
    pub(crate) fn claim(&mut self, key: TypeId) -> Option<&'static str> {
        let found = self.overrides.iter_mut().find(|o| o.key == key)?;
        found.claimed = true;
        Some(found.type_name)
    }

    /// What registers now skips `key`, which a seed or a test's override
    /// already holds: it claims the override and takes its place as the key's
    /// registrant, where a seed keeps its own, as without the override.
    pub(crate) fn stand_down(mut self, key: TypeId) -> Self {
        if self.claim(key).is_some() {
            let origin = self.registration_origin();
            if let Some(held @ (Origin::Override, _)) = self.origins.get_mut(&key) {
                *held = (origin, ProviderScope::Singleton);
            }
        }
        self
    }

    /// Whether a test overrode `key`.
    pub(crate) fn overrides(&self, key: TypeId) -> bool {
        self.overrides.iter().any(|o| o.key == key)
    }

    /// The first override nothing claimed, in the order the test gave them.
    pub(crate) fn unclaimed_override(&self) -> Option<&'static str> {
        self.overrides
            .iter()
            .find(|o| !o.claimed)
            .map(|o| o.type_name)
    }

    /// The overridden keys nothing claimed so far: after the factory phase,
    /// those a module's provider will claim, which are not global.
    pub(crate) fn unclaimed_override_ids(&self) -> impl Iterator<Item = TypeId> + '_ {
        self.overrides.iter().filter(|o| !o.claimed).map(|o| o.key)
    }

    /// The providers a test override stands in for.
    pub(crate) fn overridden_providers(&self) -> &[OverriddenProvider] {
        &self.overridden_providers
    }

    /// The check a seed of the factory output `id` passes, if its module
    /// declared one.
    pub(crate) fn seed_check(&self, id: TypeId) -> Option<SeedCheck> {
        self.seed_checks.get(&id).copied()
    }

    /// Note the registration of `T` under `scope` by whatever is registering
    /// now ([`note`](Self::note)).
    fn record<T: ?Sized + 'static>(&mut self, scope: ProviderScope) {
        if self.note(TypeId::of::<T>(), std::any::type_name::<T>(), scope) {
            self.installed.push(TypeId::of::<T>());
        }
    }

    /// Note the registration of `key`, named `type_name`, under `scope` by
    /// whatever is registering now, true when it is the key's first. A test's
    /// override holds the key until the first registration that would have
    /// made it takes its place; a second registration of one type, whatever
    /// the two scopes and with an override or without, is kept for
    /// [`App`](crate::App) to refuse once the register phase ends, naming
    /// both.
    fn note(&mut self, key: TypeId, type_name: &'static str, scope: ProviderScope) -> bool {
        let second = (self.registration_origin(), scope);
        match self.origins.entry(key) {
            Entry::Occupied(mut first) if matches!(first.get().0, Origin::Override) => {
                first.insert(second);
                false
            }
            Entry::Occupied(first) => {
                self.duplicates.push(DuplicateProvider {
                    type_name,
                    first: *first.get(),
                    second,
                });
                false
            }
            Entry::Vacant(slot) => {
                slot.insert(second);
                true
            }
        }
    }

    /// What is registering now: the factory being installed, else the import
    /// whose phase runs, else — before any root is entered — a seed, or a call
    /// on a container built by hand.
    fn registration_origin(&self) -> Origin {
        match (self.installing, self.import_sites.last()) {
            (Some(factory), _) => factory,
            (None, Some(site)) => Origin::Import(*site),
            (None, None) if self.booting => Origin::Seed,
            (None, None) => Origin::Hand,
        }
    }

    /// What the close step reads off the registrations so far: the order each
    /// type was installed in, and the types the app handed in rather than a
    /// module opening them — what it seeded before any root was entered, and
    /// what a test overrode.
    pub(crate) fn install_record(&self) -> InstallRecord {
        let handed_in = self
            .origins
            .iter()
            .filter(|(_, (origin, _))| matches!(origin, Origin::Seed))
            .map(|(id, _)| *id)
            .chain(self.overrides.iter().map(|o| o.key))
            .collect();
        InstallRecord::new(&self.installed, handed_in)
    }

    /// Run `steps` — a factory's output and the trait objects bound off it —
    /// as registrations of the factory `queued_by` queued.
    pub(crate) fn install(
        mut self,
        queued_by: Option<ImportSite>,
        steps: impl IntoIterator<Item = Registrar>,
    ) -> Self {
        let outer = self.installing.replace(Origin::Factory(queued_by));
        for step in steps {
            self = step(self);
        }
        self.installing = outer;
        self
    }

    /// Register a trait-object provider, stored as `Arc<Arc<T>>` so the outer
    /// `Arc` is sized.
    ///
    /// A second binding of `T` is a duplicate the boot refuses
    /// ([`DuplicateProviderError`](crate::DuplicateProviderError)); a test swaps
    /// one with [`AppBuilder::override_dyn`](crate::AppBuilder::override_dyn).
    pub fn provide_dyn<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.record::<Arc<T>>(ProviderScope::Singleton);
        if self.claim(TypeId::of::<Arc<T>>()).is_some() {
            return self;
        }
        self.providers
            .insert(TypeId::of::<Arc<T>>(), Arc::new(value));
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

    /// Attach a transport this module serves through: [`App::run`](crate::App::run)
    /// builds it from the assembled container with `build`, configures it, has
    /// it listen, and serves it until the app stops. Its boot lines name it by
    /// its short type name, read off `T`.
    ///
    /// An app runs one transport of each type: a second `T`, attached by any
    /// module, fails the boot naming both imports
    /// ([`ContestedDeclarationError`](crate::ContestedDeclarationError)) — the
    /// module that attaches it is imported by type, so a second import of it
    /// attaches nothing. A transport generic over a marker (`Consumer<Orders>`)
    /// is one type per marker.
    ///
    /// `build` is a function, never a closure holding a value: what the
    /// transport needs is read from the container, where the module's
    /// `for_root` value has landed — as HTTP reads its `HttpConfig`.
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use nest_rs_core::{App, Container, ContainerBuilder, Discovery, Module, Registering, Transport, TransportContribution, async_trait};
    /// use tokio_util::sync::CancellationToken;
    ///
    /// struct MetricsPort;
    ///
    /// #[async_trait]
    /// impl Transport for MetricsPort {
    ///     async fn configure(&mut self, _: &Container) -> anyhow::Result<()> {
    ///         Ok(())
    ///     }
    ///
    ///     async fn serve(self: Box<Self>, cancel: CancellationToken) -> anyhow::Result<()> {
    ///         cancel.cancelled().await;
    ///         Ok(())
    ///     }
    ///
    ///     fn stop_bound(&self) -> Duration {
    ///         Duration::from_secs(1)
    ///     }
    /// }
    ///
    /// struct MetricsModule;
    ///
    /// impl Module for MetricsModule {
    ///     fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
    ///         builder.provide_transport(|_| Ok(MetricsPort))
    ///     }
    /// }
    ///
    /// let app = App::new::<MetricsModule>()?;
    /// let attached = Discovery::new(app.container()).meta::<TransportContribution>();
    /// assert_eq!(attached[0].meta.name(), "MetricsPort");
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    ///
    /// A closure capturing a value is refused at compile time:
    ///
    /// ```compile_fail
    /// # use std::time::Duration;
    /// # use nest_rs_core::{Container, Transport, async_trait};
    /// # #[derive(Clone)]
    /// # struct MetricsPort;
    /// # #[async_trait]
    /// # impl Transport for MetricsPort {
    /// #     async fn configure(&mut self, _: &Container) -> anyhow::Result<()> { Ok(()) }
    /// #     async fn serve(self: Box<Self>, _: tokio_util::sync::CancellationToken) -> anyhow::Result<()> { Ok(()) }
    /// #     fn stop_bound(&self) -> Duration { Duration::ZERO }
    /// # }
    /// let captured = MetricsPort;
    /// let _ = Container::builder().provide_transport(move |_| Ok(captured.clone()));
    /// ```
    pub fn provide_transport<T: crate::Transport>(
        mut self,
        build: fn(&Container) -> anyhow::Result<T>,
    ) -> Self {
        let site = self.declaring_site();
        match self.transports.entry(TypeId::of::<T>()) {
            Entry::Occupied(first) => {
                self.contested.push(crate::ContestedDeclarationError {
                    type_name: std::any::type_name::<T>(),
                    first: first.get().clone(),
                    second: site,
                    remedy: crate::transport::ONE_TRANSPORT_PER_TYPE,
                });
                self
            }
            Entry::Vacant(slot) => {
                slot.insert(site);
                self.provide_meta(crate::TransportContribution::new(build))
            }
        }
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

    /// Close the `T` this module opened once the app ends: after every shutdown
    /// hook, the `T` installed last closing first — a client a factory opened
    /// over a pool closes before the pool, whatever the import order — within
    /// what the hooks left of their budget
    /// ([`SHUTDOWN_HOOKS_TIMEOUT`](crate::SHUTDOWN_HOOKS_TIMEOUT)).
    ///
    /// The kernel resolves `T` from the final container; a `T` nothing
    /// provides, or one the app was handed (a seed, a test's override), is not
    /// the module's to close and is skipped. An error is filed at `error`, a
    /// close still waiting when the budget is spent is abandoned at `warn`, and
    /// the next close still starts.
    ///
    /// A close for a factory's output is attached in `collect`, beside its
    /// factory: a boot whose later factory fails closes what the earlier ones
    /// opened, and a `register` that boot never reaches would attach nothing.
    /// A value `register` builds attaches its close there, as here:
    ///
    /// ```
    /// use nest_rs_core::{App, ContainerBuilder, Module, Registering};
    ///
    /// struct Pool;
    ///
    /// impl Pool {
    ///     async fn goodbye(&self) -> anyhow::Result<()> {
    ///         Ok(())
    ///     }
    /// }
    ///
    /// struct PoolModule;
    ///
    /// impl Module for PoolModule {
    ///     fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
    ///         builder
    ///             .provide(Pool)
    ///             .provide_close::<Pool, _, _>(|pool| async move { pool.goodbye().await })
    ///     }
    /// }
    /// # #[nest_rs_core::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// App::new::<PoolModule>()?.close().await;
    /// # Ok(())
    /// # }
    /// ```
    pub fn provide_close<T, F, Fut>(self, close: F) -> Self
    where
        T: Any + Send + Sync,
        F: Fn(Arc<T>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        self.provide_meta(crate::close::CloseContribution::new(close))
    }

    /// Refuse the boot from a `register`, which has no `Result` to return: the
    /// boot fails with `error` once the register phase ends, and a container
    /// built by hand at [`build`](Self::build). The first refusal is the one
    /// reported.
    pub fn refuse(self, error: impl Into<anyhow::Error>) -> Self {
        self.file(crate::ContainerError::Refused(error.into()))
    }

    /// File `refusal` unless one is filed already: the kernel's own, typed.
    fn file(mut self, refusal: crate::ContainerError) -> Self {
        if self.refusal.is_none() {
            self.refusal = Some(refusal);
        }
        self
    }

    /// The refusal filed first, taken as the register phase ends.
    fn take_refusal(&mut self) -> Option<crate::ContainerError> {
        self.refusal.take()
    }

    /// The refusal of `queued`, which nothing drains, in the words of this
    /// builder's remedy.
    fn late_factory(&self, queued: &QueuedFactory) -> crate::LateFactoryError {
        if self.booting {
            return crate::LateFactoryError::in_boot(queued.name);
        }
        let kind = if queued.declared {
            FactoryKind::Declaration
        } else if queued.provides.len() > 1 {
            FactoryKind::Binding
        } else {
            FactoryKind::Default
        };
        crate::LateFactoryError::by_hand(queued.name, kind)
    }

    /// Whether a provider for `id` has already been registered.
    pub fn contains(&self, id: TypeId) -> bool {
        self.providers.contains_key(&id)
    }

    /// The singleton registered so far under `T`, without snapshotting.
    pub(crate) fn get<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.providers
            .get(&TypeId::of::<T>())
            .and_then(|any| any.clone().downcast::<T>().ok())
    }

    /// Import the module `M`: in the collect phase its
    /// [`collect`](Module::collect), in the register phase its
    /// [`register`](Module::register) — each once per app, `collect` first. A
    /// module first imported in the register phase is collected too late for a
    /// factory, so a hand-written module imports in both of its phases: one
    /// whose late `collect` queued a factory nothing will drain is refused
    /// ([`LateFactoryError`](crate::LateFactoryError)) and never registers.
    ///
    /// A builder made outside a boot ([`Container::builder`]) is in the
    /// register phase and drains no factory, so a module whose `collect`
    /// queues one registers there only over defaults whose types are already
    /// provided — a bare import's `#[config]`, seeded with
    /// [`provide`](Self::provide) — and never over a declaration, a pinned
    /// `for_root` value; [`build`](Self::build) returns the refusal.
    #[must_use]
    pub fn import<M: Module>(mut self) -> Self {
        let id = TypeId::of::<M>();
        // Marked before the phase runs, so an import cycle ends.
        if self.collected_modules.insert(id) {
            self.collected_order.push((id, std::any::type_name::<M>()));
            let phase = std::mem::replace(&mut self.phase, Phase::Collect);
            let collected_from = self.next_factory_seq;
            self = M::collect(self, Collecting::new());
            self.phase = phase;
            if phase == Phase::Register
                && let Some(refusal) = self.late_factory_queued_from(collected_from)
            {
                // Marked registered, so no later import runs a `register`
                // whose factory never ran; refused here, since a later
                // `register` providing the type would hide the late factory.
                self.registered_modules.insert(id);
                return self.file(refusal.into());
            }
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
    /// other factories' outputs from its snapshot — its [`FactoryInputs`], a
    /// tuple: `provide_factory_after::<Client, (Pool,), _, _>`. The drain runs
    /// it once each input is present, wherever the factories fall in
    /// `imports = [..]`.
    pub fn provide_factory_after<T, I, F, Fut>(self, factory: F) -> Self
    where
        T: Any + Send + Sync,
        I: FactoryInputs,
        F: FnOnce(Container) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.queue(
            QueueSpec {
                after: I::type_ids(),
                ..QueueSpec::default()
            },
            factory,
            |builder, value| builder.provide(value),
        )
    }

    /// [`provide_declared_factory`](Self::provide_declared_factory) for a
    /// factory that reads other factories' outputs — its [`FactoryInputs`] —
    /// from its snapshot. See [`provide_factory_after`](Self::provide_factory_after).
    pub fn provide_declared_factory_after<T, I, F, Fut>(
        self,
        remedy: &'static str,
        factory: F,
    ) -> Self
    where
        T: Any + Send + Sync,
        I: FactoryInputs,
        F: FnOnce(Container) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.queue(
            QueueSpec {
                remedy: Some(remedy),
                after: I::type_ids(),
                ..QueueSpec::default()
            },
            factory,
            |builder, value| builder.provide(value),
        )
    }

    /// [`provide_factory_dyn`](Self::provide_factory_dyn) for a factory that
    /// reads other factories' outputs — its [`FactoryInputs`] — from its
    /// snapshot. See [`provide_factory_after`](Self::provide_factory_after).
    pub fn provide_factory_dyn_after<T, D, I, F, Fut>(
        self,
        factory: F,
        bind: fn(T) -> Arc<D>,
    ) -> Self
    where
        T: Any + Clone + Send + Sync,
        D: ?Sized + Send + Sync + 'static,
        I: FactoryInputs,
        F: FnOnce(Container) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.queue(
            QueueSpec {
                binds: Some(DynBinding::of(bind)),
                after: I::type_ids(),
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
                    self.contested.push(crate::ContestedDeclarationError {
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
            seq: self.next_factory_seq,
            queued_by: self.import_sites.last().copied(),
            provides: vec![id],
            after,
            declared: remedy.is_some(),
            derives: Vec::new(),
            factory: boxed,
        };
        self.next_factory_seq += 1;
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
    /// `#[injectable(scope = request)]`. A second registration of `T`, in any
    /// scope, is a duplicate the boot refuses.
    pub fn provide_scoped<T, F>(mut self, factory: F) -> Self
    where
        T: Any + Send + Sync,
        F: Fn(&RequestScope) -> T + Send + Sync + 'static,
    {
        self.record::<T>(ProviderScope::Request);
        if self.claim(TypeId::of::<T>()).is_some() {
            return self;
        }
        self.scoped.insert(
            TypeId::of::<T>(),
            Arc::new(move |scope| Arc::new(factory(scope)) as AnyArc),
        );
        self
    }

    /// Register a transient provider: `factory` builds a fresh `T` every time
    /// `Container::get::<T>()` (or a [`RequestScope`]) resolves it — emitted by
    /// `#[injectable(scope = transient)]`. A second registration of `T`, in
    /// any scope, is a duplicate the boot refuses.
    pub fn provide_transient<T, F>(mut self, factory: F) -> Self
    where
        T: Any + Send + Sync,
        F: Fn(&RequestScope) -> T + Send + Sync + 'static,
    {
        self.record::<T>(ProviderScope::Transient);
        if self.claim(TypeId::of::<T>()).is_some() {
            return self;
        }
        self.transient.insert(
            TypeId::of::<T>(),
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

    /// The refusal of the first factory still queued that would provide a type
    /// nothing does, read as the register phase ends. A declaration always
    /// counts: the value present is not the one it chose.
    fn late_factory_refusal(&self) -> Option<crate::LateFactoryError> {
        self.late_factory_queued_from(0)
    }

    /// [`late_factory_refusal`](Self::late_factory_refusal) among the
    /// factories queued from the `seq`-th on — what one late `collect` queued.
    fn late_factory_queued_from(&self, seq: u64) -> Option<crate::LateFactoryError> {
        self.factories
            .iter()
            .filter(|queued| queued.seq >= seq)
            .find(|queued| {
                queued.declared || queued.provides.iter().any(|key| !self.contains(*key))
            })
            .map(|queued| self.late_factory(queued))
    }

    /// The first module collected that no register phase reached — imported in
    /// a collect alone — read as the register phase ends.
    fn unregistered_module(&self) -> Option<&'static str> {
        self.collected_order
            .iter()
            .find(|(id, _)| !self.registered_modules.contains(id))
            .map(|&(_, name)| name)
    }

    /// Every singleton `TypeId` registered so far — after the factory phase,
    /// the access graph's **global** set.
    pub(crate) fn provider_ids(&self) -> HashSet<TypeId> {
        self.providers.keys().copied().collect()
    }

    /// Every `TypeId` resolvable from this builder — singleton values **plus**
    /// request-scoped and transient factories.
    pub(crate) fn registered_ids(&self) -> HashSet<TypeId> {
        self.providers
            .keys()
            .copied()
            .chain(self.scoped.keys().copied())
            .chain(self.transient.keys().copied())
            .collect()
    }

    /// Every `TypeId` registered as a **request-scoped or transient**
    /// factory, which a singleton may not inject.
    pub(crate) fn scoped_or_transient_ids(&self) -> HashSet<TypeId> {
        // A double stands in as a singleton; its consumers keep the
        // production scope check.
        let overridden = self
            .overridden_providers
            .iter()
            .filter(|o| o.scope != ProviderScope::Singleton)
            .map(|o| o.provider.0);
        self.scoped
            .keys()
            .copied()
            .chain(self.transient.keys().copied())
            .chain(overridden)
            .collect()
    }

    /// The scope of every type registered as a request-scoped or transient
    /// factory.
    pub(crate) fn dynamic_scopes(&self) -> HashMap<TypeId, ProviderScope> {
        self.scoped
            .keys()
            .map(|id| (*id, ProviderScope::Request))
            .chain(
                self.transient
                    .keys()
                    .map(|id| (*id, ProviderScope::Transient)),
            )
            .collect()
    }

    /// Freeze the accumulated registrations into the immutable [`Container`],
    /// or refuse them as the boot would.
    ///
    /// # Errors
    ///
    /// The first refusal the builder holds, in the boot's order
    /// ([`ContainerError`](crate::ContainerError)): a declaration contested, a
    /// seed the check its queued factory declares refuses, what a `register`
    /// filed with [`refuse`](Self::refuse), a factory
    /// nothing drains — a module whose `collect` queued one is left out by
    /// [`import`](Self::import) —, a module collected alone, a type registered
    /// twice, then a cycle among request-scoped and transient providers, one
    /// among singletons of modules importing each other, a provider a module
    /// lists that was never built, one built before an optional dependency
    /// registered after it, and a budget at or past a net reaching it.
    ///
    /// ```
    /// use nest_rs_core::{Container, ContainerError};
    ///
    /// struct Counter(u32);
    ///
    /// let refused = Container::builder()
    ///     .provide(Counter(1))
    ///     .provide(Counter(2))
    ///     .build();
    /// assert!(matches!(refused, Err(ContainerError::DuplicateProvider(_))));
    /// ```
    pub fn build(mut self) -> Result<Container, crate::ContainerError> {
        // The boot reads a contest before its factory phase, where a seed
        // passes its check (`.claude/decisions/boot-refusal-order.md`).
        if let Some(contested) = self.contested_refusal() {
            return Err(contested.into());
        }
        self.seed_refusal()?;
        self.register_refusal()?;
        self.seal_refusal()?;
        Ok(self.freeze())
    }

    /// The first seed a queued factory's check refuses, as the boot's factory
    /// phase runs it where the seed makes the factory skip: a builder made by
    /// hand drains no factory, so its seeds pass their checks here.
    fn seed_refusal(&self) -> Result<(), crate::ContainerError> {
        let mut seeded = self
            .factories
            .iter()
            .map(QueuedFactory::id)
            .filter(|id| self.contains(*id))
            .filter_map(|id| self.seed_check(id))
            .peekable();
        if seeded.peek().is_none() {
            return Ok(());
        }
        let built = self.snapshot();
        seeded.try_for_each(|check| check(&built).map_err(crate::ContainerError::SeedRefused))
    }

    /// What the register phase leaves to refuse, on every path, in the boot's
    /// order: a declaration contested first, then the refusal filed — a late
    /// import's among them —, a factory still queued that nothing drains, a
    /// module no register phase reached, then a type registered twice.
    pub(crate) fn register_refusal(&mut self) -> Result<(), crate::ContainerError> {
        // A module or dynamic import first reached in a `register` collects there,
        // so it may declare there too; the contest survives every refusal's remedy
        // (`.claude/decisions/boot-refusal-order.md`).
        if let Some(contested) = self.contested_refusal() {
            return Err(contested.into());
        }
        if let Some(refusal) = self.take_refusal() {
            return Err(refusal);
        }
        if let Some(late) = self.late_factory_refusal() {
            return Err(late.into());
        }
        if let Some(module) = self.unregistered_module() {
            return Err(crate::UnregisteredModuleError { module }.into());
        }
        match self.duplicates.first() {
            Some(duplicate) => Err(duplicate.error().into()),
            None => Ok(()),
        }
    }

    /// The first declaration contested, as the boot refuses it.
    pub(crate) fn contested_refusal(&self) -> Option<crate::ContestedDeclarationError> {
        self.contested
            .first()
            .map(|contested| crate::ContestedDeclarationError {
                type_name: contested.type_name,
                first: contested.first.clone(),
                second: contested.second.clone(),
                remedy: contested.remedy,
            })
    }

    /// What the boot's seal would read off a container built by hand, which
    /// has no root, its modules standing for the roots: a cycle among
    /// request-scoped and transient providers first, then one among singletons
    /// of modules importing each other, a provider never built, one built
    /// before an optional dependency registered after it, and a budget at or
    /// past a net reaching it.
    fn seal_refusal(&self) -> Result<(), crate::ContainerError> {
        let descriptors: Vec<&ModuleDescriptor> = inventory::iter::<ModuleDescriptor>().collect();
        let roots: Vec<TypeId> = self.collected_order.iter().map(|(id, _)| *id).collect();
        let overridden: HashSet<TypeId> = self
            .overridden_providers
            .iter()
            .map(|o| o.provider.0)
            .collect();
        let scopes = self.dynamic_scopes();
        if !scopes.is_empty()
            && let Some(cycle) = scoped_cycle(&descriptors, &roots, &scopes, &overridden)
        {
            return Err(cycle.into());
        }
        let waiting = self.waiting();
        if !waiting.is_empty()
            && let Some(cycle) =
                singleton_cycle(&descriptors, &roots, &scopes, &waiting, &overridden)
        {
            return Err(cycle.into());
        }
        let reached = reachable_descriptors(&descriptors, &roots);
        if let Some(unbuilt) = self.unbuilt_refusal(&reached) {
            return Err(unbuilt.into());
        }
        if let Some(late) = self.late_optional_refusal(&reached) {
            return Err(late.into());
        }
        check_budgets(self, &reached, None).map_err(crate::ContainerError::from)
    }

    /// The providers a cycle among singletons may hold back: each one a
    /// `#[module]` never built, then each built without an optional dependency
    /// registered after it.
    pub(crate) fn waiting(&self) -> Vec<TypeId> {
        self.unbuilt()
            .filter_map(|stall| stall.provides.first().copied())
            .chain(self.late_optionals().map(|(built, _)| built.provider.0))
            .collect()
    }

    /// The first provider a `#[module]` lists that the register phase never
    /// built, naming why what it injects was absent.
    pub(crate) fn unbuilt_refusal(
        &self,
        reached: &[&ModuleDescriptor],
    ) -> Option<crate::UnbuiltProviderError> {
        let stall = self.unbuilt().next()?;
        let (module, module_name) = stall.module;
        let (keys, missing): (Vec<TypeId>, _) = stall.missing.iter().copied().unzip();
        Some(crate::UnbuiltProviderError {
            module: module_name,
            provider: stall.provider,
            missing,
            cause: self.absence_cause(reached, module, &keys),
        })
    }

    /// The first provider a `#[module]` built without an optional dependency
    /// registered afterwards, naming why it was absent then.
    pub(crate) fn late_optional_refusal(
        &self,
        reached: &[&ModuleDescriptor],
    ) -> Option<crate::LateOptionalDependencyError> {
        let (built, (key, dependency)) = self.late_optionals().next()?;
        Some(crate::LateOptionalDependencyError {
            module: built.module.1,
            provider: built.provider.1,
            dependency,
            cause: self.absence_cause(reached, built.module.0, &[key]),
        })
    }

    /// Why `keys` were absent when `module` built its providers. On a container
    /// built by hand, one registered later was registered after the call that
    /// imported `module`.
    fn absence_cause(
        &self,
        reached: &[&ModuleDescriptor],
        module: TypeId,
        keys: &[TypeId],
    ) -> crate::UnbuiltCause {
        match unbuilt_cause(reached, module, keys) {
            crate::UnbuiltCause::RegisteredLater if !self.booting => {
                crate::UnbuiltCause::ImportedFirst
            }
            cause => cause,
        }
    }

    /// The providers `#[module]`s list that the register phase never built, in
    /// the order recorded. One whose keys are present by now is no longer
    /// missing.
    fn unbuilt(&self) -> impl Iterator<Item = &Stall> {
        self.stalls
            .iter()
            .filter(|stall| !stall.provides.iter().any(|key| self.contains(*key)))
    }

    /// The providers `#[module]`s built without an optional dependency
    /// registered afterwards, each with the first such dependency.
    fn late_optionals(&self) -> impl Iterator<Item = (&AbsentOptional, (TypeId, &'static str))> {
        self.absent_optionals.iter().filter_map(|built| {
            let late = built.absent.iter().find(|(key, _)| self.contains(*key));
            late.map(|late| (built, *late))
        })
    }

    /// Freeze the registrations into the immutable [`Container`], once the boot
    /// has read every refusal.
    pub(crate) fn freeze(self) -> Container {
        Container {
            id: next_container_id(),
            providers: Arc::new(self.providers),
            metadata: Arc::new(self.metadata),
            scoped: Arc::new(self.scoped),
            transient: Arc::new(self.transient),
            sites: Arc::default(),
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
            sites: Arc::default(),
        }
    }
}

/// `container` is public: its tier-2 items live here, reached only
/// through the crate's `__private`.
pub(crate) mod __private {
    use std::any::TypeId;
    use std::collections::HashSet;
    use std::sync::PoisonError;

    use super::{Container, ContainerBuilder, ImportSite};
    use crate::access::{cycle_from, labelled};
    use crate::layer_chain::__private::SiteChains;
    use crate::module::{Collecting, DynamicModule, Registering};

    /// Refuse the boot from a site a transport composes at `configure` — a route,
    /// a gateway — against the built `container`: a mount has no `Result` to
    /// return, so the site files the refusal, mounts something that denies, and
    /// the boot fails on [`take_site_refusal`] once its transports are
    /// configured. The first refusal is the one kept.
    pub fn refuse_site(container: &Container, error: anyhow::Error) {
        let mut slot = container
            .sites
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
        &container.sites.chains
    }

    /// The refusal a site filed against `container` ([`refuse_site`]), taken by
    /// whatever configured its transports, to end the boot with.
    pub fn take_site_refusal(container: &Container) -> Option<anyhow::Error> {
        container
            .sites
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

    /// Whether a test overrode any of `keys` — the keys one provider
    /// registers. `#[module]` emits it before building a provider, which it
    /// then never builds.
    pub fn overrides_any(builder: &ContainerBuilder, keys: &[TypeId]) -> bool {
        keys.iter().any(|key| builder.overrides(*key))
    }

    /// Stand a test's override in for the provider `provider`, named `name`
    /// and built under `scope`, which registers `keys`, each with its type's
    /// name: each overridden key is claimed, the others are left absent, and
    /// the provider is never built. Every key still counts as registered by
    /// it, so a second registration is the duplicate it is without the
    /// override. `#[module]` and the decorators that build a provider emit it.
    pub fn claim_override(
        mut builder: ContainerBuilder,
        provider: TypeId,
        name: &'static str,
        scope: crate::ProviderScope,
        keys: &[(TypeId, &'static str)],
    ) -> ContainerBuilder {
        let mut through = None;
        let mut absent = Vec::new();
        for (key, type_name) in keys {
            builder.note(*key, type_name, scope);
            match builder.claim(*key) {
                Some(name) => through = through.or(Some(name)),
                None => absent.push(*key),
            }
        }
        if let Some(through) = through {
            builder
                .overridden_providers
                .push(super::OverriddenProvider {
                    provider: (provider, name),
                    scope,
                    through,
                    absent,
                });
        }
        builder
    }

    /// [`claim_override`] for a provider that registers its own type alone,
    /// as `#[injectable]` emits it.
    pub fn claim_own_override<P: 'static>(
        builder: ContainerBuilder,
        scope: crate::ProviderScope,
    ) -> ContainerBuilder {
        let (own, name) = (TypeId::of::<P>(), std::any::type_name::<P>());
        claim_override(builder, own, name, scope, &[(own, name)])
    }

    /// Hold every seed of the factory output `id` to `check` before anything
    /// reads it: the factory drain runs it where the seed makes the factory
    /// skip. `ConfigModule` declares it for each config it queues.
    pub fn check_seed(
        mut builder: ContainerBuilder,
        id: TypeId,
        check: super::SeedCheck,
    ) -> ContainerBuilder {
        builder.seed_checks.insert(id, check);
        builder
    }

    /// A provider `#[module]`'s register step left unbuilt once nothing more
    /// could progress, as its expansion reports it to [`record_stall`].
    pub struct StalledProvider {
        /// The provider, as its module lists it.
        pub name: &'static str,
        /// Every key it registers: its own type, and each trait it is bound as.
        pub provides: Vec<TypeId>,
        /// Its required dependencies.
        pub dependencies: Vec<TypeId>,
        /// The label of each of [`dependencies`](Self::dependencies), in order.
        pub dependency_names: Vec<&'static str>,
        /// Its optional dependencies, which it waits on while a sibling would
        /// provide them.
        pub optional_dependencies: Vec<TypeId>,
        /// The label of each of
        /// [`optional_dependencies`](Self::optional_dependencies), in order.
        pub optional_dependency_names: Vec<&'static str>,
    }

    /// File what `module`'s register step could not build: a cycle among its
    /// providers is refused now ([`ProviderCycleError`](crate::ProviderCycleError));
    /// every other stalled provider is kept for the seal, which refuses it
    /// once the access graph passes — those waiting on something no sibling
    /// provides first, since the rest wait on them. `#[module]` emits it.
    pub fn record_stall(
        mut builder: ContainerBuilder,
        module: TypeId,
        module_name: &'static str,
        stalled: Vec<StalledProvider>,
    ) -> ContainerBuilder {
        let pending: HashSet<TypeId> = stalled
            .iter()
            .flat_map(|provider| provider.provides.iter().copied())
            .collect();
        let waits: Vec<Vec<(TypeId, &'static str)>> = stalled
            .iter()
            .map(|provider| {
                let required = labelled(&provider.dependencies, &provider.dependency_names)
                    .filter(|(id, _)| !builder.contains(*id));
                let optional = labelled(
                    &provider.optional_dependencies,
                    &provider.optional_dependency_names,
                )
                .filter(|(id, _)| !builder.contains(*id) && pending.contains(id));
                required.chain(optional).collect()
            })
            .collect();
        let cyclic = waiting_in_a_cycle(&stalled, &waits, &pending);
        if let Some(chain) = cycle_chain(&stalled, &waits, &cyclic) {
            builder = builder.file(
                crate::ProviderCycleError {
                    module: module_name,
                    scope: crate::ProviderScope::Singleton,
                    chain,
                }
                .into(),
            );
        }
        let waits_outside = |i: &usize| waits[*i].iter().any(|(id, _)| !pending.contains(id));
        let off_cycle = (0..stalled.len()).filter(|i| !cyclic[*i]);
        let (outside, inside): (Vec<usize>, Vec<usize>) = off_cycle.partition(waits_outside);
        for i in outside.into_iter().chain(inside) {
            builder.stalls.push(super::Stall {
                module: (module, module_name),
                provider: stalled[i].name,
                provides: stalled[i].provides.clone(),
                missing: waits[i].clone(),
            });
        }
        builder
    }

    /// Note which of `provider`'s optional dependencies are absent as
    /// `module`'s register step builds it, for the seal to refuse one the boot
    /// registered afterwards. `#[module]` emits it before building a provider.
    pub fn record_absent_optional(
        mut builder: ContainerBuilder,
        module: TypeId,
        module_name: &'static str,
        provider: TypeId,
        provider_name: &'static str,
        optional_dependencies: &[TypeId],
        optional_dependency_names: &[&'static str],
    ) -> ContainerBuilder {
        let absent: Vec<(TypeId, &'static str)> =
            labelled(optional_dependencies, optional_dependency_names)
                .filter(|(id, _)| !builder.contains(*id))
                .collect();
        if !absent.is_empty() {
            builder.absent_optionals.push(super::AbsentOptional {
                module: (module, module_name),
                provider: (provider, provider_name),
                absent,
            });
        }
        builder
    }

    /// The first cycle among the `cyclic` providers, in the order each waits on
    /// the next, its first provider repeated last: walked from the first of
    /// them along each one's waits on the others, in order.
    fn cycle_chain(
        stalled: &[StalledProvider],
        waits: &[Vec<(TypeId, &'static str)>],
        cyclic: &[bool],
    ) -> Option<Vec<&'static str>> {
        let provider_on_cycle = |id: &TypeId| {
            (0..stalled.len()).find(|j| cyclic[*j] && stalled[*j].provides.contains(id))
        };
        let edges: Vec<Vec<usize>> = waits
            .iter()
            .map(|wait| {
                wait.iter()
                    .filter_map(|(id, _)| provider_on_cycle(id))
                    .collect()
            })
            .collect();
        let start = cyclic.iter().position(|on_cycle| *on_cycle)?;
        let cycle = cycle_from(start, &edges, &mut vec![false; stalled.len()])?;
        Some(cycle.into_iter().map(|i| stalled[i].name).collect())
    }

    /// Which of `stalled` wait on each other in a cycle — or on one that does:
    /// every one of their `waits` is a sibling's, and no sibling they wait on
    /// waits on anything else.
    fn waiting_in_a_cycle(
        stalled: &[StalledProvider],
        waits: &[Vec<(TypeId, &'static str)>],
        pending: &HashSet<TypeId>,
    ) -> Vec<bool> {
        let mut cyclic: Vec<bool> = waits
            .iter()
            .map(|wait| !wait.is_empty() && wait.iter().all(|(id, _)| pending.contains(id)))
            .collect();
        loop {
            let off: Vec<TypeId> = stalled
                .iter()
                .zip(&cyclic)
                .filter(|(_, on_cycle)| !**on_cycle)
                .flat_map(|(provider, _)| provider.provides.iter().copied())
                .collect();
            let mut changed = false;
            for (on_cycle, wait) in cyclic.iter_mut().zip(waits) {
                if *on_cycle && wait.iter().any(|(id, _)| off.contains(id)) {
                    *on_cycle = false;
                    changed = true;
                }
            }
            if !changed {
                return cyclic;
            }
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
        let container = Container::builder()
            .provide(Greeter("hi"))
            .build()
            .expect("the container builds");
        let resolved: Arc<Greeter> = container.get().expect("greeter is registered");
        assert_eq!(resolved.0, "hi");
    }

    #[test]
    fn resolves_multiple_distinct_types() {
        let container = Container::builder()
            .provide(Greeter("hi"))
            .provide(Counter(42))
            .build()
            .expect("the container builds");
        assert_eq!(container.get::<Greeter>().unwrap().0, "hi");
        assert_eq!(container.get::<Counter>().unwrap().0, 42);
    }

    #[test]
    fn missing_type_returns_none() {
        let container = Container::builder().build().expect("the container builds");
        assert!(container.get::<Greeter>().is_none());
    }

    /// The sentence the first duplicate `builder` kept would fail the boot with.
    fn first_duplicate(builder: &ContainerBuilder) -> crate::DuplicateProviderError {
        builder
            .duplicates
            .first()
            .expect("a duplicate is kept")
            .error()
    }

    #[test]
    fn a_second_value_of_one_type_is_kept_as_a_duplicate_for_the_boot() {
        let builder = Container::builder().provide(Counter(1)).provide(Counter(2));
        let duplicate = first_duplicate(&builder);
        assert_eq!(duplicate.type_name, std::any::type_name::<Counter>());
        assert_eq!(duplicate.scopes, None);
    }

    #[test]
    fn a_seed_and_a_modules_registration_of_one_type_name_the_seed() {
        let builder = ContainerBuilder::booting().provide(Counter(1));
        let builder =
            __private::enter_import(builder, "Root", 0, "CountersModule").provide(Counter(2));
        let sentence = first_duplicate(&builder).to_string();
        assert!(
            sentence.contains(
                "registered by a seed (`App::builder().provide…`) and again by \
                 `CountersModule` at `imports[0]` of `Root`"
            ),
            "{sentence}"
        );
    }

    #[test]
    fn provide_arc_preserves_the_same_instance() {
        let shared = Arc::new(Counter(7));
        let container = Container::builder()
            .provide_arc(shared.clone())
            .build()
            .expect("the container builds");
        let resolved: Arc<Counter> = container.get().unwrap();
        assert!(Arc::ptr_eq(&shared, &resolved));
    }

    #[test]
    fn container_is_cheap_to_clone() {
        let container = Container::builder()
            .provide(Greeter("hi"))
            .build()
            .expect("the container builds");
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
        let container = Container::builder()
            .provide_dyn(polite)
            .build()
            .expect("the container builds");

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
            .duplicates
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
            .build()
            .expect("the container builds");
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
            .build()
            .expect("the container builds");
        let entries = container.metadata_entries(TypeId::of::<Marker>()).unwrap();
        assert_eq!(entries[0].provider_type_id, Some(TypeId::of::<Host>()));
    }

    #[test]
    fn provide_meta_has_no_host() {
        let container = Container::builder()
            .provide_meta(Marker("free"))
            .build()
            .expect("the container builds");
        let entries = container.metadata_entries(TypeId::of::<Marker>()).unwrap();
        assert_eq!(entries[0].provider_type_id, None);
    }

    #[test]
    fn metadata_returns_none_when_absent() {
        let container = Container::builder().build().expect("the container builds");
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
    fn a_module_first_imported_in_a_register_registers_when_its_collect_queues_no_factory() {
        struct Greets;
        impl Module for Greets {
            fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
                builder.provide(Greeter("hi"))
            }
        }
        let mut builder = Container::builder().import::<Greets>();
        assert!(builder.take_refusal().is_none());
        assert_eq!(builder.get::<Greeter>().map(|g| g.0), Some("hi"));
    }

    #[test]
    fn a_module_first_imported_in_a_register_never_registers_over_a_factory_nothing_drains() {
        struct ReadsItsFactory;
        impl Module for ReadsItsFactory {
            fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
                builder.provide_factory(|_| async { Ok(Late) })
            }
            fn register(_: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
                panic!("a register runs only once the factory its collect queued ran")
            }
        }
        let mut builder = ContainerBuilder::booting()
            .import::<ReadsItsFactory>()
            .import::<ReadsItsFactory>();
        let Some(crate::ContainerError::LateFactory(late)) = builder.take_refusal() else {
            panic!("not the late-factory refusal");
        };
        assert_eq!(late.type_name, std::any::type_name::<Late>());
        assert!(
            late.to_string()
                .contains("queued during the register phase"),
            "{late}"
        );
    }

    #[test]
    fn a_module_first_imported_in_a_register_answers_only_for_the_factories_its_collect_queued() {
        struct Greets;
        impl Module for Greets {
            fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
                builder.provide(Greeter("hi"))
            }
        }
        let mut builder = ContainerBuilder::booting()
            .import::<QueuesLate>()
            .import::<Greets>();
        let Some(crate::ContainerError::LateFactory(late)) = builder.take_refusal() else {
            panic!("not the late-factory refusal");
        };
        assert_eq!(late.type_name, std::any::type_name::<Late>());
        assert_eq!(
            builder.get::<Greeter>().map(|g| g.0),
            Some("hi"),
            "the factory pending is another module's"
        );
    }

    struct Late;

    /// Queues a factory for [`Late`] in its `collect`, then registers a
    /// greeter over it.
    struct QueuesLate;

    impl Module for QueuesLate {
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder.provide_factory(|_| async { Ok(Late) })
        }
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder.provide(Greeter("registered"))
        }
    }

    #[test]
    fn a_container_built_by_hand_returns_the_refusal_of_a_module_whose_factory_nothing_drains() {
        let Err(crate::ContainerError::LateFactory(late)) =
            Container::builder().import::<QueuesLate>().build()
        else {
            panic!("the container would be built without the module, and nothing would say why");
        };
        assert_eq!(late.type_name, std::any::type_name::<Late>());
        let sentence = late.to_string();
        assert!(
            sentence.contains("which a container built by hand never runs")
                && sentence.contains("provide `")
                && sentence.contains("App::builder()"),
            "the remedy is the hand-built builder's: {sentence}"
        );
    }

    #[test]
    fn a_container_built_by_hand_refuses_a_factory_queued_on_it() {
        let Err(crate::ContainerError::LateFactory(late)) = Container::builder()
            .provide_factory(|_| async { Ok(Late) })
            .build()
        else {
            panic!("the factory's type would be absent, and nothing would say why");
        };
        assert_eq!(late.type_name, std::any::type_name::<Late>());
    }

    #[test]
    fn a_container_built_by_hand_refuses_a_declaration_whatever_it_provides() {
        let Err(crate::ContainerError::LateFactory(late)) = Container::builder()
            .provide(Late)
            .provide_declared_factory("pin it in one place", |_| async { Ok(Late) })
            .build()
        else {
            panic!("the value provided would stand in for the one declared");
        };
        let sentence = late.to_string();
        assert!(
            sentence.contains("is declared by an async factory")
                && sentence
                    .contains("never stands in for a declaration: boot through `App::builder()`")
                && !sentence.contains("provide `"),
            "no seed clears a declaration, so the boot is the remedy: {sentence}"
        );
    }

    #[test]
    fn a_container_built_by_hand_names_the_trait_objects_a_factory_binds_off_its_value() {
        trait Port: Send + Sync {}
        #[derive(Clone)]
        struct Bound;
        impl Port for Bound {}
        let queued = || {
            Container::builder()
                .provide(Bound)
                .provide_factory_dyn::<Bound, dyn Port, _, _>(
                    |_| async { Ok(Bound) },
                    |bound| Arc::new(bound),
                )
        };
        let Err(crate::ContainerError::LateFactory(late)) = queued().build() else {
            panic!("the trait object would be absent, and nothing would say why");
        };
        let sentence = late.to_string();
        assert!(
            sentence.contains("and each trait object its factory binds off it (`provide_dyn`)"),
            "the value alone leaves the trait object absent: {sentence}"
        );
        let container = queued()
            .provide_dyn::<dyn Port>(Arc::new(Bound))
            .build()
            .expect("the value and its trait object stand in for the factory");
        assert!(container.get_dyn::<dyn Port>().is_some());
    }

    #[test]
    fn a_container_built_by_hand_refuses_a_budget_at_the_net_over_its_resource() {
        struct Pool(std::time::Duration);
        const NET: std::time::Duration = std::time::Duration::from_secs(20);
        let pooled = |budget| {
            Container::builder()
                .provide(Pool(budget))
                .provide_meta(crate::Budget::of::<Pool>(
                    "the test pool",
                    "TEST_POOL_WAIT",
                    |pool| Some(pool.0),
                ))
                .provide_meta(crate::Net::over::<Pool>("the test port", NET))
        };
        let Err(crate::ContainerError::BudgetPastNet(refused)) = pooled(NET).build() else {
            panic!("the port would cut a call the pool was still answering");
        };
        assert_eq!(
            (refused.resource, refused.budget, refused.port, refused.net),
            ("the test pool", NET, "the test port", NET)
        );
        pooled(NET / 2)
            .build()
            .expect("a budget under the net builds");
    }

    #[test]
    fn a_container_built_by_hand_registers_a_module_whose_factory_a_seed_answers() {
        let container = Container::builder()
            .provide(Late)
            .import::<QueuesLate>()
            .build()
            .expect("the seed answers the factory");
        assert_eq!(container.get::<Greeter>().map(|g| g.0), Some("registered"));
    }

    #[test]
    fn a_container_built_by_hand_refuses_a_type_registered_twice_naming_both_calls() {
        let Err(crate::ContainerError::DuplicateProvider(duplicate)) = Container::builder()
            .provide(Counter(1))
            .provide(Counter(2))
            .build()
        else {
            panic!("the second registration would replace the first in silence");
        };
        assert_eq!(duplicate.type_name, std::any::type_name::<Counter>());
        let by_hand = "a call on a container built by hand (`Container::builder()…`)";
        assert!(
            duplicate
                .to_string()
                .contains(&format!("registered by {by_hand} and again by {by_hand}")),
            "{duplicate}"
        );
    }

    #[test]
    fn a_container_built_by_hand_returns_the_first_refusal_a_register_filed_before_a_duplicate() {
        let Err(crate::ContainerError::Refused(refused)) = Container::builder()
            .provide(Counter(1))
            .refuse(anyhow::anyhow!("the first refusal"))
            .refuse(anyhow::anyhow!("a second refusal"))
            .provide(Counter(2))
            .build()
        else {
            panic!("the module's refusal would be dropped");
        };
        assert_eq!(refused.to_string(), "the first refusal");
    }

    #[test]
    fn a_container_built_by_hand_refuses_a_contested_declaration_ahead_of_its_factory() {
        let Err(crate::ContainerError::ContestedDeclaration(contested)) = Container::builder()
            .provide_declared_factory("pick one", |_| async { Ok(Late) })
            .provide_declared_factory("pick one", |_| async { Ok(Late) })
            .build()
        else {
            panic!("one declaration would win on queue position");
        };
        assert_eq!(contested.type_name, std::any::type_name::<Late>());
    }

    #[test]
    fn a_container_built_by_hand_refuses_a_module_its_imports_collected_alone() {
        struct CollectedAlone;
        impl Module for CollectedAlone {
            fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
                builder.provide(Greeter("never"))
            }
        }
        struct ImportsInCollectAlone;
        impl Module for ImportsInCollectAlone {
            fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
                builder.import::<CollectedAlone>()
            }
            fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
                builder
            }
        }
        let Err(crate::ContainerError::UnregisteredModule(unregistered)) = Container::builder()
            .import::<ImportsInCollectAlone>()
            .build()
        else {
            panic!("the module's providers would be absent, and nothing would say why");
        };
        assert_eq!(unregistered.module, std::any::type_name::<CollectedAlone>());
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
            .build()
            .expect("the container builds");

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
            .build()
            .expect("the container builds");

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
            .build()
            .expect("the container builds");

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
            .build()
            .expect("the container builds");
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
                .build()
                .expect("the container builds");
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
    fn a_second_transient_factory_of_one_type_is_a_duplicate() {
        let builder = Container::builder()
            .provide_transient(|_| Counter(1))
            .provide_transient(|_| Counter(2));
        let duplicate = first_duplicate(&builder);
        assert_eq!(duplicate.type_name, std::any::type_name::<Counter>());
        assert_eq!(duplicate.scopes, None, "one scope twice names none");
    }

    fn overriding<T: Any + Send + Sync>(value: T) -> ContainerBuilder {
        Container::builder().install_override::<T>(Arc::new(value))
    }

    fn holds_the_double_alone(builder: ContainerBuilder) {
        assert!(builder.duplicates.is_empty());
        assert_eq!(
            builder.unclaimed_override(),
            None,
            "the override is claimed"
        );
        let container = builder.build().expect("the container builds");
        assert!(!container.has_dynamic_scopes(), "no factory registered");
        assert_eq!(container.get::<Counter>().map(|c| c.0), Some(0));
    }

    #[test]
    fn each_singleton_registration_of_an_overridden_type_claims_it_instead() {
        holds_the_double_alone(overriding(Counter(0)).provide(Counter(1)));
        holds_the_double_alone(overriding(Counter(0)).provide_arc(Arc::new(Counter(1))));
    }

    #[test]
    fn a_request_scoped_or_transient_registration_of_an_overridden_type_claims_it_instead() {
        holds_the_double_alone(overriding(Counter(0)).provide_scoped(|_| Counter(1)));
        holds_the_double_alone(overriding(Counter(0)).provide_transient(|_| Counter(1)));
    }

    #[test]
    fn a_trait_object_binding_of_an_overridden_trait_object_claims_it_instead() {
        let builder = Container::builder()
            .install_override::<Arc<dyn Hello>>(Arc::new(Arc::new(Curt) as Arc<dyn Hello>))
            .provide_dyn::<dyn Hello>(Arc::new(Polite));
        assert!(builder.duplicates.is_empty());
        assert_eq!(builder.unclaimed_override(), None);
        let hello = builder
            .build()
            .expect("the container builds")
            .get_dyn::<dyn Hello>()
            .expect("bound");
        assert_eq!(hello.say(), "hi");
    }

    #[test]
    fn a_second_registration_of_an_overridden_type_is_the_duplicate_it_is_without_it() {
        let twice = [
            overriding(Counter(0))
                .provide(Counter(1))
                .provide(Counter(2)),
            overriding(Counter(0))
                .provide_scoped(|_| Counter(1))
                .provide_transient(|_| Counter(2)),
        ];
        for builder in &twice {
            assert_eq!(
                first_duplicate(builder).type_name,
                std::any::type_name::<Counter>()
            );
        }
        let bound_twice = Container::builder()
            .install_override::<Arc<dyn Hello>>(Arc::new(Arc::new(Curt) as Arc<dyn Hello>))
            .provide_dyn::<dyn Hello>(Arc::new(Polite))
            .provide_dyn::<dyn Hello>(Arc::new(Polite));
        assert_eq!(
            first_duplicate(&bound_twice).type_name,
            std::any::type_name::<Arc<dyn Hello>>()
        );
    }

    #[test]
    fn a_stand_down_takes_the_overrides_place_so_a_later_registration_is_a_duplicate() {
        let builder = overriding(Counter(0))
            .stand_down(TypeId::of::<Counter>())
            .stand_down(TypeId::of::<Counter>());
        assert!(builder.duplicates.is_empty());
        assert_eq!(builder.unclaimed_override(), None);
        let then_registered = overriding(Counter(0))
            .stand_down(TypeId::of::<Counter>())
            .provide(Counter(1));
        assert_eq!(
            first_duplicate(&then_registered).type_name,
            std::any::type_name::<Counter>()
        );
    }

    #[test]
    fn an_override_nothing_registers_stays_unclaimed() {
        let builder = overriding(Counter(0)).provide(Greeter("hi"));
        assert_eq!(
            builder.unclaimed_override(),
            Some(std::any::type_name::<Counter>())
        );
    }

    #[test]
    fn a_second_request_scoped_factory_of_one_type_is_a_duplicate() {
        let builder = Container::builder()
            .provide_scoped(|_| Counter(1))
            .provide_scoped(|_| Counter(2));
        assert_eq!(
            first_duplicate(&builder).type_name,
            std::any::type_name::<Counter>()
        );
    }

    #[test]
    fn one_type_under_two_scopes_is_a_duplicate_naming_both_in_order() {
        let cases: [(ContainerBuilder, (ProviderScope, ProviderScope)); 4] = [
            (
                Container::builder()
                    .provide(Counter(1))
                    .provide_transient(|_| Counter(2)),
                (ProviderScope::Singleton, ProviderScope::Transient),
            ),
            (
                Container::builder()
                    .provide_transient(|_| Counter(1))
                    .provide(Counter(2)),
                (ProviderScope::Transient, ProviderScope::Singleton),
            ),
            (
                Container::builder()
                    .provide(Counter(1))
                    .provide_scoped(|_| Counter(2)),
                (ProviderScope::Singleton, ProviderScope::Request),
            ),
            (
                Container::builder()
                    .provide_scoped(|_| Counter(1))
                    .provide_transient(|_| Counter(2)),
                (ProviderScope::Request, ProviderScope::Transient),
            ),
        ];
        for (builder, scopes) in cases {
            assert_eq!(first_duplicate(&builder).scopes, Some(scopes));
        }
    }

    #[test]
    fn the_scopes_clause_reads_first_then_second() {
        let builder = Container::builder()
            .provide_scoped(|_| Counter(1))
            .provide(Counter(2));
        let sentence = first_duplicate(&builder).to_string();
        assert!(
            sentence.contains("(first request-scoped, then as a singleton)"),
            "{sentence}"
        );
    }

    #[test]
    fn transient_panic_clears_reentrancy_stack() {
        let container = Container::builder()
            .provide_transient(|_| -> Counter { panic!("boom from factory") })
            .provide_transient(|_| Greeter("recovered"))
            .build()
            .expect("the container builds");

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

    mod stall {
        use std::any::TypeId;

        use super::super::__private::{StalledProvider, record_stall};
        use super::super::{Container, ContainerBuilder};

        struct Module;
        struct Absent;
        struct First;
        struct Second;
        struct Third;

        fn stalled(
            name: &'static str,
            provides: TypeId,
            waits: &[(TypeId, &'static str)],
        ) -> StalledProvider {
            StalledProvider {
                name,
                provides: vec![provides],
                dependencies: waits.iter().map(|(id, _)| *id).collect(),
                dependency_names: waits.iter().map(|(_, name)| *name).collect(),
                optional_dependencies: Vec::new(),
                optional_dependency_names: Vec::new(),
            }
        }

        fn record(stalled: Vec<StalledProvider>) -> ContainerBuilder {
            record_stall(
                Container::builder(),
                TypeId::of::<Module>(),
                "Module",
                stalled,
            )
        }

        fn recorded(builder: &ContainerBuilder) -> Vec<&'static str> {
            builder.stalls.iter().map(|stall| stall.provider).collect()
        }

        #[test]
        fn what_waits_outside_the_module_is_kept_before_what_waits_on_it() {
            let mut builder = record(vec![
                stalled(
                    "First",
                    TypeId::of::<First>(),
                    &[(TypeId::of::<Second>(), "Second")],
                ),
                stalled(
                    "Second",
                    TypeId::of::<Second>(),
                    &[(TypeId::of::<Absent>(), "Absent")],
                ),
            ]);
            assert!(builder.take_refusal().is_none(), "no cycle among them");
            assert_eq!(recorded(&builder), ["Second", "First"]);
            assert_eq!(
                builder.stalls[0].missing,
                [(TypeId::of::<Absent>(), "Absent")]
            );
            assert_eq!(builder.stalls[0].module, (TypeId::of::<Module>(), "Module"));
        }

        #[test]
        fn providers_waiting_on_each_other_are_refused_whatever_else_stalls() {
            let mut builder = record(vec![
                stalled(
                    "First",
                    TypeId::of::<First>(),
                    &[(TypeId::of::<Second>(), "Second")],
                ),
                stalled(
                    "Second",
                    TypeId::of::<Second>(),
                    &[(TypeId::of::<First>(), "First")],
                ),
                stalled(
                    "Third",
                    TypeId::of::<Third>(),
                    &[(TypeId::of::<Absent>(), "Absent")],
                ),
            ]);
            let Some(crate::ContainerError::ProviderCycle(cycle)) = builder.take_refusal() else {
                panic!("the cycle is refused");
            };
            assert_eq!(cycle.chain, ["First", "Second", "First"]);
            assert_eq!(cycle.scope, crate::ProviderScope::Singleton);
            assert_eq!(recorded(&builder), ["Third"]);
        }

        #[test]
        fn an_optional_dependency_a_stalled_sibling_provides_is_waited_on() {
            let mut waiting = stalled("First", TypeId::of::<First>(), &[]);
            waiting.optional_dependencies = vec![TypeId::of::<Second>(), TypeId::of::<Absent>()];
            waiting.optional_dependency_names = vec!["Second", "Absent"];
            let mut builder = record(vec![
                waiting,
                stalled(
                    "Second",
                    TypeId::of::<Second>(),
                    &[(TypeId::of::<Absent>(), "Absent")],
                ),
            ]);
            assert!(builder.take_refusal().is_none());
            assert_eq!(recorded(&builder), ["Second", "First"]);
            assert_eq!(
                builder.stalls[1].missing,
                [(TypeId::of::<Second>(), "Second")],
                "an optional no sibling provides is no wait"
            );
        }

        #[test]
        fn a_dependency_without_a_label_is_named_unnamed() {
            let mut unlabelled = stalled(
                "First",
                TypeId::of::<First>(),
                &[(TypeId::of::<Absent>(), "Absent")],
            );
            unlabelled.dependency_names.clear();
            let builder = record(vec![unlabelled]);
            assert_eq!(
                builder.stalls[0].missing[0].1,
                crate::access::UNNAMED_DEPENDENCY
            );
        }
    }
}

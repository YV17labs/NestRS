use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;

type AnyArc = Arc<dyn Any + Send + Sync>;

/// Builds a fresh instance of a request-scoped provider from the (singleton)
/// root container. Stored by [`ContainerBuilder::provide_scoped`] and invoked
/// once per request by a [`RequestScope`](crate::RequestScope), which caches the
/// result for the life of that request.
pub(crate) type ScopedFactory = Arc<dyn Fn(&Container) -> AnyArc + Send + Sync>;

/// A registration applied once a factory has produced its value: it `provide`s
/// the awaited result, so a factory output flows through the same path — and the
/// same duplicate detection — as any other provider.
pub(crate) type Registrar = Box<dyn FnOnce(ContainerBuilder) -> ContainerBuilder + Send>;
type FactoryFuture = Pin<Box<dyn Future<Output = Result<Registrar>> + Send>>;
pub(crate) type BoxedFactory = Box<dyn FnOnce(Container) -> FactoryFuture + Send>;

/// A piece of metadata attached to a provider, or free-standing if no host
/// is recorded. Discovered via [`crate::DiscoveryService::meta`].
#[derive(Clone)]
pub(crate) struct MetaEntry {
    pub(crate) provider_type_id: Option<TypeId>,
    pub(crate) meta: AnyArc,
}

#[derive(Clone, Default)]
pub struct Container {
    providers: Arc<HashMap<TypeId, AnyArc>>,
    metadata: Arc<HashMap<TypeId, Vec<MetaEntry>>>,
    /// Factories for request-scoped providers (`#[injectable(scope = request)]`).
    /// Never built into `providers`; a [`RequestScope`](crate::RequestScope)
    /// invokes one per request and caches the instance for that request.
    scoped: Arc<HashMap<TypeId, ScopedFactory>>,
}

impl Container {
    pub fn builder() -> ContainerBuilder {
        ContainerBuilder::default()
    }

    /// Resolve a provider by type. Returns `None` if no provider was registered for `T`.
    ///
    /// This is an **unchecked escape hatch** — the `ModuleRef.get()` analog. The
    /// container is flat and resolves by `TypeId` with no caller identity, so
    /// this bypasses the build-time access contract (see
    /// [`crate::access`]): it can fetch any registered provider regardless of the
    /// import graph. Prefer declarative `#[inject]`, which *is* under contract;
    /// reach for `get` only for genuinely dynamic resolution.
    pub fn get<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.providers
            .get(&TypeId::of::<T>())
            .and_then(|any| any.clone().downcast::<T>().ok())
    }

    /// Resolve a trait-object provider registered via [`ContainerBuilder::provide_dyn`].
    ///
    /// Like [`get`](Self::get), an **unchecked escape hatch** that bypasses the
    /// access contract — see its note.
    pub fn get_dyn<T: ?Sized + Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.providers
            .get(&TypeId::of::<Arc<T>>())
            .and_then(|any| any.clone().downcast::<Arc<T>>().ok())
            .map(|outer| (*outer).clone())
    }

    /// Crate-internal read of the metadata index. Public callers go through
    /// [`crate::DiscoveryService`].
    pub(crate) fn metadata_entries(&self, key: TypeId) -> Option<&Vec<MetaEntry>> {
        self.metadata.get(&key)
    }

    /// The request-scoped factory for `id`, if one was registered. Cloned (it is
    /// an `Arc`) so a [`RequestScope`](crate::RequestScope) can invoke it without
    /// holding a borrow on the container.
    pub(crate) fn scoped_factory(&self, id: TypeId) -> Option<ScopedFactory> {
        self.scoped.get(&id).cloned()
    }
}

#[derive(Default)]
pub struct ContainerBuilder {
    providers: HashMap<TypeId, AnyArc>,
    metadata: HashMap<TypeId, Vec<MetaEntry>>,
    /// `TypeId`s of [`Module`](crate::Module)s already registered (register
    /// phase) through this builder. Lets the same module imported via several
    /// paths register once.
    registered_modules: HashSet<TypeId>,
    /// `TypeId`s of modules already visited in the collect phase — the
    /// equivalent dedup for [`Module::collect`](crate::Module::collect).
    collected_modules: HashSet<TypeId>,
    /// Async factories awaiting their turn in [`AppBuilder::build`](crate::AppBuilder::build).
    /// Seeded by [`provide_factory`](Self::provide_factory), whether at the
    /// composition root or by a module's [`collect`](crate::Module::collect).
    /// Builder-only state: drained before [`build`](Self::build), never copied
    /// into the [`Container`] or a [`snapshot`](Self::snapshot).
    factories: Vec<BoxedFactory>,
    /// Request-scoped provider factories, copied into the built [`Container`].
    scoped: HashMap<TypeId, ScopedFactory>,
}

impl ContainerBuilder {
    /// Register a value; it will be wrapped in `Arc` internally.
    pub fn provide<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.warn_if_replacing(TypeId::of::<T>(), std::any::type_name::<T>());
        self.providers.insert(TypeId::of::<T>(), Arc::new(value));
        self
    }

    /// Register an already-shared `Arc<T>`. Useful when the same instance must
    /// be reused across modules.
    pub fn provide_arc<T: Any + Send + Sync>(mut self, value: Arc<T>) -> Self {
        self.warn_if_replacing(TypeId::of::<T>(), std::any::type_name::<T>());
        self.providers.insert(TypeId::of::<T>(), value);
        self
    }

    /// Replace a concrete provider without the override warning. The intentional
    /// counterpart of [`provide`](Self::provide) for a deliberate swap — used by
    /// [`AppBuilder::override_value`](crate::AppBuilder::override_value) so a test
    /// can substitute a mock without `nestrs::container` logging a collision it
    /// asked for.
    pub(crate) fn replace<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.providers.insert(TypeId::of::<T>(), Arc::new(value));
        self
    }

    /// Warn when a concrete-type registration silently replaces an earlier one.
    /// In a flat singleton container that usually means two modules registered
    /// the same type by mistake — the kind of collision NestJS's per-module scope
    /// hides but ours cannot. Trait-object bindings ([`provide_dyn`](Self::provide_dyn))
    /// are deliberately exempt: last-binding-wins is their documented override
    /// mechanism (an app replacing a library's default `dyn` provider).
    fn warn_if_replacing(&self, id: TypeId, type_name: &'static str) {
        if self.providers.contains_key(&id) {
            tracing::warn!(
                target: "nestrs::container",
                provider = type_name,
                "provider override: a value of this type was already registered and is being replaced",
            );
        }
    }

    /// Register a trait-object provider. Stored as `Arc<Arc<T>>` so the outer
    /// `Arc` is sized and retrievable via the trait's `TypeId`. Apps use this
    /// to bind a concrete type to a trait dependency injected elsewhere.
    pub fn provide_dyn<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.providers
            .insert(TypeId::of::<Arc<T>>(), Arc::new(value));
        self
    }

    /// Attach a piece of metadata of type `M` to the provider type `P`.
    /// Discovery scanners (HTTP transport, future cron module, …) iterate
    /// these via [`crate::DiscoveryService::meta`].
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

    /// Attach a piece of metadata not bound to a specific provider — e.g. a
    /// module-level config descriptor that a scanner aggregates globally.
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

    /// Whether a provider for `id` has already been registered. The `#[module]`
    /// macro checks a provider's declared
    /// [`Discoverable::dependencies`](crate::Discoverable::dependencies) against
    /// this before building it, so providers can be listed in any order.
    pub fn contains(&self, id: TypeId) -> bool {
        self.providers.contains_key(&id)
    }

    /// Record that a [`Module`](crate::Module) of type `id` is being registered.
    /// Returns `true` the first time (the caller should proceed) and `false`
    /// thereafter (the caller should skip). The `#[module]` macro calls this at
    /// the top of every generated `Module::register`, so a module pulled in
    /// through multiple import paths registers its providers exactly once.
    pub fn mark_registered(&mut self, id: TypeId) -> bool {
        self.registered_modules.insert(id)
    }

    /// The collect-phase counterpart of [`mark_registered`](Self::mark_registered):
    /// returns `true` the first time a module is visited for collection and
    /// `false` thereafter, so a diamond import collects its async factories once.
    pub fn mark_collected(&mut self, id: TypeId) -> bool {
        self.collected_modules.insert(id)
    }

    /// Queue an async factory that builds a provider of type `T` from the
    /// container assembled so far. Its awaited output is stored as a provider
    /// (so the module tree can inject `Arc<T>`). Called both at the composition
    /// root ([`AppBuilder::provide_factory`](crate::AppBuilder::provide_factory))
    /// and by a module's [`collect`](crate::Module::collect) for a resource it
    /// owns (a DB pool). The queue is drained by
    /// [`AppBuilder::build`](crate::AppBuilder::build) before providers are built.
    pub fn provide_factory<T, F, Fut>(mut self, factory: F) -> Self
    where
        T: Any + Send + Sync,
        F: FnOnce(Container) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.factories.push(Box::new(move |container| {
            Box::pin(async move {
                let value = factory(container).await?;
                let registrar: Registrar = Box::new(move |builder| builder.provide(value));
                Ok(registrar)
            })
        }));
        self
    }

    /// Register a **request-scoped** provider: instead of one shared singleton,
    /// `factory` builds a fresh `T` for each request, which a
    /// [`RequestScope`](crate::RequestScope) caches for that request's lifetime.
    /// Emitted by `#[injectable(scope = request)]`; the factory resolves the
    /// provider's `#[inject]` dependencies from the (singleton) root container,
    /// so a request-scoped provider may depend on singletons but not — in this
    /// model — on other request-scoped providers.
    pub fn provide_scoped<T, F>(mut self, factory: F) -> Self
    where
        T: Any + Send + Sync,
        F: Fn(&Container) -> T + Send + Sync + 'static,
    {
        let id = TypeId::of::<T>();
        if self.scoped.contains_key(&id) {
            tracing::warn!(
                target: "nestrs::container",
                provider = std::any::type_name::<T>(),
                "request-scoped provider override: a factory of this type was already registered and is being replaced",
            );
        }
        self.scoped.insert(
            id,
            Arc::new(move |container| Arc::new(factory(container)) as AnyArc),
        );
        self
    }

    /// Drain the queued async factories — called by `AppBuilder::build` once,
    /// after the collect phase, to run them in order.
    pub(crate) fn take_factories(&mut self) -> Vec<BoxedFactory> {
        std::mem::take(&mut self.factories)
    }

    /// Every provider key registered so far. `AppBuilder::build` snapshots this
    /// after the factory phase, before any module registers, to form the
    /// **global** set (seeds + factory outputs) the access-graph check treats as
    /// reachable from any module.
    pub(crate) fn provider_ids(&self) -> HashSet<TypeId> {
        self.providers.keys().copied().collect()
    }

    pub fn build(self) -> Container {
        Container {
            providers: Arc::new(self.providers),
            metadata: Arc::new(self.metadata),
            scoped: Arc::new(self.scoped),
        }
    }

    /// Take a snapshot of the providers registered so far. Used by `#[module]`
    /// to let a provider being built resolve its dependencies via the container
    /// while the builder is still under construction.
    pub fn snapshot(&self) -> Container {
        Container {
            providers: Arc::new(self.providers.clone()),
            metadata: Arc::new(self.metadata.clone()),
            scoped: Arc::new(self.scoped.clone()),
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
        // Overriding a concrete provider logs a warning (flat container), but the
        // last registration still wins — mirroring `provide_dyn`'s documented
        // last-binding-wins behaviour.
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
        // A distinct type is independent.
        assert!(builder.mark_registered(TypeId::of::<Marker>()));
    }
}

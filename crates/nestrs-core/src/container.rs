use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;

type AnyArc = Arc<dyn Any + Send + Sync>;

/// Builds a fresh instance of a request-scoped provider from the (singleton)
/// root container, invoked once per request by a
/// [`RequestScope`](crate::RequestScope).
pub(crate) type ScopedFactory = Arc<dyn Fn(&Container) -> AnyArc + Send + Sync>;

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

#[derive(Clone, Default)]
pub struct Container {
    providers: Arc<HashMap<TypeId, AnyArc>>,
    metadata: Arc<HashMap<TypeId, Vec<MetaEntry>>>,
    scoped: Arc<HashMap<TypeId, ScopedFactory>>,
}

impl Container {
    pub fn builder() -> ContainerBuilder {
        ContainerBuilder::default()
    }

    /// Resolve a provider by type. Returns `None` if no provider was registered.
    ///
    /// This is the `ModuleRef.get()` analog and bypasses the build-time access
    /// contract (see [`crate::access`]) — prefer declarative `#[inject]`.
    pub fn get<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.providers
            .get(&TypeId::of::<T>())
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

    pub(crate) fn metadata_entries(&self, key: TypeId) -> Option<&Vec<MetaEntry>> {
        self.metadata.get(&key)
    }

    pub(crate) fn scoped_factory(&self, id: TypeId) -> Option<ScopedFactory> {
        self.scoped.get(&id).cloned()
    }
}

#[derive(Default)]
pub struct ContainerBuilder {
    providers: HashMap<TypeId, AnyArc>,
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
}

impl ContainerBuilder {
    /// Register a value, wrapped in `Arc` internally.
    pub fn provide<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.warn_if_replacing(TypeId::of::<T>(), std::any::type_name::<T>());
        self.providers.insert(TypeId::of::<T>(), Arc::new(value));
        self
    }

    /// Register an already-shared `Arc<T>`.
    pub fn provide_arc<T: Any + Send + Sync>(mut self, value: Arc<T>) -> Self {
        self.warn_if_replacing(TypeId::of::<T>(), std::any::type_name::<T>());
        self.providers.insert(TypeId::of::<T>(), value);
        self
    }

    /// Replace a concrete provider without the override warning — the
    /// intentional swap path used by
    /// [`AppBuilder::override_value`](crate::AppBuilder::override_value).
    pub(crate) fn replace<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.providers.insert(TypeId::of::<T>(), Arc::new(value));
        self
    }

    /// Warn when a concrete-type registration silently replaces an earlier
    /// one — usually two modules registering the same type by mistake.
    /// Trait-object bindings ([`provide_dyn`](Self::provide_dyn)) are exempt:
    /// last-binding-wins is their documented override mechanism.
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
    /// `Arc` is sized and retrievable via the trait's `TypeId`.
    pub fn provide_dyn<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.providers
            .insert(TypeId::of::<Arc<T>>(), Arc::new(value));
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
        self.providers.contains_key(&id)
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
    /// each request, cached by a [`RequestScope`](crate::RequestScope).
    ///
    /// Emitted by `#[injectable(scope = request)]`. The factory resolves
    /// dependencies from the (singleton) root container, so a request-scoped
    /// provider may depend on singletons but not on other request-scoped
    /// providers.
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

    pub(crate) fn take_factories(&mut self) -> Vec<(TypeId, BoxedFactory)> {
        std::mem::take(&mut self.factories)
    }

    /// Provider keys registered so far. Snapshotted by `AppBuilder::build`
    /// after the factory phase to form the **global** set (seeds + factory
    /// outputs) for the access-graph check.
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

    /// Snapshot the providers registered so far. Used by `#[module]` to let a
    /// provider being built resolve its dependencies while the builder is
    /// still under construction.
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
}

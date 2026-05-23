use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

type AnyArc = Arc<dyn Any + Send + Sync>;

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
}

impl Container {
    pub fn builder() -> ContainerBuilder {
        ContainerBuilder::default()
    }

    /// Resolve a provider by type. Returns `None` if no provider was registered for `T`.
    pub fn get<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.providers
            .get(&TypeId::of::<T>())
            .and_then(|any| any.clone().downcast::<T>().ok())
    }

    /// Resolve a trait-object provider registered via [`ContainerBuilder::provide_dyn`].
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
}

#[derive(Default)]
pub struct ContainerBuilder {
    providers: HashMap<TypeId, AnyArc>,
    metadata: HashMap<TypeId, Vec<MetaEntry>>,
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

    pub fn build(self) -> Container {
        Container {
            providers: Arc::new(self.providers),
            metadata: Arc::new(self.metadata),
        }
    }

    /// Take a snapshot of the providers registered so far. Used by `#[module]`
    /// to let a provider being built resolve its dependencies via the container
    /// while the builder is still under construction.
    pub fn snapshot(&self) -> Container {
        Container {
            providers: Arc::new(self.providers.clone()),
            metadata: Arc::new(self.metadata.clone()),
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
}

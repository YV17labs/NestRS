use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

type AnyArc = Arc<dyn Any + Send + Sync>;

/// Type-keyed provider registry — the nestrs equivalent of Nest's IoC container.
#[derive(Clone, Default)]
pub struct Container {
    providers: Arc<HashMap<TypeId, AnyArc>>,
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
}

#[derive(Default)]
pub struct ContainerBuilder {
    providers: HashMap<TypeId, AnyArc>,
}

impl ContainerBuilder {
    /// Register a value; it will be wrapped in `Arc` internally.
    pub fn provide<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.providers.insert(TypeId::of::<T>(), Arc::new(value));
        self
    }

    /// Register an already-shared `Arc<T>`. Useful when the same instance must
    /// be reused across modules.
    pub fn provide_arc<T: Any + Send + Sync>(mut self, value: Arc<T>) -> Self {
        self.providers.insert(TypeId::of::<T>(), value);
        self
    }

    /// Register a trait-object provider. Stored as `Arc<Arc<T>>` so the outer
    /// `Arc` is sized and retrievable via the trait's `TypeId`. Apps use this
    /// to bind a concrete type to a trait dependency injected elsewhere.
    pub fn provide_dyn<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.providers
            .insert(TypeId::of::<Arc<T>>(), Arc::new(value));
        self
    }

    pub fn build(self) -> Container {
        Container {
            providers: Arc::new(self.providers),
        }
    }

    /// Take a snapshot of the providers registered so far. Used by `#[module]`
    /// to let a provider being built resolve its dependencies via the container
    /// while the builder is still under construction.
    pub fn snapshot(&self) -> Container {
        Container {
            providers: Arc::new(self.providers.clone()),
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
}

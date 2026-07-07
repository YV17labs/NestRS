//! Per-request resolution for request-scoped providers.
//!
//! The container is a flat singleton store; a `#[injectable(scope = request)]`
//! provider is the exception — built fresh per request and cached for that
//! request by a [`RequestScope`]. Non-scoped types fall through to the
//! singleton container.
//!
//! The model is one level deep: a request-scoped provider depends on
//! singletons, never on other request-scoped providers; singletons cannot
//! depend on a request-scoped provider (they're built before any request
//! exists). Reach a request-scoped provider through the request boundary
//! (`Scoped<T>`), never a `#[inject]` field on a singleton.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::Container;

type AnyArc = Arc<dyn Any + Send + Sync>;

/// Request-scoped resolution layer over the singleton [`Container`]. Built
/// once per request by the serving transport.
pub struct RequestScope {
    root: Container,
    cache: Mutex<HashMap<TypeId, AnyArc>>,
}

impl RequestScope {
    pub fn new(root: Container) -> Self {
        Self {
            root,
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn root(&self) -> &Container {
        &self.root
    }

    /// Resolve `T`. Request-scoped providers are built once and cached for
    /// this scope; transient providers are rebuilt on every call; non-scoped
    /// types fall through to the singleton container.
    pub fn get<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        let id = TypeId::of::<T>();
        if let Some(factory) = self.root.scoped_factory(id) {
            // Fast path: already built for this request.
            if let Some(any) = self.cache.lock().get(&id).cloned() {
                return any.downcast::<T>().ok();
            }
            // Build the provider *outside* the lock. The factory may
            // transitively resolve another request-scoped provider, which
            // re-enters this method; `cache` is a non-reentrant
            // `parking_lot::Mutex`, so building under the lock would deadlock
            // the request rather than resolve it. Double-checked insert: if a
            // concurrent or re-entrant resolution beat us to it, keep the
            // already-cached instance and drop ours (a rare extra build, never
            // a divergent cached instance).
            let built = factory(&self.root);
            let any = self.cache.lock().entry(id).or_insert(built).clone();
            return any.downcast::<T>().ok();
        }
        // Transients route through `Container::get` so the re-entrancy guard
        // catches a self-cycle, regardless of which surface initiates the call.
        self.root.get::<T>()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    struct Counter(u32);
    struct Greeter(&'static str);

    #[test]
    fn caches_a_scoped_provider_building_it_once() {
        let builds = Arc::new(AtomicU32::new(0));
        let builds_factory = builds.clone();
        let container = Container::builder()
            .provide_scoped::<Counter, _>(move |_| {
                Counter(builds_factory.fetch_add(1, Ordering::SeqCst))
            })
            .build();
        let scope = RequestScope::new(container);

        let first: Arc<Counter> = scope.get().expect("scoped provider resolves");
        let second: Arc<Counter> = scope.get().expect("scoped provider resolves again");

        // Built once for the request, then served from cache: the double-checked
        // insert must still return the *same* instance and run the factory once.
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(builds.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn scoped_factory_resolves_singleton_deps() {
        // The factory reads a singleton from the root container while the scope
        // lock is not held (the fix builds outside the lock) — a dependency
        // resolve inside the factory therefore never contends the cache mutex.
        let container = Container::builder()
            .provide(Greeter("hello"))
            .provide_scoped::<Counter, _>(|c| {
                let g: Arc<Greeter> = c.get().expect("singleton resolves inside factory");
                Counter(g.0.len() as u32)
            })
            .build();
        let scope = RequestScope::new(container);

        let resolved: Arc<Counter> = scope.get().expect("scoped provider resolves");
        assert_eq!(resolved.0, 5);
    }

    #[test]
    fn unscoped_types_fall_through_to_the_singleton_container() {
        let container = Container::builder().provide(Greeter("hi")).build();
        let scope = RequestScope::new(container);
        let resolved: Arc<Greeter> = scope.get().expect("singleton falls through");
        assert_eq!(resolved.0, "hi");
    }
}

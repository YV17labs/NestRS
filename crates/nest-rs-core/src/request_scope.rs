//! Per-request resolution for request-scoped providers.
//!
//! The container is a flat singleton store; a `#[injectable(scope = request)]`
//! provider is the exception — built fresh per request and cached for that
//! request by a [`RequestScope`]. Non-scoped types fall through to the
//! singleton container.
//!
//! A request-scoped provider may depend on singletons **and** on other
//! request-scoped providers (resolved through this scope, so they share one
//! per-request instance). The reverse is structurally impossible: a singleton
//! cannot depend on a request-scoped provider (singletons are built before any
//! request exists). Reach a request-scoped provider through the request
//! boundary (`Scoped<T>`), never a `#[inject]` field on a singleton.

use std::any::{Any, TypeId, type_name};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::Container;
use crate::cycle_guard::{BuildStack, Cycle, CycleGuard};

type AnyArc = Arc<dyn Any + Send + Sync>;

thread_local! {
    /// Re-entrancy guard for request-scoped resolution: a scoped provider that
    /// (transitively) depends on itself would recurse forever. We catch the
    /// second entry for the same `TypeId` and panic with a chain naming every
    /// type on the cycle (`A → B → A`).
    ///
    /// This is a **thread-local** (not a per-scope stack) on purpose: a scoped
    /// build chain is synchronous on one thread (`factory(scope)` calls
    /// `scope.get::<Dep>()` inline), so the cycle is always same-thread
    /// recursion — whereas a *legitimate* concurrent resolution of the same
    /// provider (two async-graphql fields polled on different worker threads)
    /// must not be mistaken for a cycle. A shared stack would raise a false
    /// positive there; a thread-local cannot.
    static SCOPED_BUILDING: BuildStack = const { RefCell::new(Vec::new()) };
}

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
            // transitively resolve another request-scoped provider through
            // `self`, which re-enters this method; `cache` is a non-reentrant
            // `parking_lot::Mutex`, so building under the lock would deadlock
            // the request rather than resolve it. The re-entrancy guard turns a
            // genuine *self*-cycle (a scoped provider that transitively depends
            // on itself) into a clear panic instead of an unbounded recursion.
            let _guard = CycleGuard::push(&SCOPED_BUILDING, id, type_name::<T>()).unwrap_or_else(
                |Cycle { chain }| {
                    panic!(
                        "request-scoped provider cycle: {chain} — break the cycle by injecting \
                         `Arc<dyn Trait>` or picking a different scope"
                    )
                },
            );
            // Pass the scope (not the bare root): a request-scoped dep of this
            // provider resolves through the same cache and is shared for the
            // request.
            let built = factory(self);
            drop(_guard);
            // Double-checked insert: if a concurrent resolution beat us to it,
            // keep the already-cached instance and drop ours (a rare extra
            // build, never a divergent cached instance).
            let any = self.cache.lock().entry(id).or_insert(built).clone();
            return any.downcast::<T>().ok();
        }
        // Transients route through `Container::get` so the re-entrancy guard
        // catches a self-cycle, regardless of which surface initiates the call.
        self.root.get::<T>()
    }

    /// Resolve a trait-object provider (`Arc<dyn Trait>`). Trait-object
    /// bindings are singleton-only, so this forwards straight to the root —
    /// the scope-aware constructor (`from_scope`) calls it for
    /// `#[inject] Arc<dyn Trait>` fields on a request-scoped provider.
    pub fn get_dyn<T: ?Sized + Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.root.get_dyn::<T>()
    }

    /// Resolve a **keyed** singleton (`#[inject(key = "…")]`). Keyed providers
    /// are singleton-only, so this forwards to the root.
    pub fn get_keyed<T: Any + Send + Sync>(&self, name: &'static str) -> Option<Arc<T>> {
        self.root.get_keyed::<T>(name)
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

    struct Inner(u32);
    struct Outer(Arc<Inner>);

    #[test]
    fn a_scoped_dep_of_a_scoped_provider_is_shared_within_one_request() {
        // WI-8: request→request deps. `Outer` (scoped) depends on `Inner`
        // (scoped), resolved through the scope. Building `Outer` then resolving
        // `Inner` directly must yield the *same* `Inner` — one per request,
        // built exactly once — proving the scoped factory resolves its deps
        // through the per-request cache, not the bare root.
        let builds = Arc::new(AtomicU32::new(0));
        let builds_factory = builds.clone();
        let container = Container::builder()
            .provide_scoped::<Inner, _>(move |_| Inner(builds_factory.fetch_add(1, Ordering::SeqCst)))
            .provide_scoped::<Outer, _>(|scope| {
                Outer(scope.get::<Inner>().expect("scoped dep resolves through the scope"))
            })
            .build();
        let scope = RequestScope::new(container);

        let outer: Arc<Outer> = scope.get().expect("outer resolves");
        let inner: Arc<Inner> = scope.get().expect("inner resolves");

        assert!(
            Arc::ptr_eq(&outer.0, &inner),
            "the scoped dep must be the same instance the outer provider received",
        );
        assert_eq!(
            builds.load(Ordering::SeqCst),
            1,
            "the shared scoped dep is built exactly once per request",
        );
    }

    #[test]
    fn scoped_instances_differ_across_requests() {
        // A fresh `RequestScope` is a fresh request: nothing carries over.
        let builds = Arc::new(AtomicU32::new(0));
        let builds_factory = builds.clone();
        let container = Container::builder()
            .provide_scoped::<Inner, _>(move |_| Inner(builds_factory.fetch_add(1, Ordering::SeqCst)))
            .build();

        let scope_a = RequestScope::new(container.clone());
        let scope_b = RequestScope::new(container);
        let a: Arc<Inner> = scope_a.get().expect("resolves in request A");
        let b: Arc<Inner> = scope_b.get().expect("resolves in request B");

        assert!(
            !Arc::ptr_eq(&a, &b),
            "two requests must not share a request-scoped instance",
        );
        assert_eq!(builds.load(Ordering::SeqCst), 2);
    }

    #[test]
    #[should_panic(expected = "request-scoped provider cycle")]
    fn scoped_self_dependency_panics_with_cycle_diagnostic() {
        let container = Container::builder()
            .provide_scoped::<Counter, _>(|scope| {
                // Resolving the same scoped provider inside its own factory
                // loops; the re-entrancy guard catches the second entry.
                let _self: Arc<Counter> = scope.get().expect("re-entrant resolution");
                Counter(0)
            })
            .build();
        let scope = RequestScope::new(container);
        let _ = scope.get::<Counter>();
    }

    #[test]
    fn scoped_transitive_cycle_diagnostic_lists_full_chain() {
        // A two-step cycle (A → B → A) must name BOTH types in order — a bug
        // printing only the type currently being built would be useless for
        // diagnosing which intermediate provider closes the loop.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let container = Container::builder()
                .provide_scoped::<Greeter, _>(|scope| {
                    let _b: Arc<Counter> = scope.get().expect("B resolves");
                    Greeter("A")
                })
                .provide_scoped::<Counter, _>(|scope| {
                    let _a: Arc<Greeter> = scope.get().expect("A resolves");
                    Counter(0)
                })
                .build();
            let scope = RequestScope::new(container);
            let _: Option<Arc<Greeter>> = scope.get();
        }));

        let payload = result.expect_err("the cycle must panic");
        let msg = payload
            .downcast_ref::<String>()
            .map(|s| s.as_str())
            .or_else(|| payload.downcast_ref::<&'static str>().copied())
            .unwrap_or("<non-string panic>");
        assert!(
            msg.contains("request-scoped provider cycle"),
            "missing prefix: {msg}",
        );
        assert!(msg.contains("Greeter"), "diagnostic must name A: {msg}");
        assert!(msg.contains("Counter"), "diagnostic must name B: {msg}");
        let greeter_at = msg.find("Greeter").unwrap();
        let counter_at = msg.find("Counter").unwrap();
        assert!(greeter_at < counter_at, "chain must read A then B: {msg}");
    }

    #[test]
    fn a_panicking_scoped_factory_clears_the_reentrancy_stack() {
        // A factory that panics must still pop its entry so the next resolution
        // on this thread is not poisoned with a spurious cycle diagnostic.
        let container = Container::builder()
            .provide_scoped::<Counter, _>(|_| -> Counter { panic!("boom from scoped factory") })
            .provide_scoped::<Greeter, _>(|_| Greeter("recovered"))
            .build();
        let scope = RequestScope::new(container);

        let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _: Option<Arc<Counter>> = scope.get();
        }));
        assert!(first.is_err(), "the factory panic propagates");

        // A different scoped provider on the same thread resolves cleanly —
        // proves the thread-local was not left poisoned by the prior panic.
        let resolved: Arc<Greeter> = scope
            .get()
            .expect("a different scoped provider resolves after a sibling factory panicked");
        assert_eq!(resolved.0, "recovered");
    }
}

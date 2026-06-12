use std::any::{Any, TypeId};
use std::cell::RefCell;
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

/// Builds a fresh instance of a transient provider on every resolution.
/// Emitted by `#[injectable(scope = transient)]`.
pub(crate) type TransientFactory = Arc<dyn Fn(&Container) -> AnyArc + Send + Sync>;

thread_local! {
    /// Re-entrancy guard for transient resolution: a transient provider that
    /// (transitively) injects itself would loop forever. We catch the cycle on
    /// the second entry for the same `TypeId` and panic with a clear message
    /// that names every type on the cycle path.
    ///
    /// The `&'static str` companion is the type name captured at push time so
    /// the panic diagnostic can render the full chain (`A → B → A`), not just
    /// the type currently being built.
    static TRANSIENT_BUILDING: RefCell<Vec<(TypeId, &'static str)>> = const { RefCell::new(Vec::new()) };
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

#[derive(Clone, Default)]
pub struct Container {
    providers: Arc<HashMap<TypeId, AnyArc>>,
    metadata: Arc<HashMap<TypeId, Vec<MetaEntry>>>,
    scoped: Arc<HashMap<TypeId, ScopedFactory>>,
    transient: Arc<HashMap<TypeId, TransientFactory>>,
}

impl Container {
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
            let any = build_transient(id, std::any::type_name::<T>(), factory, self);
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

    pub(crate) fn metadata_entries(&self, key: TypeId) -> Option<&Vec<MetaEntry>> {
        self.metadata.get(&key)
    }

    pub(crate) fn scoped_factory(&self, id: TypeId) -> Option<ScopedFactory> {
        self.scoped.get(&id).cloned()
    }
}

/// RAII drop guard for the transient re-entrancy stack: pushes on construction,
/// pops on drop (including panic unwind). Without this, a panicking factory
/// would leave the entry permanently on the thread-local stack, poisoning
/// every later resolution of the same transient on that thread with a spurious
/// cycle diagnostic.
struct TransientGuard {
    id: TypeId,
}

/// Signals the cycle detection at `push` time, carrying the rendered chain
/// (`A → B → A`) so the caller can panic with the full path, not just the
/// outer type. Stays an internal recoverable signal so the public API still
/// panics with a clear diagnostic.
struct TransientCycle {
    chain: String,
}

impl TransientGuard {
    fn push(id: TypeId, type_name: &'static str) -> Result<Self, TransientCycle> {
        TRANSIENT_BUILDING.with(|stack| {
            let mut s = stack.borrow_mut();
            if let Some(start) = s.iter().position(|(sid, _)| *sid == id) {
                // The cycle path is every entry from the first occurrence up
                // to the top of the stack, plus the offender being re-entered.
                let mut names: Vec<&'static str> = s[start..].iter().map(|(_, n)| *n).collect();
                names.push(type_name);
                let chain = names.join(" → ");
                return Err(TransientCycle { chain });
            }
            s.push((id, type_name));
            Ok(())
        })?;
        Ok(Self { id })
    }
}

impl Drop for TransientGuard {
    fn drop(&mut self) {
        // `rposition` + `swap_remove` rather than `pop`: even though transient
        // resolution on a single thread is sequential, this stays correct if a
        // future change ever interleaves entries on the same thread (e.g. a
        // factory recursing into a *different* transient).
        TRANSIENT_BUILDING.with(|stack| {
            let mut s = stack.borrow_mut();
            if let Some(pos) = s.iter().rposition(|(sid, _)| *sid == self.id) {
                s.swap_remove(pos);
            }
        });
    }
}

/// Resolve a transient provider with re-entrancy detection. A transient that
/// (transitively) injects itself would recurse forever; we catch the second
/// entry for the same `TypeId` and panic with a chain naming every type on
/// the cycle.
///
/// The push/pop pairing is panic-safe via [`TransientGuard`]: a factory that
/// panics still pops the stack as the guard unwinds.
fn build_transient(
    id: TypeId,
    type_name: &'static str,
    factory: &TransientFactory,
    container: &Container,
) -> AnyArc {
    let _guard = TransientGuard::push(id, type_name).unwrap_or_else(|TransientCycle { chain }| {
        panic!(
            "transient provider cycle: {chain} — break the cycle by injecting `Arc<dyn Trait>` or picking a different scope"
        )
    });
    factory(container)
    // `_guard` drops here — pops the stack even if `factory` panics and the
    // value path above is skipped.
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
    transient: HashMap<TypeId, TransientFactory>,
}

impl ContainerBuilder {
    /// Register a value, wrapped in `Arc` internally.
    pub fn provide<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.warn_if_replacing(TypeId::of::<T>(), std::any::type_name::<T>());
        self.warn_if_cross_kind_singleton(TypeId::of::<T>(), std::any::type_name::<T>());
        self.providers.insert(TypeId::of::<T>(), Arc::new(value));
        self
    }

    /// Register an already-shared `Arc<T>`.
    pub fn provide_arc<T: Any + Send + Sync>(mut self, value: Arc<T>) -> Self {
        self.warn_if_replacing(TypeId::of::<T>(), std::any::type_name::<T>());
        self.warn_if_cross_kind_singleton(TypeId::of::<T>(), std::any::type_name::<T>());
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

    /// Replace a concrete provider with a pre-shared `Arc<T>` without the
    /// override warning — the intentional swap path used by
    /// [`AppBuilder::override_provider`](crate::AppBuilder::override_provider).
    pub(crate) fn replace_arc<T: Any + Send + Sync>(mut self, value: Arc<T>) -> Self {
        self.providers.insert(TypeId::of::<T>(), value);
        self
    }

    /// Warn when a concrete-type registration silently replaces an earlier
    /// one — usually two modules registering the same type by mistake.
    /// Trait-object bindings ([`provide_dyn`](Self::provide_dyn)) are exempt:
    /// last-binding-wins is their documented override mechanism.
    fn warn_if_replacing(&self, id: TypeId, type_name: &'static str) {
        if self.providers.contains_key(&id) {
            tracing::warn!(
                target: "nest_rs::container",
                provider = type_name,
                "provider override",
            );
        }
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
        if self.providers.contains_key(&id) {
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
                target: "nest_rs::container",
                provider = std::any::type_name::<T>(),
                kind = "request_scoped",
                "provider override",
            );
        }
        self.scoped.insert(
            id,
            Arc::new(move |container| Arc::new(factory(container)) as AnyArc),
        );
        self
    }

    /// Register a transient provider: `factory` builds a fresh `T` every time
    /// `Container::get::<T>()` (or a [`RequestScope`](crate::RequestScope))
    /// resolves it. There is no caching — same scope, multiple resolutions,
    /// different instances.
    ///
    /// Emitted by `#[injectable(scope = transient)]`. A transient may depend
    /// on singletons or request-scoped providers; a transient depending
    /// (transitively) on itself panics at resolution with a cycle diagnostic.
    pub fn provide_transient<T, F>(mut self, factory: F) -> Self
    where
        T: Any + Send + Sync,
        F: Fn(&Container) -> T + Send + Sync + 'static,
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
            transient: Arc::new(self.transient),
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

//! Per-request resolution for request-scoped providers, cached per request by
//! a [`RequestScope`]; non-scoped types fall through to the singleton container.
//!
//! A request-scoped provider may depend on singletons and on other
//! request-scoped providers (sharing the request's instance); a singleton never
//! depends on one — reach it through the request boundary (`Scoped<T>`).

use std::any::{Any, TypeId, type_name};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::Container;
use crate::cycle_guard::{BuildStack, Cycle, CycleGuard};

type AnyArc = Arc<dyn Any + Send + Sync>;

/// The ambient per-request context a transport edge installs around its
/// inner tree, as a task-local.
pub(crate) struct RequestCtx {
    /// The DI scope, when the edge had a container to open one over; without
    /// one only `Scoped<T>` goes without.
    scope: Option<Arc<RequestScope>>,
    /// What this unit of work is filed under — its id, and who, once
    /// authentication has resolved anyone.
    pub(crate) correlation: crate::Correlation,
}

tokio::task_local! {
    /// Behind an `Arc`: a streaming response body re-installs it on every poll.
    static REQUEST_CTX: Arc<RequestCtx>;
}

/// Read one field off the ambient context, or `None` off the request task.
pub(crate) fn current_request_ctx<T>(read: impl FnOnce(&Arc<RequestCtx>) -> T) -> Option<T> {
    REQUEST_CTX.try_with(read).ok()
}

/// The current request's [`RequestScope`], installed by the transport edge.
/// `None` off the request task (or before the edge — a transport wiring bug).
pub fn current_request_scope() -> Option<Arc<RequestScope>> {
    current_request_ctx(|ctx| ctx.scope.clone()).flatten()
}

/// Run `fut` under an ambient request context — the transport edges' installer,
/// and the seam for driving handlers outside a transport (in-process test
/// harnesses, a transport's mirror of the edge).
///
/// `scope` is optional — an edge may have no container to open one over — and
/// `correlation` is not: whoever accepts a unit of work decides its identity
/// ([`Correlation`](crate::Correlation)).
pub async fn with_request_scope<F: std::future::Future>(
    scope: Option<Arc<RequestScope>>,
    correlation: crate::Correlation,
    fut: F,
) -> F::Output {
    RequestContinuation::new(scope, correlation)
        .scope(fut)
        .await
}

/// The ambient request context, held so work that continues the **same** unit
/// after the future that accepted it has returned — a streaming response body,
/// written after the handler with its task-locals unwound — can re-install it.
///
/// It carries the request's scope, so it is sound only for the same request: a
/// continuation outliving it (a WebSocket after its upgrade) inherits the
/// [`Correlation`](crate::Correlation) alone and opens its own scope through
/// [`with_request_scope`].
#[derive(Clone)]
pub struct RequestContinuation(Arc<RequestCtx>);

impl RequestContinuation {
    /// Build the context an edge is about to install — [`scope`](Self::scope)
    /// around the handler, [`enter`](Self::enter) around the body.
    pub fn new(scope: Option<Arc<RequestScope>>, correlation: crate::Correlation) -> Self {
        Self(Arc::new(RequestCtx { scope, correlation }))
    }

    /// Capture whatever context is ambient, to re-install around work that
    /// continues this same unit on another task; `None` off a request task.
    pub fn current() -> Option<Self> {
        current_request_ctx(|ctx| Self(Arc::clone(ctx)))
    }

    /// Run `fut` under this context — the async installer every edge reaches
    /// through [`with_request_scope`].
    pub async fn scope<F: std::future::Future>(&self, fut: F) -> F::Output {
        REQUEST_CTX.scope(Arc::clone(&self.0), fut).await
    }

    /// Run `f` under this context — `current_trace_id()`, `current_actor_id()`
    /// and [`current_request_scope`] answer what they answered inside the
    /// handler. Synchronous, since a body's `poll` is not a future.
    pub fn enter<T>(&self, f: impl FnOnce() -> T) -> T {
        REQUEST_CTX.sync_scope(Arc::clone(&self.0), f)
    }
}

/// Everything a unit of work has to carry across a **task boundary**, captured
/// where the work is handed off and re-installed where it runs.
///
/// Both the span and the request context cross: the span alone leaves
/// [`current_trace_id`](crate::current_trace_id) answering `None` in the
/// spawned work. A guard spawning from `Drop` captures at construction, since a
/// dropped future may be dropped on another task.
#[derive(Clone)]
pub struct TaskContext {
    span: tracing::Span,
    request: Option<RequestContinuation>,
}

impl TaskContext {
    /// Capture whatever span and request context are ambient right now.
    pub fn current() -> Self {
        Self {
            span: tracing::Span::current(),
            request: RequestContinuation::current(),
        }
    }

    /// The captured span, for the events a hand-off point emits synchronously
    /// before it spawns.
    pub fn span(&self) -> &tracing::Span {
        &self.span
    }

    /// Wrap `fut` so it runs under the captured span and request context — what
    /// goes to `spawn`, which starts with neither.
    pub async fn carry<F: std::future::Future>(self, fut: F) -> F::Output {
        let Self { span, request } = self;
        let carried = async move {
            match request {
                Some(request) => request.scope(fut).await,
                None => fut.await,
            }
        };
        tracing::Instrument::instrument(carried, span).await
    }
}

thread_local! {
    /// Re-entrancy guard for request-scoped resolution. Thread-local, not per
    /// scope: a build chain is synchronous on one thread, while two fields
    /// resolving the same provider on two threads is no cycle.
    static SCOPED_BUILDING: BuildStack = const { RefCell::new(Vec::new()) };
}

/// Request-scoped resolution layer over the singleton [`Container`]. Built
/// once per request by the serving transport.
pub struct RequestScope {
    root: Container,
    cache: Mutex<HashMap<TypeId, AnyArc>>,
}

impl RequestScope {
    /// Open a fresh request scope over the singleton container — one per request.
    pub fn new(root: Container) -> Self {
        Self {
            root,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// The underlying singleton container, for resolving non-scoped providers.
    pub fn root(&self) -> &Container {
        &self.root
    }

    /// Resolve `T`. Request-scoped providers are built once and cached for
    /// this scope; transient providers are rebuilt on every call; non-scoped
    /// types fall through to the singleton container.
    pub fn get<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        let id = TypeId::of::<T>();
        if let Some(factory) = self.root.scoped_factory(id) {
            if let Some(any) = self.cache.lock().get(&id).cloned() {
                return any.downcast::<T>().ok();
            }
            // Built outside the lock: the factory may re-enter `get` through
            // `self`, and `cache` is a non-reentrant `parking_lot::Mutex`.
            #[expect(
                clippy::panic,
                reason = "a provider cycle is a wiring defect, and resolving from a scope has no Result to report it through"
            )]
            let _guard = CycleGuard::push(&SCOPED_BUILDING, id, type_name::<T>()).unwrap_or_else(
                |Cycle { chain }| {
                    panic!(
                        "request-scoped provider cycle: {chain} — break the cycle by injecting \
                         `Arc<dyn Trait>` or picking a different scope"
                    )
                },
            );
            let built = factory(self);
            drop(_guard);
            // A concurrent resolution may have won: keep its instance.
            let any = self.cache.lock().entry(id).or_insert(built).clone();
            return any.downcast::<T>().ok();
        }
        // Through this scope, not the root, so a transient's request-scoped
        // deps resolve to the request's instance.
        if let Some(factory) = self.root.transient_factory(id) {
            let any = crate::container::build_transient(id, type_name::<T>(), &factory, self);
            return any.downcast::<T>().ok();
        }
        self.root.get::<T>()
    }

    /// Resolve a trait-object provider (`Arc<dyn Trait>`) — singleton-only, so
    /// forwarded to the root.
    pub fn get_dyn<T: ?Sized + Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.root.get_dyn::<T>()
    }

    /// Resolve a **keyed** singleton (`#[inject(key = "…")]`), forwarded to
    /// the root.
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

    #[tokio::test]
    async fn a_continuation_reinstalls_the_context_the_handler_ran_under() {
        let scope = Arc::new(RequestScope::new(
            Container::builder().provide(Greeter("hi")).build(),
        ));
        let correlation = crate::Correlation::minted(None);
        let trace_id = correlation.trace_id();

        let continuation =
            with_request_scope(Some(Arc::clone(&scope)), correlation.clone(), async move {
                RequestContinuation::new(Some(scope), correlation)
            })
            .await;

        assert!(crate::current_trace_id().is_none());

        continuation.enter(|| {
            assert_eq!(crate::current_trace_id(), Some(trace_id));
            assert!(
                current_request_scope().is_some_and(|s| s.get::<Greeter>().is_some()),
                "the request's own scope answers, not a fresh one",
            );
        });

        assert!(crate::current_trace_id().is_none());
    }

    #[tokio::test]
    async fn a_continuation_sees_an_actor_resolved_after_it_was_built() {
        let scope = Arc::new(RequestScope::new(Container::builder().build()));
        let correlation = crate::Correlation::minted(None);
        let continuation = RequestContinuation::new(Some(scope.clone()), correlation.clone());

        with_request_scope(Some(scope), correlation, async {
            crate::set_actor_id("alice-42");
        })
        .await;

        continuation.enter(|| {
            assert_eq!(crate::current_actor_id().as_deref(), Some("alice-42"));
        });
    }

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

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(builds.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn scoped_factory_resolves_singleton_deps() {
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
        let builds = Arc::new(AtomicU32::new(0));
        let builds_factory = builds.clone();
        let container = Container::builder()
            .provide_scoped::<Inner, _>(move |_| {
                Inner(builds_factory.fetch_add(1, Ordering::SeqCst))
            })
            .provide_scoped::<Outer, _>(|scope| {
                Outer(
                    scope
                        .get::<Inner>()
                        .expect("scoped dep resolves through the scope"),
                )
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
    fn a_transient_can_depend_on_a_request_scoped_provider() {
        struct Dep;
        struct Trans(Arc<Dep>);

        let builds = Arc::new(AtomicU32::new(0));
        let builds_factory = builds.clone();
        let container = Container::builder()
            .provide_scoped::<Dep, _>(move |_| {
                builds_factory.fetch_add(1, Ordering::SeqCst);
                Dep
            })
            .provide_transient::<Trans, _>(|scope| {
                Trans(
                    scope
                        .get::<Dep>()
                        .expect("the request-scoped dep resolves through the scope"),
                )
            })
            .build();
        let scope = RequestScope::new(container);

        let a: Arc<Trans> = scope.get().expect("transient resolves");
        let b: Arc<Trans> = scope.get().expect("transient resolves again");

        assert!(
            !Arc::ptr_eq(&a, &b),
            "a transient is rebuilt on every resolution",
        );
        assert!(
            Arc::ptr_eq(&a.0, &b.0),
            "both transients share the request's one request-scoped instance",
        );
        assert_eq!(
            builds.load(Ordering::SeqCst),
            1,
            "the request-scoped dep is built exactly once per request",
        );
    }

    #[test]
    fn scoped_instances_differ_across_requests() {
        let builds = Arc::new(AtomicU32::new(0));
        let builds_factory = builds.clone();
        let container = Container::builder()
            .provide_scoped::<Inner, _>(move |_| {
                Inner(builds_factory.fetch_add(1, Ordering::SeqCst))
            })
            .build();

        let scope_a = RequestScope::new(container.clone());
        let scope_b = RequestScope::new(container);
        let a: Arc<Inner> = scope_a.get().expect("resolves in request A");
        let b: Arc<Inner> = scope_b.get().expect("resolves in request B");

        assert!(
            !Arc::ptr_eq(&a, &b),
            "two requests must not share a request-scoped instance",
        );
        assert_eq!((a.0, b.0), (0, 1), "each request gets its own build");
        assert_eq!(builds.load(Ordering::SeqCst), 2);
    }

    #[test]
    #[should_panic(expected = "request-scoped provider cycle")]
    fn scoped_self_dependency_panics_with_cycle_diagnostic() {
        let container = Container::builder()
            .provide_scoped::<Counter, _>(|scope| {
                let _self: Arc<Counter> = scope.get().expect("re-entrant resolution");
                Counter(0)
            })
            .build();
        let scope = RequestScope::new(container);
        let _ = scope.get::<Counter>();
    }

    #[test]
    fn scoped_transitive_cycle_diagnostic_lists_full_chain() {
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
        let container = Container::builder()
            .provide_scoped::<Counter, _>(|_| -> Counter { panic!("boom from scoped factory") })
            .provide_scoped::<Greeter, _>(|_| Greeter("recovered"))
            .build();
        let scope = RequestScope::new(container);

        let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _: Option<Arc<Counter>> = scope.get();
        }));
        assert!(first.is_err(), "the factory panic propagates");

        let resolved: Arc<Greeter> = scope
            .get()
            .expect("a different scoped provider resolves after a sibling factory panicked");
        assert_eq!(resolved.0, "recovered");
    }
}

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

/// The ambient per-request context a transport edge installs around its
/// inner tree. A task-local instead of request extensions: an extension
/// costs one boxed insert each — and the first insert allocates the whole
/// per-request anymap — while a task-local scope is a stack cell. Guards,
/// extractors and handlers all run inside the transport's endpoint task, so
/// the scope provably covers them (the same pattern as the ambient executor
/// and ability).
pub(crate) struct RequestCtx {
    /// The DI scope, when the edge had a container to open one over. `None` is
    /// a real answer rather than a degenerate one: an edge may accept a unit of
    /// work with no container in hand (an MCP endpoint mounted outside the
    /// transport edge, a WS gateway on a bare upgrade), and the correlation is
    /// the primitive that cannot be optional — so it is installed either way and
    /// only `Scoped<T>` goes without.
    scope: Option<Arc<RequestScope>>,
    /// What this unit of work is filed under — its id, and who, once
    /// authentication has resolved anyone. Ambient for the same reason the
    /// scope is: the queue producer that copies it into a job envelope, the
    /// access log that files the line and a service stamping `created_by` are
    /// unrelated call sites, and threading an argument through all of them is
    /// how a field ends up missing at the fourth.
    pub(crate) correlation: crate::Correlation,
}

tokio::task_local! {
    /// Behind an `Arc` so re-installing it is one refcount bump rather than a
    /// deep clone of every handle it holds — a streaming response body does that
    /// on every poll, for as long as the stream runs.
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
/// **`scope` is an `Option` and `correlation` is not**, and the asymmetry is the
/// whole contract. An edge may accept a unit of work with no container to open a
/// scope over — an MCP endpoint mounted outside the transport edge, a gateway on
/// a bare upgrade — and only `Scoped<T>` goes without. The correlation has no
/// default: **whoever accepts a unit of work decides its identity**, by
/// continuing one that arrived with the work or by starting one
/// ([`Correlation`](crate::Correlation)). A default would let an edge install a
/// scope while quietly leaving its events uncorrelated, which is the failure this
/// seam exists to make impossible.
///
/// The `Option` lives here rather than at the edges because it used to be two
/// installers, and every edge whose scope was optional re-derived the same match
/// — one of them dropping the correlation on the scope-less arm, which made
/// `current_trace_id()`'s answer depend on how the endpoint happened to be
/// mounted.
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
/// after the future that accepted it has returned can re-install it.
///
/// # Why a unit of work outlives its handler
///
/// An `async fn` returning is not the work ending. An HTTP handler returns a
/// *response*, and a response with a streaming body — Server-Sent Events, a
/// download, anything built on a `Stream` — is written afterwards, by the
/// transport's connection task, with every task-local the edge installed already
/// unwound. Code inside that stream is still serving the request the handler was
/// serving, so `current_trace_id()` answering `None` there is the framework
/// contradicting itself: the id is the framework's primitive precisely so that
/// "which request is this?" has one answer everywhere the framework carries
/// work, and a body being written is work being carried.
///
/// [`enter`](Self::enter) is synchronous because that is the shape a body has:
/// a `poll` is not a future, so the context is re-installed around each poll —
/// the same thing [`crate::tracing::Instrument`] does with
/// a span, for the same reason.
///
/// **Identity, not resources.** What continues here is the whole ambient
/// context, and that is sound only because the continuation is the *same*
/// request: the scope's cache is this request's, and the response ends when the
/// request does. An edge whose continuation genuinely outlives the request — a
/// WebSocket that stays open for hours after its upgrade answered `101` —
/// inherits the [`Correlation`](crate::Correlation) alone and opens its own
/// scope — which is what a gateway does by calling [`with_request_scope`] again
/// with the upgrade's correlation and a scope of its own, rather than
/// continuing this one.
#[derive(Clone)]
pub struct RequestContinuation(Arc<RequestCtx>);

impl RequestContinuation {
    /// Build the context an edge is about to install, so the same edge can
    /// re-install it around the response body it hands back.
    ///
    /// The arguments are [`with_request_scope`]'s, deliberately: an edge builds
    /// one value and uses it twice — [`scope`](Self::scope) around the handler,
    /// [`enter`](Self::enter) around the body — rather than assembling the
    /// context twice and having the two spellings drift.
    pub fn new(scope: Option<Arc<RequestScope>>, correlation: crate::Correlation) -> Self {
        Self(Arc::new(RequestCtx { scope, correlation }))
    }

    /// Capture whatever context is ambient, to re-install around work that
    /// continues this same unit on another task.
    ///
    /// This is the shape a **task boundary** wants — a spawned dataloader batch,
    /// any `spawn` the framework adds later — because it takes no arguments and
    /// therefore cannot drift from the install it mirrors. `None` off a request
    /// task, where there is nothing to continue.
    pub fn current() -> Option<Self> {
        current_request_ctx(|ctx| Self(Arc::clone(ctx)))
    }

    /// Run `fut` under this context — the async installer every edge reaches
    /// through [`with_request_scope`].
    pub async fn scope<F: std::future::Future>(&self, fut: F) -> F::Output {
        REQUEST_CTX.scope(Arc::clone(&self.0), fut).await
    }

    /// Run `f` under this context — `current_trace_id()`, `current_actor_id()`
    /// and [`current_request_scope`] all answer exactly what they answered
    /// inside the handler.
    ///
    /// Synchronous because that is the shape a response body has: a `poll` is not
    /// a future. One refcount bump per poll, which is what the `Arc` is for — a
    /// streaming response polls this once per chunk for as long as it runs.
    pub fn enter<T>(&self, f: impl FnOnce() -> T) -> T {
        REQUEST_CTX.sync_scope(Arc::clone(&self.0), f)
    }
}

/// Everything a unit of work has to carry across a **task boundary**, captured
/// where the work is handed off and re-installed where it runs.
///
/// **Both halves cross, and neither substitutes for the other.** The span is
/// what puts `trace_id` on the events the spawned work emits; the ambient
/// context is what makes [`current_trace_id`](crate::current_trace_id) answer
/// inside it, and a queue push from that work seal the right envelope. Carrying
/// only the span leaves the events *looking* correlated while every accessor
/// below answers `None` — the more expensive of the two failures, because it
/// reads as covered.
///
/// Capture and application are separate steps on purpose: a guard that spawns
/// its cleanup from `Drop` must capture at construction, since a dropped future
/// is not guaranteed to be dropped on the task that owned it.
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
    /// before it spawns — those belong to the same unit of work as the spawned
    /// half, and `enter`ing it is how they get there.
    pub fn span(&self) -> &tracing::Span {
        &self.span
    }

    /// Wrap `fut` so it runs under the captured span and request context.
    ///
    /// The returned future is what goes to `spawn`: a bare `tokio::spawn`
    /// starts with an empty span stack *and* empty task-locals, so work handed
    /// to one without this is rooted at nothing.
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
        // Transient: rebuilt on every call, but resolved through **this** scope
        // (not the bare root) so its `#[inject]` deps see the request — a
        // request-scoped dep resolves to the request's shared instance rather
        // than panicking or building a request-of-one. The shared re-entrancy
        // guard inside `build_transient` still catches a self-cycle.
        if let Some(factory) = self.root.transient_factory(id) {
            let any = crate::container::build_transient(id, type_name::<T>(), &factory, self);
            return any.downcast::<T>().ok();
        }
        // Neither scoped nor transient: a plain singleton falls through.
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

    /// The rule the response body rests on: a *synchronous* continuation of the
    /// same unit of work reads the same ambient answers the handler read.
    ///
    /// Without it a `#[sse]` stream — polled by the connection task long after
    /// the handler returned — logs under no trace at all, which is the one thing
    /// the correlation primitive exists to make impossible.
    #[tokio::test]
    async fn a_continuation_reinstalls_the_context_the_handler_ran_under() {
        let scope = Arc::new(RequestScope::new(
            Container::builder().provide(Greeter("hi")).build(),
        ));
        let correlation = crate::Correlation::mint();
        let trace_id = correlation.trace_id();

        let continuation =
            with_request_scope(Some(Arc::clone(&scope)), correlation.clone(), async move {
                RequestContinuation::new(Some(scope), correlation)
            })
            .await;

        // Off the request task entirely — exactly where a body is polled.
        assert!(crate::current_trace_id().is_none());

        continuation.enter(|| {
            assert_eq!(crate::current_trace_id(), Some(trace_id));
            assert!(
                current_request_scope().is_some_and(|s| s.get::<Greeter>().is_some()),
                "the request's own scope answers, not a fresh one",
            );
        });

        // And it is scoped to the closure: nothing leaks onto the task after.
        assert!(crate::current_trace_id().is_none());
    }

    /// The actor is shared state on the [`Correlation`](crate::Correlation), so
    /// a principal the guard resolved *after* the continuation was built is
    /// still what the continuation reports — which is what lets the access line
    /// name who was served.
    #[tokio::test]
    async fn a_continuation_sees_an_actor_resolved_after_it_was_built() {
        let scope = Arc::new(RequestScope::new(Container::builder().build()));
        let correlation = crate::Correlation::mint();
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
        // B-CORE: a transient whose `#[inject]` dep is request-scoped must
        // resolve through the scope — not panic on a missing provider. Two
        // resolutions in one request rebuild the transient but SHARE the single
        // request-scoped instance (the transient's factory runs against `self`).
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
        // A fresh `RequestScope` is a fresh request: nothing carries over.
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

//! Per-request resolution for request-scoped providers, cached per request by
//! a [`RequestScope`]; non-scoped types fall through to the singleton container.
//!
//! A request-scoped provider may depend on singletons and on other
//! request-scoped providers (sharing the request's instance); a singleton never
//! depends on one — reach it through the request boundary (`Scoped<T>`).

use std::any::{Any, TypeId, type_name};
use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use parking_lot::Mutex;
use tokio::task::futures::TaskLocalFuture;

use crate::Container;
use crate::ambient::{Continuation, Entries};
use crate::cycle_guard::{BuildStack, Cycle, CycleGuard};

type AnyArc = Arc<dyn Any + Send + Sync>;

/// The ambient per-request context a transport edge installs around its
/// inner tree, as a task-local.
pub(crate) struct RequestCtx {
    /// The DI scope, when the edge had a container to open one over; without
    /// one only `Scoped<T>` goes without.
    scope: Option<Arc<RequestScope>>,
    /// What this unit of work is filed under — its id, and who, once
    /// authentication has resolved anyone. `None` where values were added with
    /// no unit running.
    pub(crate) correlation: Option<crate::Correlation>,
    /// What the unit carries for the code it calls.
    pub(crate) ambient: Entries,
    /// The unit this one runs nested in — a GraphQL field inside its request —
    /// so a batch they share carries the outer unit's values alone.
    parent: Option<Arc<RequestCtx>>,
}

impl RequestCtx {
    fn opened(scope: Option<Arc<RequestScope>>, correlation: crate::Correlation) -> Self {
        Self {
            scope,
            correlation: Some(correlation),
            ambient: Entries::default(),
            parent: None,
        }
    }

    /// The outermost unit this one is nested in, itself when it is not.
    fn unit_root(self: &Arc<Self>) -> &Arc<Self> {
        let mut unit = self;
        while let Some(parent) = &unit.parent {
            unit = parent;
        }
        unit
    }

    /// What crosses into `into`: never the nested unit's own values on a
    /// `Batch`, never the DI scope where the continuation opens its own, and
    /// each value through its `carry`. Flat, so nothing it did not carry stays
    /// reachable through a parent.
    fn captured(self: &Arc<Self>, into: Continuation) -> Arc<Self> {
        let unit = match into {
            Continuation::Batch => self.unit_root(),
            _ => self,
        };
        let scope = match into {
            Continuation::Batch | Continuation::Connection | Continuation::Envelope => None,
            _ => unit.scope.clone(),
        };
        let ambient = unit.ambient.carried(into);
        let same_scope = match (&scope, &unit.scope) {
            (Some(kept), Some(held)) => Arc::ptr_eq(kept, held),
            (kept, held) => kept.is_none() && held.is_none(),
        };
        if ambient.is_none() && same_scope && unit.parent.is_none() {
            return Arc::clone(unit);
        }
        Arc::new(Self {
            scope,
            correlation: unit.correlation.clone(),
            ambient: ambient.unwrap_or_else(|| unit.ambient.clone()),
            parent: None,
        })
    }
}

tokio::task_local! {
    /// Behind an `Arc`: a streaming response body re-installs it on every poll.
    static REQUEST_CTX: Arc<RequestCtx>;
}

/// Read one field off the ambient context, or `None` off the request task.
pub(crate) fn current_request_ctx<T>(read: impl FnOnce(&Arc<RequestCtx>) -> T) -> Option<T> {
    REQUEST_CTX.try_with(read).ok()
}

/// Read the running unit's correlation, or `None` where no unit is running.
pub(crate) fn current_correlation_with<T>(
    read: impl FnOnce(&crate::Correlation) -> T,
) -> Option<T> {
    current_request_ctx(|ctx| ctx.correlation.as_ref().map(read)).flatten()
}

/// Run `fut` with what `added` returns merged into what the running unit
/// carries where `fut` first runs, in one install — none when every value is
/// already the one carried.
pub(crate) fn install<F: Future>(
    added: impl FnOnce() -> Entries,
    fut: F,
) -> impl Future<Output = F::Output> {
    Install::new(move || extended(added()), fut)
}

pin_project_lite::pin_project! {
    /// `fut` under the context `derive` returns at its first poll, so an install
    /// built as another's argument merges into what the outer one installed.
    ///
    /// Not an `async fn`: one keeps `fut` beside the future it builds from it,
    /// and a debug build copies `fut` into a poll frame that stays on the stack
    /// while `fut` runs — one more copy for every install nested around it.
    #[project = InstallProj]
    #[project_replace = InstallOwned]
    enum Install<D, F> {
        Unpolled { derive: D, fut: F },
        Scoped { #[pin] fut: TaskLocalFuture<Arc<RequestCtx>, F> },
        Unchanged { #[pin] fut: F },
        Deriving,
    }
}

impl<D, F> Install<D, F>
where
    D: FnOnce() -> Option<Arc<RequestCtx>>,
    F: Future,
{
    fn new(derive: D, fut: F) -> Self {
        Self::Unpolled { derive, fut }
    }

    /// Derive the context and move `fut` under it, in a frame of its own: the
    /// copies of `fut` this makes are off the stack before `fut` is polled.
    #[inline(never)]
    fn start(mut self: Pin<&mut Self>) {
        if let InstallOwned::Unpolled { derive, fut } =
            self.as_mut().project_replace(Self::Deriving)
        {
            self.set(match derive() {
                Some(ctx) => Self::Scoped {
                    fut: REQUEST_CTX.scope(ctx, fut),
                },
                None => Self::Unchanged { fut },
            });
        }
    }
}

impl<D, F> Future for Install<D, F>
where
    D: FnOnce() -> Option<Arc<RequestCtx>>,
    F: Future,
{
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        if let InstallProj::Unpolled { .. } = self.as_mut().project() {
            self.as_mut().start();
        }
        match self.project() {
            InstallProj::Scoped { fut } => fut.poll(cx),
            InstallProj::Unchanged { fut } => fut.poll(cx),
            InstallProj::Unpolled { .. } | InstallProj::Deriving => {
                unreachable!("an install is polled again after deriving its context panicked")
            }
        }
    }
}

/// The running context with `added` merged in, or `None` when it would change
/// nothing.
fn extended(added: Entries) -> Option<Arc<RequestCtx>> {
    if added.is_empty() {
        return None;
    }
    let Some(current) = current_request_ctx(Arc::clone) else {
        return Some(Arc::new(RequestCtx {
            scope: None,
            correlation: None,
            ambient: added,
            parent: None,
        }));
    };
    if added.iter().all(|entry| current.ambient.holds(entry)) {
        return None;
    }
    let mut ambient = current.ambient.clone();
    for entry in added.iter() {
        ambient.set(entry.clone());
    }
    Some(Arc::new(RequestCtx {
        scope: current.scope.clone(),
        correlation: current.correlation.clone(),
        ambient,
        parent: current.parent.clone(),
    }))
}

/// The current request's [`RequestScope`], installed by the transport edge.
/// `None` off the request task (or before the edge — a transport wiring bug).
pub fn current_request_scope() -> Option<Arc<RequestScope>> {
    current_request_ctx(|ctx| ctx.scope.clone()).flatten()
}

/// Run `fut` as a unit of work — the transport edges' installer, and the seam
/// for driving handlers outside a transport (in-process test harnesses, a
/// transport's mirror of the edge).
///
/// `scope` is optional — an edge may have no container to open one over — and
/// `correlation` is not: whoever accepts a unit of work decides its identity
/// ([`Correlation`](crate::Correlation)). Opened inside another unit, the new
/// one is nested in it: it carries what the outer unit carries, and a batch
/// they share carries the outer unit's values alone.
pub fn with_request_scope<F: Future>(
    scope: Option<Arc<RequestScope>>,
    correlation: crate::Correlation,
    fut: F,
) -> impl Future<Output = F::Output> {
    Install::new(
        move || {
            Some(Arc::new(match current_request_ctx(Arc::clone) {
                Some(outer) => RequestCtx {
                    scope,
                    correlation: Some(correlation),
                    ambient: outer.ambient.clone(),
                    parent: Some(outer),
                },
                None => RequestCtx::opened(scope, correlation),
            }))
        },
        fut,
    )
}

/// The ambient request context, held so work that continues the unit after the
/// future that accepted it has returned — a streaming response body, written
/// after the handler with its task-locals unwound — can re-install it.
///
/// One the edge [builds](Self::new) carries the unit's scope and identity; one
/// [captured](Self::capture) carries what crosses the continuation it was
/// captured for, and nothing else.
#[derive(Clone)]
pub struct RequestContinuation(Arc<RequestCtx>);

impl RequestContinuation {
    /// Build the context an edge is about to install — [`scope`](Self::scope)
    /// around the handler, [`enter`](Self::enter) around the body.
    pub fn new(scope: Option<Arc<RequestScope>>, correlation: crate::Correlation) -> Self {
        Self(Arc::new(RequestCtx::opened(scope, correlation)))
    }

    /// Capture what the running unit carries into `into`, to re-install around
    /// the work that continues it there; `None` where no unit is running.
    pub fn capture(into: Continuation) -> Option<Self> {
        current_request_ctx(|ctx| Self(ctx.captured(into)))
    }

    /// Run `fut` under this context.
    pub fn scope<F: Future>(&self, fut: F) -> impl Future<Output = F::Output> + use<F> {
        REQUEST_CTX.scope(Arc::clone(&self.0), fut)
    }

    /// Run `f` under this context — `current_trace_id()`, `current_actor_id()`
    /// and [`current_request_scope`] answer what they answered inside the
    /// handler. Synchronous, since a body's `poll` is not a future.
    pub fn enter<T>(&self, f: impl FnOnce() -> T) -> T {
        REQUEST_CTX.sync_scope(Arc::clone(&self.0), f)
    }

    #[cfg(test)]
    pub(crate) fn is_same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
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
    /// Capture the span, and what the running unit carries into a task that
    /// may outlive it: [`capture`](Self::capture) for a
    /// [`Task`](Continuation::Task).
    pub fn current() -> Self {
        Self::capture(Continuation::Task)
    }

    /// Capture the span, and what the running unit carries into `into`.
    pub fn capture(into: Continuation) -> Self {
        Self {
            span: tracing::Span::current(),
            request: RequestContinuation::capture(into),
        }
    }

    /// The captured span, for the events a hand-off point emits synchronously
    /// before it spawns.
    pub fn span(&self) -> &tracing::Span {
        &self.span
    }

    /// Wrap `fut` so it runs under the captured span and request context — what
    /// goes to `spawn`, which starts with neither.
    pub fn carry<F: Future>(self, fut: F) -> impl Future<Output = F::Output> {
        let Self { span, request } = self;
        tracing::Instrument::instrument(
            Install::new(move || request.map(|request| request.0), fut),
            span,
        )
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
            crate::__private::set_actor_id("alice-42");
        })
        .await;

        continuation.enter(|| {
            assert_eq!(crate::current_actor_id().as_deref(), Some("alice-42"));
        });
    }

    /// Bytes a unit holds across an await: enough that a copy of it shows.
    const HELD: usize = 32 * 1024;

    /// What an install may add around the unit it runs, its own fields and
    /// frames, never a second copy of the unit.
    const INSTALL_OVERHEAD: usize = 1024;

    /// Where the stack stands in the frame that calls it.
    #[inline(never)]
    fn stack_mark() -> usize {
        let mark = 0u8;
        std::hint::black_box(&mark) as *const u8 as usize
    }

    /// A unit holding [`HELD`] bytes across an await, answering where the
    /// stack stood once it resumed.
    async fn heavy() -> usize {
        let held = [0u8; HELD];
        tokio::task::yield_now().await;
        std::hint::black_box(&held);
        stack_mark()
    }

    struct Marker;
    impl crate::AmbientValue for Marker {}

    struct Other;
    impl crate::AmbientValue for Other {}

    fn pending_other() -> crate::ambient::Pending {
        let mut pending = crate::ambient::Pending::default();
        pending.attach(Arc::new(Other));
        pending
    }

    #[test]
    fn an_install_holds_the_unit_it_runs_once() {
        let alone = std::mem::size_of_val(&heavy());
        let continuation = RequestContinuation::new(None, crate::Correlation::minted(None));
        let installed = [
            (
                "with_request_scope",
                std::mem::size_of_val(&with_request_scope(
                    None,
                    crate::Correlation::minted(None),
                    heavy(),
                )),
            ),
            (
                "with_ambient",
                std::mem::size_of_val(&crate::with_ambient(Arc::new(Marker), heavy())),
            ),
            (
                "install_pending",
                std::mem::size_of_val(&crate::ambient::install_pending(pending_other(), heavy())),
            ),
            (
                "RequestContinuation::scope",
                std::mem::size_of_val(&continuation.scope(heavy())),
            ),
            (
                "TaskContext::carry",
                std::mem::size_of_val(&TaskContext::current().carry(heavy())),
            ),
        ];
        for (installer, size) in installed {
            assert!(
                size < alone + INSTALL_OVERHEAD,
                "{installer} weighs {size} bytes around a {alone}-byte unit",
            );
        }
    }

    #[tokio::test]
    async fn nested_installs_reach_no_deeper_than_bare_task_local_scopes() {
        let opened = || Arc::new(RequestCtx::opened(None, crate::Correlation::minted(None)));
        let bare = REQUEST_CTX
            .scope(
                opened(),
                REQUEST_CTX.scope(
                    opened(),
                    REQUEST_CTX.scope(opened(), REQUEST_CTX.scope(opened(), heavy())),
                ),
            )
            .await;

        let continuation = RequestContinuation::new(None, crate::Correlation::minted(None));
        let installed = with_request_scope(
            None,
            crate::Correlation::minted(None),
            crate::with_ambient(
                Arc::new(Marker),
                crate::ambient::install_pending(pending_other(), continuation.scope(heavy())),
            ),
        )
        .await;

        let deeper = bare.saturating_sub(installed);
        assert!(
            deeper < 4 * INSTALL_OVERHEAD,
            "four installs reach {deeper} bytes deeper than four bare scopes around a \
             {HELD}-byte unit",
        );
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

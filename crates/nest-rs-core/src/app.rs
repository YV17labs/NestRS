//! The [`App`] and its [`AppBuilder`] — the boot entry point that wires the
//! root module, runs the four build phases, validates the access graph, and
//! drives the lifecycle and transports.

use std::any::{Any, TypeId};
use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::access::{
    ProviderOrder, ReachableProviders, provider_order_from_inventory,
    reachable_provider_ids_from_inventory, validate_from_inventory, validate_keyed_from_inventory,
};
use crate::container::ProviderKey;
use crate::container::{Container, ContainerBuilder, Registrar};
use crate::discovery::Discovery;
use crate::error::{
    AccessError, ContestedDeclarationError, DuplicateProviderError, FactoryCycleError,
    UnresolvedFactoryError,
};
use crate::lifecycle::{LifecyclePhase, run_phase, run_phase_lenient};
use crate::module::Module;
use crate::transport::{Transport, TransportContribution};

/// Entry point for a nestrs application. Builds the container from a root
/// [`Module`] and runs every transport its imports contribute concurrently
/// until shutdown.
pub struct App {
    container: Container,
}

/// Fail the boot if any concrete/keyed provider was registered twice — a wiring
/// mistake that would otherwise silently last-write-wins. Reports the first
/// duplicate; fixing it re-runs and surfaces the next, same as the other
/// boot-time wiring checks.
fn check_duplicate_providers(builder: &ContainerBuilder) -> Result<()> {
    if let Some(dup) = builder.duplicate_providers().first() {
        return Err(DuplicateProviderError {
            type_name: dup.type_name,
        }
        .into());
    }
    Ok(())
}

/// Fail the boot when two import sites each declared a value for one type —
/// neither may silently win on queue position.
fn check_contested_declarations(builder: &ContainerBuilder) -> Result<()> {
    if let Some((type_name, remedy)) = builder.contested_factories().first() {
        return Err(ContestedDeclarationError { type_name, remedy }.into());
    }
    Ok(())
}

/// Fail the synchronous boot when a module queued an async factory: `App::new`
/// has no factory phase, so the value would never be built and the hole would
/// only surface as a `None` at first read.
fn check_no_queued_factories(builder: &ContainerBuilder) -> Result<()> {
    if let Some(type_name) = builder.queued_factory_names().first() {
        return Err(UnresolvedFactoryError { type_name }.into());
    }
    Ok(())
}

impl App {
    /// Build the container from the root module synchronously. Every wiring
    /// failure is a `Result`: a cross-module reach returns
    /// [`AccessGraphError`](crate::AccessGraphError), a dependency no module
    /// provides returns [`MissingDependencyError`](crate::MissingDependencyError),
    /// a doubly-registered type returns
    /// [`DuplicateProviderError`]. The register
    /// phase defers a missing dependency to the access-graph check rather than
    /// panicking ahead of it; only a true provider cycle (invisible to the
    /// graph) still panics.
    pub fn new<M: Module + 'static>() -> Result<Self> {
        #[cfg(feature = "logging")]
        crate::logging::init_fallback()?;
        // `collect` runs first, exactly as the async builder runs it: a static
        // module whose `collect` queues a factory — a vendor binding, a
        // `ConfigModule::for_feature` — is then *seen* by the check below and
        // refused by name, instead of the value being silently absent because
        // nothing ever asked the module what it would have built.
        let builder = M::register(M::collect(Container::builder()));
        let roots = [TypeId::of::<M>()];
        // `ReachableProviders` is seeded after register but is global
        // infrastructure for the access graph, so it must be in `global` up
        // front regardless of seed ordering.
        let global: HashSet<TypeId> = HashSet::from([
            TypeId::of::<ReachableProviders>(),
            TypeId::of::<ProviderOrder>(),
        ]);
        check_duplicate_providers(&builder)?;
        // Before the queue check, and for the same reason the async path runs it
        // before any factory: a contested declaration is a fact that **survives
        // the remedy the queue check prescribes**. `UnresolvedFactoryError` says
        // "boot with `App::builder()…` instead", so reporting it first hands the
        // developer an edit whose only outcome is a second, different boot
        // failure — while the framework already held the fact that explains it.
        // A refusal lands at the earliest site that can see the fact.
        check_contested_declarations(&builder)?;
        // `collect` ran but nothing drains the queue on this path, so anything a
        // module queued as an async factory would never exist — refuse rather
        // than boot a container with the hole in it.
        check_no_queued_factories(&builder)?;
        // The actual registered set (singletons + scoped/transient factories +
        // imperatively-provided values) — consulted so a dependency provided
        // outside the declarative graph is not misreported as unmet.
        let registered = builder.registered_ids();
        let deferred = builder.scoped_or_transient_ids();
        validate_from_inventory(&roots, &global, &registered, &deferred)
            .map_err(AccessError::into_anyhow)?;
        // Keyed providers are configured imperatively; the sync path seeds none
        // up front, so any keyed dependency here is genuinely unmet.
        validate_keyed_from_inventory(&roots, &HashSet::new())?;
        let reachable = reachable_provider_ids_from_inventory(&roots, &global);
        let builder = builder
            .provide(ReachableProviders(reachable))
            .provide(ProviderOrder::new(provider_order_from_inventory(&roots)));
        Ok(Self {
            container: builder.build(),
        })
    }

    /// Start an [`AppBuilder`] for apps that must seed runtime values or build
    /// providers asynchronously before the module tree is wired.
    pub fn builder() -> AppBuilder {
        AppBuilder::new()
    }

    /// The assembled singleton container, for tests and tooling that resolve
    /// providers directly outside the declarative `#[inject]` surface.
    pub fn container(&self) -> &Container {
        &self.container
    }

    /// Run the init lifecycle phases (`OnModuleInit`, then
    /// `OnApplicationBootstrap`) against the built container, without serving.
    /// Exposed so a test harness can drive the same startup the server
    /// performs.
    pub async fn init(&self) -> Result<()> {
        run_phase(&self.container, LifecyclePhase::OnModuleInit).await?;
        run_phase(&self.container, LifecyclePhase::OnApplicationBootstrap).await?;
        Ok(())
    }

    /// Configure each transport against the container, run the init lifecycle
    /// hooks, then run all transports concurrently. SIGINT / SIGTERM cancels the
    /// shared token; the first transport that errors also cancels the others.
    /// Once the transports have stopped, the shutdown lifecycle hooks run.
    ///
    /// Every transport is contributed by an imported module via
    /// [`TransportContribution`] — `HttpModule` brings `HttpTransport`,
    /// `ScheduleModule` brings `Scheduler`, `RedisWorkerModule` brings
    /// `RedisWorker`. There is no imperative `.transport()` on `App` —
    /// `AppModule.imports` is the single composition seam.
    pub async fn run(self) -> Result<()> {
        let App { container } = self;

        tracing::info!(
            target: crate::target::APP,
            version = env!("CARGO_PKG_VERSION"),
            "nestrs starting",
        );

        let mut transports: Vec<Box<dyn Transport>> = Vec::new();
        for contribution in Discovery::new(&container).meta::<TransportContribution>() {
            let transport = (contribution.meta.build)(&container)?;
            tracing::info!(
                target: crate::target::APP,
                transport = contribution.meta.name,
                "attached module-contributed transport",
            );
            transports.push(transport);
        }

        for t in transports.iter_mut() {
            t.configure(&container).await?;
        }

        // Init phases run after wiring, before serving — nothing is listening
        // yet, so a failure here aborts cleanly.
        run_phase(&container, LifecyclePhase::OnModuleInit).await?;
        run_phase(&container, LifecyclePhase::OnApplicationBootstrap).await?;

        let cancel = CancellationToken::new();
        spawn_shutdown_signal(cancel.clone());

        let mut join = JoinSet::new();
        for transport in transports {
            let token = cancel.clone();
            join.spawn(async move { transport.serve(token).await });
        }

        let mut first_err: Option<anyhow::Error> = None;
        while let Some(res) = join.join_next().await {
            match res {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    if first_err.is_none() {
                        tracing::error!(target: crate::target::APP, error = %e, "transport failed; shutting down");
                        first_err = Some(e);
                        cancel.cancel();
                    }
                }
                Err(join_err) => {
                    if first_err.is_none() {
                        tracing::error!(target: crate::target::APP, error = %join_err, "transport task panicked; shutting down");
                        first_err = Some(anyhow!(join_err));
                        cancel.cancel();
                    }
                }
            }
        }

        // Shutdown is best-effort: every provider's cleanup runs even if one
        // fails or a transport errored.
        run_phase_lenient(&container, LifecyclePhase::OnModuleDestroy).await;
        run_phase_lenient(&container, LifecyclePhase::BeforeApplicationShutdown).await;
        run_phase_lenient(&container, LifecyclePhase::OnApplicationShutdown).await;

        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

struct ModuleHooks {
    type_id: TypeId,
    collect: fn(ContainerBuilder) -> ContainerBuilder,
    register: fn(ContainerBuilder) -> ContainerBuilder,
}

/// Builder for an [`App`] whose module tree needs runtime values or
/// asynchronously-built providers.
///
/// Four phases run at [`build`](AppBuilder::build), independent of call order:
///
/// 1. **Seeds** — values registered with [`provide`](AppBuilder::provide) /
///    [`provide_arc`](AppBuilder::provide_arc) /
///    [`provide_dyn`](AppBuilder::provide_dyn).
/// 2. **Collect** — each module's [`collect`](crate::Module::collect) queues
///    the async factories its import tree owns. No provider is built yet.
/// 3. **Factories** — every queued factory is `await`ed; each sees the
///    container so far. A factory whose output type a seed already supplies is
///    **skipped** (a seed wins over a module's `for_root` factory — the path
///    a test takes to inject a pre-built resource).
/// 4. **Register** — each module's [`register`](crate::Module::register) builds
///    its providers last, injecting seeds and factory outputs.
///
/// The collect/factory split is what lets a module own an async resource while
/// still being declared in `#[module(imports = [...])]` — `register` is
/// synchronous and cannot `await`.
pub struct AppBuilder {
    builder: ContainerBuilder,
    modules: Vec<ModuleHooks>,
    overrides: Vec<Registrar>,
}

impl AppBuilder {
    fn new() -> Self {
        Self {
            builder: Container::builder(),
            modules: Vec::new(),
            overrides: Vec::new(),
        }
    }

    /// Seed a runtime value, injectable as `Arc<T>`.
    pub fn provide<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.builder = self.builder.provide(value);
        self
    }

    /// Seed an already-shared `Arc<T>`.
    pub fn provide_arc<T: Any + Send + Sync>(mut self, value: Arc<T>) -> Self {
        self.builder = self.builder.provide_arc(value);
        self
    }

    /// Seed a trait-object binding, injectable as `Arc<dyn Trait>`.
    pub fn provide_dyn<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.builder = self.builder.provide_dyn(value);
        self
    }

    /// Seed a **keyed** singleton, resolvable with an `#[inject(key = "…")]`
    /// field or [`Container::get_keyed`](crate::Container::get_keyed). Several
    /// instances of one concrete type coexist, one per `name` — the composition
    /// root is where keyed providers are configured (they are imperative by
    /// nature). A keyed seed is global infrastructure for the access graph, so
    /// any provider reachable from the root may inject it.
    pub fn provide_keyed<T: Any + Send + Sync>(mut self, name: &'static str, value: T) -> Self {
        self.builder = self.builder.provide_keyed(name, value);
        self
    }

    /// [`provide_keyed`](Self::provide_keyed) for an already-shared `Arc<T>`.
    pub fn provide_keyed_arc<T: Any + Send + Sync>(
        mut self,
        name: &'static str,
        value: Arc<T>,
    ) -> Self {
        self.builder = self.builder.provide_keyed_arc(name, value);
        self
    }

    /// Seed module-less metadata of type `M` (the [`ContainerBuilder::provide_meta`]
    /// shortcut at the app root). Used by global builder extensions —
    /// `use_guards_global`, `use_interceptors_global`, etc. — that need to
    /// publish a `HttpEndpointWrap`-style descriptor without
    /// owning a [`Module`].
    pub fn provide_meta<M: Any + Send + Sync>(mut self, meta: M) -> Self {
        self.builder = self.builder.provide_meta(meta);
        self
    }

    /// Register an async factory at the composition root — for a resource not
    /// owned by any module (most module-owned resources expose a `for_root`
    /// instead). A seed of the same type wins (the factory is skipped).
    ///
    /// ```ignore
    /// App::builder()
    ///     .provide(DbConfig::from_env())
    ///     .provide_factory(|c| async move {
    ///         let cfg = c.get::<DbConfig>().expect("DbConfig seeded");
    ///         Ok(DbPool::connect(&cfg.url).await?)
    ///     })
    ///     .module::<AppModule>()
    ///     .build()
    ///     .await?
    /// ```
    pub fn provide_factory<T, F, Fut>(mut self, factory: F) -> Self
    where
        T: Any + Send + Sync,
        F: FnOnce(Container) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.builder = self.builder.provide_factory(factory);
        self
    }

    /// Replace a concrete provider of type `T` *after* the module tree
    /// registers, so this value wins. Intended for tests swapping a real
    /// provider for a fake.
    ///
    /// The override reaches consumers resolved from the **final** container,
    /// but not providers already constructed in the register phase that
    /// captured the original `Arc` (the same final-vs-snapshot timing every
    /// aggregating concern observes). Override the `dyn Trait` instead — see
    /// [`override_dyn`](Self::override_dyn).
    pub fn override_value<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.overrides
            .push(Box::new(move |builder| builder.replace(value)));
        self
    }

    /// Replace a `dyn Trait` binding after the module tree registers — the test
    /// counterpart of [`provide_dyn`](Self::provide_dyn). See
    /// [`override_value`](Self::override_value) for the eager-build caveat.
    pub fn override_dyn<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.overrides
            .push(Box::new(move |builder| builder.provide_dyn(value)));
        self
    }

    /// [`override_value`](Self::override_value) for a value the test already
    /// holds in an `Arc` — a fake carrying state it inspects after the request,
    /// for instance. Eager-build caveat applies.
    pub fn override_arc<T: Any + Send + Sync>(mut self, value: Arc<T>) -> Self {
        self.overrides
            .push(Box::new(move |builder| builder.replace_arc(value)));
        self
    }

    /// Register a root module. May be called more than once; each call adds a
    /// root to the access-graph check.
    pub fn module<M: Module + 'static>(mut self) -> Self {
        self.modules.push(ModuleHooks {
            type_id: TypeId::of::<M>(),
            collect: M::collect,
            register: M::register,
        });
        self
    }

    /// Run the four phases and return the assembled [`App`]. Propagates the
    /// first factory error.
    pub async fn build(self) -> Result<App> {
        #[cfg(feature = "logging")]
        crate::logging::init_fallback()?;
        let AppBuilder {
            mut builder,
            modules,
            overrides,
        } = self;

        for hooks in &modules {
            builder = (hooks.collect)(builder);
        }
        // Before any factory runs: two import sites declared the same type and
        // one would have to lose silently.
        check_contested_declarations(&builder)?;
        // A factory whose output type a seed already supplies is skipped, so a
        // seed wins over a module's `for_root` factory — the path a test takes
        // to boot against a pre-built resource. Otherwise the next to run is
        // the first in queue order whose `after` types are all present — or
        // are nothing still queued will provide, so its own error is the one
        // to surface. Only a cycle leaves nothing runnable.
        let mut pending = builder.take_factories();
        while !pending.is_empty() {
            let ready = pending.iter().position(|queued| {
                queued.after.iter().all(|dep| {
                    builder.contains(*dep)
                        || !pending.iter().any(|other| other.provides.contains(dep))
                })
            });
            let Some(index) = ready else {
                return Err(FactoryCycleError {
                    type_names: pending.iter().map(|queued| queued.name).collect(),
                }
                .into());
            };
            let queued = pending.remove(index);
            if builder.contains(queued.id()) {
                continue;
            }
            let register = (queued.factory)(builder.snapshot()).await?;
            builder = register(builder);
        }
        // `ReachableProviders` is seeded after register but counts as global
        // infrastructure for the access graph, so it must be in `global` up
        // front regardless of seed ordering.
        let mut global = builder.provider_ids();
        global.insert(TypeId::of::<ReachableProviders>());
        global.insert(TypeId::of::<ProviderOrder>());
        // The keyed global set: keyed seeds + keyed factory outputs, snapshotted
        // before modules register (same timing as the bare global set).
        let global_keyed: HashSet<ProviderKey> = builder.keyed_provider_keys();
        for hooks in &modules {
            builder = (hooks.register)(builder);
        }
        // Overrides last so they win over the modules' registrations.
        for ov in overrides {
            builder = ov(builder);
        }

        let roots: Vec<TypeId> = modules.iter().map(|h| h.type_id).collect();
        // The full registered set after modules and overrides — includes
        // imperatively-provided values (a hand-written `impl Module`) and
        // scoped/transient factories the declarative graph cannot see.
        check_duplicate_providers(&builder)?;
        let registered = builder.registered_ids();
        let deferred = builder.scoped_or_transient_ids();
        validate_from_inventory(&roots, &global, &registered, &deferred)
            .map_err(AccessError::into_anyhow)?;
        validate_keyed_from_inventory(&roots, &global_keyed)?;
        let reachable = reachable_provider_ids_from_inventory(&roots, &global);
        let builder = builder
            .provide(ReachableProviders(reachable))
            .provide(ProviderOrder::new(provider_order_from_inventory(&roots)));
        Ok(App {
            container: builder.build(),
        })
    }
}

fn spawn_shutdown_signal(cancel: CancellationToken) {
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let mut sigterm = match signal(SignalKind::terminate()) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(target: crate::target::APP, error = %e, "failed to install SIGTERM handler");
                    return;
                }
            };
            tokio::select! {
                _ = tokio::signal::ctrl_c() => tracing::info!(target: crate::target::APP, signal = "SIGINT", "shutdown signal received"),
                _ = sigterm.recv()          => tracing::info!(target: crate::target::APP, signal = "SIGTERM", "shutdown signal received"),
            }
        }
        #[cfg(not(unix))]
        {
            match tokio::signal::ctrl_c().await {
                Ok(()) => {
                    tracing::info!(target: crate::target::APP, signal = "ctrl-c", "shutdown signal received")
                }
                Err(e) => {
                    tracing::warn!(target: crate::target::APP, error = %e, "failed to install ctrl-c handler");
                    return;
                }
            }
        }
        cancel.cancel();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Config(u32);
    struct Doubled(u32);

    // The `#[module]` macro lives in `nest-rs-core-macros`, so this crate's tests
    // hand-write the trait impl.
    struct DoublerModule;
    impl Module for DoublerModule {
        fn register(builder: ContainerBuilder) -> ContainerBuilder {
            let cfg = builder
                .snapshot()
                .get::<Config>()
                .expect("Config is seeded before modules register");
            builder.provide(Doubled(cfg.0 * 2))
        }
    }

    #[tokio::test]
    async fn seeds_are_visible_to_modules() {
        let app = App::builder()
            .provide(Config(21))
            .module::<DoublerModule>()
            .build()
            .await
            .expect("build succeeds");
        assert_eq!(app.container().get::<Doubled>().unwrap().0, 42);
    }

    #[tokio::test]
    async fn factory_runs_async_and_reads_a_seed() {
        let app = App::builder()
            .provide(Config(10))
            .provide_factory(|c| async move {
                let cfg = c.get::<Config>().expect("seed visible to factory");
                tokio::task::yield_now().await;
                Ok(Doubled(cfg.0 + 5))
            })
            .build()
            .await
            .expect("build succeeds");
        assert_eq!(app.container().get::<Doubled>().unwrap().0, 15);
    }

    struct First(u32);
    struct Second(u32);

    #[tokio::test]
    async fn later_factory_sees_earlier_factory_output() {
        let app = App::builder()
            .provide_factory(|_| async { Ok(First(1)) })
            .provide_factory(|c| async move {
                let first = c.get::<First>().expect("earlier factory output visible");
                Ok(Second(first.0 + 1))
            })
            .build()
            .await
            .expect("build succeeds");
        assert_eq!(app.container().get::<Second>().unwrap().0, 2);
    }

    // A module whose factory reads another module's factory output — a store
    // bound over a shared connection is the shape — declares it with `_after`.
    struct SecondAfterFirst;
    impl Module for SecondAfterFirst {
        fn register(builder: ContainerBuilder) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder) -> ContainerBuilder {
            builder.provide_declared_factory_after::<Second, First, _, _>(
                "one declaration",
                |c| async move {
                    let first = c
                        .get::<First>()
                        .ok_or_else(|| anyhow!("First is not registered — import FirstModule"))?;
                    Ok(Second(first.0 + 1))
                },
            )
        }
    }

    struct FirstModule;
    impl Module for FirstModule {
        fn register(builder: ContainerBuilder) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder) -> ContainerBuilder {
            builder.provide_factory(|_| async { Ok(First(41)) })
        }
    }

    #[tokio::test]
    async fn a_factory_declared_after_another_runs_after_it_whatever_the_queue_order() {
        // `Second` is queued first; the drain reorders on the declaration, so
        // `imports` order stays a readability choice.
        let app = App::builder()
            .module::<SecondAfterFirst>()
            .module::<FirstModule>()
            .build()
            .await
            .expect("the declared order is honoured");
        assert_eq!(app.container().get::<Second>().unwrap().0, 42);
    }

    /// The portable form: a dependent names the `Arc<dyn Port>` a
    /// `provide_factory_dyn` binds, not the vendor's concrete type. The queue
    /// entry advertises both keys, so the dependent waits.
    #[derive(Clone)]
    struct PortImpl(u32);
    trait Port: Send + Sync {
        fn value(&self) -> u32;
    }
    impl Port for PortImpl {
        fn value(&self) -> u32 {
            self.0
        }
    }
    struct ReadsPort(u32);
    struct ReadsPortModule;
    impl Module for ReadsPortModule {
        fn register(builder: ContainerBuilder) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder) -> ContainerBuilder {
            builder.provide_factory_after::<ReadsPort, Arc<dyn Port>, _, _>(|c| async move {
                let port = c
                    .get_dyn::<dyn Port>()
                    .ok_or_else(|| anyhow!("dyn Port must already be bound"))?;
                Ok(ReadsPort(port.value() + 1))
            })
        }
    }
    struct BindsPortModule;
    impl Module for BindsPortModule {
        fn register(builder: ContainerBuilder) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder) -> ContainerBuilder {
            builder.provide_factory_dyn::<PortImpl, dyn Port, _, _>(
                |_| async { Ok(PortImpl(41)) },
                |p| Arc::new(p) as Arc<dyn Port>,
            )
        }
    }

    #[tokio::test]
    async fn a_factory_declared_after_a_dyn_binding_waits_for_the_dyn_side() {
        let app = App::builder()
            .module::<ReadsPortModule>()
            .module::<BindsPortModule>()
            .build()
            .await
            .expect("the dyn key is a key the binding's entry provides");
        assert_eq!(app.container().get::<ReadsPort>().unwrap().0, 42);
    }

    #[test]
    fn the_synchronous_boot_refuses_a_static_modules_queued_factory() {
        // `App::new` has no factory phase; a static module whose `collect`
        // queues one is refused by name rather than booted with the value
        // silently absent.
        let err = match App::new::<BindsPortModule>() {
            Ok(_) => panic!("a queued factory cannot be drained synchronously"),
            Err(e) => e,
        };
        assert!(
            err.downcast_ref::<UnresolvedFactoryError>().is_some(),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_factory_waiting_on_nothing_queued_runs_and_reports_its_own_error() {
        // Nothing will ever provide `First`, so the drain must not hang on the
        // declaration: the factory runs and its own remedy is what surfaces.
        let err = match App::builder().module::<SecondAfterFirst>().build().await {
            Ok(_) => panic!("the factory's own error must surface"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("import FirstModule"), "{err}");
    }

    struct FirstAfterSecond;
    impl Module for FirstAfterSecond {
        fn register(builder: ContainerBuilder) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder) -> ContainerBuilder {
            builder.provide_declared_factory_after::<First, Second, _, _>(
                "one declaration",
                |_| async { Ok(First(0)) },
            )
        }
    }

    #[tokio::test]
    async fn two_factories_waiting_on_each_other_fail_the_boot_naming_both() {
        let err = match App::builder()
            .module::<SecondAfterFirst>()
            .module::<FirstAfterSecond>()
            .build()
            .await
        {
            Ok(_) => panic!("a cycle cannot boot"),
            Err(e) => e,
        };
        let cycle = err
            .downcast_ref::<FactoryCycleError>()
            .expect("the typed cycle error");
        assert_eq!(cycle.type_names.len(), 2, "{cycle:?}");
    }

    #[tokio::test]
    async fn factory_error_aborts_build() {
        // `App` is not `Debug`, so match rather than `expect_err`.
        let err = match App::builder()
            .provide_factory::<Config, _, _>(|_| async { Err(anyhow!("connection refused")) })
            .build()
            .await
        {
            Ok(_) => panic!("a failing factory must abort the build"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("connection refused"));
    }

    // Module owning its provider's factory via `collect` (the `SeaOrmDatabaseModule`
    // shape).
    struct ConfigModule;
    impl Module for ConfigModule {
        fn register(builder: ContainerBuilder) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder) -> ContainerBuilder {
            builder.provide_factory(|_| async { Ok(Config(7)) })
        }
    }

    #[tokio::test]
    async fn module_owns_a_factory_via_collect() {
        let app = App::builder()
            .module::<ConfigModule>()
            .module::<DoublerModule>()
            .build()
            .await
            .expect("build succeeds");
        assert_eq!(app.container().get::<Doubled>().unwrap().0, 14);
    }

    #[tokio::test]
    async fn modules_inject_factory_output() {
        let app = App::builder()
            .provide_factory(|_| async { Ok(Config(7)) })
            .module::<DoublerModule>()
            .build()
            .await
            .expect("build succeeds");
        assert_eq!(app.container().get::<Doubled>().unwrap().0, 14);
    }

    #[tokio::test]
    async fn a_seed_short_circuits_a_factory_of_the_same_type() {
        let app = App::builder()
            .provide(Config(99))
            .provide_factory::<Config, _, _>(|_| async { panic!("skipped factory must not run") })
            .build()
            .await
            .expect("build succeeds");
        assert_eq!(app.container().get::<Config>().unwrap().0, 99);
    }

    #[tokio::test]
    async fn a_seed_short_circuits_a_module_owned_collect_factory() {
        let app = App::builder()
            .provide(Config(1))
            .module::<ConfigModule>()
            .module::<DoublerModule>()
            .build()
            .await
            .expect("build succeeds");
        assert_eq!(app.container().get::<Doubled>().unwrap().0, 2);
    }

    struct NullTransport;
    #[async_trait::async_trait]
    impl Transport for NullTransport {
        async fn configure(&mut self, _: &Container) -> Result<()> {
            Ok(())
        }
        async fn serve(self: Box<Self>, cancel: CancellationToken) -> Result<()> {
            cancel.cancelled().await;
            Ok(())
        }
    }

    struct WithTransportModule;
    impl Module for WithTransportModule {
        fn register(builder: ContainerBuilder) -> ContainerBuilder {
            builder.provide_meta(TransportContribution {
                name: "NullTransport",
                build: |_| Ok(Box::new(NullTransport)),
            })
        }
    }

    #[tokio::test]
    async fn module_contributes_a_transport_via_meta() {
        let app = App::builder()
            .module::<WithTransportModule>()
            .build()
            .await
            .expect("build succeeds");
        // The contribution lands in the container's metadata so `App::run`
        // can drain it at boot.
        let contributions = Discovery::new(app.container()).meta::<TransportContribution>();
        assert_eq!(contributions.len(), 1);
        assert_eq!(contributions[0].meta.name, "NullTransport");
    }
}

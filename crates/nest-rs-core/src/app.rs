use std::any::{Any, TypeId};
use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::access::{
    ReachableProviders, ResolverSchemaActive, UnreachableResolversError,
    reachable_provider_ids_from_inventory, unreachable_resolvers_from_inventory,
    validate_from_inventory, warn_unreachable_resolvers_from_inventory,
};
use crate::container::{Container, ContainerBuilder, Registrar};
use crate::discovery::DiscoveryService;
use crate::lifecycle::{LifecyclePhase, run_phase, run_phase_lenient};
use crate::module::Module;
use crate::transport::{Transport, TransportContribution};

/// Entry point for a nestrs application. Builds the container from a root
/// [`Module`] and runs every transport its imports contribute concurrently
/// until shutdown.
pub struct App {
    container: Container,
}

impl App {
    /// Build the container from the root module synchronously. Returns
    /// [`AccessGraphError`](crate::AccessGraphError) on contract violations.
    /// A missing provider still panics inside the sync register-phase fixpoint.
    pub fn new<M: Module + 'static>() -> Result<Self> {
        let builder = M::register(Container::builder());
        let roots = [TypeId::of::<M>()];
        // `ReachableProviders` is seeded after register but is global
        // infrastructure for the access graph, so it must be in `global` up
        // front regardless of seed ordering.
        let global: HashSet<TypeId> = HashSet::from([TypeId::of::<ReachableProviders>()]);
        validate_from_inventory(&roots, &global)?;
        let reachable = reachable_provider_ids_from_inventory(&roots, &global);
        let builder = builder.provide(ReachableProviders(reachable));
        let container = builder.build();
        if container.get::<ResolverSchemaActive>().is_some() {
            warn_unreachable_resolvers_from_inventory(&roots);
        }
        Ok(Self { container })
    }

    /// Start an [`AppBuilder`] for apps that must seed runtime values or build
    /// providers asynchronously before the module tree is wired.
    pub fn builder() -> AppBuilder {
        AppBuilder::new()
    }

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
    /// `ScheduleModule` brings `Scheduler`, `QueueWorkerModule` brings
    /// `QueueWorker`. There is no imperative `.transport()` on `App` —
    /// `AppModule.imports` is the single composition seam.
    pub async fn run(self) -> Result<()> {
        let App { container } = self;

        let mut transports: Vec<Box<dyn Transport>> = Vec::new();
        for contribution in DiscoveryService::new(&container).meta::<TransportContribution>() {
            let transport = (contribution.meta.build)(&container)?;
            tracing::info!(
                target: "nest_rs::transport",
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
                        first_err = Some(e);
                        cancel.cancel();
                    }
                }
                Err(join_err) => {
                    if first_err.is_none() {
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
    strict_resolver_membership: bool,
}

impl AppBuilder {
    fn new() -> Self {
        Self {
            builder: Container::builder(),
            modules: Vec::new(),
            overrides: Vec::new(),
            strict_resolver_membership: false,
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

    /// Seed module-less metadata of type `M` (the [`ContainerBuilder::provide_meta`]
    /// shortcut at the app root). Used by global builder extensions —
    /// `use_guards_global`, `use_interceptors_global`, etc. — that need to
    /// publish a [`HttpInterceptorMeta`](crate)-style descriptor without
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

    /// Replace a concrete provider with a pre-shared `Arc<T>` after the module
    /// tree registers — the same swap as [`override_value`](Self::override_value)
    /// for cases a test already has the value in an `Arc` (a fake holding state
    /// the test inspects after the request, for instance). Eager-build caveat
    /// applies (see [`override_value`](Self::override_value)).
    pub fn override_provider<T: Any + Send + Sync>(mut self, value: Arc<T>) -> Self {
        self.overrides
            .push(Box::new(move |builder| builder.replace_arc(value)));
        self
    }

    /// Promote the default unreachable-resolver `warn` into a boot
    /// [`UnreachableResolversError`]. Use in apps where a forgotten
    /// `<Feature>GraphqlModule` import should be a CI gate; leave default in
    /// apps that intentionally link broader surfaces than they expose.
    pub fn strict_resolver_membership(mut self) -> Self {
        self.strict_resolver_membership = true;
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
        let AppBuilder {
            mut builder,
            modules,
            overrides,
            strict_resolver_membership,
        } = self;

        for hooks in &modules {
            builder = (hooks.collect)(builder);
        }
        // A factory whose output type a seed already supplies is skipped, so a
        // seed wins over a module's `for_root` factory — the path a test takes
        // to boot against a pre-built resource.
        for (type_id, factory) in builder.take_factories() {
            if builder.contains(type_id) {
                continue;
            }
            let register = factory(builder.snapshot()).await?;
            builder = register(builder);
        }
        // `ReachableProviders` is seeded after register but counts as global
        // infrastructure for the access graph, so it must be in `global` up
        // front regardless of seed ordering.
        let mut global = builder.provider_ids();
        global.insert(TypeId::of::<ReachableProviders>());
        for hooks in &modules {
            builder = (hooks.register)(builder);
        }
        // Overrides last so they win over the modules' registrations.
        for ov in overrides {
            builder = ov(builder);
        }

        let roots: Vec<TypeId> = modules.iter().map(|h| h.type_id).collect();
        validate_from_inventory(&roots, &global)?;
        let reachable = reachable_provider_ids_from_inventory(&roots, &global);
        let builder = builder.provide(ReachableProviders(reachable));
        if builder.contains(TypeId::of::<ResolverSchemaActive>()) {
            if strict_resolver_membership {
                let unreachable = unreachable_resolvers_from_inventory(&roots);
                if !unreachable.is_empty() {
                    return Err(UnreachableResolversError(unreachable).into());
                }
            } else {
                warn_unreachable_resolvers_from_inventory(&roots);
            }
        }

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
                    tracing::warn!(error = %e, "failed to install SIGTERM handler");
                    return;
                }
            };
            tokio::select! {
                _ = tokio::signal::ctrl_c() => tracing::info!("SIGINT received, shutting down"),
                _ = sigterm.recv()          => tracing::info!("SIGTERM received, shutting down"),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("ctrl-c received, shutting down");
        }
        cancel.cancel();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Config(u32);
    struct Doubled(u32);

    // The `#[module]` macro lives in `nestrs-macros`, so this crate's tests
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

    // Module owning its provider's factory via `collect` (the `DatabaseModule`
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
        let contributions = DiscoveryService::new(app.container()).meta::<TransportContribution>();
        assert_eq!(contributions.len(), 1);
        assert_eq!(contributions[0].meta.name, "NullTransport");
    }
}

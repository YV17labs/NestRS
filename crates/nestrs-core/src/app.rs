use std::any::{Any, TypeId};
use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::access::{
    validate_from_inventory, validate_resolver_membership_from_inventory, ResolverSchemaActive,
};
use crate::container::{Container, ContainerBuilder, Registrar};
use crate::lifecycle::{run_phase, run_phase_lenient, LifecyclePhase};
use crate::module::Module;
use crate::transport::Transport;

/// Entry point for a nestrs application. Builds the container from a root
/// [`Module`], attaches zero or more [`Transport`]s, and runs them
/// concurrently until shutdown.
pub struct App {
    container: Container,
    transports: Vec<Box<dyn Transport>>,
}

impl App {
    /// Build the container from the root module and return an empty app.
    ///
    /// The sync path has no seeds or factories, so the access-graph check runs
    /// against an empty global set: every provider's dependency must be reachable
    /// through the import graph. A violation returns an
    /// [`AccessGraphError`](crate::AccessGraphError), mirroring
    /// [`AppBuilder::build`](AppBuilder::build) — `main` propagates it with `?`.
    /// (A *missing provider* still panics inside the register-phase fixpoint,
    /// which is sync and has no `Result` to thread through — the same in both
    /// paths.)
    pub fn new<M: Module + 'static>() -> Result<Self> {
        let container = M::register(Container::builder()).build();
        let roots = [TypeId::of::<M>()];
        validate_from_inventory(&roots, &HashSet::new())?;
        // Resolver membership only matters when the resolver schema is actually
        // composed (the schema-composing layer marks it). An app that links
        // resolvers transitively but composes no schema is not their home.
        if container.get::<ResolverSchemaActive>().is_some() {
            validate_resolver_membership_from_inventory(&roots)?;
        }
        Ok(Self {
            container,
            transports: Vec::new(),
        })
    }

    /// Start an [`AppBuilder`] for apps that must seed runtime values (a loaded
    /// config, parsed CLI args) or build providers asynchronously (a DB pool, a
    /// cache client) before the module tree is wired. Apps that need none of
    /// that use [`App::new`].
    pub fn builder() -> AppBuilder {
        AppBuilder::new()
    }

    /// Container reference, in case the caller needs to resolve services
    /// before attaching transports (e.g. to build a GraphQL schema from a
    /// resolver that lives in the container).
    pub fn container(&self) -> &Container {
        &self.container
    }

    /// Run the init lifecycle phases (`OnModuleInit`, then
    /// `OnApplicationBootstrap`) against the built container, without serving.
    /// [`run`](App::run) calls this internally before serving; it is exposed so a
    /// test harness ([`nestrs-testing`](https://docs.rs/nestrs-testing)) can drive
    /// the same startup the server performs — the NestJS `app.init()` analog. A
    /// failing hook aborts with its error.
    pub async fn init(&self) -> Result<()> {
        run_phase(&self.container, LifecyclePhase::OnModuleInit).await?;
        run_phase(&self.container, LifecyclePhase::OnApplicationBootstrap).await?;
        Ok(())
    }

    pub fn transport<T: Transport>(mut self, transport: T) -> Self {
        self.transports.push(Box::new(transport));
        self
    }

    /// Configure each transport against the container, run the init lifecycle
    /// hooks, then run all transports concurrently. SIGINT / SIGTERM cancels the
    /// shared token; the first transport that errors also cancels the others.
    /// Once the transports have stopped, the shutdown lifecycle hooks run.
    pub async fn run(self) -> Result<()> {
        let App {
            container,
            mut transports,
        } = self;

        for t in transports.iter_mut() {
            t.configure(&container).await?;
        }

        // Init phases run after wiring, before serving. A failure here aborts
        // the boot — nothing is listening yet, so there is nothing to tear down.
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

        // Shutdown phases run after the transports have stopped, best-effort, so
        // every provider's cleanup runs even if one fails or a transport errored.
        run_phase_lenient(&container, LifecyclePhase::OnModuleDestroy).await;
        run_phase_lenient(&container, LifecyclePhase::BeforeApplicationShutdown).await;
        run_phase_lenient(&container, LifecyclePhase::OnApplicationShutdown).await;

        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// The two registration entry points of a [`Module`], plus its `TypeId`,
/// captured so [`AppBuilder::build`] can run them in separate phases and root
/// the access-graph check at the module tree's entry points.
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
///    [`provide_dyn`](AppBuilder::provide_dyn): runtime values a `main` computes
///    (a loaded config, parsed CLI args) for the DI graph to inject.
/// 2. **Collect** — each module's [`collect`](crate::Module::collect) runs,
///    queuing the async factories its import tree owns (a DB pool, a queue
///    connection — a [`DynamicModule`](crate::DynamicModule) whose `collect`
///    registers one). No provider is built yet.
/// 3. **Factories** — every queued factory (from a module's `collect` or from
///    [`provide_factory`](AppBuilder::provide_factory) at the root) is `await`ed.
///    Each sees the container so far, so it may depend on a seed or an earlier
///    factory; a returned `Err` aborts the build. A factory whose output type a
///    seed already supplies is **skipped** — a seed wins over a module's
///    `for_root` factory, the path a test takes to inject a pre-built resource.
/// 4. **Register** — each module's [`register`](crate::Module::register) builds
///    its providers last, injecting the seeds and factory outputs above.
///
/// The collect/factory split is what lets a module *own* an async resource (its
/// `for_root` returns a `DynamicModule` whose `collect` queues the factory)
/// while still being declared in `#[module(imports = [...])]` — `register` is
/// synchronous and cannot `await`, so the value is produced in the factory
/// phase before any provider needs it. Apps with no runtime values use
/// [`App::new`] instead.
pub struct AppBuilder {
    builder: ContainerBuilder,
    modules: Vec<ModuleHooks>,
    /// Provider replacements applied *after* the register phase, so they win
    /// over a module's own registration. Seeded by
    /// [`override_value`](Self::override_value) / [`override_dyn`](Self::override_dyn),
    /// mainly for tests swapping a real provider for a mock.
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

    /// Seed a runtime value, wrapped in `Arc` internally. Injectable as
    /// `Arc<T>` by any provider in the module tree.
    pub fn provide<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.builder = self.builder.provide(value);
        self
    }

    /// Seed an already-shared `Arc<T>`.
    pub fn provide_arc<T: Any + Send + Sync>(mut self, value: Arc<T>) -> Self {
        self.builder = self.builder.provide_arc(value);
        self
    }

    /// Seed a trait-object binding, injectable elsewhere as `Arc<dyn Trait>`.
    pub fn provide_dyn<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.builder = self.builder.provide_dyn(value);
        self
    }

    /// Register an async factory at the composition root — for a resource not
    /// owned by any module (most module-owned resources expose a `for_root`
    /// instead). Its awaited output is stored as a provider, injectable as
    /// `Arc<T>`; a returned `Err` aborts the build. If a seed already supplies
    /// `T`, this factory is skipped (the seed wins):
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
    /// registers, so this value wins. Intended for tests
    /// ([`nestrs-testing`](https://docs.rs/nestrs-testing)) swapping a real
    /// provider for a fake.
    ///
    /// Because the container builds providers eagerly, the override reaches any
    /// consumer resolved from the **final** container — controllers, resolvers,
    /// guards, transports, lifecycle hooks — but not a provider already
    /// constructed in the register phase that captured the original `Arc` (the
    /// same final-vs-snapshot timing every aggregating concern observes). Override
    /// the `dyn Trait` a service is injected behind ([`override_dyn`](Self::override_dyn))
    /// and that caveat rarely bites in practice.
    pub fn override_value<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.overrides
            .push(Box::new(move |builder| builder.replace(value)));
        self
    }

    /// Replace a `dyn Trait` binding after the module tree registers — the test
    /// counterpart of [`provide_dyn`](Self::provide_dyn). A consumer injecting
    /// `Arc<dyn Trait>` from the final container resolves this value. See
    /// [`override_value`](Self::override_value) for the eager-build caveat.
    pub fn override_dyn<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.overrides
            .push(Box::new(move |builder| builder.provide_dyn(value)));
        self
    }

    /// Register a root module. May be called more than once; all modules
    /// collect their factories and register their providers together, and each
    /// roots the access-graph check.
    pub fn module<M: Module + 'static>(mut self) -> Self {
        self.modules.push(ModuleHooks {
            type_id: TypeId::of::<M>(),
            collect: M::collect,
            register: M::register,
        });
        self
    }

    /// Run the four phases and return the assembled [`App`], ready for
    /// [`transport`](App::transport) and [`run`](App::run). Propagates the first
    /// factory error.
    pub async fn build(self) -> Result<App> {
        let AppBuilder {
            mut builder,
            modules,
            overrides,
        } = self;

        // Collect phase: every module queues the async factories its import
        // tree owns, before any provider is built.
        for hooks in &modules {
            builder = (hooks.collect)(builder);
        }
        // Factory phase: run all queued factories (module-owned and root-level).
        // A factory whose output type a seed already supplies is skipped, so a
        // seed wins over a module's `for_root` factory — the path a test takes to
        // boot against a pre-built resource (an `EphemeralDatabase` connection in
        // place of `DatabaseModule`'s). In production nothing seeds a type a
        // module factory owns, so every factory runs.
        for (type_id, factory) in builder.take_factories() {
            if builder.contains(type_id) {
                continue;
            }
            let register = factory(builder.snapshot()).await?;
            builder = register(builder);
        }
        // The global set for the access-graph check: seeds + factory outputs,
        // i.e. everything present before any module registers. Reachable from
        // any module.
        let global = builder.provider_ids();
        // Register phase: build providers, now that factory outputs are present.
        for hooks in &modules {
            builder = (hooks.register)(builder);
        }
        // Override phase: apply test substitutions last so they win over the
        // modules' own registrations.
        for ov in overrides {
            builder = ov(builder);
        }

        // Enforce the import contract: every provider's dependency must be
        // reachable through its module's imports or be global infrastructure.
        let roots: Vec<TypeId> = modules.iter().map(|h| h.type_id).collect();
        validate_from_inventory(&roots, &global)?;
        // Resolver membership only matters when the resolver schema is actually
        // composed (the schema-composing layer marks it via `ResolverSchemaActive`).
        // An app that links resolvers transitively but composes no schema is not
        // their home.
        if builder.contains(TypeId::of::<ResolverSchemaActive>()) {
            validate_resolver_membership_from_inventory(&roots)?;
        }

        Ok(App {
            container: builder.build(),
            transports: Vec::new(),
        })
    }
}

fn spawn_shutdown_signal(cancel: CancellationToken) {
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
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

    // Hand-written module (the `#[module]` macro lives in `nestrs-macros`, which
    // this crate cannot use on its own types). It builds `Doubled` from a seeded
    // `Config`, proving seeds are visible by the time modules register.
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

    // A module that owns its provider's factory via `collect` — the hand-written
    // equivalent of a `DatabaseModule`. `register` adds nothing.
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
        // `ConfigModule::collect` queues the `Config` factory; `DoublerModule`
        // injects its output in the register phase — proving collect runs, and
        // its factory is awaited, before any provider is built.
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
        // The factory builds `Config`; the module then reads it — the cross-phase
        // contract a DB pool (factory) + repositories (module) will rely on.
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
        // The seed supplies `Config`, so the (would-be-explosive) factory of the
        // same type never runs — the seed wins. This is how a test injects a
        // pre-built resource in place of a module's `for_root` factory.
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
        // `ConfigModule::collect` queues a `Config(7)` factory; the seed `Config(1)`
        // shadows it, so `DoublerModule` reads the seed — the `EphemeralDatabase`
        // path, where a seeded connection replaces `DatabaseModule`'s.
        let app = App::builder()
            .provide(Config(1))
            .module::<ConfigModule>()
            .module::<DoublerModule>()
            .build()
            .await
            .expect("build succeeds");
        assert_eq!(app.container().get::<Doubled>().unwrap().0, 2);
    }
}

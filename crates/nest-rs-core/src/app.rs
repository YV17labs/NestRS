//! The [`App`] and its [`AppBuilder`] — the boot entry point that wires the
//! root module, runs the four build phases, validates the access graph, and
//! drives the lifecycle and transports.

use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::access::{
    Composition, ModuleDescriptor, ProviderOrder, ReachableProviders, provider_order,
    reachable_descriptors, reachable_provider_ids, validate_access_graph,
    validate_keyed_access_graph,
};
use crate::container::ProviderKey;
use crate::container::{Container, ContainerBuilder, Phase, Registrar};
use crate::discovery::Discovery;
use crate::error::{
    AccessError, ContestedDeclarationError, DuplicateProviderError, FactoryCycleError,
    LateFactoryError, UnregisteredModuleError, UnresolvedFactoryError,
};
use crate::lifecycle::{LifecyclePhase, run_phase, run_phase_lenient};
use crate::module::Module;
use crate::net::check_budgets;
use crate::transport::{Transport, TransportContribution};
use crate::way_down::{WayDown, watch_signals};

/// Entry point for a nestrs application. Builds the container from a root
/// [`Module`] and runs every transport its imports contribute concurrently
/// until shutdown.
pub struct App {
    container: Container,
}

/// Fail the boot if any unkeyed provider was registered twice, rather than let
/// the last write win silently.
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
    if let Some(contested) = builder.contested_factories().first() {
        return Err(ContestedDeclarationError {
            type_name: contested.type_name,
            first: contested.first.clone(),
            second: contested.second.clone(),
            remedy: contested.remedy,
        }
        .into());
    }
    Ok(())
}

/// Fail the synchronous boot when a module queued an async factory: `App::new`
/// has no factory phase, so the value would never be built.
fn check_no_queued_factories(builder: &ContainerBuilder) -> Result<()> {
    if let Some(type_name) = builder.queued_factory_names().first() {
        return Err(UnresolvedFactoryError { type_name }.into());
    }
    Ok(())
}

/// What the register phase leaves the boot to refuse, on either path: a
/// module's own refusal first, then what an import declared or queued there, a
/// module no register phase reached, then a type registered twice.
fn check_register_phase(builder: &mut ContainerBuilder) -> Result<()> {
    if let Some(refusal) = builder.take_refusal() {
        return Err(refusal);
    }
    // A dynamic import whose value no collect parked collects in `register`,
    // so it may declare there too.
    check_contested_declarations(builder)?;
    if let Some(type_name) = builder.late_factory_name() {
        return Err(LateFactoryError { type_name }.into());
    }
    if let Some(module) = builder.unregistered_module() {
        return Err(UnregisteredModuleError { module }.into());
    }
    check_duplicate_providers(builder)
}

impl App {
    /// Build the container from the root module synchronously.
    ///
    /// # Errors
    ///
    /// Every wiring failure: [`AccessGraphError`](crate::AccessGraphError),
    /// [`MissingDependencyError`](crate::MissingDependencyError),
    /// [`DuplicateProviderError`],
    /// [`ProviderCycleError`](crate::ProviderCycleError),
    /// [`UnresolvedFactoryError`] for a module that queued an async factory, and
    /// a module's own refusal ([`ContainerBuilder::refuse`]).
    pub fn new<M: Module + 'static>() -> Result<Self> {
        #[cfg(feature = "logging")]
        crate::logging::init_fallback()?;
        // `collect` runs first so a module queuing a factory is refused below.
        let root = std::any::type_name::<M>();
        let builder = Container::builder()
            .enter_phase(Phase::Collect)
            .enter_root(root)
            .import::<M>()
            .leave_import();
        // Before the queue check: a contested declaration survives the remedy
        // `UnresolvedFactoryError` prescribes (`.claude/decisions/boot-refusal-order.md`).
        check_contested_declarations(&builder)?;
        check_no_queued_factories(&builder)?;
        let mut builder = builder
            .enter_phase(Phase::Register)
            .enter_root(root)
            .import::<M>()
            .leave_import();
        check_register_phase(&mut builder)?;
        // Seeded after register, yet global infrastructure for the access graph.
        let global: HashSet<TypeId> = HashSet::from([
            TypeId::of::<ReachableProviders>(),
            TypeId::of::<ProviderOrder>(),
            TypeId::of::<Composition>(),
        ]);
        let descriptors: Vec<&ModuleDescriptor> = inventory::iter::<ModuleDescriptor>().collect();
        let builder = seal(
            builder,
            &descriptors,
            &[(TypeId::of::<M>(), root)],
            &global,
            &HashSet::new(),
        )?;
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
    pub async fn init(&self) -> Result<()> {
        run_phase(&self.container, LifecyclePhase::OnModuleInit).await?;
        run_phase(&self.container, LifecyclePhase::OnApplicationBootstrap).await?;
        Ok(())
    }

    /// Configure each transport against the container, run the init lifecycle
    /// hooks, then run all transports concurrently. SIGINT / SIGTERM cancels the
    /// shared token; the first transport that errors also cancels the others.
    /// Once the transports have stopped, the shutdown hooks run within
    /// [`SHUTDOWN_HOOKS_TIMEOUT`](crate::SHUTDOWN_HOOKS_TIMEOUT); a hook that
    /// fails or panics is reported and the rest still run.
    ///
    /// Each transport bounds its own stop ([`Transport::stop_bound`]); the
    /// `way down bounded` boot line files the longest beside the hooks' budget.
    /// A signal received once the way down has begun exits at once (130 /
    /// 143). Transports come only from imported modules.
    pub async fn run(self) -> Result<()> {
        let App { container } = self;

        tracing::info!(
            target: crate::target::APP,
            version = env!("CARGO_PKG_VERSION"),
            "nestrs starting",
        );

        let mut transports: Vec<(&'static str, Box<dyn Transport>)> = Vec::new();
        for contribution in Discovery::new(&container).meta::<TransportContribution>() {
            let transport = (contribution.meta.build)(&container)?;
            tracing::info!(
                target: crate::target::APP,
                transport = contribution.meta.name,
                "attached module-contributed transport",
            );
            transports.push((contribution.meta.name, transport));
        }

        for (_, t) in transports.iter_mut() {
            t.configure(&container).await?;
        }
        let stop_bound = transports
            .iter()
            .map(|(_, t)| t.stop_bound())
            .max()
            .unwrap_or_default();
        tracing::info!(
            target: crate::target::APP,
            stop_bound_ms = u64::try_from(stop_bound.as_millis()).unwrap_or(u64::MAX),
            hooks_budget_ms = u64::try_from(crate::SHUTDOWN_HOOKS_TIMEOUT.as_millis()).unwrap_or(u64::MAX),
            "way down bounded",
        );

        run_phase(&container, LifecyclePhase::OnModuleInit).await?;
        run_phase(&container, LifecyclePhase::OnApplicationBootstrap).await?;

        let cancel = CancellationToken::new();
        let way_down = Arc::new(WayDown::default());
        watch_signals(cancel.clone(), Arc::clone(&way_down));

        let mut join = JoinSet::new();
        let mut serving: HashMap<tokio::task::Id, &'static str> = HashMap::new();
        for (name, transport) in transports {
            let token = cancel.clone();
            let task = join.spawn(crate::panic::contain(async move {
                transport.serve(token).await
            }));
            serving.insert(task.id(), name);
        }
        way_down.transports(still_serving(&serving));

        let mut first_err: Option<anyhow::Error> = None;
        while let Some(res) = join.join_next_with_id().await {
            let ended = match &res {
                Ok((task, _)) => *task,
                Err(join_err) => join_err.id(),
            };
            let transport = serving.remove(&ended).unwrap_or("<unknown>");
            way_down.transports(still_serving(&serving));
            match res.map(|(_, served)| served) {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(e))) => {
                    if first_err.is_none() {
                        tracing::error!(target: crate::target::APP, error = %crate::error_message(&*e), "transport failed; shutting down");
                        first_err = Some(e);
                        cancel.cancel();
                    }
                }
                // Said even after the first error: no one else files a contained panic.
                Ok(Err(payload)) => {
                    crate::contained_panic!(
                        target: crate::target::APP,
                        payload.as_ref(),
                        "transport task panicked; shutting down",
                        transport,
                    );
                    if first_err.is_none() {
                        first_err = Some(anyhow!("transport `{transport}` panicked"));
                        cancel.cancel();
                    }
                }
                // Cancelled with its runtime; a panic raised as it was dropped is the hook's.
                Err(join_err) => {
                    if first_err.is_none() {
                        tracing::error!(
                            target: crate::target::APP,
                            transport,
                            error = %crate::error_message(&join_err),
                            "transport task ended before it returned; shutting down",
                        );
                        first_err =
                            Some(anyhow!("transport `{transport}` ended before it returned"));
                        cancel.cancel();
                    }
                }
            }
        }

        // The three phases share one deadline, which `way_down::__main` reads too.
        let deadline = tokio::time::Instant::now() + crate::SHUTDOWN_HOOKS_TIMEOUT;
        crate::way_down::hooks_deadline(crate::SHUTDOWN_HOOKS_TIMEOUT);
        for phase in [
            LifecyclePhase::OnModuleDestroy,
            LifecyclePhase::BeforeApplicationShutdown,
            LifecyclePhase::OnApplicationShutdown,
        ] {
            run_phase_lenient(&container, phase, deadline, &way_down).await;
        }
        way_down.exit();

        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

struct ModuleHooks {
    type_id: TypeId,
    name: &'static str,
    import: fn(ContainerBuilder) -> ContainerBuilder,
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
///    **skipped**.
/// 4. **Register** — each module's [`register`](crate::Module::register) builds
///    its providers last, injecting seeds and factory outputs. A module that
///    cannot build what it must [`refuse`](ContainerBuilder::refuse)s, and the
///    boot fails with its error once the phase ends.
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
    /// field or [`Container::get_keyed`](crate::Container::get_keyed); one
    /// instance per `name`, injectable by any provider reachable from the root.
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

    /// Seed module-less metadata of type `M` ([`ContainerBuilder::provide_meta`]
    /// at the app root), for global builder extensions such as `use_guards_global`.
    pub fn provide_meta<M: Any + Send + Sync>(mut self, meta: M) -> Self {
        self.builder = self.builder.provide_meta(meta);
        self
    }

    /// Register an async factory at the composition root, for a resource no
    /// module owns. A seed of the same type wins (the factory is skipped).
    ///
    /// ```
    /// # use anyhow::Context as _;
    /// # use nest_rs_core::{App, module};
    /// # struct DbConfig { url: String }
    /// # impl DbConfig {
    /// #     fn from_env() -> Self { Self { url: "postgres://localhost/app".into() } }
    /// # }
    /// # struct DbPool { url: String }
    /// # impl DbPool {
    /// #     async fn connect(url: &str) -> anyhow::Result<Self> { Ok(Self { url: url.into() }) }
    /// # }
    /// # #[module]
    /// # struct AppModule;
    /// # #[nest_rs_core::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// let app = App::builder()
    ///     .provide(DbConfig::from_env())
    ///     .provide_factory(|c| async move {
    ///         let cfg = c.get::<DbConfig>().context("DbConfig seeded")?;
    ///         Ok(DbPool::connect(&cfg.url).await?)
    ///     })
    ///     .module::<AppModule>()
    ///     .build()
    ///     .await?;
    /// let pool = app.container().get::<DbPool>();
    /// assert_eq!(pool.map(|p| p.url.clone()).as_deref(), Some("postgres://localhost/app"));
    /// # Ok(())
    /// # }
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
    /// registers, so this value wins — for tests swapping a real provider for a
    /// fake.
    ///
    /// Providers built in the register phase keep the original `Arc`; override
    /// the `dyn Trait` instead ([`override_dyn`](Self::override_dyn)).
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
            .push(Box::new(move |builder| builder.replace_dyn(value)));
        self
    }

    /// [`override_value`](Self::override_value) for a value the test already
    /// holds in an `Arc`. Eager-build caveat applies.
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
            name: std::any::type_name::<M>(),
            import: ContainerBuilder::import::<M>,
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

        builder = builder.enter_phase(Phase::Collect);
        for hooks in &modules {
            builder = (hooks.import)(builder.enter_root(hooks.name)).leave_import();
        }
        check_contested_declarations(&builder)?;
        let descriptors: Vec<&ModuleDescriptor> = inventory::iter::<ModuleDescriptor>().collect();
        let roots: Vec<(TypeId, &'static str)> =
            modules.iter().map(|h| (h.type_id, h.name)).collect();
        let ids: Vec<TypeId> = roots.iter().map(|(id, _)| *id).collect();
        let reached = reachable_descriptors(&descriptors, &ids);
        // Refused as soon as the resource exists, before a later factory's I/O
        // fails on something else first.
        check_budgets(&builder, &reached, None)?;
        // A seed supplying a factory's output type skips it. The next to run is
        // the first whose `after` types are present or provided by nothing still
        // queued; only a cycle leaves nothing runnable.
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
            if !builder.contains(queued.id()) {
                let register = (queued.factory)(builder.snapshot()).await?;
                builder = register(builder);
            }
            for derive in queued.derives {
                builder = derive(builder);
            }
            check_budgets(&builder, &reached, Some(&queued.provides))?;
        }
        // Seeded after register, yet global infrastructure for the access graph.
        let mut global = builder.provider_ids();
        global.insert(TypeId::of::<ReachableProviders>());
        global.insert(TypeId::of::<ProviderOrder>());
        global.insert(TypeId::of::<Composition>());
        let global_keyed: HashSet<ProviderKey> = builder.keyed_provider_keys();
        builder = builder.enter_phase(Phase::Register);
        for hooks in &modules {
            builder = (hooks.import)(builder.enter_root(hooks.name)).leave_import();
        }
        check_register_phase(&mut builder)?;
        for ov in overrides {
            builder = ov(builder);
        }

        let builder = seal(builder, &descriptors, &roots, &global, &global_keyed)?;
        Ok(App {
            container: builder.build(),
        })
    }
}

/// The boot's last pass, shared by both paths: the access graph checked over
/// what registered, every budget against the nets reaching it, then the seeds
/// the transports read.
fn seal(
    builder: ContainerBuilder,
    descriptors: &[&ModuleDescriptor],
    roots: &[(TypeId, &'static str)],
    global: &HashSet<TypeId>,
    global_keyed: &HashSet<ProviderKey>,
) -> Result<ContainerBuilder> {
    let ids: Vec<TypeId> = roots.iter().map(|(id, _)| *id).collect();
    validate_access_graph(
        descriptors,
        &ids,
        global,
        &builder.registered_ids(),
        &builder.scoped_or_transient_ids(),
    )
    .map_err(AccessError::into_anyhow)?;
    validate_keyed_access_graph(descriptors, &ids, global_keyed)?;
    check_budgets(&builder, &reachable_descriptors(descriptors, &ids), None)?;
    let order = provider_order(descriptors, &ids);
    let reachable = ReachableProviders(reachable_provider_ids(&order, global));
    let composition = Composition::from_descriptors(descriptors, roots);
    Ok(builder
        .provide(reachable)
        .provide(ProviderOrder::new(order))
        .provide(composition))
}

fn still_serving(serving: &HashMap<tokio::task::Id, &'static str>) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = serving.values().copied().collect();
    names.sort_unstable();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::{Collecting, Registering};

    struct Config(u32);
    struct Doubled(u32);

    // `#[module]` lives in `nest-rs-core-macros`, so these tests hand-write the impl.
    struct DoublerModule;
    impl Module for DoublerModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
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

    struct SecondAfterFirst;
    impl Module for SecondAfterFirst {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
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
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder.provide_factory(|_| async { Ok(First(41)) })
        }
    }

    #[tokio::test]
    async fn a_factory_declared_after_another_runs_after_it_whatever_the_queue_order() {
        let app = App::builder()
            .module::<SecondAfterFirst>()
            .module::<FirstModule>()
            .build()
            .await
            .expect("the declared order is honoured");
        assert_eq!(app.container().get::<Second>().unwrap().0, 42);
    }

    struct Third(u32);
    struct ThirdAfterBoth;
    impl Module for ThirdAfterBoth {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder.provide_declared_factory_after_both::<Third, First, Second, _, _>(
                "one declaration",
                |c| async move {
                    let first = c
                        .get::<First>()
                        .ok_or_else(|| anyhow!("First runs first"))?;
                    let second = c
                        .get::<Second>()
                        .ok_or_else(|| anyhow!("Second runs first"))?;
                    Ok(Third(first.0 + second.0))
                },
            )
        }
    }

    struct SecondModule;
    impl Module for SecondModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder.provide_factory(|_| async { Ok(Second(1)) })
        }
    }

    #[tokio::test]
    async fn a_factory_declared_after_two_others_runs_after_both_whatever_the_queue_order() {
        for modules in [[0, 1, 2], [0, 2, 1], [1, 0, 2], [2, 0, 1]] {
            let mut app = App::builder();
            for module in modules {
                app = match module {
                    0 => app.module::<ThirdAfterBoth>(),
                    1 => app.module::<FirstModule>(),
                    _ => app.module::<SecondModule>(),
                };
            }
            let app = app.build().await.expect("the declared order is honoured");
            assert_eq!(app.container().get::<Third>().unwrap().0, 42, "{modules:?}");
        }
    }

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
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder.provide_factory_after::<ReadsPort, Arc<dyn Port>, _, _>(|c| async move {
                let port = c
                    .get_dyn::<dyn Port>()
                    .ok_or_else(|| anyhow!("dyn Port must already be bound"))?;
                Ok(ReadsPort(port.value() + 1))
            })
        }
    }
    fn binds_port(builder: ContainerBuilder) -> ContainerBuilder {
        builder.provide_factory_dyn::<PortImpl, dyn Port, _, _>(
            |_| async { Ok(PortImpl(41)) },
            |p| Arc::new(p) as Arc<dyn Port>,
        )
    }
    struct BindsPortModule;
    impl Module for BindsPortModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            binds_port(builder)
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

    #[tokio::test]
    async fn a_seeded_trait_object_wins_over_the_binding_a_factory_makes_of_it() {
        let app = App::builder()
            .provide_dyn::<dyn Port>(Arc::new(PortImpl(7)))
            .module::<BindsPortModule>()
            .build()
            .await
            .expect("the factory still builds the concrete type nobody seeded");
        assert_eq!(
            app.container().get_dyn::<dyn Port>().map(|p| p.value()),
            Some(7)
        );
        assert_eq!(app.container().get::<PortImpl>().map(|p| p.0), Some(41));
    }

    #[derive(Clone)]
    struct OtherPortImpl(u32);
    impl Port for OtherPortImpl {
        fn value(&self) -> u32 {
            self.0
        }
    }
    struct BindsPortOtherwiseModule;
    impl Module for BindsPortOtherwiseModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder.provide_factory_dyn::<OtherPortImpl, dyn Port, _, _>(
                |_| async { Ok(OtherPortImpl(9)) },
                |p| Arc::new(p) as Arc<dyn Port>,
            )
        }
    }

    #[tokio::test]
    async fn two_dyn_factories_binding_one_trait_object_fail_the_boot_naming_both() {
        for binds_first in [true, false] {
            let (app, first, second) = if binds_first {
                (
                    App::builder()
                        .module::<BindsPortModule>()
                        .module::<BindsPortOtherwiseModule>(),
                    "BindsPortModule`",
                    "BindsPortOtherwiseModule`",
                )
            } else {
                (
                    App::builder()
                        .module::<BindsPortOtherwiseModule>()
                        .module::<BindsPortModule>(),
                    "BindsPortOtherwiseModule`",
                    "BindsPortModule`",
                )
            };
            let Err(refused) = app.build().await else {
                panic!("one binding would be dropped for the other");
            };
            let contested = refused
                .downcast_ref::<ContestedDeclarationError>()
                .unwrap_or_else(|| panic!("not the contest: {refused:#}"));
            assert_eq!(contested.type_name, std::any::type_name::<Arc<dyn Port>>());
            assert!(contested.first.contains(first), "{contested:?}");
            assert!(contested.second.contains(second), "{contested:?}");
        }
    }

    struct BindsPortBothWaysModule;
    impl Module for BindsPortBothWaysModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder
                .import::<BindsPortModule>()
                .import::<BindsPortOtherwiseModule>()
        }
    }

    #[test]
    fn the_synchronous_boot_names_a_contested_binding_ahead_of_the_queue() {
        let Err(refused) = App::new::<BindsPortBothWaysModule>() else {
            panic!("one binding would be dropped for the other");
        };
        assert!(
            refused
                .downcast_ref::<ContestedDeclarationError>()
                .is_some(),
            "{refused:#}"
        );
    }

    const PORT_REMEDY: &str = "Import exactly one port binding.";
    struct DeclaresPortModule;
    impl Module for DeclaresPortModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder.provide_declared_factory::<Arc<dyn Port>, _, _>(PORT_REMEDY, |_| async {
                Ok(Arc::new(PortImpl(3)) as Arc<dyn Port>)
            })
        }
    }

    #[tokio::test]
    async fn a_dyn_factory_contests_a_declared_binding_of_its_trait_object_in_the_ports_words() {
        for declared_first in [true, false] {
            let app = if declared_first {
                App::builder()
                    .module::<DeclaresPortModule>()
                    .module::<BindsPortModule>()
            } else {
                App::builder()
                    .module::<BindsPortModule>()
                    .module::<DeclaresPortModule>()
            };
            let Err(refused) = app.build().await else {
                panic!("declared first: {declared_first}: one binding would be dropped");
            };
            let contested = refused
                .downcast_ref::<ContestedDeclarationError>()
                .unwrap_or_else(|| panic!("not the contest: {refused:#}"));
            assert_eq!(
                contested.remedy, PORT_REMEDY,
                "declared first: {declared_first}"
            );
        }
    }

    // A port's default implementation, as the port's own module queues it.
    struct DefaultPortModule;
    impl Module for DefaultPortModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder.provide_factory::<Arc<dyn Port>, _, _>(|_| async {
                Ok(Arc::new(PortImpl(0)) as Arc<dyn Port>)
            })
        }
    }

    #[tokio::test]
    async fn a_dyn_factory_supersedes_its_ports_default_whatever_the_queue_order() {
        for default_first in [true, false] {
            let app = if default_first {
                App::builder()
                    .module::<DefaultPortModule>()
                    .module::<BindsPortModule>()
            } else {
                App::builder()
                    .module::<BindsPortModule>()
                    .module::<DefaultPortModule>()
            };
            let app = app
                .build()
                .await
                .unwrap_or_else(|e| panic!("default first: {default_first}: {e:#}"));
            assert_eq!(
                app.container().get_dyn::<dyn Port>().map(|p| p.value()),
                Some(41),
                "default first: {default_first}"
            );
        }
    }

    #[tokio::test]
    async fn a_seeded_concrete_type_is_what_a_dyn_factory_binds_its_trait_object_to() {
        let app = App::builder()
            .provide(PortImpl(5))
            .module::<BindsPortModule>()
            .build()
            .await
            .expect("the seed wins over the factory, and the binding follows it");
        assert_eq!(
            app.container().get_dyn::<dyn Port>().map(|p| p.value()),
            Some(5)
        );
    }

    // An import site's choice of the concrete type a dyn factory binds.
    struct DeclaresPortImplModule;
    impl Module for DeclaresPortImplModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder.provide_declared_factory::<PortImpl, _, _>("one declaration", |_| async {
                Ok(PortImpl(8))
            })
        }
    }

    #[tokio::test]
    async fn a_declared_concrete_type_is_what_a_dyn_factory_binds_its_trait_object_to() {
        for declared_first in [true, false] {
            // The dependent comes first: it waits on the trait object only if
            // the entry building the declared type says it binds it.
            let app = App::builder().module::<ReadsPortModule>();
            let app = if declared_first {
                app.module::<DeclaresPortImplModule>()
                    .module::<BindsPortModule>()
            } else {
                app.module::<BindsPortModule>()
                    .module::<DeclaresPortImplModule>()
            };
            let app = app
                .build()
                .await
                .unwrap_or_else(|e| panic!("declared first: {declared_first}: {e:#}"));
            let container = app.container();
            assert_eq!(container.get::<PortImpl>().map(|p| p.0), Some(8));
            assert_eq!(
                container.get_dyn::<dyn Port>().map(|p| p.value()),
                Some(8),
                "declared first: {declared_first}"
            );
            assert_eq!(container.get::<ReadsPort>().map(|r| r.0), Some(9));
        }
    }

    #[tokio::test]
    async fn a_dyn_factory_over_a_declared_type_supersedes_its_ports_default_in_every_order() {
        let orders: [[u8; 3]; 6] = [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ];
        for order in orders {
            let mut app = App::builder();
            for module in order {
                app = match module {
                    0 => app.module::<DefaultPortModule>(),
                    1 => app.module::<DeclaresPortImplModule>(),
                    _ => app.module::<BindsPortModule>(),
                };
            }
            let app = app
                .build()
                .await
                .unwrap_or_else(|e| panic!("{order:?}: {e:#}"));
            assert_eq!(
                app.container().get_dyn::<dyn Port>().map(|p| p.value()),
                Some(8),
                "{order:?}: the declared type binds the port, not its default"
            );
        }
    }

    #[tokio::test]
    async fn a_dyn_factory_whose_concrete_type_is_declared_still_contests_its_trait_object() {
        for declared_first in [true, false] {
            let app = App::builder().module::<BindsPortOtherwiseModule>();
            let app = if declared_first {
                app.module::<DeclaresPortImplModule>()
                    .module::<BindsPortModule>()
            } else {
                app.module::<BindsPortModule>()
                    .module::<DeclaresPortImplModule>()
            };
            let Err(refused) = app.build().await else {
                panic!("declared first: {declared_first}: one binding would be dropped");
            };
            let contested = refused
                .downcast_ref::<ContestedDeclarationError>()
                .unwrap_or_else(|| panic!("not the contest: {refused:#}"));
            assert_eq!(contested.type_name, std::any::type_name::<Arc<dyn Port>>());
        }
    }

    // The same binding queued by a second module — two setups of one module
    // are the shape, since dynamic imports are not deduplicated.
    struct BindsPortAgainModule;
    impl Module for BindsPortAgainModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            binds_port(builder)
        }
    }

    #[tokio::test]
    async fn one_dyn_binding_queued_twice_is_one_binding() {
        let app = App::builder()
            .module::<BindsPortModule>()
            .module::<BindsPortAgainModule>()
            .build()
            .await
            .expect("a diamond is not a contest");
        assert_eq!(
            app.container().get_dyn::<dyn Port>().map(|p| p.value()),
            Some(41)
        );
    }

    #[test]
    fn the_synchronous_boot_refuses_a_static_modules_queued_factory() {
        let err = match App::new::<BindsPortModule>() {
            Ok(_) => panic!("a queued factory cannot be drained synchronously"),
            Err(e) => e,
        };
        assert!(
            err.downcast_ref::<UnresolvedFactoryError>().is_some(),
            "{err}"
        );
    }

    // A static module whose `register` reads what its own `collect` queued —
    // a binding building its interceptor over the pool its factory checked.
    struct ReadsItsFactoryModule;
    impl Module for ReadsItsFactoryModule {
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder.import::<ConfigModule>()
        }
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder.import::<DoublerModule>()
        }
    }

    #[test]
    fn the_synchronous_boot_refuses_a_queued_factory_before_a_register_reads_its_output() {
        let err = match App::new::<ReadsItsFactoryModule>() {
            Ok(_) => panic!("a queued factory cannot be drained synchronously"),
            Err(e) => e,
        };
        let unresolved = err
            .downcast_ref::<UnresolvedFactoryError>()
            .unwrap_or_else(|| panic!("not the queue refusal: {err:#}"));
        assert!(unresolved.type_name.ends_with("Config"), "{unresolved:?}");
    }

    // Every `for_root` queues its config, then a factory reading it, in one
    // `collect`; a bare import and a pinned one of the same module both do.
    struct Connection(u32);
    fn connect(builder: ContainerBuilder) -> ContainerBuilder {
        builder.provide_factory(|c| async move {
            let config = c
                .get::<Config>()
                .ok_or_else(|| anyhow!("Config is queued ahead of the factory reading it"))?;
            Ok(Connection(config.0))
        })
    }
    struct BareSetup;
    impl Module for BareSetup {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            connect(builder.provide_factory(|_| async { Ok(Config(1)) }))
        }
    }
    struct PinnedSetup;
    impl Module for PinnedSetup {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            connect(
                builder.provide_declared_factory("one declaration", |_| async { Ok(Config(7)) }),
            )
        }
    }

    #[tokio::test]
    async fn a_declaration_takes_the_slot_of_the_default_it_displaces() {
        for pinned_first in [false, true] {
            let app = if pinned_first {
                App::builder().module::<PinnedSetup>().module::<BareSetup>()
            } else {
                App::builder().module::<BareSetup>().module::<PinnedSetup>()
            };
            let app = app
                .build()
                .await
                .unwrap_or_else(|e| panic!("pinned first: {pinned_first}: {e:#}"));
            assert_eq!(app.container().get::<Connection>().unwrap().0, 7);
        }
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
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
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
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
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
        fn stop_bound(&self) -> std::time::Duration {
            std::time::Duration::ZERO
        }
    }

    struct WithTransportModule;
    impl Module for WithTransportModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
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
        let contributions = Discovery::new(app.container()).meta::<TransportContribution>();
        assert_eq!(contributions.len(), 1);
        assert_eq!(contributions[0].meta.name, "NullTransport");
    }

    // A module queuing its factory from `register`: the collect phase every
    // factory is queued in has already ended, on either path.
    struct Late(u32);
    struct QueuesInRegisterModule;
    impl Module for QueuesInRegisterModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder.provide_factory(|_| async { Ok(Late(1)) })
        }
    }

    #[tokio::test]
    async fn a_factory_queued_in_register_fails_either_boot_naming_its_type() {
        let Err(built) = App::builder()
            .module::<QueuesInRegisterModule>()
            .build()
            .await
        else {
            panic!("nothing drains a factory queued after the factory phase");
        };
        let Err(synchronous) = App::new::<QueuesInRegisterModule>() else {
            panic!("nothing drains a factory on the synchronous path");
        };
        for refused in [built, synchronous] {
            let late = refused
                .downcast_ref::<LateFactoryError>()
                .unwrap_or_else(|| panic!("not the late-factory refusal: {refused:#}"));
            assert!(late.type_name.ends_with("Late"), "{late:?}");
        }
    }

    struct SeedsLateModule;
    impl Module for SeedsLateModule {
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder.provide_factory(|_| async { Ok(Late(7)) })
        }
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder.import::<QueuesInRegisterModule>()
        }
    }

    #[tokio::test]
    async fn a_factory_queued_in_register_for_a_present_type_is_passed_over() {
        let app = App::builder()
            .module::<SeedsLateModule>()
            .build()
            .await
            .expect("the factory phase already built what the late factory would");
        assert_eq!(app.container().get::<Late>().map(|late| late.0), Some(7));
    }

    // A pin made in `register`: the value its type already holds is the
    // default's, not the one the pin chose.
    struct DeclaresLateModule;
    impl Module for DeclaresLateModule {
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder.provide_factory(|_| async { Ok(Late(7)) })
        }
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder.provide_declared_factory("pin it in `collect`", |_| async { Ok(Late(1)) })
        }
    }

    #[tokio::test]
    async fn a_declaration_queued_in_register_fails_the_boot_though_its_type_is_present() {
        let Err(refused) = App::builder().module::<DeclaresLateModule>().build().await else {
            panic!("the pinned value would be dropped for the default's");
        };
        let late = refused
            .downcast_ref::<LateFactoryError>()
            .unwrap_or_else(|| panic!("not the late-factory refusal: {refused:#}"));
        assert!(late.type_name.ends_with("Late"), "{late:?}");
    }

    // A dyn binding made in `register`, over a concrete type an import declared.
    struct BindsDeclaredPortLateModule;
    impl Module for BindsDeclaredPortLateModule {
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder.import::<DeclaresPortImplModule>()
        }
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder.import::<BindsPortModule>()
        }
    }

    #[tokio::test]
    async fn a_dyn_binding_queued_in_register_over_a_declared_type_fails_the_boot() {
        let Err(refused) = App::builder()
            .module::<BindsDeclaredPortLateModule>()
            .build()
            .await
        else {
            panic!("the trait object would be left unbound");
        };
        let late = refused
            .downcast_ref::<LateFactoryError>()
            .unwrap_or_else(|| panic!("not the late-factory refusal: {refused:#}"));
        assert!(late.type_name.ends_with("PortImpl"), "{late:?}");
    }

    // A module providing a value in its register, and a hand-written importer
    // that imports it in its collect alone.
    struct RegistersFirstModule;
    impl Module for RegistersFirstModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder.provide(First(1))
        }
    }
    struct ImportsInCollectAloneModule;
    impl Module for ImportsInCollectAloneModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder.import::<RegistersFirstModule>()
        }
    }

    #[tokio::test]
    async fn a_module_imported_in_a_collect_alone_fails_either_boot_naming_it() {
        let Err(built) = App::builder()
            .module::<ImportsInCollectAloneModule>()
            .build()
            .await
        else {
            panic!("its providers would never register");
        };
        let Err(synchronous) = App::new::<ImportsInCollectAloneModule>() else {
            panic!("its providers would never register");
        };
        for refused in [built, synchronous] {
            let unregistered = refused
                .downcast_ref::<UnregisteredModuleError>()
                .unwrap_or_else(|| panic!("not the unregistered-module refusal: {refused:#}"));
            assert!(
                unregistered.module.ends_with("RegistersFirstModule"),
                "{unregistered:?}"
            );
        }
    }

    struct Refused;
    struct RefusingModule;
    impl Module for RefusingModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
                .refuse(anyhow!("RefusingModule: the first refusal"))
                .refuse(anyhow!("RefusingModule: a second refusal"))
                .provide(Refused)
                .provide(Refused)
        }
    }

    #[tokio::test]
    async fn a_register_refusing_fails_either_boot_with_its_first_error() {
        let Err(built) = App::builder().module::<RefusingModule>().build().await else {
            panic!("a refusal fails the boot");
        };
        let Err(synchronous) = App::new::<RefusingModule>() else {
            panic!("a refusal fails the synchronous boot");
        };
        for refused in [built, synchronous] {
            assert_eq!(
                format!("{refused:#}"),
                "RefusingModule: the first refusal",
                "the module's own error, ahead of the duplicate it also left"
            );
        }
    }
}

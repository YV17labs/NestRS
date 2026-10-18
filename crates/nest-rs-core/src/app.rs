//! The [`App`] and its [`AppBuilder`] — the boot entry point that wires the
//! root module, runs the four build phases, validates the access graph, and
//! drives the lifecycle and transports.

use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::access::{
    Composition, ModuleDescriptor, ProviderOrder, ReachableProviders, dependency_order,
    provider_order, reachable_descriptors, reachable_provider_ids, scoped_cycle, singleton_cycle,
    validate_access_graph,
};
use crate::app_lifecycle::{AppLifecycle, StopCause};
use crate::close::InstallRecord;
use crate::container::ProviderScope;
use crate::container::{Container, ContainerBuilder, Phase, Registrar};
use crate::discovery::Discovery;
use crate::error::{
    AccessError, ContainerError, FactoryCycleError, FactoryFailedError, InitFailedError,
    OverriddenDependencyError, UnclaimedOverrideError, UnresolvedFactoryError,
};
use crate::lifecycle::{HookFailure, HookOrder, LifecyclePhase, run_phase, run_phase_lenient};
use crate::millis::millis;
use crate::module::Module;
use crate::net::check_budgets;
use crate::transport::{Transport, TransportContribution};

/// Entry point for a nestrs application. Builds the container from a root
/// [`Module`] and runs every transport its imports contribute concurrently
/// until shutdown.
pub struct App {
    container: Container,
    lifecycle: Arc<AppLifecycle>,
    init: tokio::sync::Mutex<Init>,
    /// The order its types were installed in, and what the app handed in
    /// rather than a module opening it: what the closes read.
    installed: InstallRecord,
}

/// Where the app's init phases stand: they run once per [`App`], and an init
/// that failed or never finished is kept as failed, so no hook runs twice.
#[derive(Clone, Copy)]
enum Init {
    NotRun,
    Done,
    /// An init that never finished: hooks may have run, none was torn down.
    Failed,
    /// An init that failed and ran the teardown, within this deadline: what
    /// remains is the container's drop.
    TornDown(Instant),
}

/// The step of the boot a stop interrupted.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum BootStep {
    /// An async factory, building its type.
    Factory {
        /// The type the factory builds.
        type_name: &'static str,
    },
    /// A transport's [`configure`](Transport::configure).
    Configure {
        /// The transport, by its short type name.
        transport: String,
    },
    /// A transport's [`listen`](Transport::listen), once every init hook ran.
    Listen {
        /// The transport, by its short type name.
        transport: String,
    },
    /// An init lifecycle hook.
    InitHook {
        /// The init phase it runs in.
        phase: LifecyclePhase,
        /// The host provider's name.
        provider: &'static str,
        /// The hook method's name.
        method: &'static str,
        /// `module_path!()` at the `#[hooks]` site.
        origin: &'static str,
    },
}

impl std::fmt::Display for BootStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Factory { type_name } => write!(f, "the factory for `{type_name}`"),
            Self::Configure { transport } => {
                write!(f, "the `configure` of transport `{transport}`")
            }
            Self::Listen { transport } => write!(f, "the `listen` of transport `{transport}`"),
            Self::InitHook {
                phase,
                provider,
                method,
                origin,
            } => write!(
                f,
                "lifecycle hook {origin}::{provider}::{method} ({phase:?})"
            ),
        }
    }
}

/// Fail the boot when two import sites each declared a value for one type —
/// neither may silently win on queue position.
fn check_contested_declarations(builder: &ContainerBuilder) -> Result<()> {
    match builder.contested_refusal() {
        Some(contested) => Err(contested.into()),
        None => Ok(()),
    }
}

/// Fail the synchronous boot when a module queued an async factory: `App::new`
/// has no factory phase, so the value would never be built.
fn check_no_queued_factories(builder: &ContainerBuilder) -> Result<()> {
    if let Some(type_name) = builder.queued_factory_names().first() {
        return Err(UnresolvedFactoryError { type_name }.into());
    }
    Ok(())
}

/// What the register phase leaves the boot to refuse, on either path
/// ([`ContainerBuilder::register_refusal`]).
fn check_register_phase(builder: &mut ContainerBuilder) -> Result<()> {
    builder
        .register_refusal()
        .map_err(ContainerError::into_anyhow)
}

/// Fail the boot on the first cycle among singletons that a provider never
/// built, or built without an optional dependency, waits on: across modules
/// importing each other, which the register phase cannot see whole. Read
/// before either is refused, whose remedy — a module both import — would only
/// lead here.
fn check_singleton_cycles(
    builder: &ContainerBuilder,
    descriptors: &[&ModuleDescriptor],
    roots: &[TypeId],
    scopes: &HashMap<TypeId, ProviderScope>,
    overridden: &HashSet<TypeId>,
) -> Result<()> {
    match singleton_cycle(descriptors, roots, scopes, &builder.waiting(), overridden) {
        Some(cycle) => Err(cycle.into()),
        None => Ok(()),
    }
}

/// Fail the boot on the first override nothing in the graph claimed: no seed,
/// factory or provider would have registered its type, so nothing reads it.
fn check_unclaimed_overrides(builder: &ContainerBuilder) -> Result<()> {
    match builder.unclaimed_override() {
        Some(type_name) => Err(UnclaimedOverrideError { type_name }.into()),
        None => Ok(()),
    }
}

/// Fail the boot on the first provider the roots reach that injects a key a
/// test override removed — one its overridden provider would have registered
/// beside the key the test overrode. The providers `standing_in` are never
/// built, so what they inject is not read.
fn check_overridden_dependencies(
    builder: &ContainerBuilder,
    reached: &[&ModuleDescriptor],
    standing_in: &HashSet<TypeId>,
) -> Result<()> {
    let overridden = builder.overridden_providers();
    if overridden.iter().all(|o| o.absent.is_empty()) {
        return Ok(());
    }
    for module in reached {
        for p in module.providers {
            if standing_in.contains(&(p.provider)()) {
                continue;
            }
            let (required, required_names) = ((p.injects)(), (p.inject_names)());
            let (optional, optional_names) = ((p.injects_optional)(), (p.inject_optional_names)());
            let injected = crate::access::labelled(&required, &required_names)
                .chain(crate::access::labelled(&optional, &optional_names))
                .chain((p.injects_per_request)());
            for (key, dependency) in injected {
                if let Some(removed) = overridden.iter().find(|o| o.absent.contains(&key)) {
                    return Err(OverriddenDependencyError {
                        module: module.name,
                        consumer: p.name,
                        dependency,
                        provider: removed.provider.1,
                        through: removed.through,
                    }
                    .into());
                }
            }
        }
    }
    Ok(())
}

impl App {
    /// Build the container from the root module synchronously.
    ///
    /// # Errors
    ///
    /// Every wiring failure: [`AccessGraphError`](crate::AccessGraphError),
    /// [`MissingDependencyError`](crate::MissingDependencyError),
    /// [`DuplicateProviderError`](crate::DuplicateProviderError),
    /// [`ProviderCycleError`](crate::ProviderCycleError),
    /// [`UnbuiltProviderError`](crate::UnbuiltProviderError) for a provider the
    /// register phase never built,
    /// [`LateOptionalDependencyError`](crate::LateOptionalDependencyError) for
    /// one built before an optional
    /// dependency the boot registered afterwards,
    /// [`UnresolvedFactoryError`] for a module that queued an async factory, a
    /// module's own refusal ([`ContainerBuilder::refuse`]), and
    /// [`WiringFailedError`](crate::WiringFailedError) for a module's wiring that
    /// failed ([`ContainerBuilder::provide_wiring`]).
    pub fn new<M: Module + 'static>() -> Result<Self> {
        #[cfg(feature = "logging")]
        crate::logging::init_fallback()?;
        let lifecycle = booting();
        // `collect` runs first so a module queuing a factory is refused below.
        let root = std::any::type_name::<M>();
        let builder = ContainerBuilder::booting()
            .provide_arc(Arc::clone(&lifecycle))
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
        // Global infrastructure for the access graph: the lifecycle seeded
        // before collect, the rest after register.
        let global: HashSet<TypeId> = HashSet::from([
            TypeId::of::<AppLifecycle>(),
            TypeId::of::<ReachableProviders>(),
            TypeId::of::<ProviderOrder>(),
            TypeId::of::<Composition>(),
        ]);
        let descriptors: Vec<&ModuleDescriptor> = inventory::iter::<ModuleDescriptor>().collect();
        let sealed = seal(
            &builder,
            &descriptors,
            &[(TypeId::of::<M>(), root)],
            &global,
        )?;
        let installed = builder.install_record();
        let container = sealed.provide(builder).freeze();
        crate::wiring::wire(&container)?;
        Ok(Self::assembled(container, lifecycle, installed))
    }

    fn assembled(
        container: Container,
        lifecycle: Arc<AppLifecycle>,
        installed: InstallRecord,
    ) -> Self {
        Self {
            container,
            lifecycle,
            init: tokio::sync::Mutex::new(Init::NotRun),
            installed,
        }
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

    /// The app's [`AppLifecycle`], the one its providers inject.
    pub fn lifecycle(&self) -> &Arc<AppLifecycle> {
        &self.lifecycle
    }

    /// Run the init lifecycle phases (`OnModuleInit`, then
    /// `OnApplicationBootstrap`) against the built container, without serving.
    ///
    /// They run once per app: a later `init` or [`run`](Self::run) skips them,
    /// and after an init that failed, was interrupted, or was dropped before it
    /// finished, both return [`InitFailedError`] without running a hook. A stop
    /// asked while a hook waits drops it there and returns
    /// [`BootInterruptedError`](crate::BootInterruptedError).
    ///
    /// An init that fails, panics or is interrupted tears the app down before
    /// it returns: the app turns [`Draining`](crate::AppState::Draining), every
    /// shutdown hook runs, in reverse, then the resources close — within
    /// [`SHUTDOWN_HOOKS_TIMEOUT`](crate::SHUTDOWN_HOOKS_TIMEOUT) — and the init's
    /// error is still what it returns.
    pub async fn init(&self) -> Result<()> {
        self.init_tearing_down_after(std::future::ready(())).await
    }

    /// [`init`](Self::init), the teardown of an init that fails waiting for
    /// `stopped` once the app turned `Draining`.
    async fn init_tearing_down_after(&self, stopped: impl Future<Output = ()>) -> Result<()> {
        let mut init = self.init.lock().await;
        match *init {
            Init::Done => return Ok(()),
            Init::Failed | Init::TornDown(_) => return Err(InitFailedError.into()),
            Init::NotRun => {}
        }
        // Failed until the phases return: a caller dropping this future while
        // a hook waits leaves it so, and no hook that finished runs again.
        *init = Init::Failed;
        let Err(failed) = self.init_phases().await else {
            *init = Init::Done;
            return Ok(());
        };
        tracing::warn!(
            target: crate::target::LIFECYCLE,
            phase = ?failed.hook.phase,
            provider = failed.hook.provider,
            method = failed.hook.method,
            origin = failed.hook.origin,
            "the boot failed after init hooks ran: running the shutdown hooks",
        );
        self.lifecycle.drain(StopCause::BootFailed);
        stopped.await;
        let deadline = hooks_budget();
        self.shut_down(true, deadline).await;
        *init = Init::TornDown(deadline);
        Err(failed.error)
    }

    async fn init_phases(&self) -> Result<(), HookFailure> {
        run_phase(
            &self.container,
            &self.lifecycle,
            LifecyclePhase::OnModuleInit,
        )
        .await?;
        run_phase(
            &self.container,
            &self.lifecycle,
            LifecyclePhase::OnApplicationBootstrap,
        )
        .await
    }

    /// The shutdown phases when `hooks`, then the resources' closes, under
    /// `deadline`.
    async fn shut_down(&self, hooks: bool, deadline: Instant) {
        if hooks {
            for phase in [
                LifecyclePhase::OnModuleDestroy,
                LifecyclePhase::BeforeApplicationShutdown,
                LifecyclePhase::OnApplicationShutdown,
            ] {
                run_phase_lenient(&self.container, phase, deadline).await;
            }
        }
        crate::close::close_all(&self.container, &self.installed, deadline).await;
    }

    /// Stop an app that is not serving: it turns
    /// [`Draining`](crate::AppState::Draining) with
    /// [`StopCause::Closed`], runs the shutdown phases in reverse when its init
    /// ran, closes the resources its modules opened, then drops its container —
    /// all within [`SHUTDOWN_HOOKS_TIMEOUT`](crate::SHUTDOWN_HOOKS_TIMEOUT).
    ///
    /// It is how a program that runs work rather than serving ends — a
    /// migration, a one-shot command, a test. A failure on the way is filed by
    /// the rules the shutdown hooks follow, never returned. An app dropped
    /// without `close` runs none of it.
    ///
    /// ```
    /// use nest_rs_core::{App, module};
    ///
    /// #[module]
    /// struct ReportModule;
    ///
    /// # #[nest_rs_core::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// let app = App::builder().module::<ReportModule>().build().await?;
    /// app.init().await?;
    /// // … the work, with what `app.container()` provides …
    /// app.close().await;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn close(self) {
        self.lifecycle.drain(StopCause::Closed);
        self.wind_down().await;
    }

    /// The tail every end of an app shares: the shutdown phases when an init
    /// ran and nothing tore it down yet, the closes, then the container's
    /// drop, under the hooks' one deadline.
    async fn wind_down(mut self) {
        let deadline = match *self.init.get_mut() {
            Init::TornDown(deadline) => deadline,
            init => {
                let deadline = hooks_budget();
                self.shut_down(!matches!(init, Init::NotRun), deadline)
                    .await;
                deadline
            }
        };
        crate::way_down::exit();
        drop_within(self, deadline).await;
    }

    /// Configure each transport against the container, run the init lifecycle
    /// hooks unless [`init`](Self::init) already did, have each transport
    /// [`listen`](Transport::listen), then run all transports concurrently
    /// under the app's stop token. The app is [`Ready`](crate::AppState::Ready)
    /// once every transport listens — the `app ready` line names them and the
    /// addresses they bound — and turns [`Draining`](crate::AppState::Draining)
    /// at `SIGINT` / `SIGTERM` or when a transport fails, which stops the
    /// others. A transport whose `serve` returns before a stop was asked, `Ok`
    /// included, has failed.
    ///
    /// A site a transport composes as it configures — a route, a gateway — that
    /// could not compose (a layer it declares resolves to no provider, a
    /// misordered guard chain) fails the boot here, before any hook runs, as
    /// does a transport stating a stop past
    /// [`MAX_STOP_BOUND`](crate::MAX_STOP_BOUND)
    /// ([`UnboundedTransportError`](crate::UnboundedTransportError)); a
    /// `listen` that fails fails it with the error carrying
    /// [`BootStep::Listen`], before any transport serves; a transport that
    /// panics as it is built, configures or listens fails it as one that
    /// returns an error does, the panic filed once at `error`. A stop
    /// asked while a transport configures or listens, or an init hook waits,
    /// stops the boot there ([`BootInterruptedError`](crate::BootInterruptedError)).
    /// Once the transports have stopped, the shutdown hooks run, then the
    /// resources the modules opened close and the container drops, all within
    /// [`SHUTDOWN_HOOKS_TIMEOUT`](crate::SHUTDOWN_HOOKS_TIMEOUT); a hook that
    /// fails or panics is reported and the rest still run. A boot that fails
    /// gets the same teardown — the shutdown hooks only when an init hook ran —
    /// and its error is still what `run` returns.
    ///
    /// Each transport bounds its own stop ([`Transport::stop_bound`]); the
    /// `way down bounded` boot line files the longest beside the hooks' budget,
    /// and the kernel holds it: a transport still serving at the longest bound
    /// plus [`SHUTDOWN_SETTLE_TIMEOUT`](crate::SHUTDOWN_SETTLE_TIMEOUT) after
    /// the stop is abandoned, named at `error`, and is the app's error. A
    /// signal received once the way down has begun exits at once (130 / 143).
    /// Transports come only from imported modules.
    pub async fn run(self) -> Result<()> {
        let served = self.boot_and_serve().await;
        if served.is_err() {
            // A boot that stopped short of serving; one that served and failed
            // already drains for its transport.
            self.lifecycle.drain(StopCause::BootFailed);
        }
        self.wind_down().await;
        served
    }

    /// `run` up to the shutdown phases: boot what remains of the boot, serve,
    /// and return once every transport has stopped.
    async fn boot_and_serve(&self) -> Result<()> {
        tracing::info!(
            target: crate::target::APP,
            version = env!("CARGO_PKG_VERSION"),
            "nestrs starting",
        );
        if !crate::way_down::watched() {
            crate::way_down::watch_app(&self.lifecycle);
        }

        let mut transports: Vec<(Arc<str>, Box<dyn Transport>)> = Vec::new();
        for contribution in Discovery::new(&self.container).meta::<TransportContribution>() {
            let name: Arc<str> = Arc::from(contribution.meta.name());
            let built =
                crate::panic::contain(async { contribution.meta.build(&self.container) }).await;
            let transport = unwound(&name, None, built)?;
            tracing::info!(
                target: crate::target::APP,
                transport = &*name,
                "attached module-contributed transport",
            );
            transports.push((name, transport));
        }

        for (name, transport) in transports.iter_mut() {
            let step = BootStep::Configure {
                transport: name.to_string(),
            };
            let configured = self
                .lifecycle
                .boot(
                    &step,
                    crate::panic::contain(transport.configure(&self.container)),
                )
                .await?;
            unwound(name, Some(&step), configured).context(step)?;
        }
        // A site composed at `configure` (a route, a gateway) refuses here, its
        // mount having no `Result` to fail with.
        if let Some(refusal) = crate::container::__private::take_site_refusal(&self.container) {
            return Err(refusal);
        }
        let mut stop_bound = Duration::ZERO;
        for (name, transport) in &transports {
            let bound = transport.stop_bound();
            __private::check_stop_bound(name, bound)?;
            stop_bound = stop_bound.max(bound);
        }
        tracing::info!(
            target: crate::target::APP,
            stop_bound_ms = millis(stop_bound),
            hooks_budget_ms = millis(crate::SHUTDOWN_HOOKS_TIMEOUT),
            "way down bounded",
        );

        self.init().await?;

        let mut addresses: Vec<String> = Vec::new();
        for (name, transport) in transports.iter_mut() {
            let step = BootStep::Listen {
                transport: name.to_string(),
            };
            let listened = self
                .lifecycle
                .boot(&step, crate::panic::contain(transport.listen()))
                .await?;
            let listening = unwound(name, Some(&step), listened).context(step)?;
            addresses.extend(listening.addresses.iter().map(ToString::to_string));
        }

        let names: Vec<&str> = transports.iter().map(|(name, _)| &**name).collect();
        self.lifecycle
            .turn_ready(&names.join(", "), &addresses.join(", "));
        self.serve_all(transports, stop_bound).await
    }

    /// Serve every transport under the app's stop token until each has
    /// returned, or, once the stop is asked, until `stop_bound` and the settle
    /// have passed: what still serves then is abandoned.
    async fn serve_all(
        &self,
        transports: Vec<(Arc<str>, Box<dyn Transport>)>,
        stop_bound: Duration,
    ) -> Result<()> {
        let mut join = JoinSet::new();
        let mut serving = Serving::new();
        for (name, transport) in transports {
            let bound = transport.stop_bound();
            let stop = self.lifecycle.serving();
            let task = join.spawn(crate::panic::contain(
                async move { transport.serve(stop).await },
            ));
            serving.insert(task.id(), (name, bound));
        }
        crate::way_down::transports(still_serving(&serving));

        let stopping = self.lifecycle.stopping();
        let held = async {
            stopping.cancelled().await;
            tokio::time::sleep(stop_bound + crate::SHUTDOWN_SETTLE_TIMEOUT).await;
        };
        let mut held = std::pin::pin!(held);
        let mut first_err: Option<anyhow::Error> = None;
        loop {
            let next = tokio::select! {
                biased;
                next = join.join_next_with_id() => next,
                () = &mut held => {
                    // Aborted, never awaited: a task blocking its thread would hold the
                    // way down past every bound.
                    join.abort_all();
                    first_err.get_or_insert(abandon(&serving));
                    break;
                }
            };
            let Some(res) = next else { break };
            let ended = match &res {
                Ok((task, _)) => *task,
                Err(join_err) => join_err.id(),
            };
            let removed = serving.remove(&ended);
            let transport = removed.as_ref().map_or("<unknown>", |(name, _)| &**name);
            crate::way_down::transports(still_serving(&serving));
            let failed = match res.map(|(_, served)| served) {
                Ok(Ok(Ok(()))) if stopping.is_cancelled() => None,
                Ok(Ok(Ok(()))) => {
                    tracing::error!(
                        target: crate::target::APP,
                        transport,
                        "transport stopped serving before the app was asked to stop",
                    );
                    Some(anyhow!(
                        "transport `{transport}` stopped serving before the app was asked to stop"
                    ))
                }
                Ok(Ok(Err(e))) => {
                    tracing::error!(
                        target: crate::target::APP,
                        transport,
                        error = %crate::error_message(&*e),
                        "transport failed; shutting down",
                    );
                    Some(e)
                }
                Ok(Err(payload)) => {
                    crate::contained_panic!(
                        target: crate::target::APP,
                        payload.as_ref(),
                        "transport task panicked; shutting down",
                        transport,
                    );
                    Some(anyhow!("transport `{transport}` panicked"))
                }
                // Cancelled with its runtime; a panic raised as it was dropped is the hook's.
                Err(join_err) => {
                    tracing::error!(
                        target: crate::target::APP,
                        transport,
                        error = %crate::error_message(&join_err),
                        "transport task ended before it returned; shutting down",
                    );
                    Some(anyhow!("transport `{transport}` ended before it returned"))
                }
            };
            if let Some(failed) = failed {
                self.lifecycle.drain(StopCause::TransportFailed);
                first_err.get_or_insert(failed);
            }
        }
        // When every transport ended on its own, no stop was asked yet: the
        // shutdown hooks must still find the app draining, or a loop one of
        // them joins on `stopping()` would hold it to the budget.
        self.lifecycle.drain(StopCause::TransportFailed);

        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// What a transport's boot step returned, or, when it panicked, the panic filed
/// once at `error` and the boot's error naming the transport.
fn unwound<T>(
    transport: &str,
    step: Option<&BootStep>,
    outcome: Result<Result<T>, Box<dyn Any + Send>>,
) -> Result<T> {
    outcome.unwrap_or_else(|payload| {
        crate::contained_panic!(
            target: crate::target::APP,
            payload.as_ref(),
            "transport panicked during the boot; the boot is aborted",
            transport,
            step = step.map(tracing::field::display),
        );
        Err(anyhow!("transport `{transport}` panicked"))
    })
}

/// Each transport still serving, by its task: its name and stop bound.
type Serving = HashMap<tokio::task::Id, (Arc<str>, Duration)>;

/// Name every transport still serving past the way down's bound at `error`,
/// and the first, by name, as the app's error.
fn abandon(serving: &Serving) -> anyhow::Error {
    let mut abandoned: Vec<&(Arc<str>, Duration)> = serving.values().collect();
    abandoned.sort_unstable();
    for (transport, bound) in &abandoned {
        tracing::error!(
            target: crate::target::APP,
            transport = &**transport,
            stop_bound_ms = millis(*bound),
            "transport did not stop within its declared bound: abandoned",
        );
    }
    crate::way_down::transports(Vec::new());
    let first = abandoned.first().map_or("<unknown>", |(name, _)| &**name);
    anyhow!("transport `{first}` did not stop within its declared bound")
}

/// Start the shutdown hooks' budget, which the shutdown phases, the closes and
/// the container's drop share, and record it for `#[nest_rs::main]`'s teardown.
fn hooks_budget() -> Instant {
    crate::way_down::hooks_deadline(crate::SHUTDOWN_HOOKS_TIMEOUT);
    Instant::now() + crate::SHUTDOWN_HOOKS_TIMEOUT
}

/// Drop `owned` — what holds the last of a container — on the blocking pool,
/// waiting for it until `deadline`: a provider whose `Drop` blocks is
/// abandoned at `warn` rather than holding the exit.
async fn drop_within<T: Send + 'static>(owned: T, deadline: Instant) {
    let dropping = tokio::task::spawn_blocking(move || drop(owned));
    // A `Drop` that panics is filed by the process hook as it unwinds.
    if tokio::time::timeout_at(deadline, dropping).await.is_err() {
        tracing::warn!(
            target: crate::target::APP,
            budget_ms = millis(crate::SHUTDOWN_HOOKS_TIMEOUT),
            "a provider was still dropping when the shutdown hooks' budget ran out: abandoned",
        );
    }
}

/// A new app's lifecycle, its boot begun: the process's signals now reach it.
fn booting() -> Arc<AppLifecycle> {
    let lifecycle = Arc::new(AppLifecycle::new());
    crate::way_down::booting(&lifecycle);
    lifecycle
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
///    [`provide_dyn`](AppBuilder::provide_dyn), then a test's overrides
///    ([`override_value`](AppBuilder::override_value)), which replace them.
/// 2. **Collect** — each module's [`collect`](crate::Module::collect) queues
///    the async factories its import tree owns. No provider is built yet.
/// 3. **Factories** — every queued factory is `await`ed; each sees the
///    container so far. A factory whose output type a seed or an override
///    already supplies is **skipped**, and a config so supplied passes the
///    checks a read one does first.
/// 4. **Register** — each module's [`register`](crate::Module::register) builds
///    its providers last, injecting seeds and factory outputs. A module that
///    cannot build what it must [`refuse`](ContainerBuilder::refuse)s, and the
///    boot fails with its error once the phase ends.
///
/// The container sealed, each wiring a module attached
/// ([`ContainerBuilder::provide_wiring`]) fills its registry, in registration
/// order, before the app is returned.
pub struct AppBuilder {
    builder: ContainerBuilder,
    modules: Vec<ModuleHooks>,
    overrides: Vec<Registrar>,
}

impl AppBuilder {
    fn new() -> Self {
        Self {
            builder: ContainerBuilder::booting(),
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

    /// Seed module-less metadata of type `M` ([`ContainerBuilder::provide_meta`]
    /// at the app root), for global builder extensions such as `use_guards_global`.
    pub fn provide_meta<M: Any + Send + Sync>(mut self, meta: M) -> Self {
        self.builder = self.builder.provide_meta(meta);
        self
    }

    /// Attach a wiring at the app root ([`ContainerBuilder::provide_wiring`]),
    /// for a global builder extension whose registry or check reads the
    /// assembled container; it runs before every module's.
    pub fn provide_wiring(
        mut self,
        name: &'static str,
        wire: fn(&Container) -> Result<()>,
    ) -> Self {
        self.builder = self.builder.provide_wiring(name, wire);
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

    /// Replace the provider of `T` with a test's double, before anything is
    /// built from it: every consumer — a singleton built at boot included —
    /// injects the double, and the provider it replaces is never built, so its
    /// own dependencies need not exist.
    ///
    /// Whatever would have registered `T` stands down for it: a seed, a
    /// factory — which never runs, so no connection opens and no variable is
    /// read for it — or a module's provider, of any scope. A request-scoped or
    /// transient `T` becomes one double serving every request. A provider bound
    /// `Foo as dyn Trait` is overridden whole through either key: overriding
    /// `Foo` leaves no `dyn Trait` and the converse, and a consumer injecting
    /// the key left absent fails the boot
    /// ([`OverriddenDependencyError`]). The
    /// double keeps the provider's module, so the access graph holds its
    /// consumers to the imports production holds them to.
    ///
    /// A type nothing in the app's graph would register fails the boot
    /// ([`UnclaimedOverrideError`]): the double would never be read. So does an
    /// edge host (`#[controller]` and its kin), which registers no value.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use nest_rs_core::{App, injectable, module};
    ///
    /// pub trait Mailer: Send + Sync {
    ///     fn send(&self, to: &str) -> bool;
    /// }
    ///
    /// #[injectable]
    /// #[derive(Default)]
    /// struct SmtpMailer;
    ///
    /// impl Mailer for SmtpMailer {
    ///     fn send(&self, _: &str) -> bool {
    ///         true
    ///     }
    /// }
    ///
    /// #[injectable]
    /// struct UsersService {
    ///     #[inject]
    ///     mailer: Arc<dyn Mailer>,
    /// }
    ///
    /// #[module(providers = [SmtpMailer as dyn Mailer, UsersService])]
    /// struct UsersModule;
    ///
    /// struct Refusing;
    ///
    /// impl Mailer for Refusing {
    ///     fn send(&self, _: &str) -> bool {
    ///         false
    ///     }
    /// }
    ///
    /// # #[nest_rs_core::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// let app = App::builder()
    ///     .module::<UsersModule>()
    ///     .override_dyn::<dyn Mailer>(Arc::new(Refusing))
    ///     .build()
    ///     .await?;
    /// let users = app.container().get::<UsersService>().expect("built at boot");
    /// assert!(!users.mailer.send("ada@example.com"));
    /// # Ok(())
    /// # }
    /// ```
    pub fn override_value<T: Any + Send + Sync>(self, value: T) -> Self {
        self.override_arc(Arc::new(value))
    }

    /// [`override_value`](Self::override_value) of the trait object
    /// `Arc<dyn Trait>`, for a provider its consumers inject through a trait —
    /// the double of a `Foo as dyn Trait` binding.
    pub fn override_dyn<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.overrides.push(Box::new(move |builder| {
            builder.install_override::<Arc<T>>(Arc::new(value))
        }));
        self
    }

    /// [`override_value`](Self::override_value) for a double the test already
    /// holds in an `Arc`, so it can read what the double recorded.
    pub fn override_arc<T: Any + Send + Sync>(mut self, value: Arc<T>) -> Self {
        self.overrides.push(Box::new(move |builder| {
            builder.install_override::<T>(value)
        }));
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

    /// Run the four phases, then the wiring step, and return the assembled
    /// [`App`]. Propagates the first factory error, named
    /// ([`FactoryFailedError`]), and the first wiring's
    /// ([`WiringFailedError`](crate::WiringFailedError)); a stop asked while a
    /// factory waits drops it there and returns
    /// [`BootInterruptedError`](crate::BootInterruptedError). A boot that
    /// fails closes what the factories already opened
    /// ([`ContainerBuilder::provide_close`]) before it returns.
    pub async fn build(self) -> Result<App> {
        #[cfg(feature = "logging")]
        crate::logging::init_fallback()?;
        let lifecycle = booting();
        let AppBuilder {
            mut builder,
            modules,
            overrides,
        } = self;
        let (container, installed) = match assemble(&mut builder, &lifecycle, &modules, overrides)
            .await
        {
            Ok(assembled) => assembled,
            Err(error) => {
                close_unserved(&lifecycle, &builder.snapshot(), &builder.install_record()).await;
                return Err(error);
            }
        };
        if let Err(error) = crate::wiring::wire(&container) {
            close_unserved(&lifecycle, &container, &installed).await;
            return Err(error.into());
        }
        Ok(App::assembled(container, lifecycle, installed))
    }
}

/// The four phases and the seal, on `slot`, returning the container with what
/// the app handed in: on an error, the slot keeps what registered so far, for
/// the boot to close what the factories opened.
async fn assemble(
    slot: &mut ContainerBuilder,
    lifecycle: &Arc<AppLifecycle>,
    modules: &[ModuleHooks],
    overrides: Vec<Registrar>,
) -> Result<(Container, InstallRecord)> {
    let mut builder = std::mem::take(slot).provide_arc(Arc::clone(lifecycle));
    // Before any module collects, so each path that would register an
    // overridden key finds it and claims it instead.
    for install in overrides {
        builder = install(builder);
    }
    let mut builder = builder.enter_phase(Phase::Collect);
    for hooks in modules {
        builder = (hooks.import)(builder.enter_root(hooks.name)).leave_import();
    }
    *slot = builder;
    check_contested_declarations(slot)?;
    let descriptors: Vec<&ModuleDescriptor> = inventory::iter::<ModuleDescriptor>().collect();
    let roots: Vec<(TypeId, &'static str)> = modules.iter().map(|h| (h.type_id, h.name)).collect();
    let ids: Vec<TypeId> = roots.iter().map(|(id, _)| *id).collect();
    let reached = reachable_descriptors(&descriptors, &ids);
    // Refused as soon as the resource exists, before a later factory's I/O
    // fails on something else first.
    check_budgets(slot, &reached, None)?;
    // A seed supplying a factory's output type skips it. The next to run is
    // the first whose `after` types are present or provided by nothing still
    // queued; only a cycle leaves nothing runnable.
    let mut pending = slot.take_factories();
    while !pending.is_empty() {
        let ready = pending.iter().position(|queued| {
            queued.after.iter().all(|dep| {
                slot.contains(*dep) || !pending.iter().any(|other| other.provides.contains(dep))
            })
        });
        let Some(index) = ready else {
            return Err(FactoryCycleError {
                type_names: pending.iter().map(|queued| queued.name).collect(),
            }
            .into());
        };
        let queued = pending.remove(index);
        let id = queued.id();
        let output: Registrar = if slot.contains(id) {
            // A seed or an override skips the factory: before any factory
            // reads it, it passes what the factory's own value would have.
            if let Some(check) = slot.seed_check(id) {
                check(&slot.snapshot())?;
            }
            Box::new(move |builder| builder.stand_down(id))
        } else {
            let type_name = queued.name;
            let factory = (queued.factory)(slot.snapshot());
            lifecycle
                .boot(&BootStep::Factory { type_name }, factory)
                .await?
                .map_err(|source| FactoryFailedError { type_name, source })?
        };
        *slot = std::mem::take(slot).install(
            queued.queued_by,
            std::iter::once(output).chain(queued.derives),
        );
        check_budgets(slot, &reached, Some(&queued.provides))?;
    }
    // An override a module's provider will claim keeps that module as its
    // owner: global, it would let a consumer outside the module's closure
    // boot in a test and fail in production.
    let mut global = slot.provider_ids();
    for key in slot.unclaimed_override_ids() {
        global.remove(&key);
    }
    // Seeded after register, yet global infrastructure for the access graph.
    global.insert(TypeId::of::<ReachableProviders>());
    global.insert(TypeId::of::<ProviderOrder>());
    global.insert(TypeId::of::<Composition>());
    let mut builder = std::mem::take(slot).enter_phase(Phase::Register);
    for hooks in modules {
        builder = (hooks.import)(builder.enter_root(hooks.name)).leave_import();
    }
    *slot = builder;
    check_register_phase(slot)?;
    check_unclaimed_overrides(slot)?;

    let sealed = seal(slot, &descriptors, &roots, &global)?;
    let installed = slot.install_record();
    Ok((sealed.provide(std::mem::take(slot)).freeze(), installed))
}

/// Close what a boot that never became an app opened, within the shutdown
/// hooks' budget: no hook ran, but a factory's pool is open. What was built
/// then drops by ownership, as the error returns.
async fn close_unserved(lifecycle: &AppLifecycle, opened: &Container, installed: &InstallRecord) {
    lifecycle.drain(StopCause::BootFailed);
    crate::close::close_all(opened, installed, hooks_budget()).await;
}

/// The seeds the seal hands the container: what the transports and the hook
/// runners read.
struct Sealed {
    reachable: ReachableProviders,
    order: ProviderOrder,
    composition: Composition,
    hook_order: HookOrder,
}

impl Sealed {
    fn provide(self, builder: ContainerBuilder) -> ContainerBuilder {
        builder
            .provide(self.reachable)
            .provide(self.order)
            .provide(self.composition)
            .provide(self.hook_order)
    }
}

/// The boot's last pass, shared by both paths: the access graph checked over
/// what registered, then a cycle among request-scoped and transient
/// providers, one among singletons across modules, every provider the register
/// phase never built or built before an optional dependency arrived, every
/// budget against the nets reaching it, and the seeds the transports and the
/// hook runners read.
fn seal(
    builder: &ContainerBuilder,
    descriptors: &[&ModuleDescriptor],
    roots: &[(TypeId, &'static str)],
    global: &HashSet<TypeId>,
) -> Result<Sealed> {
    let ids: Vec<TypeId> = roots.iter().map(|(id, _)| *id).collect();
    // A provider a test's double stands in for is never built: what it
    // injects is checked and walked by nothing.
    let overridden: HashSet<TypeId> = builder
        .overridden_providers()
        .iter()
        .map(|o| o.provider.0)
        .collect();
    validate_access_graph(
        descriptors,
        &ids,
        global,
        &builder.registered_ids(),
        &builder.scoped_or_transient_ids(),
        &overridden,
    )
    .map_err(AccessError::into_anyhow)?;
    let scopes = builder.dynamic_scopes();
    if let Some(cycle) = scoped_cycle(descriptors, &ids, &scopes, &overridden) {
        return Err(cycle.into());
    }
    // After the access graph, whose remedy names the owning module
    // (`.claude/decisions/boot-refusal-order.md`).
    check_singleton_cycles(builder, descriptors, &ids, &scopes, &overridden)?;
    let reached = reachable_descriptors(descriptors, &ids);
    check_overridden_dependencies(builder, &reached, &overridden)?;
    if let Some(unbuilt) = builder.unbuilt_refusal(&reached) {
        return Err(unbuilt.into());
    }
    if let Some(late) = builder.late_optional_refusal(&reached) {
        return Err(late.into());
    }
    check_budgets(builder, &reached, None)?;
    let order = provider_order(descriptors, &ids);
    let hook_order = HookOrder::new(dependency_order(descriptors, &ids, &overridden));
    Ok(Sealed {
        reachable: ReachableProviders(reachable_provider_ids(&order, global)),
        order: ProviderOrder::new(order),
        composition: Composition::from_descriptors(descriptors, roots, overridden),
        hook_order,
    })
}

fn still_serving(serving: &Serving) -> Vec<Arc<str>> {
    let mut names: Vec<Arc<str>> = serving.values().map(|(name, _)| Arc::clone(name)).collect();
    names.sort_unstable();
    names
}

/// `app` is public: its tier-2 items live here, reached only through the
/// crate's `__private`.
pub(crate) mod __private {
    use std::future::Future;

    use super::{App, AppBuilder};
    use crate::app_lifecycle::StopCause;
    use crate::exception_filter::{ExceptionFilterSpec, GlobalExceptionFilters};
    use crate::interceptor::{GlobalInterceptors, InterceptorSpec};
    use crate::layer_chain::__private::check_specs_resolvable;

    /// Refuse a transport whose stop the kernel cannot hold: past
    /// [`MAX_STOP_BOUND`](crate::MAX_STOP_BOUND), `Duration::MAX` among them — the
    /// check [`App::run`] makes at the boot, for a harness that runs transports
    /// itself.
    pub fn check_stop_bound(
        transport: &str,
        bound: std::time::Duration,
    ) -> Result<(), crate::UnboundedTransportError> {
        if bound <= crate::MAX_STOP_BOUND {
            return Ok(());
        }
        Err(crate::UnboundedTransportError {
            transport: transport.to_owned(),
            stop_bound_ms: crate::millis::millis(bound),
        })
    }

    /// [`App::close`], once `stopped` resolves: the app turns `Draining` first,
    /// so what a harness serves under the app's stop token winds down before
    /// the shutdown hooks run, as it does under [`App::run`].
    pub async fn close_after(app: App, stopped: impl Future<Output = ()>) {
        app.lifecycle.drain(StopCause::Closed);
        stopped.await;
        app.wind_down().await;
    }

    /// [`App::init`], an init that fails awaiting `stopped` once the app
    /// turned `Draining`, before its teardown: what a harness serves under the
    /// app's stop token winds down before the shutdown hooks run, as no
    /// transport serves before [`App::run`]'s init.
    pub async fn init_tearing_down_after(
        app: &App,
        stopped: impl Future<Output = ()>,
    ) -> anyhow::Result<()> {
        app.init_tearing_down_after(stopped).await
    }

    /// List `specs` as the interceptors every unit runs at the handler stage,
    /// first listed outermost; one no imported module provides fails the boot
    /// at the wiring step, in every app.
    pub fn use_interceptors_global(
        builder: AppBuilder,
        specs: impl IntoIterator<Item = InterceptorSpec>,
    ) -> AppBuilder {
        let specs = specs.into_iter().map(|spec| spec.0).collect();
        builder.provide(GlobalInterceptors(specs)).provide_wiring(
            "nest_rs::layers::interceptors",
            |container| {
                check_specs_resolvable::<GlobalInterceptors>(
                    container,
                    "interceptor",
                    "an unresolvable global interceptor would silently drop",
                )
                .map_err(anyhow::Error::msg)
            },
        )
    }

    /// List `specs` as the exception filters every unit tries after its
    /// method's and its host's; one no imported module provides fails the boot
    /// at the wiring step, in every app.
    pub fn use_exception_filters_global(
        builder: AppBuilder,
        specs: impl IntoIterator<Item = ExceptionFilterSpec>,
    ) -> AppBuilder {
        let specs = specs.into_iter().map(|spec| spec.0).collect();
        builder
            .provide(GlobalExceptionFilters(specs))
            .provide_wiring("nest_rs::layers::exception_filters", |container| {
                check_specs_resolvable::<GlobalExceptionFilters>(
                    container,
                    "exception filter",
                    "an unresolvable global exception filter would silently drop",
                )
                .map_err(anyhow::Error::msg)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{ContestedDeclarationError, LateFactoryError, UnregisteredModuleError};
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
            builder.provide_declared_factory_after::<Second, (First,), _, _>(
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
            builder.provide_declared_factory_after::<Third, (First, Second), _, _>(
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

    /// Every order of `0..n`.
    fn every_order(n: usize) -> Vec<Vec<usize>> {
        let Some(last) = n.checked_sub(1) else {
            return vec![Vec::new()];
        };
        let mut orders = Vec::new();
        for order in every_order(last) {
            for at in 0..=order.len() {
                let mut next = order.clone();
                next.insert(at, last);
                orders.push(next);
            }
        }
        orders
    }

    /// A builder importing `modules` in each of their orders, beside that order
    /// as indices into `modules`, for a boot that must not depend on
    /// `imports = [..]`.
    fn in_every_order(
        modules: &[fn(AppBuilder) -> AppBuilder],
    ) -> impl Iterator<Item = (Vec<usize>, AppBuilder)> + '_ {
        every_order(modules.len()).into_iter().map(|order| {
            let app = order
                .iter()
                .fold(App::builder(), |app, &at| modules[at](app));
            (order, app)
        })
    }

    #[tokio::test]
    async fn a_factory_declared_after_two_others_runs_after_both_whatever_the_queue_order() {
        for (order, app) in in_every_order(&[
            AppBuilder::module::<ThirdAfterBoth>,
            AppBuilder::module::<FirstModule>,
            AppBuilder::module::<SecondModule>,
        ]) {
            let app = app.build().await.expect("the declared order is honoured");
            assert_eq!(app.container().get::<Third>().unwrap().0, 42, "{order:?}");
        }
    }

    struct ThirdModule;
    impl Module for ThirdModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder.provide_factory(|_| async { Ok(Third(100)) })
        }
    }

    struct Fourth(u32);
    struct FourthAfterThree;
    impl Module for FourthAfterThree {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder.provide_factory_after::<Fourth, (First, Second, Third), _, _>(|c| async move {
                let read = |value: Option<u32>| value.ok_or_else(|| anyhow!("an input ran late"));
                Ok(Fourth(
                    read(c.get::<First>().map(|v| v.0))?
                        + read(c.get::<Second>().map(|v| v.0))?
                        + read(c.get::<Third>().map(|v| v.0))?,
                ))
            })
        }
    }

    #[tokio::test]
    async fn a_factory_after_three_others_runs_after_all_three_in_every_order() {
        for (order, app) in in_every_order(&[
            AppBuilder::module::<FourthAfterThree>,
            AppBuilder::module::<FirstModule>,
            AppBuilder::module::<SecondModule>,
            AppBuilder::module::<ThirdModule>,
        ]) {
            let app = app
                .build()
                .await
                .unwrap_or_else(|e| panic!("{order:?}: {e:#}"));
            assert_eq!(app.container().get::<Fourth>().unwrap().0, 142, "{order:?}");
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
            builder.provide_factory_after::<ReadsPort, (Arc<dyn Port>,), _, _>(|c| async move {
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

    #[derive(Clone)]
    struct SummedPort(u32);
    impl Port for SummedPort {
        fn value(&self) -> u32 {
            self.0
        }
    }
    struct BindsSummedPortModule;
    impl Module for BindsSummedPortModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder.provide_factory_dyn_after::<SummedPort, dyn Port, (First, Second), _, _>(
                |c| async move {
                    let first = c
                        .get::<First>()
                        .ok_or_else(|| anyhow!("First runs first"))?;
                    let second = c
                        .get::<Second>()
                        .ok_or_else(|| anyhow!("Second runs first"))?;
                    Ok(SummedPort(first.0 + second.0))
                },
                |port| Arc::new(port) as Arc<dyn Port>,
            )
        }
    }

    #[tokio::test]
    async fn a_dyn_binding_after_two_factories_runs_after_both_in_every_order() {
        for (order, app) in in_every_order(&[
            AppBuilder::module::<BindsSummedPortModule>,
            AppBuilder::module::<FirstModule>,
            AppBuilder::module::<SecondModule>,
        ]) {
            let app = app
                .build()
                .await
                .unwrap_or_else(|e| panic!("{order:?}: {e:#}"));
            assert_eq!(
                app.container().get_dyn::<dyn Port>().map(|p| p.value()),
                Some(42),
                "{order:?}"
            );
        }
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
        for (order, app) in in_every_order(&[
            AppBuilder::module::<DefaultPortModule>,
            AppBuilder::module::<DeclaresPortImplModule>,
            AppBuilder::module::<BindsPortModule>,
        ]) {
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
        assert!(format!("{err:#}").contains("import FirstModule"), "{err:#}");
    }

    struct FirstAfterSecond;
    impl Module for FirstAfterSecond {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder.provide_declared_factory_after::<First, (Second,), _, _>(
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
        assert!(format!("{err:#}").contains("connection refused"), "{err:#}");
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

    // The shape of every `for_root`: a `register` reading what its own
    // `collect` queued, here first imported in an importer's `register`.
    struct ReadsLateModule;
    impl Module for ReadsLateModule {
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder.provide_factory(|_| async { Ok(Late(1)) })
        }
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            let late = builder
                .get::<Late>()
                .expect("a register runs once the factory its collect queued ran");
            builder.provide(Doubled(late.0 * 2))
        }
    }
    struct ImportsReadsLateInRegister;
    impl Module for ImportsReadsLateInRegister {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder.import::<ReadsLateModule>()
        }
    }

    #[tokio::test]
    async fn a_module_first_imported_in_a_register_is_refused_before_it_registers() {
        let Err(built) = App::builder()
            .module::<ImportsReadsLateInRegister>()
            .build()
            .await
        else {
            panic!("nothing drains the factory its register reads");
        };
        let Err(synchronous) = App::new::<ImportsReadsLateInRegister>() else {
            panic!("nothing drains a factory on the synchronous path");
        };
        for refused in [built, synchronous] {
            let late = refused
                .downcast_ref::<LateFactoryError>()
                .unwrap_or_else(|| panic!("not the late-factory refusal: {refused:#}"));
            assert_eq!(late.type_name, std::any::type_name::<Late>());
        }
    }

    struct ProvidesLateModule;
    impl Module for ProvidesLateModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder.provide(Late(2))
        }
    }
    struct ImportsReadsLateThenProvidesIt;
    impl Module for ImportsReadsLateThenProvidesIt {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
                .import::<ReadsLateModule>()
                .import::<ProvidesLateModule>()
        }
    }

    #[tokio::test]
    async fn a_module_refused_as_late_stays_refused_once_a_later_register_provides_the_type() {
        let Err(refused) = App::builder()
            .module::<ImportsReadsLateThenProvidesIt>()
            .build()
            .await
        else {
            panic!("the module whose register never ran would be dropped in silence");
        };
        let late = refused
            .downcast_ref::<LateFactoryError>()
            .unwrap_or_else(|| panic!("not the late-factory refusal: {refused:#}"));
        assert_eq!(late.type_name, std::any::type_name::<Late>());
    }

    #[tokio::test]
    async fn a_late_collects_factory_for_a_seeded_type_is_passed_over_and_its_module_registers() {
        let app = App::builder()
            .provide(Late(21))
            .module::<ImportsReadsLateInRegister>()
            .build()
            .await
            .expect("the seed is what the late factory would build");
        assert_eq!(app.container().get::<Doubled>().map(|d| d.0), Some(42));
    }

    struct ImportsRegistersFirstInRegister;
    impl Module for ImportsRegistersFirstInRegister {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder.import::<RegistersFirstModule>()
        }
    }

    #[tokio::test]
    async fn a_module_first_imported_in_a_register_registers_while_another_modules_factory_waits() {
        let app = App::builder()
            .module::<QueuesInRegisterModule>()
            .module::<ImportsRegistersFirstInRegister>()
            .module::<ProvidesLateModule>()
            .build()
            .await
            .unwrap_or_else(|e| {
                panic!("a later register provides what the factory waits for: {e:#}")
            });
        assert_eq!(app.container().get::<First>().map(|f| f.0), Some(1));
        assert_eq!(app.container().get::<Late>().map(|late| late.0), Some(2));
    }

    // `PinnedSetup`'s declaration of `Config` made again in a late `collect`,
    // beside a default nothing drains.
    struct RepinsConfigBesideALateFactory;
    impl Module for RepinsConfigBesideALateFactory {
        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder
                .provide_declared_factory("one declaration", |_| async { Ok(Config(8)) })
                .provide_factory(|_| async { Ok(Late(1)) })
        }
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
    }
    struct ImportsRepinInRegister;
    impl Module for ImportsRepinInRegister {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder.import::<RepinsConfigBesideALateFactory>()
        }
    }

    #[tokio::test]
    async fn a_declaration_contested_in_a_register_is_named_ahead_of_every_late_factory() {
        let one_collect = App::builder()
            .module::<PinnedSetup>()
            .module::<ImportsRepinInRegister>();
        let an_earlier_import = App::builder()
            .module::<PinnedSetup>()
            .module::<ImportsReadsLateInRegister>()
            .module::<ImportsRepinInRegister>();
        for (case, app) in [
            ("its own collect", one_collect),
            ("an earlier import", an_earlier_import),
        ] {
            let Err(refused) = app.build().await else {
                panic!("{case}: one declaration would be dropped for the other");
            };
            let contested = refused
                .downcast_ref::<ContestedDeclarationError>()
                .unwrap_or_else(|| panic!("{case}: not the contest: {refused:#}"));
            assert_eq!(
                contested.type_name,
                std::any::type_name::<Config>(),
                "{case}"
            );
        }
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

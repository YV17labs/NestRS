//! The nestrs core: the IoC container, the module system, the boot-time access
//! graph, and the application lifecycle. Every other `nest-rs-*` crate composes
//! on these primitives; an app rarely calls into this crate beyond
//! [`App::builder`] in `main`.
//!
//! # The composition model
//!
//! An [`App`] is built from a root [`Module`]. A `#[module]` declares the
//! `providers` it owns and the `imports` it depends on; a *provider* is any
//! injectable value — a service, a controller, a guard — that names its
//! dependencies with `#[inject]`. The container is a single flat registry keyed
//! by `TypeId`; to hide an implementation, expose a `pub trait`, bind it with
//! `provide_dyn`, and inject `Arc<dyn Trait>`.
//!
//! # Wiring is checked at boot
//!
//! The boot walks the module tree from the root and fails with a named error
//! before any transport starts: [`AccessGraphError`] when a provider reaches
//! across a module boundary no `import` covers, [`MissingDependencyError`] when
//! no module provides a dependency, [`BudgetPastNetError`] when a resource's
//! [`Budget`] reaches past a [`Net`] that waits on it. `#[use_guards]` /
//! `#[use_filters]` / `#[use_interceptors]` are checked the same way.
//!
//! # Scopes and lifecycle
//!
//! Providers are singletons by default. `#[injectable(scope = request)]` builds
//! one instance per request, reached through a transport's request boundary;
//! `#[injectable(scope = transient)]` rebuilds on every resolution. Lifecycle
//! hooks (`#[on_module_init]`, `#[on_application_bootstrap]`,
//! `#[on_module_destroy]`, …) run per phase as [`App::run`] drains them —
//! init runs once and its failure aborts boot, shutdown is best-effort. A
//! registry a module fills from the assembled container is filled before the
//! first hook, by the wiring step it attaches with
//! [`ContainerBuilder::provide_wiring`]. Where the app stands and why it stops
//! are [`AppLifecycle`]'s, which any provider injects and only the kernel
//! moves.
//!
//! A hook host — like a scheduled method, a listener, an indicator and a
//! processor — is resolved outside any request, so it must be a **singleton
//! stored under its own type** ([`ProviderResidency`]). `Foo as dyn Trait`
//! binds a singleton only, and holds that one `Foo` under its own type and
//! each trait, so a bound host runs too; one the boot finds absent carries
//! [`INERT_HOST_HINT`].
//!
//! # One pipeline
//!
//! [`Interceptor`] and [`ExceptionFilter`] name no edge. An edge that
//! dispatches through this crate runs each unit through one staged pipeline,
//! written and tested once: the interceptors a module brings at the unit stage
//! ([`macro@interceptor`]), the guards and the posture's class gate, the
//! exception filters, the handler stage's interceptors, then the handler. A
//! failure stays a failure whatever a layer answers, so the unit's data rolls
//! back.
//!
//! # Discovery
//!
//! Module-wired items implement [`Discoverable`] and are found through link-time
//! `inventory`, gated on reachability from the running app's root
//! ([`ReachableProviders`]). An item linked into the binary but living in no
//! reachable module is inert, with a boot `warn`.
//!
//! ```
//! use nest_rs_core::{App, injectable, module};
//!
//! #[injectable]
//! #[derive(Default)]
//! struct GreetingService;
//!
//! #[module(providers = [GreetingService])]
//! struct AppModule;
//!
//! // Boot fails here with a named error if the graph is misconfigured.
//! let app = App::new::<AppModule>()?;
//! assert!(app.container().get::<GreetingService>().is_some());
//! # Ok::<(), anyhow::Error>(())
//! ```
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

pub mod access;
mod ambient;
mod answer;
pub mod app;
mod app_lifecycle;
mod backoff;
mod budget;
mod close;
pub mod container;
pub(crate) mod cycle_guard;
pub mod discoverable;
pub mod discovery;
mod dispatch;
pub mod env_flag;
pub mod env_prefix;
pub mod error;
pub mod error_message;
mod exception_filter;
mod factory_inputs;
mod handler;
mod identifier;
mod instance;
mod interceptor;
mod job;
pub mod layer;
pub mod layer_chain;
pub mod lifecycle;
mod line_safe;
#[cfg(feature = "logging")]
pub mod logging;
mod millis;
pub mod module;
mod net;
mod opaque;
pub mod operation_log;
mod outcome;
pub mod panic;
mod param;
pub mod problem;
mod process_start;
mod proof;
pub mod request_scope;
mod site;
mod stderr_lines;
pub mod target;
#[cfg(test)]
mod testing;
pub mod trace_context;
pub mod transport;
mod type_name;
mod unit_context;
mod way_down;
mod wiring;

pub use access::{Composition, ProviderOrder, ReachableProviders};
pub use ambient::{AmbientValue, Continuation, Deadline, ambient, with_ambient};
pub use app::{App, AppBuilder, BootStep};
pub use app_lifecycle::{AppLifecycle, AppState, StopCause};
pub use backoff::{Backoff, Retries, process_seed};
pub use budget::Budget;
pub use close::CloseContribution;
pub use container::{Container, ContainerBuilder, ContainerId, ProviderScope};
pub use discoverable::{
    Discoverable, INERT_HOST_HINT, InertHost, ProviderResidency, inert_host, unresolved_host,
};
pub use discovery::{Discovered, Discovery};
pub use env_flag::parse_bool;
pub use env_prefix::EnvPrefix;
pub use error::{
    AccessGraphError, BootInterruptedError, BudgetPastNetError, ContestedDeclarationError,
    DecodeError, DuplicateProviderError, FactoryCycleError, FactoryFailedError, InitFailedError,
    LateFactoryError, LateOptionalDependencyError, MissingDependencyError,
    OverriddenDependencyError, ProviderCycleError, ScopeViolationError, UnboundedTransportError,
    UnbuiltCause, UnbuiltProviderError, UnclaimedOverrideError, UnregisteredModuleError,
    UnresolvedFactoryError, UnresolvedLayerError, WiringFailedError,
};
pub use error_message::{SafeLink, boxed_error, error_message};
pub use exception_filter::{Catch, Caught, ExceptionFilter, ExceptionFilterSpec, exception_filter};
pub use factory_inputs::FactoryInputs;
pub use handler::{Handler, Posture, Reflector};
pub use identifier::UUID_V7_REQUIRED;
pub use instance::{Instance, Instanced, Unnamed};
pub use interceptor::{Interceptor, InterceptorSpec, Next, interceptor};
pub use job::{JOB_TIMEOUT, JobTimedOut};
pub use layer::{Layer, LayerKind, LayerSite};
pub use layer_chain::LayerSpec;
pub use lifecycle::{LifecyclePhase, SHUTDOWN_HOOKS_TIMEOUT, SHUTDOWN_SETTLE_TIMEOUT};
pub use module::{Collecting, DynamicModule, Module, Registering};
pub use net::{Net, Unanswered, within};
pub use opaque::{OPAQUE_CLIENT_MESSAGE, UNAVAILABLE_CLIENT_MESSAGE};
pub use operation_log::Edge;
pub use outcome::{Failure, Outcome, Settled};
pub use panic::panic_message;
pub use param::ResolvesProvider;
pub use problem::{Code, Problem, ToProblem};
pub use proof::{Attempt, Proving, Unproved, prove};
pub use request_scope::{
    RequestContinuation, RequestScope, TaskContext, current_request_scope, with_request_scope,
};
pub use trace_context::{
    Correlation, SpanId, TraceFlags, TraceId, TraceParent, TraceState, current_actor_id,
    current_span_id, current_trace_id, current_traceparent, current_tracestate,
};
pub use transport::{Listening, MAX_STOP_BOUND, Transport};
pub use unit_context::{Peer, UnitContext, UnitView};
pub use wiring::WiringContribution;

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::access::__private::{ModuleDescriptor, ProviderDescriptor};
    pub use crate::ambient::{
        Pending, UnmaskedReply, carries_ambient, install_pending, with_ambient_from,
    };
    pub use crate::answer::{Answer, AnswerFallback, ResultAnswer, ValueAnswer};
    pub use crate::app::__private::{
        check_stop_bound, close_after, init_tearing_down_after, use_exception_filters_global,
        use_interceptors_global,
    };
    pub use crate::container::__private::{
        StalledProvider, check_seed, claim_override, claim_own_override, collect_dynamic_import,
        enter_import, leave_import, overrides_any, record_absent_optional, record_stall,
        refuse_site, register_dynamic_import, site_chains, take_site_refusal,
    };
    pub use crate::dispatch::{ClassGate, GuardStage, GuardStageFactory, Tail, dispatch};
    pub use crate::error_message::__private::find_in_chain;
    pub use crate::exception_filter::__private::ErasedExceptionFilter;
    pub use crate::handler::{HandlerDeclaration, MetaLevels, declare_handler, meta_value};
    pub use crate::instance::{InstanceDeclaration, InstanceSeal};
    pub use crate::interceptor::__private::{
        InterceptorRegistration, InterceptorStage, register_interceptor,
    };
    pub use crate::layer_chain::__private::{
        ResolvedLayer, SiteChains, check_specs_resolvable, compose_chain, dedup_bucket,
        log_effective_chain, refuse_unresolved, report_unresolved, resolve_global_layers,
        resolve_scoped,
    };
    pub use crate::lifecycle::__private::LifecycleHook;
    #[cfg(feature = "logging")]
    pub use crate::logging::__private::{
        ConsoleWriter, JsonFormat, TextFormat, install_global_subscriber,
    };
    pub use crate::module::__private::{dynamic_import_module, module_registered};
    pub use crate::opaque::__private::OpaqueCause;
    pub use crate::operation_log::__private::{declare_unit, unit_opened_by};
    pub use crate::outcome::__private::{caught_problem, settled};
    pub use crate::panic::__private::{Unwound, unwound};
    pub use crate::param::{ParamProbe, ParamProbeFallback, ParamProbeResolves};
    pub use crate::problem::__private::{Withheld, withheld};
    pub use crate::process_start::ProcessStart;
    pub use crate::site::{
        GlobalLayers, Shape, Site, SiteDeclaration, SiteLayers, UnitChains, declare_site,
    };
    pub use crate::trace_context::__private::{
        current_correlation, hex, link_span, pending_ids, set_actor_id, set_sampled,
        set_span_linker, with_pending_ids,
    };
    pub use crate::transport::__private::TransportContribution;
    pub use crate::type_name::short_type_name;
    pub use crate::unit_context::{new_unit_context, take_pending};
    pub use crate::way_down::main;

    pub use schemars;
    pub use serde;
    pub use validator;
}

// Macro output resolves these through the framework, so an app needs no
// direct dependency on them.
pub use anyhow;
pub use inventory;
pub use tracing;

pub use async_trait::async_trait;

/// Declare application lifecycle hooks on a provider's impl block, for the
/// module/application init and shutdown phases.
///
/// ```
/// use std::sync::atomic::{AtomicBool, Ordering};
/// use nest_rs_core::{App, hooks, injectable, module};
///
/// #[injectable]
/// #[derive(Default)]
/// struct CacheWarmer {
///     warm: AtomicBool,
/// }
///
/// #[hooks]
/// impl CacheWarmer {
///     #[on_module_init]
///     async fn warm_up(&self) {
///         self.warm.store(true, Ordering::SeqCst);
///     }
/// }
///
/// #[module(providers = [CacheWarmer])]
/// struct AppModule;
///
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// let app = App::builder().module::<AppModule>().build().await?;
/// app.init().await?;
/// let warmer = app.container().get::<CacheWarmer>();
/// assert!(warmer.is_some_and(|w| w.warm.load(Ordering::SeqCst)));
/// # Ok(())
/// # }
/// ```
pub use nest_rs_core_macros::hooks;

/// Run an app's `async fn main` on the runtime the framework owns, and end the
/// process within the shutdown budget.
///
/// Before the runtime exists it installs the process panic hook: a panic no
/// unit of work contains is one `error` on `nest_rs::app`, its payload
/// redacted by [`panic_message`], never Rust's default line on stderr.
///
/// Before `main` runs it installs the `SIGINT` / `SIGTERM` handlers, so the
/// first signal stops the boot at its next await or drains the serving app,
/// and one received before any app exists exits at once with 130 / 143.
///
/// ```
/// # use nest_rs_core::{App, module};
/// # #[module]
/// # struct AppModule;
/// #[nest_rs_core::main]
/// async fn main() -> anyhow::Result<()> {
///     App::builder().module::<AppModule>().build().await?.run().await
/// }
/// ```
///
/// The function keeps its name and becomes a synchronous one returning what
/// its body returns:
///
/// ```
/// #[nest_rs_core::main]
/// async fn answer() -> u8 {
///     42
/// }
///
/// assert_eq!(answer(), 42);
/// ```
pub use nest_rs_core_macros::main;

/// `#[module(imports = [...], providers = [...])]` — a module of the app's
/// tree, implementing [`Module`].
///
/// ```
/// use nest_rs_core::{App, Module, injectable, module};
///
/// trait Greeter: Send + Sync {
///     fn greet(&self) -> &'static str;
/// }
///
/// #[injectable]
/// #[derive(Default)]
/// struct English;
///
/// impl Greeter for English {
///     fn greet(&self) -> &'static str {
///         "hello"
///     }
/// }
///
/// #[module(providers = [English as dyn Greeter])]
/// struct GreetingModule;
///
/// #[module(imports = [GreetingModule])]
/// struct AppModule;
///
/// fn implements<T: Module>() {}
/// implements::<AppModule>();
///
/// let app = App::new::<AppModule>()?;
/// let greeter = app.container().get_dyn::<dyn Greeter>();
/// assert_eq!(greeter.map(|g| g.greet()), Some("hello"));
/// assert!(app.container().get::<English>().is_some());
/// # Ok::<(), anyhow::Error>(())
/// ```
pub use nest_rs_core_macros::module;

/// Mark a struct as a DI provider built from the container.
///
/// ```
/// use std::sync::Arc;
/// use nest_rs_core::{App, Discoverable, ProviderResidency, injectable, module};
///
/// #[injectable]
/// #[derive(Default)]
/// struct Clock;
///
/// #[injectable]
/// struct Greeter {
///     #[inject]
///     clock: Arc<Clock>,
/// }
///
/// #[injectable(scope = transient)]
/// #[derive(Default)]
/// struct Draft;
///
/// #[module(providers = [Clock, Greeter, Draft])]
/// struct AppModule;
///
/// fn implements<T: Discoverable>() {}
/// implements::<Greeter>();
/// assert!(<Greeter as ProviderResidency>::SINGLETON);
/// assert!(!<Draft as ProviderResidency>::SINGLETON);
///
/// let app = App::new::<AppModule>()?;
/// assert!(app.container().get::<Greeter>().is_some());
/// # Ok::<(), anyhow::Error>(())
/// ```
///
/// Every `#[inject]` field must be an `Arc<T>` or `Arc<dyn Trait>`; anything
/// else is a compile error:
///
/// ```compile_fail
/// use nest_rs_core::injectable;
///
/// #[injectable]
/// struct Bad {
///     #[inject]
///     dep: u32, // not an `Arc` — compile error
/// }
/// ```
pub use nest_rs_core_macros::injectable;

/// Declare a unit struct as an [`Instance`] marker, named by its ident in snake
/// case — read, never chosen.
///
/// ```
/// use nest_rs_core::{Instance, Instanced, instance};
///
/// #[instance]
/// pub struct ReadReplica;
///
/// assert_eq!(<ReadReplica as Instance>::NAME, "read_replica");
/// let held: <ReadReplica as Instance>::Of<u16> = 5432.into();
/// let _: Instanced<ReadReplica, u16> = held;
/// ```
///
/// It takes no argument, and refuses a struct with fields or generics, the
/// default's own name `Unnamed`, a name holding `__` or a word the layout
/// reserves (`queue`, `http`, `service`, …):
///
/// ```compile_fail
/// #[nest_rs_core::instance]
/// pub struct Queue;
/// ```
pub use nest_rs_core_macros::instance;

/// The wire-DTO shorthand: carries `Serialize`/`Deserialize`/`Validate`/
/// `JsonSchema` and routes each back here, so a DTO crossing any transport
/// declares none of them.
pub use nest_rs_core_macros::input;

/// Declare a struct as an interceptor its module brings for every unit it
/// serves — `#[interceptor]` after the guards and the class gate, or
/// `#[interceptor(stage = unit)]` around them.
///
/// The struct is a singleton provider, built like an `#[injectable]` one, and
/// only an imported module's reaches a unit. A type the app also lists runs
/// once; its order within its stage is its [`Layer::priority`], then module
/// import order.
///
/// ```
/// use std::sync::Arc;
/// use std::sync::atomic::{AtomicUsize, Ordering};
/// use nest_rs_core::{
///     App, Interceptor, Layer, Next, Outcome, UnitContext, async_trait, injectable, interceptor,
///     module,
/// };
///
/// #[injectable]
/// #[derive(Default)]
/// struct Metrics {
///     units: AtomicUsize,
/// }
///
/// #[interceptor]
/// struct CountUnits {
///     #[inject]
///     metrics: Arc<Metrics>,
/// }
///
/// impl Layer for CountUnits {}
///
/// #[async_trait]
/// impl Interceptor for CountUnits {
///     async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
///         self.metrics.units.fetch_add(1, Ordering::Relaxed);
///         next.run(cx).await
///     }
/// }
///
/// #[module(providers = [Metrics, CountUnits])]
/// struct MetricsModule;
/// # use std::any::TypeId;
/// # use nest_rs_core::__private::{
/// #     GlobalLayers, HandlerDeclaration, MetaLevels, Site, SiteDeclaration, SiteLayers, Tail,
/// #     declare_handler, declare_site, declare_unit, dispatch, new_unit_context, settled,
/// # };
/// # use nest_rs_core::{Handler, Posture, UnitView, operation_log::Kind};
/// # struct Jobs;
/// # struct JobView;
/// # impl UnitView for JobView {}
/// # struct Run;
/// # impl Tail for Run {
/// #     async fn call(&mut self, _: &mut UnitContext<'_>) -> Outcome {
/// #         Ok(settled())
/// #     }
/// # }
/// # static RUN: Handler = declare_handler(HandlerDeclaration {
/// #     unit: declare_unit("nest-rs-queue", "nest_rs::queue", Kind::Consumer, "queue.job"),
/// #     host: TypeId::of::<Jobs>(),
/// #     host_name: "Jobs",
/// #     method: "run",
/// #     address: "jobs",
/// #     posture: Posture::System,
/// #     meta: MetaLevels::default,
/// # });
/// # static SITE: Site = declare_site(&RUN, SiteDeclaration {
/// #     layers: SiteLayers::default,
/// #     global: GlobalLayers::ALL,
/// #     guard_stage: None,
/// #     class_gate: None,
/// # });
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// let app = App::builder().module::<MetricsModule>().build().await?;
/// # let chains = SITE.chains(app.container())?;
/// # let mut view = JobView;
/// # let mut cx = new_unit_context(&RUN, &mut view, None, None);
/// # assert!(dispatch(&mut cx, &chains, &mut Run).await.is_ok());
/// let metrics = app.container().get::<Metrics>().expect("provided");
/// assert_eq!(metrics.units.load(Ordering::Relaxed), 1);
/// # Ok(())
/// # }
/// ```
pub use nest_rs_core_macros::interceptor;

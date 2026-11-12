//! The nestrs core: the IoC container, the module system, the boot-time access
//! graph, the application lifecycle, and the one pipeline every unit of work
//! runs through — its [`Interceptor`] and [`ExceptionFilter`] layers included.
//! Every other `nest-rs-*` crate composes on these primitives; an app rarely
//! calls into this crate beyond [`App::builder`] in `main`.
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
//! no module provides a dependency — each naming the line to add when the crate
//! owning its type submits a [`DependencyRemedy`], or the type carries one
//! ([`CarriesRemedy`]) — [`BudgetPastNetError`] when a resource's
//! [`Budget`] reaches past a [`Net`] that waits on it. The layers a site
//! binds — `#[use_guards]`, `#[use_pipes]`, `#[use_interceptors]`,
//! `#[use_exception_filters]` — are checked the same way.
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
//! # Transports
//!
//! What serves the app — the HTTP server, the queue worker, the scheduler — is
//! a [`Transport`] a module attaches with
//! [`ContainerBuilder::provide_transport`], and [`App::run`] builds,
//! configures, listens and serves each one; an app runs one transport of each
//! type. A third-party transport is supported, and it adds no edge: the edges
//! are the framework's. So it serves infrastructure no caller identifies (a
//! metrics port), carries system work into an edge that exists (an event
//! published, a job pushed) under the actor [`Correlation::continued`]
//! carries, or, when it is request-shaped and must authenticate, rides HTTP as
//! a self-mount, whose guards decide. Transport code makes no authentication
//! or authorization decision of its own.
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
//! # #[nest_rs_core::main]
//! # async fn main() -> anyhow::Result<()> {
//! let app = App::builder().module::<AppModule>().build().await?;
//! assert!(app.container().get::<GreetingService>().is_some());
//! # Ok(())
//! # }
//! ```
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

// The kernel's unit tests write the framework's derives, which name `::nest_rs_core`.
#[cfg(test)]
extern crate self as nest_rs_core;

mod access;
mod ambient;
mod answer;
mod app;
mod app_dir;
mod app_identity;
mod app_lifecycle;
mod audience;
mod backoff;
mod budget;
pub mod client_operation;
mod close;
mod container;
pub(crate) mod cycle_guard;
mod decode_path;
mod denial_document;
mod discoverable;
mod discovery;
mod dispatch;
mod edge;
mod env_flag;
mod env_prefix;
mod error;
mod error_message;
mod exception_filter;
#[cfg(feature = "logging")]
mod export;
mod factory_inputs;
mod handler;
mod host_runtime;
mod identifier;
mod injected_factory;
mod injects;
mod instance;
mod interceptor;
mod job;
mod layer;
mod layer_chain;
mod lifecycle;
mod line_safe;
#[cfg(feature = "logging")]
pub mod logging;
mod millis;
mod module;
mod net;
mod opaque;
pub mod operation_log;
mod outcome;
pub mod panic;
mod param;
mod probe_runtime;
mod problem;
mod process_start;
mod proof;
mod remedy;
mod request_scope;
mod row_probe;
mod scope;
mod secret;
mod served_address;
mod site;
mod stderr_lines;
pub mod target;
mod tenant;
#[cfg(test)]
mod testing;
mod trace_context;
mod transport;
mod type_name;
pub mod unit;
mod unit_context;
mod way_down;
mod wiring;

pub use access::{Composition, ProviderOrder, ReachableProviders};
pub use ambient::{Ambient, AmbientValue, Continuation, Deadline, ambient, with_ambient};
pub use app::{App, AppBuilder, BootStep};
pub use app_identity::AppIdentity;
pub use app_lifecycle::{AppLifecycle, AppState, StopCause};
pub use backoff::{Backoff, Retries, process_seed};
pub use budget::Budget;
pub use close::CloseContribution;
pub use container::{Container, ContainerBuilder, ContainerId, ProviderScope};
pub use denial_document::{ChallengeScheme, DenialDocument};
pub use discoverable::{
    Discoverable, INERT_HOST_HINT, InertHost, ProviderResidency, inert_host, unresolved_host,
};
pub use discovery::{Discovered, Discovery};
pub use edge::Edge;
pub use env_flag::parse_bool;
pub use env_prefix::EnvPrefix;
pub use error::{
    AccessGraphError, BootInterruptedError, BudgetPastNetError, ContainerError,
    ContestedDeclarationError, DecodeError, DuplicateMetaError, DuplicateProviderError,
    FactoryCycleError, FactoryFailedError, InitFailedError, LateFactoryError,
    LateOptionalDependencyError, MissingAmbientError, MissingDependencyError,
    OverriddenDependencyError, ProviderCycleError, ScopeViolationError, ScopedLayerError,
    UnboundedTransportError, UnbuiltCause, UnbuiltProviderError, UnclaimedOverrideError,
    UngatedDiscoveryError, UnregisteredModuleError, UnresolvedLayerError, UnservedExportError,
    WiringFailedError,
};
pub use error_message::{SafeLink, boxed_error, error_message};
pub use exception_filter::{
    Catch, Caught, ExceptionFilter, GlobalExceptionFilterList, GlobalExceptionFilters,
};
pub use factory_inputs::FactoryInputs;
pub use handler::{Handler, Posture, Reflector};
pub use identifier::UUID_V7_REQUIRED;
pub use instance::{Instance, Instanced, Unnamed};
pub use interceptor::{GlobalInterceptorList, GlobalInterceptors, Interceptor, Next};
pub use job::{JOB_TIMEOUT, JobTimedOut};
pub use layer::{Layer, LayerKind, LayerSite};
pub use layer_chain::LayerSpec;
pub use lifecycle::{LifecyclePhase, SHUTDOWN_HOOKS_TIMEOUT, SHUTDOWN_SETTLE_TIMEOUT};
pub use module::{Collecting, DynamicModule, Module, Registering};
pub use net::{Net, Unanswered, within};
pub use opaque::{OPAQUE_CLIENT_MESSAGE, UNAVAILABLE_CLIENT_MESSAGE};
pub use outcome::{Failure, Outcome, Settled};
pub use param::ResolvesProvider;
pub use problem::{Code, Problem, ToProblem, code};
pub use proof::{Attempt, Proving, Unproved, prove};
pub use remedy::{CarriesRemedy, DependencyRemedy, InjectedRemedy};
pub use request_scope::{
    RequestContinuation, RequestScope, TaskContext, current_request_scope, with_request_scope,
};
pub use scope::Scoped;
pub use secret::Secret;
pub use served_address::ServedAddress;
pub use tenant::current_tenant_id;
pub use trace_context::{
    Correlation, SpanId, TraceFlags, TraceId, TraceParent, TraceState, current_actor_id,
    current_span_id, current_trace_id, current_traceparent, current_tracestate,
};
pub use transport::{Listening, MAX_STOP_BOUND, Transport, TransportContribution};
pub use unit_context::{Peer, UnitContext, UnitView};
pub use wiring::WiringContribution;

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::access::__private::{
        HostLayers, HostRequires, ModuleDescriptor, ProviderDescriptor, reachable_providers,
    };
    pub use crate::ambient::{
        Pending, ReadsWithheld, attached, install_pending, with_ambient_from,
    };
    pub use crate::answer::{Answer, AnswerFallback, ResultAnswer, ValueAnswer};
    pub use crate::app::__private::{
        check_stop_bound, close_after, init_tearing_down_after, provide_meta,
    };
    pub use crate::app_dir::{app_dir, in_app_dir};
    pub use crate::app_identity::{capture_app_identity, package_identity};
    pub use crate::audience::{
        AnswerRefusal, Audience, FileRefusal, RefusedAnswer, WireRow, answering, as_pushed,
        audience, for_others, is_pushed, refuse_answer,
    };
    pub use crate::client_operation::__private::set_client_recorder;
    pub use crate::container::__private::{
        StalledProvider, check_seed, claim_override, claim_own_override, collect_dynamic_import,
        declaring_site, enter_import, leave_import, overrides_any, reading, record_absent_optional,
        record_stall, refuse_site, register_dynamic_import, site_chains, stand_down,
        take_site_refusal,
    };
    pub use crate::discoverable::__private::Residency;
    pub use crate::dispatch::{ClassGate, GuardStage, GuardStageFactory, Tail, dispatch};
    pub use crate::env_prefix::__private::RETIRED_VARIABLE;
    pub use crate::error_message::__private::{SaidFailure, find_in_chain};
    pub use crate::exception_filter::__private::ErasedExceptionFilter;
    pub use crate::exception_filter::{ExceptionFilterSpec, exception_filter};
    pub use crate::handler::{
        HandlerDeclaration, MetaLevels, MetaValue, declare_handler, meta_value,
    };
    pub use crate::host_runtime::{
        HostMount, HostRuntime, RegisteredHost, RuntimeModule, import_host_runtime,
    };
    pub use crate::injected_factory::InjectedFactory;
    pub use crate::injects::Injects;
    pub use crate::instance::{InstanceDeclaration, InstanceSeal};
    pub use crate::interceptor::__private::{InterceptorRegistration, register_interceptor};
    pub use crate::interceptor::{InterceptorSpec, interceptor};
    pub use crate::layer_chain::__private::{
        GlobalLayer, PoolReport, PooledProvider, ResolvedLayer, SiteChains, compose_chain,
        declare_global, dedup_bucket, global_pool_declared, log_effective_chain, refuse_unresolved,
        report_unresolved, resolve_global_layers, resolve_scoped,
    };
    pub use crate::lifecycle::__private::{HookFuture, LifecycleHook, LifecycleUnitOfWork};
    #[cfg(feature = "logging")]
    pub use crate::logging::__private::{
        ConsoleLayer, LibraryClamp, LogFilter, ProfileSource, TextFormat, filter_dispatch,
        install_global_subscriber, log_filter,
    };
    pub use crate::module::__private::{ForRootValue, dynamic_import_module, module_registered};
    pub use crate::opaque::__private::OpaqueCause;
    pub use crate::operation_log::__private::{declare_unit, measured_unit, unit_opened_by};
    pub use crate::outcome::__private::{caught_problem, is_same_failure, kept, settled};
    pub use crate::panic::__private::{Unwound, unwound};
    pub use crate::param::{
        ParamProbe, ParamProbeAmbient, ParamProbeFallback, ParamProbeResolves, ProtocolArgument,
        TailParam,
    };
    pub use crate::probe_runtime::ProbeRuntime;
    pub use crate::problem::__private::{Withheld, retry_after_secs, withheld, withheld_over};
    pub use crate::process_start::ProcessStart;
    pub use crate::remedy::{RemedyProbe, RemedyProbeCarries, RemedyProbeFallback};
    pub use crate::request_scope::__private::{RequestScopes, nested_continuation};
    pub use crate::request_scope::AnswerContext;
    pub use crate::row_probe::{CrossesAWire, RowProbe, RowProbeFallback};
    pub use crate::site::{
        GlobalLayers, Shape, Site, SiteDeclaration, SiteLayers, UnitChains, compose_site,
        declare_site,
    };
    pub use crate::tenant::TenantId;
    pub use crate::trace_context::__private::{
        current_correlation, hex, link_span, pending_ids, pending_unit, set_actor_id, set_sampled,
        set_span_linker, with_pending_ids, with_pending_unit,
    };
    pub use crate::trace_context::field;
    pub use crate::type_name::{short_type_name, shorten};
    pub use crate::unit_context::{
        carries, new_unit_context, pending_of, take_pending, withhold_from_reads,
    };
    pub use crate::way_down::{MainCrate, main};

    pub use schemars;
    pub use serde;
    pub use validator;
}

// Macro output resolves these through the framework, so an app needs no
// direct dependency on them.
pub use anyhow;
pub use inventory;
pub use tracing;

/// The attribute every async trait of the framework is implemented through —
/// a `Guard`, an `Interceptor`, a `JobProducer` — so an implementation needs
/// no `async-trait` dependency of its own; the one path to it, beside
/// `nest_rs::prelude`.
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
/// redacted by [`panic_message`](panic::panic_message), never Rust's default
/// line on stderr.
///
/// Before `main` runs it installs the `SIGINT` / `SIGTERM` handlers, so the
/// first signal stops the boot at its next await or drains the serving app,
/// and one received before any app exists exits at once with 130 / 143.
///
/// It records the directory of the crate it expands in: a development
/// artifact's relative path — GraphQL's and OpenAPI's `emit_path` — lands
/// there, whatever directory the process runs from. And it records that
/// crate's name, version and description as the process's [`AppIdentity`],
/// which OpenAPI's `info`, MCP's `serverInfo` and OpenTelemetry's
/// `service.name` and `service.version` default to.
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
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// fn implements<T: Module>() {}
/// implements::<AppModule>();
///
/// let app = App::builder().module::<AppModule>().build().await?;
/// let greeter = app.container().get_dyn::<dyn Greeter>();
/// assert_eq!(greeter.map(|g| g.greet()), Some("hello"));
/// assert!(app.container().get::<English>().is_some());
/// # Ok(())
/// # }
/// ```
pub use nest_rs_core_macros::module;

/// Mark a struct as a DI provider built from the container.
///
/// ```
/// use std::sync::Arc;
/// use nest_rs_core::{App, Discoverable, ProviderResidency, injectable, module};
///
/// #[injectable]
/// struct Clock;
///
/// #[injectable]
/// struct Greeter {
///     #[inject]
///     clock: Arc<Clock>,
/// }
///
/// #[injectable(scope = transient)]
/// struct Draft;
///
/// #[module(providers = [Clock, Greeter, Draft])]
/// struct AppModule;
///
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// fn implements<T: Discoverable>() {}
/// implements::<Greeter>();
/// assert!(<Greeter as ProviderResidency>::SINGLETON);
/// assert!(!<Draft as ProviderResidency>::SINGLETON);
///
/// let app = App::builder().module::<AppModule>().build().await?;
/// assert!(app.container().get::<Greeter>().is_some());
/// # Ok(())
/// # }
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
/// declares none of them. Every edge decoding one holds it to its rules.
pub use nest_rs_core_macros::input;

/// The reply-DTO shorthand: carries `Serialize`/`JsonSchema` and routes each
/// back here, for a value an edge sends and never decodes.
///
/// ```
/// use nest_rs_core::output;
///
/// #[output]
/// pub struct PersonDto {
///     pub name: String,
/// }
///
/// let sent = serde_json::to_value(PersonDto { name: "ada".into() })?;
/// assert_eq!(sent["name"], "ada");
/// # Ok::<(), serde_json::Error>(())
/// ```
pub use nest_rs_core_macros::output;

/// The enum both decoded and sent: carries `Serialize`/`Deserialize`/`JsonSchema`
/// and routes each back here, for a column's enum or a claims role.
///
/// ```
/// use nest_rs_core::wire_enum;
///
/// #[wire_enum]
/// #[serde(rename_all = "lowercase")]
/// pub enum Role {
///     Admin,
///     User,
/// }
///
/// assert_eq!(serde_json::to_value(Role::Admin)?, "admin");
/// assert_eq!(serde_json::from_value::<Role>("user".into())?, Role::User);
/// # Ok::<(), serde_json::Error>(())
/// ```
pub use nest_rs_core_macros::wire_enum;

/// Declare a struct as an interceptor its module brings around the guards of
/// every unit it serves — `#[interceptor(stage = unit)]`, the one spelling, whose
/// key is the visible opening. An app-wide interceptor after the guards and the
/// class gate is the root's [`GlobalInterceptors`], and a bare `#[interceptor]`
/// is refused naming it.
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
/// #[interceptor(stage = unit)]
/// #[derive(Layer)]
/// struct CountUnits {
///     #[inject]
///     metrics: Arc<Metrics>,
/// }
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

/// Implement [`Layer`](trait@Layer) — the trait every layer family extends —
/// so a guard, an interceptor, an exception filter or a global pipe writes
/// only its family's trait. `#[layer(priority = …)]` orders it within one
/// site of its stage, lower first; a layer choosing the units it serves
/// ([`Layer::serves`]) writes the impl by hand.
///
/// ```
/// use nest_rs_core::{Interceptor, Layer, Next, Outcome, UnitContext, async_trait, injectable};
///
/// #[injectable]
/// #[derive(Layer)]
/// struct Timed;
///
/// #[injectable]
/// #[derive(Layer)]
/// #[layer(priority = -10)]
/// struct TimedFirst;
///
/// #[async_trait]
/// impl Interceptor for Timed {
///     async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
///         next.run(cx).await
///     }
/// }
///
/// assert_eq!(Layer::priority(&Timed), 0);
/// assert_eq!(Layer::priority(&TimedFirst), -10);
/// ```
///
/// It takes one key:
///
/// ```compile_fail
/// #[derive(nest_rs_core::Layer)]
/// #[layer(order = 1)]
/// struct Misnamed;
/// ```
pub use nest_rs_core_macros::Layer;

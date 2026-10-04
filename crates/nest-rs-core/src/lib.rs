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
//! by `TypeId`. Visibility is Rust's job, not a per-module export list: expose a
//! `pub trait`, bind an implementation with `provide_dyn`, and consumers inject
//! `Arc<dyn Trait>`.
//!
//! # Wiring is checked, not reflected
//!
//! The dependency graph is validated at boot, never resolved by reflection at
//! runtime. The boot walks the module tree from the root and fails with a
//! named error before any transport starts: [`AccessGraphError`] when a provider reaches across a
//! module boundary no `import` covers, [`MissingDependencyError`] when a
//! dependency no module provides would otherwise panic at first resolution. A
//! misconfigured import is a startup error naming the fix, not a `Cannot
//! resolve` on the first request. `#[use_guards]` / `#[use_filters]` /
//! `#[use_interceptors]` are checked the same way.
//!
//! # Scopes and lifecycle
//!
//! Providers are singletons by default. `#[injectable(scope = request)]` builds
//! one instance per request, reached through a transport's request boundary;
//! `#[injectable(scope = transient)]` rebuilds on every resolution. Lifecycle
//! hooks (`#[on_module_init]`, `#[on_application_bootstrap]`,
//! `#[on_module_destroy]`, …) run per phase as [`App::run`] drains them —
//! init failure aborts boot, shutdown is best-effort.
//!
//! The two are not free to combine: a hook — like a scheduled method, a
//! listener, an indicator and a processor — resolves its host with
//! `Container::get::<Host>()` outside any request, so the host must be a
//! **singleton stored under its own type**. [`ProviderResidency`] is that requirement
//! made checkable, and the three shapes that can never satisfy it are compile
//! errors rather than a boot-time notice. The fourth, a host bound only as
//! `dyn Trait`, is the app's composition to fix and carries
//! [`INERT_HOST_HINT`].
//!
//! # Discovery
//!
//! Module-wired items implement [`Discoverable`] and are found through link-time
//! `inventory`, gated on reachability from the running app's root
//! ([`ReachableProviders`]). An item linked into the binary but living in no
//! reachable module is inert, with a boot `warn` — which is what lets one shared
//! feature crate serve different per-binary subsets (an API mounts HTTP and
//! GraphQL; a worker mounts only the queue).
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
mod answer;
pub mod app;
pub mod container;
pub(crate) mod cycle_guard;
pub mod discoverable;
pub mod discovery;
pub mod env_flag;
pub mod env_prefix;
pub mod error;
pub mod error_message;
mod identifier;
pub mod layer;
pub mod layer_chain;
pub mod lifecycle;
#[cfg(feature = "logging")]
pub mod logging;
pub mod module;
mod opaque;
pub mod operation_log;
pub mod panic;
pub mod request_scope;
pub mod target;
pub mod trace_context;
pub mod transport;
mod type_name;
mod way_down;

// The three access-graph validators have no caller outside `src/` in either
// workspace and are `pub(crate)`: visibility wider than its use is a promise
// nobody priced. `AccessError` left this list for the same reason — it is the
// bare pass's internal wrapper, discarded before any error leaves the crate, so
// no public signature could hand a caller one. Why the eight below live in
// `error` rather than beside the pass is that module's own doc.
pub use access::{
    Composition, ModuleDescriptor, ProviderDescriptor, ProviderOrder, ReachableProviders,
};
#[doc(hidden)]
pub use answer::{Answer, AnswerFallback, ResultAnswer, ValueAnswer};
pub use app::{App, AppBuilder};
pub use container::{Container, ContainerBuilder, ContainerId, KeyedDependency, ProviderKey};
pub use discoverable::{
    Discoverable, INERT_HOST_HINT, InertHost, ProviderResidency, inert_host, is_framework_owned,
    unresolved_host,
};
pub use discovery::{Discovered, Discovery};
pub use env_flag::parse_bool;
pub use env_prefix::EnvPrefix;
pub use error::{
    AccessGraphError, ContestedDeclarationError, DecodeError, DuplicateProviderError,
    FactoryCycleError, KeyedDependencyError, MissingDependencyError, ScopeViolationError,
    UnresolvedFactoryError,
};
pub use error_message::{boxed_error, error_message};
pub use identifier::UUID_V7_REQUIRED;
pub use layer::{Layer, LayerKind, LayerSite};
pub use layer_chain::LayerSpec;
pub use lifecycle::{
    LifecycleHook, LifecyclePhase, SHUTDOWN_HOOKS_TIMEOUT, SHUTDOWN_SETTLE_TIMEOUT,
};
pub use module::{DynamicModule, Module};
pub use opaque::OPAQUE_CLIENT_MESSAGE;
pub use panic::panic_message;
pub use request_scope::{
    RequestContinuation, RequestScope, TaskContext, current_request_scope, with_request_scope,
};
pub use trace_context::{
    Correlation, SpanId, TraceFlags, TraceId, TraceParent, TraceState, current_actor_id,
    current_span_id, current_trace_id, current_traceparent, current_tracestate,
};
#[doc(hidden)]
pub use trace_context::{current_correlation, set_actor_id};
pub use transport::{Transport, TransportContribution};
// The pipes word a refusal with it; not public API.
#[doc(hidden)]
pub use type_name::short_type_name;
// `#[nest_rs::main]`'s expansion — see the `way_down` module.
#[doc(hidden)]
pub use way_down::__main;

// Cross-crate Layer-System wiring — `pub` for the five registry crates and
// macro output, not public API. `LayerSpec` (above) is the one deliberate
// vocabulary type; the chain-composition primitives around it are plumbing.
#[doc(hidden)]
pub use layer_chain::{ResolvedLayer, check_specs_resolvable, compose_chain};

// Macro plumbing — `#[module]`-generated code names this to register a module in
// the boot inventory. Hidden at its definition; kept off the curated list here.
#[doc(hidden)]
pub use module::{__dynamic_import_module, __module_registered};

// Re-exported so `#[hooks]`-generated `inventory::submit!` resolves through the
// framework — apps never depend on `inventory` directly.
pub use inventory;

// Re-exported so `operation_span!` resolves `info_span!` and `field::Empty`
// through the kernel rather than against the expanding crate's extern prelude.
// Every framework crate happens to declare `tracing`, so this changes nothing
// today — and the day one does not, the macro keeps working instead of failing
// inside an expansion nobody can read.
pub use tracing;

// Re-exported so the `#[hooks]`-generated run-fn signature
// (`anyhow::Result<()>`) resolves through the framework — a downstream app
// using `#[hooks]` without a direct `anyhow` dependency must still compile.
pub use anyhow;
// `#[input]` carries the wire-DTO derives so the developer does not; routing
// them through the kernel keeps them reachable from every capability.
#[doc(hidden)]
pub use schemars;
#[doc(hidden)]
pub use serde;
#[doc(hidden)]
pub use validator;

// A `pub trait` + `as dyn Trait` provider — the documented way to hide an impl
// behind the DI container — needs `#[async_trait]` on both the trait and the
// impl. That is a *container* concern, not a transport one, so it is reachable
// as `nest_rs::core::async_trait`. Every surface crate re-exports it too, but
// routing a plain service trait through `nest_rs::guards::async_trait` (the
// only path that used to exist for one) reads as a mistake.
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
/// Every `#[inject]` field must be an `Arc<T>` or `Arc<dyn Trait>` — a
/// dependency is resolved from the container as a shared `Arc` — so a non-`Arc`
/// injected field is rejected at compile time rather than failing with a
/// cryptic type error in generated code:
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

/// The wire-DTO shorthand: carries `Serialize`/`Deserialize`/`Validate`/
/// `JsonSchema` and routes each back here, so a DTO crossing any transport
/// declares none of them.
pub use nest_rs_core_macros::input;

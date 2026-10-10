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
//! init failure aborts boot, shutdown is best-effort. A registry a module fills
//! from the assembled container is filled before the first hook, by the wiring
//! step it attaches with [`ContainerBuilder::provide_wiring`].
//!
//! A hook host — like a scheduled method, a listener, an indicator and a
//! processor — is resolved outside any request, so it must be a **singleton
//! stored under its own type** ([`ProviderResidency`]); a host bound only as
//! `dyn Trait` is inert and carries [`INERT_HOST_HINT`].
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
mod answer;
pub mod app;
mod budget;
pub mod container;
pub(crate) mod cycle_guard;
pub mod discoverable;
pub mod discovery;
pub mod env_flag;
pub mod env_prefix;
pub mod error;
pub mod error_message;
mod handler;
mod identifier;
pub mod layer;
pub mod layer_chain;
pub mod lifecycle;
mod line_safe;
#[cfg(feature = "logging")]
pub mod logging;
pub mod module;
mod net;
mod opaque;
pub mod operation_log;
pub mod panic;
pub mod problem;
mod process_start;
pub mod request_scope;
pub mod target;
pub mod trace_context;
pub mod transport;
mod type_name;
mod unit_context;
mod way_down;
mod wiring;

pub use access::{Composition, ProviderOrder, ReachableProviders};
pub use app::{App, AppBuilder};
pub use budget::Budget;
pub use container::{Container, ContainerBuilder, ContainerId, KeyedDependency, ProviderKey};
pub use discoverable::{
    Discoverable, INERT_HOST_HINT, InertHost, ProviderResidency, inert_host, is_framework_owned,
    unresolved_host,
};
pub use discovery::{Discovered, Discovery};
pub use env_flag::parse_bool;
pub use env_prefix::EnvPrefix;
pub use error::{
    AccessGraphError, BudgetPastNetError, ContestedDeclarationError, DecodeError,
    DuplicateProviderError, FactoryCycleError, KeyedDependencyError, LateFactoryError,
    MissingDependencyError, ProviderCycleError, ScopeViolationError, UnregisteredModuleError,
    UnresolvedFactoryError, WiringFailedError,
};
pub use error_message::{boxed_error, error_message};
pub use handler::{Handler, Posture, Reflector};
pub use identifier::UUID_V7_REQUIRED;
pub use layer::{Layer, LayerKind, LayerSite};
pub use layer_chain::LayerSpec;
pub use lifecycle::{LifecyclePhase, SHUTDOWN_HOOKS_TIMEOUT, SHUTDOWN_SETTLE_TIMEOUT};
pub use module::{Collecting, DynamicModule, Module, Registering};
pub use net::Net;
pub use opaque::{OPAQUE_CLIENT_MESSAGE, UNAVAILABLE_CLIENT_MESSAGE};
pub use operation_log::Edge;
pub use panic::panic_message;
pub use problem::{Code, Problem, ToProblem};
pub use request_scope::{
    RequestContinuation, RequestScope, TaskContext, current_request_scope, with_request_scope,
};
pub use trace_context::{
    Correlation, SpanId, TraceFlags, TraceId, TraceParent, TraceState, current_actor_id,
    current_span_id, current_trace_id, current_traceparent, current_tracestate,
};
pub use transport::Transport;
pub use unit_context::{Peer, UnitContext, UnitView};
pub use wiring::WiringContribution;

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::access::__private::{ModuleDescriptor, ProviderDescriptor};
    pub use crate::answer::{Answer, AnswerFallback, ResultAnswer, ValueAnswer};
    pub use crate::container::__private::{
        collect_dynamic_import, enter_import, leave_import, register_dynamic_import,
    };
    pub use crate::error_message::__private::find_in_chain;
    pub use crate::handler::{HandlerDeclaration, MetaLevels, declare_handler};
    pub use crate::layer_chain::__private::{
        ResolvedLayer, check_specs_resolvable, compose_chain, dedup_bucket, resolve_global_layers,
    };
    pub use crate::lifecycle::__private::LifecycleHook;
    pub use crate::module::__private::{dynamic_import_module, module_registered};
    pub use crate::operation_log::__private::{declare_unit, unit_opened_by};
    pub use crate::panic::__private::{Unwound, unwound};
    pub use crate::problem::__private::withheld;
    pub use crate::process_start::ProcessStart;
    pub use crate::trace_context::__private::{
        current_correlation, hex, link_span, pending_ids, set_actor_id, set_sampled,
        set_span_linker, with_pending_ids,
    };
    pub use crate::transport::__private::TransportContribution;
    pub use crate::type_name::short_type_name;
    pub use crate::unit_context::new_unit_context;
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

/// The wire-DTO shorthand: carries `Serialize`/`Deserialize`/`Validate`/
/// `JsonSchema` and routes each back here, so a DTO crossing any transport
/// declares none of them.
pub use nest_rs_core_macros::input;
